"""Pinned Ghostty ingress hooks; native adapters own all implementation/state."""

def edits(platform):
    rules = {}
    def add(path, before, after):
        rules.setdefault(path, []).append((before, after))
    # One producer for the option consumed by the shared native link hooks.
    # GTK-only client-library wiring remains in the Linux platform overlay.
    build_config = 'src/build/Config.zig'
    add(build_config, 'wasm_shared: bool = true,',
        'wasm_shared: bool = true,\npolar_tmux: bool = false,')
    add(build_config, '    // Feature Flags\n', '''    // Feature Flags

    config.polar_tmux = b.option(
        bool,
        "polar-tmux",
        "Compile the managed Polar Station integration.",
    ) orelse false;
''')
    add(build_config, '    step.addOption(bool, "wasm_shared", self.wasm_shared);',
        '    step.addOption(bool, "polar_tmux", self.polar_tmux);\n    step.addOption(bool, "wasm_shared", self.wasm_shared);')
    # Reuse one native Ghostty link engine on both managed platforms. URL
    # remains first (Config.link-url depends on index zero); OSC8 keeps its
    # existing priority. Patterns come from the Resource domain, not Python.
    anchor = '    return result;\n}\n\n/// Load configuration from an iterator'
    add('src/config/Config.zig', anchor, '''    if (comptime @import("build_options").polar_tmux) {
        const resource = struct {
            extern fn polar_station_resource_file_rule_count() usize;
            extern fn polar_station_resource_file_rule_at(usize) ?[*:0]const u8;
        };
        for (0..resource.polar_station_resource_file_rule_count()) |index| {
            const pattern = resource.polar_station_resource_file_rule_at(index) orelse continue;
            try result.link.links.append(alloc, .{
                .regex = try alloc.dupe(u8, std.mem.span(pattern)),
                .action = .{ .open = {} },
                .highlight = .{ .hover_mods = inputpkg.ctrlOrSuper(.{}) },
                .station_file_reference = true,
            });
        }
    }
''' + anchor)
    # This is native hit-testing metadata, not a second recognizer. A file
    # candidate must precede the broad default path rule for modifier-clicks,
    # while unmodified double-click selection keeps the original link order.
    link = 'src/input/Link.zig'
    add(link, 'highlight: Highlight,', 'highlight: Highlight,\nstation_file_reference: bool = false,')
    add(link, '        .highlight = self.highlight,', '        .highlight = self.highlight,\n        .station_file_reference = self.station_file_reference,')
    add(link, '    return std.meta.eql(self.action, other.action) and', '    return self.station_file_reference == other.station_file_reference and\n        std.meta.eql(self.action, other.action) and')
    surface = 'src/Surface.zig'
    add(surface, '        highlight: input.Link.Highlight,', '        highlight: input.Link.Highlight,\n        station_file_reference: bool = false,')
    add(surface, '                    .highlight = link.highlight,', '                    .highlight = link.highlight,\n                    .station_file_reference = link.station_file_reference,')
    add(surface, 'const Link = struct {\n    action: input.Link.Action,',
        'const Link = struct {\n    station_file_reference: bool = false,\n    action: input.Link.Action,')
    add(surface, '                .action = link.action,\n                .selection = sel,',
        '                .action = link.action,\n                .selection = sel,\n                .station_file_reference = link.station_file_reference,')
    add(surface, '            const resolved_path = try self.resolvePathForOpening(str);',
        '            const resolved_path = if (link.station_file_reference) null else try self.resolvePathForOpening(str);')
    anchor = '    for (self.config.links) |link| {\n        // Skip highlight/mods check when mouse_mods is null (double-click mode)'
    add(surface, anchor, '''    const passes: usize = if (mouse_mods != null) 2 else 1;
    for (0..passes) |pass| {
    const file_pass = passes == 2 and pass == 0;
    for (self.config.links) |link| {
        if (link.station_file_reference != file_pass) continue;
        // Skip highlight/mods check when mouse_mods is null (double-click mode)''')
    anchor = '    return null;\n}\n\n/// This returns the mouse mods to consider for link highlighting or'
    add(surface, anchor, '    } // ordered file/default link passes\n\n' + anchor)
    # TUI mouse reporting must not hide the terminal's explicit link chord.
    # Keep ownership for the complete physical left-button pair: over_link is
    # transient hover state and cannot safely own a press after dragging away.
    add(surface, '    left_click_count: u8 = 0,',
        '    left_click_count: u8 = 0,\n    station_link_press: bool = false,')
    add(surface, '    return self.config.mouse_reporting and',
        '    if (comptime @import("build_options").polar_tmux) {\n        if (self.mouse.station_link_press) return false;\n    }\n    return self.config.mouse_reporting and')
    add(surface, '    // This is set to true if the terminal is allowed to capture the shift', """    if (comptime @import("build_options").polar_tmux) {
        if (button == .left and action == .press) {
            self.mouse.station_link_press = false;
            if (mods.binding().equal(input.ctrlOrSuper(.{}))) {
                const pos = try self.rt_surface.getCursorPos();
                self.renderer_state.mutex.lock();
                defer self.renderer_state.mutex.unlock();
                self.mouse.station_link_press = (try self.linkAtPos(pos)) != null;
                if (self.mouse.station_link_press) self.mouse.over_link = true;
            }
        }
    }
    defer {
        if (comptime @import("build_options").polar_tmux) {
            if (button == .left and action == .release) self.mouse.station_link_press = false;
        }
    }

    // This is set to true if the terminal is allowed to capture the shift""")
    add(surface, '    // If our focus state is the same we do nothing.',
        '    if (comptime @import("build_options").polar_tmux) {\n        if (!focused) self.mouse.station_link_press = false;\n    }\n\n    // If our focus state is the same we do nothing.')
    add(surface, '        (self.io.terminal.flags.mouse_event == .none or\n            (self.mouse.mods.shift and !self.mouseShiftCapture(false))))',
        '        (self.io.terminal.flags.mouse_event == .none or\n            (@import("build_options").polar_tmux and self.mouseModsWithCapture(self.mouse.mods).equal(input.ctrlOrSuper(.{}))) or\n            (self.mouse.mods.shift and !self.mouseShiftCapture(false))))')
    if platform == 'macos':
        config = 'src/config/Config.zig'
        for suffix, code in [('b', 'D'), ('f', 'C')]:
            add(config, '.{ .esc = "' + suffix + '" },',
                '.{ .csi = "1;3' + code + '" },')
        terminal = 'macos/Sources/Features/Terminal/TerminalController.swift'
        base = 'macos/Sources/Features/Terminal/BaseTerminalController.swift'
        def hook(path, anchor, action, result='return'):
            add(path, anchor, anchor + '\n        #if POLAR_STATION\n        if PolarStationRuntime.shared.nativeAction("' + action + '", controller: self) { ' + result + ' }\n        #endif')
        for method, action in [('closeTab', 'close_tab'), ('closeOtherTabs', 'close_other_tabs'), ('closeTabsOnTheRight', 'close_right_tabs')]:
            hook(terminal, f'    @IBAction func {method}(_ sender: Any?) {{', action)
        # Reuse Ghostty's group-hash reorder observation, which already handles
        # native tab chrome frame changes that are not accessory-button KVO.
        add(terminal, '        tabWindowsHash = v\n        self.relabelTabs()',
            '        tabWindowsHash = v\n        self.relabelTabs()\n        #if POLAR_STATION\n        _ = PolarStationRuntime.shared.nativeAction("native_tab_order_changed", controller: self)\n        #endif')
        hook(base, '    func promptTabTitle() {', 'rename_tab')
        add(terminal, '    static func closeAllWindows() {', '    static func closeAllWindows() {\n        #if POLAR_STATION\n        PolarStationRuntime.shared.closeAllManagedWindows()\n        return\n        #endif')
        ghostty_app = 'macos/Sources/Ghostty/Ghostty.App.swift'
        before = '                return openURL(action.action.open_url)'
        add(ghostty_app,before,'                #if POLAR_STATION\n                if MainActor.assumeIsolated({\n                    guard target.tag == GHOSTTY_TARGET_SURFACE, let pointer = target.target.surface, let source = self.surfaceView(from: pointer) else { return false }\n                    return PolarStationRuntime.shared.openResourceFile(Ghostty.Action.OpenURL(c: action.action.open_url).url, surface: source)\n                }) { return true }\n                #endif\n'+before)

        before = '                controller.titleOverride = titleOverride'
        add(ghostty_app, before, '                #if POLAR_STATION\n                Task { @MainActor in _ = PolarStationRuntime.shared.nativeAction("rename_tab", controller: controller, title: title) }\n                return true\n                #endif\n'+before)

        before = '        // This node must be part of our tree'
        add(base, before, '        #if POLAR_STATION\n        if PolarStationRuntime.shared.closeNativeSurfaces(node.leaves(), controller: self) { return }\n        #endif\n'+before)

        anchor = '        // We can only create new splits for surfaces in our tree.'
        add(base, anchor, '#if POLAR_STATION\n        if PolarStationRuntime.shared.nativeAction("split", controller: self) { return nil }\n        #endif\n' + anchor)
        split_view = 'macos/Sources/Features/Splits/SplitView.swift'
        anchor = '    /// Calculates the bounding rect for the left view.'
        before = '            }\n    }\n\n' + anchor
        after = '            }\n            #if POLAR_STATION\n            .onEnded { _ in PolarStationRuntime.shared.nativeSplitResizeEnded() }\n            #endif\n    }\n\n' + anchor
        add(split_view, before, after)
        for method, action in [('ghosttyDidEqualizeSplits','equalize_splits'), ('ghosttyDidResizeSplit','resize_split'), ('ghosttyDidToggleSplitZoom','zoom_split')]:
            # Insert only after the notification's ownership check.
            start = f'    @objc private func {method}(_ notification: Notification) {{'
            hook(base, start, action)
        anchor = '    func performSplitAction(_ action: TerminalSplitOperation) {'
        add(base, anchor, anchor+'\n        #if POLAR_STATION\n        if case .drop = action, PolarStationRuntime.shared.nativeAction("split", controller: self) { return }\n        #endif')
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
        before = '    fn actionPolarSettings('
        add(app,before,'    fn actionPolarOpenImage(_: *gio.SimpleAction, _: ?*glib.Variant, self: *Self) callconv(.c) void { polar_tmux.openSelectedFile(self); }\n\n'+before)
        before = '            .open_url => Action.openUrl(self, value),'
        add(app,before,'            .open_url => { if (comptime @import("build_options").polar_tmux) { if (polar_tmux.openResourceFile(value.url, switch(target){ .surface=>|surface|surface.rt_surface.gobj(), .app=>null })) return true; } Action.openUrl(self, value); },')

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
    return rules
