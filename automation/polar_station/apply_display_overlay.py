#!/usr/bin/env python3
"""Install the processless display edits after the pinned platform overlay.

Exact, unique anchors are deliberate: upstream drift must fail closed. All edits
are preflighted before writing. Writes are per-file atomic and a partially
interrupted install can be resumed; this is not a multi-file transaction.
"""
from __future__ import annotations
import argparse
import hashlib
import json
import os
from pathlib import Path
import tempfile

ROOT = Path(__file__).resolve().parents[2]
PIN = "332b2aefc6e72d363aa93ab6ecfc86eeeeb5ed28"


def replace_one(text: str, before: str, after: str, label: str) -> str:
    # after can contain before (e.g. inserting a field); check the result first.
    if text.count(after) == 1:
        # A partially patched file plus a second unpatched occurrence is drift,
        # not an idempotent install. Do not silently accept duplicate targets.
        if text.count(before) != after.count(before):
            raise ValueError(f"ambiguous already-patched anchor: {label}")
        return text
    if text.count(before) != 1:
        raise ValueError(f"upstream anchor mismatch: {label}")
    return text.replace(before, after, 1)


def renderer_edits() -> dict[str, list[tuple[str, str]]]:
    """Shared production renderer corrections for both native backends."""
    out: dict[str, list[tuple[str, str]]] = {}
    def add(path: str, before: str, after: str) -> None:
        out.setdefault(path, []).append((before, after))

    generic = "src/renderer/generic.zig"
    before = "                self.terminal_state = .empty;\n            }\n            self.terminal_state_frame_count += 1;"
    add(generic, before, "                self.terminal_state = .empty;\n                self.terminal_state_frame_count = 0;\n            }\n            self.terminal_state_frame_count += 1;")
    before = "                state.mouse.point,\n                state.mouse.mods,"
    add(generic, before, "                critical.mouse.point,\n                critical.mouse.mods,")
    add(generic, "        focused: bool,", "        focused: bool,\n\n        /// Latest native visibility, combined with focus on the render thread.\n        visible: bool = true,")
    before = '''            try display_link.setOutputCallback(
                xev.Async,
                &displayLinkCallback,
                &thr.draw_now,
            );
            display_link.start() catch {};'''
    add(generic, before, before.replace("            display_link.start() catch {};", "            self.setVisible(thr.flags.visible);"))
    before = '''        pub fn setFocus(self: *Self, focus: bool) !void {
            assert(self.focused != focus);

            self.focused = focus;

            // Flag that we need to update our custom shaders
            self.custom_shader_focused_changed = true;

            // If we're not focused, then we want to stop the display link
            // because it is a waste of resources and we can move to pure
            // change-driven updates.
            if (comptime DisplayLink != void) link: {
                const display_link = self.display_link orelse break :link;
                if (focus) {
                    display_link.start() catch {};
                } else {
                    display_link.stop() catch {};
                }
            }
        }

        /// Callback when the window is visible or occluded.
        ///
        /// Must be called on the render thread.
        pub fn setVisible(self: *Self, visible: bool) void {
            // If we're not visible, then we want to stop the display link
            // because it is a waste of resources and we can move to pure
            // change-driven updates.
            if (comptime DisplayLink != void) link: {
                const display_link = self.display_link orelse break :link;
                if (visible and self.focused) {
                    display_link.start() catch {};
                } else {
                    display_link.stop() catch {};
                }
            }
        }'''
    add(generic, before, '''        pub fn setFocus(self: *Self, focus: bool) !void {
            assert(self.focused != focus);
            self.focused = focus;
            self.custom_shader_focused_changed = true;
            self.syncDisplayLink();
        }

        /// Callback when the window is visible or occluded.
        /// Must be called on the render thread, independently of focus.
        pub fn setVisible(self: *Self, visible: bool) void {
            self.visible = visible;
            self.syncDisplayLink();
        }

        fn syncDisplayLink(self: *Self) void {
            if (comptime DisplayLink == void) return;
            const display_link = self.display_link orelse return;
            const should_run = self.visible and self.focused;
            if (should_run == display_link.isRunning()) return;
            if (should_run) {
                display_link.start() catch {};
            } else {
                display_link.stop() catch {};
            }
        }''')
    before = '''        try render_state.string(&builder.writer, .{
            .alloc = alloc,
            .map = &map,
        });

        const str = builder.writer.buffered();'''
    add("src/renderer/link.zig", before, "        var prepared = false;")
    before = "            var offset: usize = 0;\n            while (offset < str.len) {"
    add("src/renderer/link.zig", before, '''            // Preserve the existing policy checks above. Most frames have
            // no eligible hover rule; only materialize text for a real match.
            if (!prepared) {
                try render_state.string(&builder.writer, .{
                    .alloc = alloc,
                    .map = &map,
                });
                prepared = true;
            }
            const str = builder.writer.buffered();

''' + before)

    # Cursor modes hide the display cursor but do not stop Ghostty's timer.
    # Reuse the immutable construction capability at both timer start sites;
    # native focus and all other renderer wakeups keep their upstream meaning.
    renderer = "src/renderer/Thread.zig"
    add(renderer, "    // Start blinking the cursor.\n    self.cursor_h.run(",
        "    // Processless displays have no terminal cursor.\n    if (self.cursorBlinkAllowed()) self.cursor_h.run(")
    add(renderer, "                    if (self.cursor_c.state() != .active) {",
        "                    if (self.cursorBlinkAllowed() and self.cursor_c.state() != .active) {")
    before = "/// Returns the interval for the blinking cursor in milliseconds."
    add(renderer, before, '''// This capability is set before renderer-thread construction and never changes.
fn cursorBlinkAllowed(self: *const Thread) bool {
    if (comptime @hasDecl(apprt.Surface, "stationDisplayOnly")) {
        return !self.surface.stationDisplayOnly();
    }
    return true;
}

''' + before)

    return out


