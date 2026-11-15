#if POLAR_STATION
import AppKit
import GhosttyKit
import PolarTmuxHostClient

/// A native interactive PTY. Commands are consumed once and never saved here.
@MainActor final class PolarStationToolSetupSurface:NSObject {
    let view:Ghostty.SurfaceView
    let content:StationPaneContentV1
    private var exited:(@MainActor (UInt32?)->Void)?
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
    }
    @objc private func childExited(_ event:Notification) {
        finish((event.userInfo?["exit_code"] as? NSNumber)?.uint32Value)
    }
    private func finish(_ code:UInt32?) {
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
