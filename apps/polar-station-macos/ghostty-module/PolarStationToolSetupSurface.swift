#if POLAR_STATION
import AppKit
import GhosttyKit
import PolarTmuxHostClient

/// A native interactive PTY. Commands are consumed once and never saved here.
@MainActor final class PolarStationToolSetupSurface:NSObject {
    let view:Ghostty.SurfaceView
    let content:StationPaneContentV1
    private var exited:(@MainActor (UInt32?)->Void)?
    private var pasteNotice: NSTextField?
    var running:Bool {exited != nil}
    init(app:ghostty_app_t,setup:ToolSetupLaunchV1,fontSize:UInt8,exited:@escaping @MainActor (UInt32?)->Void) throws {
        var config=Ghostty.SurfaceConfiguration()
        config.command=setup.command
        config.fontSize=Float32(fontSize)
        config.waitAfterCommand=true
        self.view=Ghostty.SurfaceView(app,baseConfig:config)
        self.content=StationPaneContentV1(setupID:setup.id,executionTargetID:setup.execution_target_id)
        self.exited=exited
        super.init()
        view.setTitle(setup.title)
        guard view.surface != nil else {throw PolarStationNativeExecutorError.workspaceUnavailable}
        NotificationCenter.default.addObserver(self,selector:#selector(childExited(_:)),name:.init("PolarStationChildExited"),object:view)
        NotificationCenter.default.addObserver(self,selector:#selector(pasteCommitted),name:.init("PolarStationPasteCommitted"),object:view)
    }
    @objc private func pasteCommitted() {
        guard running else { return }
        pasteNotice?.removeFromSuperview()
        let notice = NSTextField(labelWithString: "Pasted into terminal")
        notice.font = .systemFont(ofSize: 13, weight: .semibold)
        notice.backgroundColor = .windowBackgroundColor
        notice.drawsBackground = true
        notice.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(notice)
        NSLayoutConstraint.activate([
            notice.leadingAnchor.constraint(equalTo: view.leadingAnchor, constant: 12),
            notice.bottomAnchor.constraint(equalTo: view.bottomAnchor, constant: -12),
        ])
        pasteNotice = notice
        DispatchQueue.main.asyncAfter(deadline: .now() + 3) { [weak notice] in notice?.removeFromSuperview() }
    }
    @objc private func childExited(_ event:Notification) {
        finish((event.userInfo?["exit_code"] as? NSNumber)?.uint32Value)
    }
    private func finish(_ code:UInt32?) {
        pasteNotice?.removeFromSuperview()
        let callback=exited;exited=nil;callback?(code)
    }
    func confirmClose() async -> Bool {
        guard running else {return true}
        guard let copy=ToolSetupCopy.load() else {return false}
        let alert=NSAlert()
        alert.messageText=copy.close_title;alert.informativeText=copy.close_detail
        alert.addButton(withTitle:copy.keep);alert.addButton(withTitle:copy.stop)
        if let window=view.window {
            return await withCheckedContinuation { continuation in
                alert.beginSheetModal(for:window){continuation.resume(returning:$0 == .alertSecondButtonReturn)}
            }
        }
        return alert.runModal() == .alertSecondButtonReturn
    }
    /// Called after accepted native removal. Ghostty releases the PTY with its view.
    func retire() {
        NotificationCenter.default.removeObserver(self)
        finish(nil)
    }
}
#endif
