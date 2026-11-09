"""Pinned Ghostty ingress hooks; native adapters own all implementation/state."""
from pathlib import Path

def edits(platform):
    rules = {}
    def add(path, before, after):
        rules.setdefault(path, []).append((before, after))
    if platform == 'macos':
        config = 'src/config/Config.zig'
        for suffix, code in [('b', 'D'), ('f', 'C')]:
            add(config, '.{ .esc = "' + suffix + '" },',
                '.{ .csi = "1;3' + code + '" },')
        test_anchor = '    test "parseCLI chain applies to most recent table binding" {'
        test_source = Path(__file__).resolve().parents[2] / 'apps/polar-tmux-shared/ghostty/StationOptionArrowTests.zig'
        add(config, test_anchor, test_source.read_text() + '\n' + test_anchor)
        terminal = 'macos/Sources/Features/Terminal/TerminalController.swift'
        base = 'macos/Sources/Features/Terminal/BaseTerminalController.swift'
        def hook(path, anchor, action, result='return'):
            add(path, anchor, anchor + '\n        #if POLAR_STATION\n        if PolarStationRuntime.shared.nativeAction("' + action + '", controller: self) { ' + result + ' }\n        #endif')
        for method, action in [('closeTab', 'close_tab'), ('closeOtherTabs', 'close_other_tabs'), ('closeTabsOnTheRight', 'close_right_tabs')]:
            hook(terminal, f'    @IBAction func {method}(_ sender: Any?) {{', action)
        hook(base, '    func promptTabTitle() {', 'rename_tab')
        add(terminal, '    static func closeAllWindows() {', '    static func closeAllWindows() {\n        #if POLAR_STATION\n        PolarStationRuntime.shared.closeAllManagedWindows()\n        return\n        #endif')
        ghostty_app = 'macos/Sources/Ghostty/Ghostty.App.swift'
        before = '                controller.titleOverride = titleOverride'
        add(ghostty_app, before, '                #if POLAR_STATION\n                Task { @MainActor in _ = PolarStationRuntime.shared.nativeAction("rename_tab", controller: controller, title: title) }\n                return true\n                #endif\n'+before)

        before = '        // This node must be part of our tree'
        add(base, before, '        #if POLAR_STATION\n        if PolarStationRuntime.shared.closeNativeSurfaces(node.leaves(), controller: self) { return }\n        #endif\n'+before)

        anchor = '        // We can only create new splits for surfaces in our tree.'
        add(base, anchor, '#if POLAR_STATION\n        if PolarStationRuntime.shared.nativeAction("split", controller: self) { return nil }\n        #endif\n' + anchor)
        for method, action in [('ghosttyDidEqualizeSplits','equalize_splits'), ('ghosttyDidResizeSplit','resize_split'), ('ghosttyDidToggleSplitZoom','zoom_split')]:
            # Insert only after the notification's ownership check.
            start = f'    @objc private func {method}(_ notification: Notification) {{'
            hook(base, start, action)
        hook(base, '    func performSplitAction(_ action: TerminalSplitOperation) {', 'split')
        add(base, '    override var undoManager: ExpiringUndoManager? {', '    override var undoManager: ExpiringUndoManager? {\n        #if POLAR_STATION\n        return nil // Layout history is owned by Station; text editors keep their own UndoManager.\n        #else')
        add(base, '        return nil\n    }\n\n    struct SavedFrame', '        return nil\n        #endif\n    }\n\n    struct SavedFrame')
        for path, anchor in [(base,'    func validateMenuItem(_ item: NSMenuItem) -> Bool {'),(terminal,'    override func validateMenuItem(_ item: NSMenuItem) -> Bool {')]:
            add(path, anchor, anchor+'\n        #if POLAR_STATION\n        if let enabled = PolarStationRuntime.shared.managedMenuValidation(item, controller: self) { return enabled }\n        #endif')
        window = 'macos/Sources/Features/Terminal/Window Styles/TerminalWindow.swift'
        before = '    func tabTitleEditor(\n        _ editor: TabTitleEditor,\n        didCommitTitle editedTitle: String,'
        # AppKit inherited actions are native entrypoints too.
        anchor = 'class TerminalWindow: NSWindow {'
        add(window, anchor, anchor + '\n    #if POLAR_STATION\n    override func moveTabToNewWindow(_ sender: Any?) {\n        if let controller = windowController as? BaseTerminalController, PolarStationRuntime.shared.nativeAction("move_tab", controller: controller) { return }\n        super.moveTabToNewWindow(sender)\n    }\n    #endif\n\n')
        anchor = '    override func mergeAllWindows(_ sender: Any?) {'
        add(window, anchor, anchor + '\n        #if POLAR_STATION\n        return // Managed geometry actions are disabled.\n        #endif')
        before = '        targetController.titleOverride = editedTitle.isEmpty ? nil : editedTitle'
        add(window, before, '        #if POLAR_STATION\n        if PolarStationRuntime.shared.nativeAction("rename_tab", controller: targetController, title: editedTitle) { return }\n        #endif\n'+before)
        app = 'macos/Sources/App/macOS/AppDelegate.swift'
        for anchor in ['    @objc private func ghosttyNewWindow(_ notification: Notification) {', '    @IBAction func newWindow(_ sender: Any?) {']:
            add(app, anchor, anchor+'\n        #if POLAR_STATION\n        Task { @MainActor in PolarStationRuntime.shared.requestManagedNewWindow() }\n        return\n        #endif')
        anchor = '    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {'
        # Managed close is view-only, so Ghostty's "terminate sessions" prompt is inapplicable.
        add(app, anchor, anchor+'\n        #if POLAR_STATION\n        return PolarStationRuntime.shared.prepareToTerminate()\n        #else')
        add(app, '    func applicationWillTerminate(_ notification: Notification) {', '    #endif\n\n    func applicationWillTerminate(_ notification: Notification) {')
        # The conditional must close inside applicationShouldTerminate, not outside its brace.
        rules[app][-1] = ('        default:\n            return .terminateCancel\n        }\n    }', '        default:\n            return .terminateCancel\n        }\n        #endif\n    }')
    else:
        app = 'src/apprt/gtk/class/application.zig'
        add(app, '    fn quitNow(self: *Self) void {\n        if (comptime build_options.polar_tmux) polar_tmux.willQuit(self);', '    pub fn quitNow(self: *Self) void {\n        if (comptime build_options.polar_tmux) { if (!polar_tmux.willQuit(self)) return; }')
        tab = 'src/apprt/gtk/class/tab.zig'
        window = 'src/apprt/gtk/class/window.zig'
        for tag, action, call in [('new_split','split','Action.newSplit(target, value)'),('equalize_splits','equalize_splits','Action.equalizeSplits(target)'),('resize_split','resize_split','Action.resizeSplit(target, value)'),('toggle_split_zoom','zoom_split','Action.toggleSplitZoom(target)')]:
            before = f'            .{tag} => return {call},'
            add(app,before,f'            .{tag} => return if (comptime @import("build_options").polar_tmux) polar_tmux.consumeNativeLayoutAction("{action}") else {call},')
        before = '                tab.setTitleOverride(if (value.title.len == 0) null else value.title);'
        add(app,before,'                if (comptime @import("build_options").polar_tmux) { if (polar_tmux.requestNativeTabAction(tab, "rename_tab", value.title)) return true; }\n'+before)
        before = '        self.setTitleOverride(if (title.len == 0) null else title);'
        add(tab,before,'        if (comptime @import("build_options").polar_tmux) { if (@import("../polar_tmux/main.zig").requestNativeTabAction(self, "rename_tab", title)) return; }\n'+before)
        before = '        // Delegate to our parent to handle this, since this will emit'
        add(tab,before,'        if (comptime @import("build_options").polar_tmux) {\n            if (@import("../polar_tmux/main.zig").requestNativeTabAction(self, switch (mode) { .this => "close_tab", .other => "close_other_tabs", .right => "close_right_tabs" }, null)) return;\n        }\n'+before)
        before = '        self.performBindingAction(.{ .close_tab = mode });'
        add(window, before, '        if (comptime @import("build_options").polar_tmux) {\n            const page = self.getTabView().getSelectedPage() orelse return;\n            const tab = gobject.ext.cast(Tab, page.getChild()) orelse return;\n            if (polar_tmux.requestNativeTabAction(tab, switch (mode) { .this => "close_tab", .other => "close_other_tabs", .right => "close_right_tabs" }, null)) return;\n        }\n'+before)
        title_dialog = 'src/apprt/gtk/class/title_dialog.zig'
        before = '        self.as(Dialog.Parent).setHeading(priv.target.title());'
        add(title_dialog, before, before+'\n        if (comptime @import("build_options").polar_tmux) {\n            if (priv.target == .tab) self.as(adw.AlertDialog).setBody("Enter a non-empty Tab name.");\n        }')
        before = '        ext.actions.add(Self, self, &actions);'
        add(window, before, before+'\n        if (comptime @import("build_options").polar_tmux) {\n            for ([_][:0]const u8{ "split-right", "split-left", "split-up", "split-down" }) |name| {\n                const action = self.as(gio.ActionMap).lookupAction(name) orelse continue;\n                if (gobject.ext.cast(gio.SimpleAction, action.as(gobject.Object))) |simple| simple.setEnabled(0);\n            }\n        }')
        before = '        // If the tab says it doesn\'t need confirmation then we go ahead'
        add(window,before,'        if (comptime @import("build_options").polar_tmux) {\n            if (polar_tmux.interceptNativePageClose(self, page)) return 1;\n        }\n'+before)
        before = '    fn tabViewCreateWindow(\n        _: *adw.TabView,\n        _: *Self,\n    ) callconv(.c) *adw.TabView {'
        add(window, before, '    fn tabViewCreateWindow(\n        _: *adw.TabView,\n        self: *Self,\n    ) callconv(.c) ?*adw.TabView {\n        if (comptime @import("build_options").polar_tmux) { polar_tmux.requestNativeTabTearOut(self); return null; }')
        before = '        if (priv.tab_view.getNPages() == 0) {\n            // If we have no pages left then we want to close window.'
        add(window, before, '        if (priv.tab_view.getNPages() == 0) {\n            if (comptime @import("build_options").polar_tmux) return;\n            // If we have no pages left then we want to close window.')
        before = '            .new_window => try Action.newWindow(\n                self,\n                switch (target) {\n                    .app => null,\n                    .surface => |v| v,\n                },\n                .none,\n            ),'
        add(app, before, '            .new_window => {\n                if (comptime @import("build_options").polar_tmux) polar_tmux.prepareNativeNewWindow(self);\n                try Action.newWindow(self, switch (target) { .app => null, .surface => |v| v }, .none);\n            },')
        before = '        log.debug("received new window action", .{});'
        add(app, before, before+'\n        if (comptime @import("build_options").polar_tmux) polar_tmux.prepareNativeNewWindow(self);')
        before = '        // Get the attached page which must be a Tab object.'
        add(window, before, '        if (comptime @import("build_options").polar_tmux) { if (polar_tmux.interceptNativeTabTransfer(self, page)) return; }\n'+before)
        split = 'src/apprt/gtk/class/split_tree.zig'
        before = '    fn surfaceCloseRequest(\n        surface: *Surface,\n        self: *Self,\n    ) callconv(.c) void {'
        add(split, before, before+'\n        if (comptime @import("build_options").polar_tmux) { if (@import("../polar_tmux/main.zig").requestNativeSurfaceClose(surface)) return; }')
        before = '        tree.resizeInPlace(priv.handle, @floatCast(current_ratio));'
        add(split, before, '        if (comptime @import("build_options").polar_tmux) {\n            const desired: c_int = @intFromFloat(@round(@as(f64, @floatFromInt(max)) * desired_ratio));\n            paned.setPosition(desired);\n            return 0;\n        }\n'+before)
    return rules