def edits() -> dict[str, list[tuple[str, str]]]:
    out = renderer_edits()
    def add(path: str, before: str, after: str) -> None:
        out.setdefault(path, []).append((before, after))

    backend = "src/termio/backend.zig"
    add(backend, "pub const Kind = enum { exec };", "pub const Kind = enum { exec, display };")
    add(backend, "    exec: termio.Exec.Config,", "    exec: termio.Exec.Config,\n    display: void,")
    add(backend, "    exec: termio.Exec,", "    exec: termio.Exec,\n    display: @import(\"StationDisplay.zig\").State,")
    for before, after in [
        (".exec => |*exec| exec.deinit(),", ".exec => |*exec| exec.deinit(),\n            .display => {},"),
        (".exec => |*exec| exec.initTerminal(t),", ".exec => |*exec| exec.initTerminal(t),\n            .display => {\n                t.modes.set(.cursor_visible, false);\n                t.modes.set(.cursor_blinking, false);\n            },"),
        (".exec => |*exec| try exec.threadEnter(alloc, io, td),", ".exec => |*exec| try exec.threadEnter(alloc, io, td),\n            .display => td.backend = .{ .display = {} },"),
        (".exec => |*exec| exec.threadExit(td),", ".exec => |*exec| exec.threadExit(td),\n            .display => {},"),
        (".exec => |*exec| try exec.focusGained(td, focused),", ".exec => |*exec| try exec.focusGained(td, focused),\n            .display => {},"),
        (".exec => |*exec| try exec.resize(grid_size, screen_size),", ".exec => |*exec| try exec.resize(grid_size, screen_size),\n            .display => {},"),
        (".exec => |*exec| try exec.queueWrite(alloc, td, data, linefeed),", ".exec => |*exec| try exec.queueWrite(alloc, td, data, linefeed),\n            // Native widgets own input; a display never echoes terminal input.\n            .display => {},"),
        ("                runtime_ms,\n            ),", "                runtime_ms,\n            ),\n            .display => {},"),
        ("    exec: termio.Exec.ThreadData,", "    exec: termio.Exec.ThreadData,\n    display: void,"),
        (".exec => |*exec| exec.deinit(alloc),", ".exec => |*exec| exec.deinit(alloc),\n            .display => {},"),
    ]: add(backend, before, after)

    surface = "src/Surface.zig"
    before = "    // The command we're going to execute\n"
    add(surface, before, '''    // A display is an explicit native construction choice, never a command.
    const station_display = if (comptime @hasDecl(apprt.runtime.Surface, "stationDisplayOnly"))
        rt_surface.stationDisplayOnly()
    else
        false;

''' + before)
    before = '''        var env = rt_surface.defaultTermioEnv() catch |err| env: {
            // If an error occurs, we don't want to block surface startup.
            log.warn("error getting env map for surface err={}", .{err});
            break :env internal_os.getEnvMap(alloc) catch
                std.process.EnvMap.init(alloc);
        };
        errdefer env.deinit();

        // don't leak GHOSTTY_LOG to any subprocesses
        env.remove("GHOSTTY_LOG");

        // Initialize our IO backend
        var io_exec = try termio.Exec.init(alloc, .{
            .command = command,
            .env = env,
            .env_override = config.env,
            .shell_integration = config.@"shell-integration",
            .shell_integration_features = config.@"shell-integration-features",
            .cursor_blink = config.@"cursor-style-blink",
            .working_directory = if (config.@"working-directory") |wd| wd.value() else null,
            .resources_dir = global_state.resources_dir.host(),
            .term = config.term,
            .rt_pre_exec_info = .init(config),
            .rt_post_fork_info = .init(config),
        });
        errdefer io_exec.deinit();'''
    after = '''        var io_backend: termio.Backend = if (station_display)
            .{ .display = .init() }
        else backend: {
            var env = rt_surface.defaultTermioEnv() catch |err| env: {
                log.warn("error getting env map for surface err={}", .{err});
                break :env internal_os.getEnvMap(alloc) catch
                    std.process.EnvMap.init(alloc);
            };
            errdefer env.deinit();
            env.remove("GHOSTTY_LOG");
            break :backend .{ .exec = try termio.Exec.init(alloc, .{
                .command = command,
                .env = env,
                .env_override = config.env,
                .shell_integration = config.@"shell-integration",
                .shell_integration_features = config.@"shell-integration-features",
                .cursor_blink = config.@"cursor-style-blink",
                .working_directory = if (config.@"working-directory") |wd| wd.value() else null,
                .resources_dir = global_state.resources_dir.host(),
                .term = config.term,
                .rt_pre_exec_info = .init(config),
                .rt_post_fork_info = .init(config),
            }) };
        };
        errdefer io_backend.deinit();'''
    add(surface, before, after)
    add(surface, "            .backend = .{ .exec = io_exec },", "            .backend = io_backend,")
    # The error-report path has no command for a processless display.
    add(surface, "        .exec => |*exec| exec.subprocess.args,", "        .exec => |*exec| exec.subprocess.args,\n        .display => &.{},")

    termio = "src/termio/Termio.zig"
    before = '''    const thread_enter_state = try ThreadEnterState.create(
        alloc,
        opts.full_config,
    );'''
    add(termio, before, '''    const thread_enter_state = switch (backend) {
        .display => null, // Never load shell initial-input files for an LGP display.
        .exec => try ThreadEnterState.create(alloc, opts.full_config),
    };''')
    before = "pub fn processOutput(self: *Termio, buf: []const u8) void {"
    add(termio, before, '''/// Native-main-thread-only display admission. The ordinary input path is
/// intentionally separate. Allocation/validation finish before parser mutation.
/// Success means accepted by the terminal parser, not a GPU-present receipt.
pub fn stationDisplayWrite(self: *Termio, operation_raw: u32, text: []const u8) !void {
    const display = @import("StationDisplay.zig");
    switch (self.backend) {
        .exec => return error.NotDisplaySurface,
        .display => |state| try state.checkOwner(),
    }
    const operation = std.meta.intToEnum(display.Operation, operation_raw) catch
        return error.InvalidDisplayOperation;
    // Snapshot actual theme colors under the same renderer mutex. Complete
    // validation/encoding before modifying the terminal parser or its grid.
    self.renderer_state.mutex.lock();
    defer self.renderer_state.mutex.unlock();
    const fg: terminalpkg.color.RGB = self.terminal.colors.foreground.get() orelse .{ .r = 216, .g = 216, .b = 216 };
    const bg: terminalpkg.color.RGB = self.terminal.colors.background.get() orelse .{ .r = 32, .g = 32, .b = 32 };
    const bytes = try display.encodeThemed(self.alloc, operation, text, .{ fg.r, fg.g, fg.b }, .{ bg.r, bg.g, bg.b });
    defer self.alloc.free(bytes);
    const screen = self.terminal.screens.active;
    const reading = screen.pages.viewport != .active;
    const offset = screen.pages.scrollbar().offset;
    if (operation == .replace_window or operation == .replace_runs) try screen.select(null);
    self.processOutputLocked(bytes);
    if (reading) screen.pages.scroll(.{ .row = offset });
}

''' + before)

    embedded = "src/apprt/embedded.zig"
    add(embedded, "    inspector: ?*Inspector = null,", "    inspector: ?*Inspector = null,\n    station_display_only: bool = false,")
    before = "    pub fn init(self: *Surface, app: *App, opts: Options) !void {\n        self.* = .{"
    add(embedded, before, '''    pub fn init(self: *Surface, app: *App, opts: Options) !void {
        return self.initWithMode(app, opts, false);
    }

    pub fn stationDisplayOnly(self: *const Surface) bool {
        return self.station_display_only;
    }

    fn initWithMode(self: *Surface, app: *App, opts: Options, display_only: bool) !void {
        self.* = .{
            .station_display_only = display_only,''')
    before = "    export fn ghostty_surface_free(ptr: *Surface) void {"
    add(embedded, before, '''    // Additive API: do not alter the layout of Surface.Options / surface_config_s.
    export fn ghostty_surface_new_display(app: *App, opts: *const apprt.Surface.Options) ?*Surface {
        const surface = app.core_app.alloc.create(Surface) catch return null;
        surface.initWithMode(app, opts.*, true) catch |err| {
            app.core_app.alloc.destroy(surface);
            log.err("error initializing Station display err={}", .{err});
            return null;
        };
        return surface;
    }

    // Thin geometry adapter: reuse the core's actual padding and content scale.
    export fn ghostty_surface_display_geometry(surface: *Surface, x: *f64, y: *f64, cw: *f64, ch: *f64) bool {
        if (!surface.station_display_only) return false;
        const scale = surface.getContentScale() catch return false;
        const size = surface.core_surface.size;
        x.* = @as(f64, @floatFromInt(size.padding.left)) / scale.x;
        y.* = @as(f64, @floatFromInt(size.padding.top)) / scale.y;
        cw.* = @as(f64, @floatFromInt(size.cell.width)) / scale.x;
        ch.* = @as(f64, @floatFromInt(size.cell.height)) / scale.y;
        return cw.* > 0 and ch.* > 0;
    }

    export fn ghostty_surface_display_text(surface: *Surface, operation: u32, text: ?[*]const u8, len: usize) bool {
        if (!surface.station_display_only or (text == null and len != 0)) return false;
        // Bound length before forming a foreign-memory slice.
        if (len > @import("../termio/StationDisplay.zig").max_text_bytes) return false;
        surface.core_surface.io.stationDisplayWrite(operation, if (text) |p| p[0..len] else "") catch return false;
        return true;
    }

''' + before)

    add(surface, "pub fn needsConfirmQuit(self: *Surface) bool {", '''pub fn needsConfirmQuit(self: *Surface) bool {
    // Draft/unsent-message confirmation belongs to Station, not shell exit.
    switch (self.io.backend) {
        .display => return false,
        .exec => {},
    }''')

    add("macos/Ghostty.xcodeproj/project.pbxproj",
        "\t\t\t\tPolarStation/PolarStationPaneSurfaceCoordinator.swift,",
        "\t\t\t\tPolarStation/PolarStationPaneSurfaceCoordinator.swift,\n\t\t\t\tPolarStation/PolarStationDisplaySurface.swift,\n\t\t\t\tPolarStation/PolarStationLgpWorkspace.swift,\n\t\t\t\tPolarStation/PolarStationInputBar.swift,")

    header = "include/ghostty.h"
    add(header, "void ghostty_surface_free(ghostty_surface_t);", '''// Station additive API; surface_config_s remains binary-compatible.
// Creator/main thread only. 1=replace bounded window, 2=append plain UTF-8.
// true is parser admission, not a renderer/GPU completion acknowledgement.
ghostty_surface_t ghostty_surface_new_display(ghostty_app_t,
                                             const ghostty_surface_config_s*);
bool ghostty_surface_display_geometry(ghostty_surface_t, double*, double*, double*, double*);
bool ghostty_surface_display_text(ghostty_surface_t, uint32_t,
                                  const uint8_t*, uintptr_t);
void ghostty_surface_free(ghostty_surface_t);''')

    swift = "macos/Sources/Ghostty/Surface View/SurfaceView_AppKit.swift"
    # Native composers/search fields own their editing shortcuts even when a
    # window activation has reasserted Ghostty's cached surface focus.
    add(swift, "        override func performKeyEquivalent(with event: NSEvent) -> Bool {", """        override func performKeyEquivalent(with event: NSEvent) -> Bool {
            #if POLAR_STATION
            guard window?.firstResponder === self else { return false }
            #endif""")
    add(swift, "        private func localEventKeyUp(_ event: NSEvent) -> NSEvent? {", """        private func localEventKeyUp(_ event: NSEvent) -> NSEvent? {
            #if POLAR_STATION
            guard let window, window.isKeyWindow, window.firstResponder === self else { return event }
            #endif""")
    mac_controller = "macos/Sources/Features/Terminal/BaseTerminalController.swift"
    before_focus = "            let focused: Bool = (window?.isKeyWindow ?? false) &&"
    add(mac_controller, before_focus, """            #if POLAR_STATION
            // A selected Pane is not necessarily the keyboard owner: the native
            // composer or a search field may be the actual first responder.
            let responderOwnsSurface = (window?.firstResponder as? NSView)?.isDescendant(of: surfaceView) == true
            #else
            let responderOwnsSurface = true
            #endif
            let focused: Bool = responderOwnsSurface && (window?.isKeyWindow ?? false) &&""")
    add(swift,
        "init(_ app: ghostty_app_t, baseConfig: SurfaceConfiguration? = nil, uuid: UUID? = nil) {",
        "init(_ app: ghostty_app_t, baseConfig: SurfaceConfiguration? = nil, uuid: UUID? = nil, stationDisplayOnly: Bool = false) {\n            self.stationIsDisplayOnly = stationDisplayOnly")
    add(swift, "                ghostty_surface_new(app, &surface_cfg_c)", '''                stationDisplayOnly
                    ? ghostty_surface_new_display(app, &surface_cfg_c)
                    : ghostty_surface_new(app, &surface_cfg_c)''')
    # Read-only displays must receive the first native selection gesture even
    # when the shared composer held the keyboard focus. Terminals keep their
    # existing focus-only click suppression.
    add(swift, "        private var suppressNextLeftMouseUp: Bool = false", "        private var suppressNextLeftMouseUp: Bool = false\n        private var stationIsDisplayOnly = false")
    add(swift, "if NSApp.isActive && window.isKeyWindow {", "if NSApp.isActive && window.isKeyWindow && !stationIsDisplayOnly {")
    # Ghostty consumes focus-transfer mouse-down before other NSEvent monitors
    # may see it. Forward the hit-tested event at its actual owner first.
    add(swift, "            guard hitTest(location) == self else { return event }", '''            guard hitTest(location) == self else { return event }
            #if POLAR_STATION
            NotificationCenter.default.post(
                name: .init("PolarStationSurfacePointerDown"), object: self,
                userInfo: ["event": event])
            #endif''')

    mac_app = "macos/Sources/App/macOS/AppDelegate.swift"
    for action, item, key, flags in [
        ("copy_to_clipboard", "menuCopy", "c", ".command"),
        ("paste_from_clipboard", "menuPaste", "v", ".command"),
        ("select_all", "menuSelectAll", "a", ".command"),
        ("undo", "menuUndo", "z", ".command"),
        ("redo", "menuRedo", "z", ".command, .shift"),
    ]:
        call = f'        syncMenuShortcut(config, action: "{action}", menuItem: self.{item})'
        add(mac_app, call, f"""        #if POLAR_STATION
        // Native editors use AppKit's responder chain, not terminal bindings.
        self.{item}?.keyEquivalent = "{key}"
        self.{item}?.keyEquivalentModifierMask = [{flags}]
        #else
{call}
        #endif""")
    anchor = "        // Reset our shortcut index since we're about to rebuild all menu bindings."
    add(mac_app, anchor, """        #if POLAR_STATION
        if let copy = menuCopy, let edit = copy.menu,
           !edit.items.contains(where: { $0.action == #selector(NSText.cut(_:)) }) {
            let cut = NSMenuItem(title: "Cut", action: #selector(NSText.cut(_:)), keyEquivalent: "x")
            cut.keyEquivalentModifierMask = [.command]
            edit.insertItem(cut, at: edit.index(of: copy))
        }
        #endif
""" + anchor)

    mac_container = "macos/Sources/Features/Terminal/TerminalViewContainer.swift"
    add(mac_container, "    private var polarStationPane: NSView?", """    private var polarStationPane: NSView?
    private var stationComposer: NSView?
    private var stationComposerConstraints: [NSLayoutConstraint] = []
    private var stationComposerHeight: NSLayoutConstraint?
    private var terminalBottomConstraint: NSLayoutConstraint!

    func showStationComposer(_ composer: NSView, height: CGFloat) {
        if stationComposer === composer {
            if abs((stationComposerHeight?.constant ?? 0) - height) > 0.5 {
                stationComposerHeight?.constant = height
                needsLayout = true
            }
            return
        }
        hideStationComposer()
        stationComposer = composer
        composer.translatesAutoresizingMaskIntoConstraints = false
        addSubview(composer)
        terminalBottomConstraint.isActive = false
        let size = composer.heightAnchor.constraint(equalToConstant: height)
        stationComposerHeight = size
        stationComposerConstraints = [
            composer.leadingAnchor.constraint(equalTo: terminalView.leadingAnchor),
            composer.trailingAnchor.constraint(equalTo: trailingAnchor),
            composer.bottomAnchor.constraint(equalTo: bottomAnchor), size,
            terminalView.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -40),
        ]
        NSLayoutConstraint.activate(stationComposerConstraints)
        needsLayout = true
    }

    func hideStationComposer() {
        NSLayoutConstraint.deactivate(stationComposerConstraints)
        stationComposerConstraints = []
        stationComposerHeight = nil
        stationComposer?.removeFromSuperview()
        stationComposer = nil
        terminalBottomConstraint?.isActive = true
        needsLayout = true
    }""")
    add(mac_container, """        NSLayoutConstraint.activate([
            terminalView.topAnchor.constraint(equalTo: topAnchor),""", """        terminalBottomConstraint = terminalView.bottomAnchor.constraint(equalTo: bottomAnchor)
        NSLayoutConstraint.activate([
            terminalView.topAnchor.constraint(equalTo: topAnchor),""")
    add(mac_container, "            terminalView.bottomAnchor.constraint(equalTo: bottomAnchor),", "            terminalBottomConstraint,")

    gtk = "src/apprt/gtk/Surface.zig"
    add(gtk, "surface: *Surface,", '''surface: *Surface,
// Selected by the native constructor before the GLArea is realized.
station_display_only: bool = false,

pub fn stationDisplayOnly(self: *const Self) bool {
    return self.station_display_only;
}''')
    gtk_class = "src/apprt/gtk/class/surface.zig"
    add(gtk_class, "if (!had_focus and button == .left) {", "if (!had_focus and button == .left and !priv.rt_surface.stationDisplayOnly()) {")
    add(gtk_class, "        focused: bool = true,", "        focused: bool = true,\n        station_visibility_pending: bool = false,")
    before = "        priv.mouse_hidden = false;"
    add(gtk_class, before, before + '''
        priv.station_visibility_pending = false;
        if (comptime @import("build_options").polar_tmux) {
            _ = gtk.Widget.signals.map.connect(self.as(gtk.Widget), *Self, stationVisibilityChanged, self, .{});
            _ = gtk.Widget.signals.unmap.connect(self.as(gtk.Widget), *Self, stationVisibilityChanged, self, .{});
        }''')
    before = "    fn ecFocusEnter(_: *gtk.EventControllerFocus, self: *Self) callconv(.c) void {"
    add(gtk_class, before, '''    fn stationVisibilityChanged(_: *gtk.Widget, self: *Self) callconv(.c) void {
        self.stationQueueVisibility();
    }

    fn stationQueueVisibility(self: *Self) void {
        const priv = self.private();
        if (priv.station_visibility_pending) return;
        priv.station_visibility_pending = true;
        // Match the existing deferred focus boundary: GTK layout may be
        // changing while libghostty owns a lock. One ref owns this delivery.
        _ = glib.idleAddOnce(stationPublishVisibility, self.ref());
    }

    fn stationPublishVisibility(ud: ?*anyopaque) callconv(.c) void {
        const self: *Self = @ptrCast(@alignCast(ud orelse return));
        defer self.unref();
        const priv = self.private();
        priv.station_visibility_pending = false;
        const surface = priv.core_surface orelse return;
        surface.occlusionCallback(self.as(gtk.Widget).getMapped() != 0) catch |err| {
            log.warn("error in visibility callback err={}", .{err});
        };
    }

''' + before)
    before = "            _ = glib.idleAddOnce(idleFocus, self.ref());\n        }\n\n        // Emit the signal that we initialized the surface."
    add(gtk_class, before, "            _ = glib.idleAddOnce(idleFocus, self.ref());\n            self.stationQueueVisibility();\n        }\n\n        // Emit the signal that we initialized the surface.")
    before = '''    pub fn new(overrides: struct {
        command: ?configpkg.Command = null,
        working_directory: ?[:0]const u8 = null,
        title: ?[:0]const u8 = null,'''
    add(gtk_class, before, before + "\n        station_display_only: bool = false,")
    before = '''        const alloc = Application.default().allocator();
        const priv: *Private = self.private();
        priv.overrides = .{'''
    add(gtk_class, before, '''        const alloc = Application.default().allocator();
        const priv: *Private = self.private();
        priv.rt_surface.station_display_only = overrides.station_display_only;
        priv.overrides = .{''')
    before = "    pub fn core(self: *Self) ?*CoreSurface {"
    display_methods = '''    /// Must be called on the GTK thread, after the init signal/realization.
    /// No buffering here: the owner retains bounded view state and can retry.
    pub fn stationDisplayText(self: *Self, operation: u32, text: []const u8) !void {
        if (!self.private().rt_surface.station_display_only) return error.NotDisplaySurface;
        const value = self.core() orelse return error.SurfaceNotReady;
        try value.io.stationDisplayWrite(operation, text);
    }

'''
    add(gtk_class, "        terminal_page: *gtk.Overlay,", "        terminal_page: *gtk.Overlay,\n        station_external_widget: ?*gtk.Widget = null,\n        station_external_focus: ?*gtk.EventControllerFocus = null,")
    before = "    pub fn core(self: *Self) ?*CoreSurface {"
    add(gtk_class, before, display_methods + """    /// Mount before realization. The hidden terminal GLArea never initializes a core.
    pub fn stationSetExternalWidget(self: *Self, widget: *gtk.Widget) void {
        const priv = self.private();
        std.debug.assert(priv.core_surface == null);
        priv.station_external_widget = widget;
        const focus = gtk.EventControllerFocus.new();
        priv.station_external_focus = focus;
        _ = gtk.EventControllerFocus.signals.enter.connect(focus, *Self, ecFocusEnter, self, .{});
        _ = gtk.EventControllerFocus.signals.leave.connect(focus, *Self, ecFocusLeave, self, .{});
        widget.addController(focus.as(gtk.EventController));
        priv.gl_area.as(gtk.Widget).setVisible(0);
        widget.setHalign(.fill);
        widget.setValign(.fill);
        priv.terminal_page.addOverlay(widget);
        priv.terminal_page.setMeasureOverlay(widget, 1);
    }

    pub fn stationRetireExternalWidget(self: *Self) void {
        const priv = self.private();
        if (priv.station_external_widget) |widget| {
            priv.station_external_widget = null;
            if (priv.station_external_focus) |focus| {
                widget.removeController(focus.as(gtk.EventController));
                priv.station_external_focus = null;
            }
            priv.terminal_page.removeOverlay(widget);
        }
    }

""" + before)
    before = """    pub fn grabFocus(self: *Self) void {
        const priv = self.private();
        _ = priv.gl_area.as(gtk.Widget).grabFocus();
    }"""
    add(gtk_class, before, """    pub fn grabFocus(self: *Self) void {
        const priv = self.private();
        _ = (priv.station_external_widget orelse priv.gl_area.as(gtk.Widget)).grabFocus();
    }""")
    # Native Station may supply a processless first Surface. Reuse the exact
    # Window/Tab setup rather than manufacturing a shell and replacing it.
    tab = "src/apprt/gtk/class/tab.zig"
    before = """    pub fn new(config: ?*Config, overrides: struct {
        command: ?configpkg.Command = null,"""
    add(tab, before, before + "\n        station_surface: ?*Surface = null,")
    before = """        priv.split_tree.newSplit(.right, null, .{
            .command = overrides.command,"""
    add(tab, before, before + "\n            .station_surface = overrides.station_surface,")
    # Reuse native split construction: reference ownership, is-split binding,
    # active-surface history and tree notifications are one operation.
    split = "src/apprt/gtk/class/split_tree.zig"
    before = """        overrides: struct {
            command: ?configpkg.Command = null,"""
    add(split, before, before + "\n            station_surface: ?*Surface = null,")
    before = """        const surface: *Surface = .new(.{
            .command = overrides.command,"""
    add(split, before, """        const surface: *Surface = overrides.station_surface orelse Surface.new(.{
            .command = overrides.command,""")
    window = "src/apprt/gtk/class/window.zig"
    before = """    fn windowCloseRequest(
        _: *gtk.Window,
        self: *Self,
    ) callconv(.c) c_int {"""
    add(window, before, before + """
        if (comptime build_options.polar_tmux) {
            if (@import("../polar_tmux/main.zig").closeManagedWindow(self)) return 1;
        }""")

    before = """    fn init(self: *Self, _: *Class) callconv(.c) void {
        gtk.Widget.initTemplate(self.as(gtk.Widget));"""
    add(window, before, before + """
        if (comptime build_options.polar_tmux)
            @import("../polar_tmux/lgp_workspace.zig").prepareWindow(self);""")
    for name, prefix in (("newTabForWindow", "    pub fn newTabForWindow("), ("newTabPage", "    fn newTabPage(")):
        before = prefix + """
        self: *Self,
        parent_: ?*CoreSurface,
""" + ("        context: apprt.surface.NewSurfaceContext,\n" if name == "newTabPage" else "") + """        overrides: struct {
            command: ?configpkg.Command = null,"""
        add(window, before, before + "\n            station_surface: ?*Surface = null,")
    for before in ("""        _ = self.newTabPage(
            parent_,
            .window,
            .{
                .command = overrides.command,""", """        const tab = Tab.new(
            priv.config,
            .{
                .command = overrides.command,"""):
        add(window, before, before + "\n                .station_surface = overrides.station_surface,")
    app = "src/apprt/gtk/class/application.zig"
    add(app, '                .init("polar-settings", actionPolarSettings, null),',
        '                .init("polar-settings", actionPolarSettings, null),\n                .init("polar-lgp", actionPolarLgp, null),\n                .init("polar-idea-tree", actionPolarIdeaTree, null),\n                .init("polar-resume-codex", actionPolarResumeCodex, null),')
    before = """    fn actionPolarSettings(
        _: *gio.SimpleAction,
        _: ?*glib.Variant,
        self: *Self,
    ) callconv(.c) void {
        polar_tmux.showSettings(self);
    }"""
    add(app, before, before + """

    fn actionPolarLgp(
        _: *gio.SimpleAction,
        _: ?*glib.Variant,
        self: *Self,
    ) callconv(.c) void {
        polar_tmux.showLgp(self);
    }

    fn actionPolarIdeaTree(_: *gio.SimpleAction, _: ?*glib.Variant, self: *Self) callconv(.c) void {
        polar_tmux.showIdeaTree(self);
    }

    fn actionPolarResumeCodex(_: *gio.SimpleAction, _: ?*glib.Variant, self: *Self) callconv(.c) void {
        polar_tmux.dispatchShortcut(self, .resume_codex);
    }""")
    add(window, '            station_menu.append("Settings…", "app.polar-settings");',
        '            station_menu.append("Settings…", "app.polar-settings");\n            station_menu.append("Open Polar Conversation…", "app.polar-lgp");\n            var idea_label: [160]u8 = undefined;\n            station_menu.append(polar_tmux.shortcutMenuLabel(12, &idea_label), "app.polar-idea-tree");\n            var resume_label: [160]u8 = undefined;\n            station_menu.append(polar_tmux.shortcutMenuLabel(13, &resume_label), "app.polar-resume-codex");')
    return out


