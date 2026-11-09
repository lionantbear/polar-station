//! NSAppleScript runs on the Host's process main thread; network work stays on Tokio.
//! Permission checks use Apple's thread-safe API on a blocking worker and never
//! request consent except in response to the explicit local authorization action.
use super::PlatformSendOutcome;
use objc2::{
    msg_send,
    rc::{autoreleasepool, Retained},
    AnyThread, ClassType,
};
use objc2_foundation::{NSAppleEventDescriptor, NSAppleScript, NSBundle, NSString};
use std::{
    ffi::c_void,
    sync::{mpsc, OnceLock},
    time::{Duration, Instant},
};
use tokio::sync::oneshot;
use zeroize::Zeroizing;

#[link(name = "CoreServices", kind = "framework")]
extern "C" {
    fn AEDeterminePermissionToAutomateTarget(
        target: *const c_void,
        event_class: u32,
        event_id: u32,
        ask_user_if_needed: bool,
    ) -> i32;
}

struct SendJob {
    recipient: Zeroizing<String>,
    text: Zeroizing<String>,
    deadline: Instant,
    reply: oneshot::Sender<PlatformSendOutcome>,
}
static MAIN_SENDER: OnceLock<mpsc::SyncSender<SendJob>> = OnceLock::new();

pub(crate) fn available() -> bool {
    MAIN_SENDER.get().is_some()
}

pub(crate) fn run_on_process_main<F>(run: F) -> anyhow::Result<()>
where
    F: FnOnce() -> anyhow::Result<()> + Send + 'static,
{
    if !objc2_foundation::NSThread::isMainThread_class() {
        anyhow::bail!("Messages dispatcher requires the process main thread");
    }
    let (tx, rx) = mpsc::sync_channel::<SendJob>(8);
    MAIN_SENDER
        .set(tx)
        .map_err(|_| anyhow::anyhow!("Messages dispatcher already initialized"))?;
    let runtime_thread = std::thread::spawn(run);
    while !runtime_thread.is_finished() {
        match rx.recv_timeout(Duration::from_millis(25)) {
            Ok(job) => {
                if job.reply.is_closed() || Instant::now() >= job.deadline {
                    let _ = job
                        .reply
                        .send(PlatformSendOutcome::Rejected("platform_request_expired"));
                    continue;
                }
                let result = autoreleasepool(|_| send_script(&job.recipient, &job.text));
                let _ = job.reply.send(result);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    runtime_thread
        .join()
        .map_err(|_| anyhow::anyhow!("Host runtime thread failed"))?
}

pub(crate) async fn authorized(ask: bool) -> bool {
    tokio::task::spawn_blocking(move || {
        autoreleasepool(|_| {
            // This call can wait for user consent; it must not block the main thread.
            // Messages changed its bundle identifier on newer macOS releases.
            // Resolve the protected system app used by the sender below.
            let Some(bundle_id) =
                NSBundle::bundleWithPath(&NSString::from_str("/System/Applications/Messages.app"))
                    .and_then(|bundle| bundle.bundleIdentifier())
            else {
                tracing::warn!("Installed Messages app could not be resolved");
                return false;
            };
            let target = NSAppleEventDescriptor::descriptorWithBundleIdentifier(&bundle_id);
            // objc2-foundation's CoreServices-gated convenience methods require a new
            // dependency. Use the existing Objective-C descriptors and C ABI directly.
            let descriptor: *const c_void = unsafe { msg_send![&*target, aeDesc] };
            let status = unsafe {
                AEDeterminePermissionToAutomateTarget(
                    descriptor,
                    u32::from_be_bytes(*b"****"),
                    u32::from_be_bytes(*b"****"),
                    ask,
                )
            };
            if ask && status != 0 {
                tracing::warn!(
                    os_status = status,
                    "Messages Automation authorization did not succeed"
                );
            }
            status == 0
        })
    })
    .await
    .unwrap_or(false)
}

pub(crate) async fn send(
    recipient: Zeroizing<String>,
    text: Zeroizing<String>,
) -> PlatformSendOutcome {
    if !authorized(false).await {
        return PlatformSendOutcome::Rejected("messages_authorization_required");
    }
    let Some(sender) = MAIN_SENDER.get() else {
        return PlatformSendOutcome::Rejected("main_thread_dispatcher_unavailable");
    };
    let (reply, rx) = oneshot::channel();
    let job = SendJob {
        recipient,
        text,
        deadline: Instant::now() + Duration::from_secs(8),
        reply,
    };
    if sender.try_send(job).is_err() {
        return PlatformSendOutcome::Rejected("platform_busy");
    }
    match tokio::time::timeout(Duration::from_secs(10), rx).await {
        Ok(Ok(result)) => result,
        _ => PlatformSendOutcome::Unknown("platform_reply_lost"),
    }
}

const SCRIPT: &str = r#"on station_notify(targetAddress, messageText)
    with timeout of 10 seconds
        tell application "/System/Applications/Messages.app"
            set targetService to first service whose service type is iMessage
            set targetBuddy to buddy targetAddress of targetService
            send messageText to targetBuddy
        end tell
    end timeout
    return "sent"
end station_notify"#;

fn send_script(recipient: &str, text: &str) -> PlatformSendOutcome {
    let Some(script) =
        NSAppleScript::initWithSource(NSAppleScript::alloc(), &NSString::from_str(SCRIPT))
    else {
        return PlatformSendOutcome::Rejected("script_unavailable");
    };
    let mut error_info = None;
    if !unsafe { script.compileAndReturnError(Some(&mut error_info)) } {
        return PlatformSendOutcome::Rejected("script_compile_failed");
    }
    let parameters = NSAppleEventDescriptor::listDescriptor();
    parameters.insertDescriptor_atIndex(
        &NSAppleEventDescriptor::descriptorWithString(&NSString::from_str(recipient)),
        1,
    );
    parameters.insertDescriptor_atIndex(
        &NSAppleEventDescriptor::descriptorWithString(&NSString::from_str(text)),
        2,
    );
    let current = NSAppleEventDescriptor::currentProcessDescriptor();
    let event: Retained<NSAppleEventDescriptor> = unsafe {
        msg_send![NSAppleEventDescriptor::class(),appleEventWithEventClass:u32::from_be_bytes(*b"ascr"),eventID:u32::from_be_bytes(*b"psbr"),targetDescriptor:&*current,returnID:-1i16,transactionID:0i32]
    };
    let handler =
        NSAppleEventDescriptor::descriptorWithString(&NSString::from_str("station_notify"));
    unsafe {
        let _: () =
            msg_send![&*event,setParamDescriptor:&*handler,forKeyword:u32::from_be_bytes(*b"snam")];
        let _: () = msg_send![&*event,setParamDescriptor:&*parameters,forKeyword:u32::from_be_bytes(*b"----")];
        let _result = script.executeAppleEvent_error(&event, Some(&mut error_info));
    }
    // Never log the error dictionary: it can contain the recipient and body.
    if error_info.is_some() {
        PlatformSendOutcome::Unknown("messages_execution_failed")
    } else {
        PlatformSendOutcome::Sent
    }
}