def plan(source: Path, platform: str) -> dict[Path, bytes]:
    source = source.resolve(strict=True)
    reference = Path('/home/chris/dev/reference')
    if source == reference or reference in source.parents:
        raise ValueError('refusing to modify an upstream reference tree')
    rules = edits()
    if platform == 'linux':
        before = "    try result.loadRecursiveFiles(alloc_gpa);\n    try result.finalize();"
        after = """    try result.loadRecursiveFiles(alloc_gpa);
    if (comptime @import("build_options").polar_tmux) {
        if (std.posix.getenv("POLAR_STATION_THEME_CONFIG")) |path|
            try result.loadFile(alloc_gpa, path);
    }
    try result.finalize();"""
        rules.setdefault('src/config/Config.zig', []).append((before, after))
    else:
        controller_path = "macos/Sources/Features/Terminal/TerminalController.swift"
        for before in ("    override func windowShouldClose(_ sender: NSWindow) -> Bool {",
                       "    @IBAction override func closeWindow(_ sender: Any?) {"):
            # Native titlebar and explicit Close Window converge before Ghostty's
            # terminal termination confirmation; only Station-owned views qualify.
            hook = ("\n        #if POLAR_STATION\n"
                    + ("        if PolarStationRuntime.shared.nativeAction(\"close_tab\", controller: self) { " if "windowShouldClose" in before else "        if let managedWindow = self.window, PolarStationRuntime.shared.closeManagedWindow(managedWindow) { ")
                    + ("return false" if "windowShouldClose" in before else "return")
                    + " }\n        #endif")
            rules.setdefault(controller_path, []).append((before, before + hook))
        before = "            ghostty_config_load_recursive_files(config)"
        # Pinned upstream uses cfg, verified when applying the exact overlay.
        path = 'macos/Sources/Ghostty/Ghostty.Config.swift'
        upstream = (source / path).read_text()
        line = next(x for x in upstream.splitlines() if 'ghostty_config_load_recursive_files(' in x)
        variable = line.split('ghostty_config_load_recursive_files(')[1].split(')')[0]
        after = line + '\n            if let themePath = getenv("POLAR_STATION_THEME_CONFIG") {\n                ghostty_config_load_file(' + variable + ', themePath)\n            }'
        rules.setdefault(path, []).append((line, after))
    import sys
    sys.path.insert(0, str(ROOT))
    from automation.polar_station.native_action_overlay import edits as native_action_edits
    for path, replacements in native_action_edits(platform).items():
        rules.setdefault(path, []).extend(replacements)
    # GTK uses the same core adapter; embedded is built on macOS only.
    if platform == 'linux':
        for path in ('src/apprt/embedded.zig', 'include/ghostty.h',
                     'macos/Ghostty.xcodeproj/project.pbxproj',
                     'macos/Sources/Ghostty/Surface View/SurfaceView_AppKit.swift',
                     'macos/Sources/Features/Terminal/TerminalViewContainer.swift'):
            rules.pop(path)
    else:
        for path in list(rules):
            if path.startswith('src/apprt/gtk/'):
                rules.pop(path)
    changes = {}
    for rel, replacements in rules.items():
        path = source / rel
        if path.is_symlink() or not path.resolve().is_relative_to(source):
            raise ValueError(f'unsafe source path: {rel}')
        text = path.read_text()
        for i, (before, after) in enumerate(replacements):
            text = replace_one(text, before, after, f'{rel}:{i}')
        changes[path] = text.encode()
    path = source / 'src/termio/StationDisplay.zig'
    data = (ROOT / 'apps/polar-tmux-shared/ghostty/StationDisplay.zig').read_bytes()
    if path.is_symlink() or not path.resolve().is_relative_to(source):
        raise ValueError('unsafe StationDisplay destination')
    if path.exists() and path.read_bytes() != data:
        raise ValueError('existing StationDisplay differs; rebuild from clean pinned upstream')
    changes[path] = data
    return changes


def install(source: Path, platform: str, check: bool = False) -> dict:
    changes = plan(source, platform)
    changed = []
    for path, data in changes.items():
        if path.exists() and path.read_bytes() == data:
            continue
        changed.append(str(path.relative_to(source.resolve())))
        if check: continue
        fd, tmp = tempfile.mkstemp(prefix='.station-display-', dir=path.parent)
        try:
            with os.fdopen(fd, 'wb') as f:
                f.write(data); f.flush(); os.fsync(f.fileno())
            os.chmod(tmp, 0o644)
            os.replace(tmp, path)
        finally:
            if os.path.exists(tmp): os.unlink(tmp)
    return {'expected_upstream_commit': PIN, 'platform': platform, 'check_only': check,
            'changed': changed, 'files': {str(p.relative_to(source.resolve())): hashlib.sha256(b).hexdigest() for p,b in changes.items()},
            'qualification': 'source overlay installation, not compilation or renderer execution'}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, required=True)
    parser.add_argument('--platform', choices=['macos', 'linux'], required=True)
    parser.add_argument('--check', action='store_true')
    args = parser.parse_args()
    try:
        print(json.dumps(install(args.source, args.platform, args.check), indent=2))
        return 0
    except (ValueError, OSError) as exc:
        parser.exit(2, str(exc) + '\n')

if __name__ == '__main__':
    raise SystemExit(main())
