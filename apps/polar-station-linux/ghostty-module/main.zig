//! Polar Station lifecycle hook compiled only into the managed Ghostty build.
//!
//! The native adapter owns Workspace behavior and Ghostty GTK reconciliation;
//! Rust owns Session runtime, terminal bytes, transport recovery, and relays.

const std = @import("std");
const adw = @import("adw");
const gio = @import("gio");
const glib = @import("glib");
const gobject = @import("gobject");
const gtk = @import("gtk");
const gdk = @import("gdk");
const Application = @import("../class/application.zig").Application;
const Window = @import("../class/window.zig").Window;
const application_refresh = @import("application_refresh.zig");
const controller_model = @import("control_center/model.zig");
const control_center_intent = @import("control_center/intent.zig");
const controller_pane = @import("controller_pane.zig");
const host_frame_batch = @import("host_frame_batch.zig");
const ipc_client = @import("ipc_client.zig");
const attention_interaction = @import("attention_interaction.zig");
const inbox = @import("inbox.zig");
const codex_window = @import("codex_window.zig");
const connector_window = @import("connector_window.zig");
const presentation_executor = @import("presentation_executor.zig");
const lgp_workspace = @import("lgp_workspace.zig");
const StationContent = @import("station_content.zig").Content;
// Public native content factory. No terminal Session is synthesized.
pub const DisplaySurface = @import("display_surface.zig").DisplaySurface;
const protocol = @import("protocol.zig");
const resources = @import("resources.zig");
const resources_window = @import("resources_window.zig");
const settings = @import("settings.zig");
const station_theme = @import("theme.zig");
const settings_window = @import("settings_window.zig");
const session_runtime = @import("session_runtime.zig");
const shortcuts = @import("shortcuts.zig");
const workspace_relay = @import("workspace_relay.zig");

const PendingControlEffectKind = enum {
    connection_update,
    ssh_target,
    session_create,
    quick_create,
    session_rename,
    session_kill,
    session_editor,
    session_replacement,
    session_retirement,
};

const PendingControlEffect = struct {
    kind: PendingControlEffectKind,
    controller_id: u64,
    selection_json: ?[]u8 = null,
    end_token: u64 = 0,
};

const BackendState = enum {
    starting,
    connecting,
    ready,
    unavailable,
};

const State = struct {
    allocator: std.mem.Allocator,
    app: *Application,
    frontend_instance_id: []u8,
    frontend_generation: u64,
    client: ipc_client.Client,
    executor: presentation_executor.Executor,
    window_executors: std.ArrayListUnmanaged(*presentation_executor.Executor) = .empty,
    lgp: ?*lgp_workspace.Workspace = null,
    controller_model: controller_model.Model,
    session_cache: session_runtime.Cache,
    attention_engine: attention_interaction.Engine = .{},
    resources_model: resources.Model,
    resources_window: resources_window.Window,
    theme: station_theme.Controller,
    settings_model: settings.Model,
    settings_window: settings_window.Window,
    inbox_model: inbox.Model,
    inbox_refresh: application_refresh.Gate = .{},
    inbox_refresh_request_id: ?[]u8 = null,
    codex_window: codex_window.Window,
    connector_window: connector_window.Window,
    resources_refresh: application_refresh.Gate = .{},
    resources_refresh_request_id: ?[]u8 = null,
    control_refresh: application_refresh.Gate = .{},
    control_refresh_request_id: ?[]u8 = null,
    pending_control_effects: std.StringHashMapUnmanaged(PendingControlEffect) = .empty,
    controllers: std.ArrayListUnmanaged(*controller_pane.Controller) = .empty,
    next_controller_sequence: u64 = 1,
    next_request_sequence: u64 = 1,
    backend_state: BackendState = .starting,
    native_dirty: bool = true,
    closing: bool = false,
    quitting: bool = false,
    quit_revision: ?u64 = null,
    quit_ready: bool = false,
    restore_initialized: bool = false,
    number_badges_visible: bool = false,
    pending_unbound_windows: usize = 0,
    resource_file_flow: @import("resource_file_open.zig").Adapter = .{},
    repairing_native_transfer: bool = false,

    fn executorForWindow(self: *State, window: *Window) ?*presentation_executor.Executor {
        if (self.executor.managed_window == window) return &self.executor;
        for (self.window_executors.items) |executor| if (executor.managed_window == window) return executor;
        return null;
    }
    fn executorForWorkspace(self: *State, workspace_id: []const u8) ?*presentation_executor.Executor {
        if (self.executor.managed_window != null) if (self.executor.nativeWorkspaceSnapshot()) |snapshot| {
            if (std.mem.eql(u8, snapshot.workspace_id, workspace_id)) return &self.executor;
        };
        for (self.window_executors.items) |executor| {
            if (executor.managed_window == null) continue;
            const snapshot = executor.nativeWorkspaceSnapshot() orelse continue;
            if (std.mem.eql(u8, snapshot.workspace_id, workspace_id)) return executor;
        }
        return null;
    }
    fn activeExecutor(self: *State) *presentation_executor.Executor {
        if (activeGhosttyWindow(self.app)) |window| {
            if (self.executorForWindow(window)) |executor| return executor;
        }
        return &self.executor;
    }
    fn controllerExecutor(self: *State, controller_id: u64) ?*presentation_executor.Executor {
        if (controller_id == 0) return self.activeExecutor();
        const controller = controllerById(self, controller_id) orelse return null;
        return self.executorForWindow(controller.host_window);
    }
    fn ensureWindowExecutor(self: *State, window: *Window) !*presentation_executor.Executor {
        if (self.executorForWindow(window)) |executor| return executor;
        if (self.executor.managed_window == null) {
            if (self.pending_unbound_windows > 0) {
                self.pending_unbound_windows -= 1;
                self.executor.workspace.unbound = true;
            } else {
                self.executor.workspace.unbound = false;
                if (self.executor.workspace.workspace_id) |id| self.allocator.free(id);
                self.executor.workspace.workspace_id = null;
                if (self.executor.nativeWorkspaceSnapshot()) |snapshot| {
                    if (self.executorForWorkspace(snapshot.workspace_id) != null) self.executor.workspace.unbound = true;
                }
            }
            self.executor.managed_window = window;
            return &self.executor;
        }
        const executor = try self.allocator.create(presentation_executor.Executor);
        errdefer self.allocator.destroy(executor);
        executor.* = try presentation_executor.Executor.initWindow(&self.executor, null);
        errdefer executor.deinit();
        try self.window_executors.ensureUnusedCapacity(self.allocator, 1);
        if (self.pending_unbound_windows > 0) self.pending_unbound_windows -= 1;
        executor.managed_window = window;
        self.window_executors.appendAssumeCapacity(executor);
        return executor;
    }
    fn initializeWindowRestoration(self: *State) void {
        if (self.restore_initialized) return;
        self.restore_initialized = true;
        // Startup mounts only the selected Workspace. Saved open IDs are
        // bookkeeping, never an instruction to create additional windows.
    }
    fn rememberOpenWindows(self: *State) void {
        if (!self.restore_initialized or self.quitting) return;
        var ids: std.ArrayListUnmanaged([]const u8) = .empty;
        defer ids.deinit(self.allocator);
        if (self.executor.managed_window != null) if (self.executor.nativeWorkspaceSnapshot()) |snapshot| {
            ids.append(self.allocator, snapshot.workspace_id) catch return;
        };
        for (self.window_executors.items) |executor| {
            if (executor.managed_window == null) continue;
            if (executor.nativeWorkspaceSnapshot()) |snapshot| ids.append(self.allocator, snapshot.workspace_id) catch return;
        }
        self.executor.rememberOpenWindows(ids.items) catch {
            self.controller_model.setLocalError("Window state could not be saved");
        };
    }

    fn syncWindowSelections(self: *State) bool {
        var changed = self.executor.adoptNativeTabSelection() catch false;
        if (changed) self.executor.activateSelectedConversation();
        for (self.window_executors.items) |executor| {
            const adopted = executor.adoptNativeTabSelection() catch false;
            if (adopted) executor.activateSelectedConversation();
            changed = adopted or changed;
        }
        return changed;
    }

    fn identity(self: *const State) protocol.FrontendIdentity {
        return .{
            .frontend_instance_id = self.frontend_instance_id,
            .frontend_generation = self.frontend_generation,
        };
    }

    fn deinit(self: *State) void {
        self.closing = true;
        for (self.controllers.items) |controller| controller.deinit();
        self.controllers.deinit(self.allocator);
        // Retirement hooks must run before their borrowed LGP controller is freed.
        for (self.window_executors.items) |executor| {
            executor.deinit();
            self.allocator.destroy(executor);
        }
        self.window_executors.deinit(self.allocator);
        self.executor.deinit();
        @import("graphics_surface.zig").shutdown();
        if (self.lgp) |lgp| lgp.destroy();
        self.client.deinit();
        self.controller_model.deinit();
        self.attention_engine.deinit();
        self.session_cache.deinit();
        clearResourceFile(self);
        self.resource_file_flow.deinit();
        self.resources_model.deinit();
        self.resources_window.deinit();
        self.theme.deinit();
        self.settings_model.deinit();
        self.settings_window.deinit();
        self.inbox_model.deinit();
        if (self.inbox_refresh_request_id) |id| self.allocator.free(id);
        self.codex_window.deinit();
        self.connector_window.deinit();
        if (self.resources_refresh_request_id) |value| {
            self.allocator.free(value);
        }
        if (self.control_refresh_request_id) |value| {
            self.allocator.free(value);
        }
        clearPendingControlEffects(self);
        self.pending_control_effects.deinit(self.allocator);
        self.allocator.free(self.frontend_instance_id);
        self.* = undefined;
    }

    fn renderControllers(self: *State) void {
        self.rememberOpenWindows();
        self.executor.syncTabFocusIndicators();
        for (self.window_executors.items) |executor| executor.syncTabFocusIndicators();
        if (self.number_badges_visible) {
            if (activeGhosttyWindow(self.app)) |window|
                self.activeExecutor().setNumberBadgesVisible(window, true);
        }
        for (self.controllers.items) |controller| {
            if (!controller.isOrphaned()) {
                if (self.executorForWindow(controller.host_window)) |executor| {
                    if (self.lgp) |lgp| if (executor.nativeWorkspaceSnapshot()) |snapshot| {
                        _ = lgp.bindWindow(controller.host_window, snapshot.workspace_id) catch {};
                    };
                }
                controller.render();
            }
        }
    }

    fn pruneOrphanedControllers(self: *State) void {
        var index: usize = 0;
        while (index < self.controllers.items.len) {
            const controller = self.controllers.items[index];
            if (!controller.isOrphaned()) {
                index += 1;
                continue;
            }
            controller.deinit();
            _ = self.controllers.orderedRemove(index);
        }
        index = 0;
        while (index < self.window_executors.items.len) {
            const executor = self.window_executors.items[index];
            if (executor.managed_window != null) { index += 1; continue; }
            executor.deinit();
            self.allocator.destroy(executor);
            _ = self.window_executors.orderedRemove(index);
        }
    }
};

var state: ?State = null;

pub fn dictateShortcut(pressed: bool) void {
    if (state) |*current| if (current.lgp) |lgp| lgp.dictateShortcut(pressed, true);
}
pub fn dictateShortcutEdge(pressed: bool) void {
    if (state) |*current| if (current.lgp) |lgp| lgp.dictateShortcut(pressed, false);
}
pub fn dictateVoiceSettingError(message:[]const u8)void{if(state)|*current|current.settings_window.showError(message);}
pub fn dictateShortcutBinding(trigger: []const u8) void {
    if (state) |*current| if (current.lgp) |lgp| lgp.dictate.shortcutBinding(trigger);
}
pub fn dictateShortcutStatus(status_: []const u8) void {
    if (state) |*current| if (current.lgp) |lgp| lgp.dictate.shortcutStatus(status_);
}
pub fn dictateShortcutLost() void {
    if (state) |*current| if (current.lgp) |lgp| { lgp.dictate.shortcutLost(); };
}

pub fn init(app: *Application) !void {
    ipc_client.trace(1, "", 0);
    if (state != null) return error.AlreadyInitialized;
    const allocator = std.heap.c_allocator;
    const socket_path = try ipc_client.resolveSocketPath(allocator);
    defer allocator.free(socket_path);
    const frontend_instance_id = try std.fmt.allocPrint(
        allocator,
        "linux-ghostty-{d}-{d}",
        .{ std.os.linux.getpid(), std.time.nanoTimestamp() },
    );
    errdefer allocator.free(frontend_instance_id);
    const frontend_generation: u64 = 1;
    const registration = try protocol.encodeRegistration(
        allocator,
        .{
            .frontend_instance_id = frontend_instance_id,
            .frontend_generation = frontend_generation,
        },
    );
    defer allocator.free(registration);
    const client = try ipc_client.Client.init(
        allocator,
        socket_path,
        registration,
        wakeApplication,
        app,
    );
    state = .{
        .allocator = allocator,
        .app = app,
        .frontend_instance_id = frontend_instance_id,
        .frontend_generation = frontend_generation,
        .client = client,
        .executor = presentation_executor.Executor.init(
            allocator,
            app,
            .{
                .frontend_instance_id = frontend_instance_id,
                .frontend_generation = frontend_generation,
            },
        ),
        .controller_model = controller_model.Model.init(allocator),
        .session_cache = session_runtime.Cache.init(allocator),
        .resources_model = resources.Model.init(allocator),
        .resources_window = resources_window.Window.init(
            allocator,
            app,
            resourceAction,
            undefined,
        ),
        .theme = station_theme.Controller.init(allocator, app),
        .settings_model = settings.Model.init(allocator),
        .settings_window = settings_window.Window.init(allocator, app, settingsAction, undefined),
        .inbox_model = inbox.Model.init(allocator),
        .codex_window = codex_window.Window.init(allocator, app, sendCodexPayload, undefined),
        .connector_window = connector_window.Window.init(allocator, app, sendCodexPayload, undefined),
    };
    state.?.resources_window.action_context = &state.?;
    state.?.codex_window.server_started=codexServerStarted;
    state.?.settings_window.action_context = &state.?;
    state.?.codex_window.context = &state.?;
    state.?.connector_window.context = &state.?;
    errdefer {
        if (state) |*current| current.deinit();
        state = null;
    }
    const notice_type = glib.VariantType.new("s");
    defer notice_type.free();
    const notice_action = gio.SimpleAction.new("polar-station-inbox", notice_type);
    defer notice_action.unref();
    _ = gio.SimpleAction.signals.activate.connect(notice_action, *Application, notificationActivated, app, .{});
    app.as(gio.ActionMap).addAction(notice_action.as(gio.Action));
    try state.?.client.start();
    state.?.lgp = try lgp_workspace.Workspace.create(allocator, app, sendLgpFrame, openLgpContent, &state.?);
    state.?.lgp.?.dictate.settings_context = &state.?;
    state.?.lgp.?.dictate.show_settings = showVoiceSettings;
    state.?.lgp.?.save_mode = saveChatMode;
    state.?.lgp.?.send_session = sendStationSession;
    state.?.lgp.?.terminal_selection = terminalSelection;
    state.?.lgp.?.retry_host = retryChatHost;
    state.?.lgp.?.ensure_terminal = ensureLgpTerminal;
    state.?.lgp.?.terminal_state = lgpTerminalState;
    state.?.lgp.?.manage_connections = chatManageConnections;
    state.?.lgp.?.choose_idea_session = chooseIdeaSession;
    state.?.executor.content_hooks = state.?.lgp.?.hooks();
    state.?.executor.content_lifecycle.submit_context = &state.?.client;
    state.?.executor.content_lifecycle.submit_content = submitPaneContent;
    state.?.executor.content_lifecycle.submit_native_state = submitNativeState;
    state.?.executor.enqueueInitialStateRead() catch {};
    _ = gtk.Application.signals.window_removed.connect(app.as(gtk.Application), ?*anyopaque, nativeWindowRemoved, null, .{});
    _ = gtk.Application.signals.window_added.connect(app.as(gtk.Application), ?*anyopaque, nativeWindowAdded, null, .{});
    _ = gobject.Object.signals.notify.connect(app.as(gobject.Object), ?*anyopaque, nativePropertyChanged, null, .{ .detail = "active-window" });
    var windows = @as(?*glib.List, app.as(gtk.Application).getWindows());
    while (windows) |node| : (windows = node.f_next) {
        const window: *gtk.Window = @ptrCast(@alignCast(node.f_data orelse continue));
        observeNativeWindow(window);
    }
    if (state) |*current| syncFocusCycleActions(current);
}

pub fn tick(app: *Application) void {
    if (state) |*current| {
        if (current.app != app) return;
        ipc_client.trace(25, current.frontend_instance_id, current.frontend_generation);
        if (!current.native_dirty and !current.client.hasPending()) return;
        const native_changed = current.native_dirty;
        current.native_dirty = false;
        var batch: host_frame_batch.Batch = .{};
        if (native_changed) batch.requestRender();
        ipc_client.trace(22, current.frontend_instance_id, current.frontend_generation);
        bindActiveManagedWindow(current, app);
        while (batch.canPop()) {
            var event = current.client.pop() orelse break;
            defer event.deinit(current.allocator);
            switch (event) {
                .request_failed => |value| {
                    var handled = current.codex_window.requestFailed(value.request_id, value.message);
                    handled = current.connector_window.requestFailed(value.request_id, value.message) or handled;
                    for (current.controllers.items) |controller| {
                        if (controller.connections) |connections| handled = connections.requestFailed(value.request_id, value.message) or handled;
                    }
                    if (current.pending_control_effects.fetchRemove(value.request_id)) |pending| {
                        handled = true;
                        releaseCreationReservation(current, pending.value);
                        presentRejectedControlEffect(current, pending.value, value.message);
                        current.allocator.free(pending.key);
                        if (pending.value.selection_json) |json| current.allocator.free(json);
                    }
                    inline for (.{ .{ "control_refresh_request_id", "control_refresh" }, .{ "resources_refresh_request_id", "resources_refresh" }, .{ "inbox_refresh_request_id", "inbox_refresh" } }) |pair| {
                        if (@field(current, pair[0])) |id| if (std.mem.eql(u8, id, value.request_id)) {
                            handled = true;
                            current.allocator.free(id); @field(current, pair[0]) = null; @field(current, pair[1]).failed();
                        };
                    }
                    if (!value.cancelled) {
                        if (std.mem.eql(u8, value.api, "settings_v1")) {
                            current.settings_window.showError(value.message); handled = true;
                        } else if (std.mem.eql(u8, value.api, "resources_v1")) {
                            current.resources_window.showError(value.message); handled = true;
                        }
                    }
                    if (!handled and !value.cancelled) current.controller_model.setLocalError(value.message);
                    batch.requestRender();
                },
                .connecting => |value| {
                    if (current.lgp) |lgp| lgp.disconnected(value.generation);
                    backendBecameUnavailable(current, .connecting);
                    current.controller_model.connection = .connecting;
                },
                .unavailable => |value| {
                    if (current.lgp) |lgp| lgp.disconnected(value.generation);
                    current.controller_model.setLocalError(value.message);
                    backendBecameUnavailable(current, .unavailable);
                    current.renderControllers();
                },
                .ready => |value| {
                    if (current.attention_engine.dispatch(current.allocator, .{ .type = "reconnect" })) |output| output.deinit() else |_| {}
                    batch.observedFrame();
                    handleHostFrame(current, value.generation, value.frame, &batch);
                },
                .frame => |value| {
                    if (current.backend_state != .ready) continue;
                    batch.observedFrame();
                    if (current.lgp) |lgp| {
                        if (lgp.hostFrame(value.generation, value.payload)) continue;
                    }
                    handleHostFrame(current, value.generation, value.payload, &batch);
                },
                .session_completed => |response| {
                    if (current.lgp) |lgp| lgp.sessionCompleted(response);
                },
                .native_state_completed => |response| {
                    if (current.quitting) {
                        const receipt = std.json.parseFromSlice(struct { revision: u64, @"error": ?[]const u8 = null }, current.allocator, response, .{ .ignore_unknown_fields = true }) catch continue;
                        defer receipt.deinit();
                        if (current.quit_revision == receipt.value.revision) {
                            if (receipt.value.@"error") |message| {
                                current.quitting = false; current.quit_revision = null;
                                current.controller_model.setLocalError(message);
                                if (activeGhosttyWindow(current.app)) |window| showNativeError(current, window, "Could not save before Quit", message);
                            } else { current.quit_ready = true; current.app.quitNow(); return; }
                        }
                    }

                    const owner = if (current.executor.pending_lifecycle) |pending| pending.owner else current.activeExecutor();
                    const failed_window = if (owner) |executor| executor.managed_window else null;
                    const changed = current.executor.applyNativeStateResponse(response) catch |err| failed: {
                        // A failed admitted save cancels Quit as well as the
                        // mutation. Do not hide it behind a later successful flush.
                        current.quitting = false;
                        current.quit_revision = null;
                        current.controller_model.setLocalError(@errorName(err));
                        if (failed_window) |window| {
                            const parsed = std.json.parseFromSlice(struct { @"error": ?[]const u8 = null }, current.allocator, response, .{ .ignore_unknown_fields = true }) catch break :failed true;
                            defer parsed.deinit();
                            showNativeError(current, window, "Workspace", parsed.value.@"error" orelse @errorName(err));
                        }
                        break :failed true;
                    };
                    current.initializeWindowRestoration();
                    // Window callbacks during a lifecycle save cannot write a
                    // competing snapshot. Recompute from live windows once the
                    // transaction finishes instead of dropping that update.
                    current.rememberOpenWindows();
                    finishQuitIfDrained(current);
                    if (changed) batch.requestRender();
                },
                .content_completed => |response| {
                    current.activeExecutor().applyContentResponse(response) catch |err| {
                        std.log.warn("Polar Station ignored content response: {s}", .{@errorName(err)});
                    };
                    for (current.window_executors.items) |executor| {
                        if (executor == current.activeExecutor()) continue;
                        executor.applyContentResponse(response) catch {};
                    }
                    if (current.activeExecutor() != &current.executor) current.executor.applyContentResponse(response) catch {};
                    if (current.lgp) |lgp| lgp.terminalViewsChanged();
                    // Focus, retirement and recovery remain Pane-local. Their
                    // accepted status does not change the Workspace View.
                },
            }
        }
        if (batch.takeSessionProjection()) {
            current.controller_model.applySessionRuntime(
                &current.session_cache,
            );
            batch.requestRender();
        }
        if (current.syncWindowSelections()) batch.requestRender();
        if (batch.takeRender()) current.renderControllers();
        // A native focus event also matters when selection was already adopted
        // or a composer mounted after that observation. Reuse the idempotent
        // input selection path after mounting; catalog mutation is not its gate.
        if (native_changed) current.activeExecutor().activateSelectedConversation();
        if (current.client.hasPending()) current.app.wakeup();
        syncFocusCycleActions(current);
    }
}

fn submitNativeState(context: ?*anyopaque, request: []const u8) !void {
    const client: *ipc_client.Client = @ptrCast(@alignCast(context orelse return error.NotStarted));
    try client.nativeStateJson(request);
}

fn submitPaneContent(context: ?*anyopaque, socket_path: []const u8, request: []const u8) !void {
    const client: *ipc_client.Client = @ptrCast(@alignCast(context orelse return error.ContentWorkerUnavailable));
    try client.sendContentJson(socket_path, request);
}

/// Control Center visibility changes only on explicit user activation.
pub fn toggleControlCenter(app: *Application) void {
    if (state) |*current| {
        if (current.app != app) return;
        const host_window = activeGhosttyWindow(app) orelse return;
        const executor = current.ensureWindowExecutor(host_window) catch return;
        executor.setManagedWindow(host_window);
        const controller = ensureWindowController(current, host_window) orelse return;
        applyControllerSettings(current, controller);
        controller.render();
        controller.toggle();
    }
}

fn ensureWindowController(current: *State, host_window: *Window) ?*controller_pane.Controller {
    current.pruneOrphanedControllers();
    for (current.controllers.items) |controller| {
        if (controller.isHostedBy(host_window)) return controller;
    }
    const executor = current.ensureWindowExecutor(host_window) catch return null;
    const controller = controller_pane.Controller.create(
        current.allocator, current.app, host_window, &current.controller_model,
        &current.inbox_model, executor, controllerAction, controllerPaneDrop,
        current, nextControllerId(current),
    ) catch return null;
    controller.codex_send = sendCodexPayload; controller.codex_context = current;
    current.controllers.append(current.allocator, controller) catch {
        controller.deinit();
        return null;
    };
    return controller;
}

fn retryChatHost(context: ?*anyopaque) void {
    const current: *State = @ptrCast(@alignCast(context orelse return));
    current.client.retryNow() catch {};
}

fn terminalSelection(context: ?*anyopaque, allocator: std.mem.Allocator, window: *Window, pane: []const u8) ?[]u8 {
    const current: *State = @ptrCast(@alignCast(context orelse return null));
    const executor = current.executorForWindow(window) orelse return null;
    return executor.terminalSelection(allocator,pane);
}

fn sendStationSession(context: ?*anyopaque, bytes: []const u8) !void {
    const current: *State = @ptrCast(@alignCast(context orelse return error.NotStarted));
    try current.client.sendSessionJson(bytes);
}

fn sendLgpFrame(context: ?*anyopaque, bytes: []const u8) !void {
    const current: *State = @ptrCast(@alignCast(context orelse return error.NotStarted));
    try current.client.sendJson(bytes);
}

fn saveChatMode(context: ?*anyopaque, modified: bool) void {
    const current: *State = @ptrCast(@alignCast(context orelse return));
    const projection = current.settings_model.projection() orelse return;
    var appearance = projection.appearance;
    appearance.chat_send_mode = if (modified) "modified_enter" else "enter";
    settingsAction(context, .{ .appearance = appearance });
}

fn ensureLgpTerminal(context:?*anyopaque, window:*Window, source:[]const u8, execution:[]const u8, session:[]const u8) ![]const u8 {
    const current:*State=@ptrCast(@alignCast(context orelse return error.NotStarted));
    const executor=current.executorForWindow(window) orelse return error.WorkspaceWindowNotBound;
    const focus=window.as(gtk.Window).getFocus();
    if(focus) |widget| _ = widget.as(gobject.Object).ref();
    defer if(focus) |widget| widget.as(gobject.Object).unref();
    const pane=try executor.ensureTerminalVisible(source,execution,session);
    if(focus) |widget| _ = widget.grabFocus();
    current.native_dirty=true;current.app.wakeup();return pane;
}
fn lgpTerminalState(context:?*anyopaque, window:*Window, pane:[]const u8, execution:[]const u8, session:[]const u8) lgp_workspace.TerminalViewState {
    const current:*State=@ptrCast(@alignCast(context orelse return .unavailable));
    const executor=current.executorForWindow(window) orelse return .unavailable;
    return switch(executor.terminalViewState(pane,execution,session)){.pending=>.pending,.ready=>.ready,.unavailable=>.unavailable};
}

fn openLgpContent(context: ?*anyopaque, window: *Window, content: StationContent, replace_pane: ?[]const u8) !void {
    const current: *State = @ptrCast(@alignCast(context orelse return error.NotStarted));
    const executor = current.executorForWindow(window) orelse return error.WorkspaceWindowNotBound;
    try executor.openContent(content, replace_pane);
    current.native_dirty = true;
    current.app.wakeup();
}

/// Explicit native action; connection/browse policy is owned by shared Rust.
pub fn showLgp(app: *Application) void {
    if (state) |*current| {
        if (current.app != app) return;
        bindActiveManagedWindow(current, app);
        if (current.lgp) |lgp| lgp.showBrowser();
    }
}

/// Open the process-wide Idea Tree view in the existing native catalog.
pub fn showIdeaTree(app: *Application) void {
    if (state) |*current| {
        if (current.app != app) return;
        bindActiveManagedWindow(current, app);
        const active = app.as(gtk.Application).getActiveWindow() orelse return;
        const window = gobject.ext.cast(Window, active) orelse return;
        const executor = current.executorForWindow(window) orelse return;
        const view_id = @import("native_workspace_state.zig").newNativeIdentifier(current.allocator) catch return;
        defer current.allocator.free(view_id);
        executor.openContent(.{ .kind = .idea_tree, .view_id = view_id }, null) catch |err| {
            current.controller_model.setLocalError(@errorName(err));
            current.renderControllers();
            std.log.err("Idea Tree open failed: {s}", .{@errorName(err)});
            return;
        };
        current.native_dirty = true;
        current.app.wakeup();
    }
}

fn chooseIdeaSession(context:?*anyopaque,window:*Window,pane:[]const u8,view:[]const u8) void {
    const current:*State=@ptrCast(@alignCast(context orelse return));
    const controller=ensureWindowController(current,window) orelse return;
    controller.presentIdeaSessionPicker(pane,view,context,ideaSessionPicked);
}
fn ideaSessionPicked(context:?*anyopaque,pane:[]const u8,view:[]const u8,owner:[]const u8,endpoint:[]const u8,thread:[]const u8,title:[]const u8) void {
    const current:*State=@ptrCast(@alignCast(context orelse return));
    if(current.lgp) |lgp| lgp.bindIdeaSession(pane,view,owner,endpoint,thread,title);
}

/// Opens one process-wide Settings window only on explicit native action.
fn chatManageConnections(context: ?*anyopaque, window: *Window) void {
    const current: *State = @ptrCast(@alignCast(context orelse return));
    const controller = ensureWindowController(current, window) orelse return;
    controller.presentConnections();
}

fn showVoiceSettings(context: ?*anyopaque) void {
    const current: *State = @ptrCast(@alignCast(context orelse return));
    current.settings_window.presentVoice(current.settings_model.projection());
    requestSettingsProjection(current);
}
pub fn showSettings(app: *Application) void {
    if (state) |*current| {
        if (current.app != app) return;
        current.settings_window.present(current.settings_model.projection());
        requestSettingsProjection(current);
    }
}

/// Routes every Polar Station New Tab entry through the same native Workspace
/// action path as the Control Center. The executor performs the physical GTK
/// mutation only after the managed Tab identity has been allocated.
pub fn prepareNativeNewWindow(app: *Application) void {
    if (state) |*current| {
        // Ghostty also uses this action to create its initial launch window.
        if (current.app == app and current.restore_initialized and current.executor.managed_window != null and current.pending_unbound_windows == 0) current.pending_unbound_windows = 1;
    }
}

pub fn requestManagedNewTab(window: *Window) void {
    if (state) |*current| {
        const executor = current.ensureWindowExecutor(window) catch return;
        executor.setManagedWindow(window);
        const title = std.fmt.allocPrint(
            current.allocator,
            "Tab {d}",
            .{executor.nativeTabCount() + 1},
        ) catch return;
        defer current.allocator.free(title);
        var controller_id: u64 = 0;
        for (current.controllers.items) |controller| {
            if (controller.ownsHostWindow(window)) {
                controller_id = controller.controller_id;
                break;
            }
        }
        sendControllerRequest(current, controller_id, .{ .add_tab = .{
            .title = title,
        } });
    }
}

/// Resolve a native page back to its owner; never use the active-window fallback.
pub fn requestNativeTabAction(tab: *@import("../class/tab.zig").Tab, action: []const u8, title: ?[]const u8) bool {
    const current = if (state) |*value| value else return true;
    if (current.quitting) return true;
    const root = tab.as(gtk.Widget).getRoot() orelse return true;
    const window = gobject.ext.cast(Window, root.as(gobject.Object)) orelse return false;
    const executor = current.executorForWindow(window) orelse return true;
    if (executor.applying_physical_snapshot) return false;
    const view = window.getTabView();
    const page = view.getPage(tab.as(gtk.Widget));
    const borrowed = executor.nativeTabIdForPage(page) orelse return true;
    const id = current.allocator.dupe(u8, borrowed) catch return true;
    defer current.allocator.free(id);
    executor.performNativeTabAction(action, id, title) catch |err| {
        current.controller_model.setLocalError(@errorName(err));
        showNativeError(current, window, "Workspace", @errorName(err));
    };
    current.renderControllers();
    return true;
}

const NativeTabDestination = struct {
    arena: std.heap.ArenaAllocator,
    source: *presentation_executor.Executor,
    workspace_id: []const u8,
    tab_id: []const u8,
    ids: []const []const u8,
    picker: *gtk.DropDown,
    name: *gtk.Entry,
};

fn presentNativeTabDestination(executor: *presentation_executor.Executor, tab_id: []const u8) void {
    const current = if (state) |*value| value else return;
    const window = executor.managed_window orelse return;
    const snapshot = executor.nativeWorkspaceSnapshot() orelse return;
    const catalog = executor.nativeWorkspaceCatalog() orelse return;
    const data = current.allocator.create(NativeTabDestination) catch return;
    data.arena = std.heap.ArenaAllocator.init(current.allocator);
    const allocator = data.arena.allocator();
    data.source = executor;
    data.workspace_id = allocator.dupe(u8, snapshot.workspace_id) catch return;
    data.tab_id = allocator.dupe(u8, tab_id) catch return;
    const ids = allocator.alloc([]const u8, catalog.workspaces.len) catch return;
    var count: usize = 0;
    const names = gtk.StringList.new(null);
    defer names.unref();
    for (catalog.workspaces) |workspace| {
        if (catalog.isArchived(workspace.workspace_id) or std.mem.eql(u8, workspace.workspace_id, snapshot.workspace_id)) continue;
        ids[count] = allocator.dupe(u8, workspace.workspace_id) catch return;
        names.append(allocator.dupeZ(u8, workspace.workspace_name) catch return);
        count += 1;
    }
    data.ids = ids[0..count];
    names.append("Create Workspace…");
    data.picker = gtk.DropDown.new(names.as(gio.ListModel), null);
    data.name = gtk.Entry.new();
    data.name.as(gtk.Editable).setText("Workspace");
    data.name.setPlaceholderText("Name for a new Workspace");
    const box = gtk.Box.new(.vertical, 8);
    box.append(data.picker.as(gtk.Widget)); box.append(data.name.as(gtk.Widget));
    const dialog = adw.MessageDialog.new(window.as(gtk.Window), "Move Tab to Workspace", "Choose a Workspace, or explicitly create one. Cancel keeps the Tab here.");
    dialog.setExtraChild(box.as(gtk.Widget));
    dialog.addResponse("cancel", "Cancel"); dialog.addResponse("move", "Move");
    dialog.setCloseResponse("cancel"); dialog.setDefaultResponse("move");
    _ = adw.MessageDialog.signals.response.connect(dialog, *NativeTabDestination, nativeTabDestinationResponse, data, .{});
    dialog.as(gtk.Window).present();
}

fn nativeTabDestinationResponse(dialog: *adw.MessageDialog, response: [*:0]const u8, data: *NativeTabDestination) callconv(.c) void {
    const current = if (state) |*value| value else return;
    defer { data.arena.deinit(); current.allocator.destroy(data); dialog.as(gtk.Window).destroy(); }
    if (!std.mem.eql(u8, std.mem.span(response), "move")) return;
    const snapshot = data.source.nativeWorkspaceSnapshot() orelse return;
    if (data.source.managed_window == null or !std.mem.eql(u8, snapshot.workspace_id, data.workspace_id)) return;
    const index = data.picker.getSelected();
    const target = if (index < data.ids.len) data.ids[index] else null;
    data.source.moveNativeTab(data.tab_id, target, std.mem.span(data.name.as(gtk.Editable).getText())) catch |err| {
        current.controller_model.setLocalError(@errorName(err)); current.renderControllers();
    };
}

pub fn requestNativeTabTearOut(window: *Window) void {
    const current = if (state) |*value| value else return;
    const executor = current.executorForWindow(window) orelse return;
    presentNativeTabDestination(executor, executor.nativeSelectedTabId() orelse return);
}

pub fn interceptNativeTabTransfer(target: *Window, page: *adw.TabPage) bool {
    const current = if (state) |*value| value else return false;
    if (current.repairing_native_transfer) return false;
    var source: ?*presentation_executor.Executor = null;
    if (current.executor.nativeTabIdForPage(page) != null) source = &current.executor;
    for (current.window_executors.items) |executor| if (executor.nativeTabIdForPage(page) != null) { source = executor; break; };
    const owner = source orelse return false;
    const window = owner.managed_window orelse return false;
    if (window == target or owner.applying_physical_snapshot) return false;
    const id = current.allocator.dupe(u8, owner.nativeTabIdForPage(page).?) catch return true;
    defer current.allocator.free(id);
    var position: c_int = 0;
    if (owner.nativeWorkspaceSnapshot()) |snapshot| for (snapshot.tabs, 0..) |tab, i| {
        if (std.mem.eql(u8, tab.id, id)) { position = @intCast(i); break; }
    };
    current.repairing_native_transfer = true;
    target.getTabView().transferPage(page, window.getTabView(), position);
    current.repairing_native_transfer = false;
    presentNativeTabDestination(owner, id);
    return true;
}

pub fn finishNativeTabTransfer(source: *presentation_executor.Executor, target_id: []const u8) !void {
    const current = if (state) |*value| value else return;
    try source.refreshTransferredWorkspace();
    if (current.executorForWorkspace(target_id)) |destination| {
        try destination.refreshTransferredWorkspace();
        if (destination.managed_window) |window| window.as(gtk.Window).present();
    } else {
        current.pending_unbound_windows += 1;
        _ = try current.app.performAction(.app, .new_window, {});
        const window = activeGhosttyWindow(current.app) orelse return error.NativeWindowUnavailable;
        const destination = try current.ensureWindowExecutor(window);
        _ = try destination.applyNativeWorkspaceAction(.{ .select_workspace = .{ .workspace_id = target_id } });
    }
    current.renderControllers();
}

pub fn requestNativeSurfaceClose(surface: *@import("../class/surface.zig").Surface) bool {
    const current = if (state) |*value| value else return true;
    const root = surface.as(gtk.Widget).getRoot() orelse return true;
    const window = gobject.ext.cast(Window, root.as(gobject.Object)) orelse return false;
    const executor = current.executorForWindow(window) orelse return true;
    if (executor.applying_physical_snapshot) return false;
    if (executor.nativePaneIdForSurface(surface)) |borrowed| {
        const id = current.allocator.dupe(u8, borrowed) catch return true;
        defer current.allocator.free(id);
        _ = executor.applyNativeWorkspaceAction(.{ .remove_pane = .{ .pane_id = id } }) catch |err| {
            current.controller_model.setLocalError(@errorName(err)); return true;
        };
        current.renderControllers();
    }
    return true;
}

pub fn interceptNativePageClose(window: *Window, page: *adw.TabPage) bool {
    const current = if (state) |*value| value else return false;
    const executor = current.executorForWindow(window) orelse return false;
    if (executor.applying_physical_snapshot) return false;
    const tab = gobject.ext.cast(@import("../class/tab.zig").Tab, page.getChild()) orelse return false;
    window.getTabView().closePageFinish(page, 0);
    return requestNativeTabAction(tab, "close_tab", null);
}

pub fn consumeNativeLayoutAction(action: []const u8) bool {
    const current = if (state) |*value| value else return true;
    var decision = current.activeExecutor().nativeActionDecision(action, current.activeExecutor().nativeSelectedTabId()) catch return true;
    defer decision.deinit();
    return !std.mem.eql(u8, decision.value.effect, "unhandled");
}

// GTK owns notification lifetime. Callbacks carry no State pointers and are
// harmless after Station teardown. All native invalidations run on the UI thread.
fn nativeStateChanged() void {
    if (state) |*current| {
        if (current.native_dirty) return;
        current.native_dirty = true;
        current.app.wakeup();
    }
}

// Window active-surface is a computed getter, not a forwarded Tab signal.
// Defer observation until GTK's focus enter/leave controllers have settled.
fn nativeFocusWidgetChanged(_: *gobject.Object, _: *gobject.ParamSpec, _: ?*anyopaque) callconv(.c) void {
    nativeStateChanged();
}

fn nativePropertyChanged(_: *gobject.Object, _: *gobject.ParamSpec, _: ?*anyopaque) callconv(.c) void {
    if (state) |*current| {
        if (current.syncWindowSelections()) {
            nativeStateChanged();
        }
    }
}

fn nativeTabReordered(view: *adw.TabView, page: *adw.TabPage, position: c_int, _: ?*anyopaque) callconv(.c) void {
    std.log.info("native Tab reordered position={d}", .{position});
    if (state) |*current| {
        var changed = current.executor.adoptNativeTabOrder(view, page, position) catch false;
        for (current.window_executors.items) |executor| {
            changed = (executor.adoptNativeTabOrder(view, page, position) catch false) or changed;
        }
        if (changed) nativeStateChanged();
    }
}

fn nativeWindowMapped(_: *gtk.Widget, _: ?*anyopaque) callconv(.c) void {
    nativeStateChanged();
}

fn nativeFocusChanged(_: *gobject.Object, _: *gobject.ParamSpec, _: ?*anyopaque) callconv(.c) void {
    if (state) |*current| {
        current.executor.syncTerminalFocus();
        for (current.window_executors.items) |executor| executor.syncTerminalFocus();
        if (current.syncWindowSelections()) nativeStateChanged();
    }
}

/// Called only after Ghostty's existing Quit confirmation has been accepted.
pub fn willQuit(app: *Application) bool {
    if (state) |*current| {
        if (current.app != app) return true;
        if (current.quit_ready) return true;
        if (current.quitting) return false;
        current.quitting = true;
        finishQuitIfDrained(current);
        return false;
    }
    return true;
}

fn finishQuitIfDrained(current: *State) void {
    if (!current.quitting or current.quit_revision != null or current.executor.pending_lifecycle != null) return;
    current.quit_revision = current.executor.flushNativeState() catch |err| {
        current.quitting = false;
        current.controller_model.setLocalError(@errorName(err));
        if (activeGhosttyWindow(current.app)) |window| showNativeError(current, window, "Could not save before Quit", @errorName(err));
        return;
    };
}

/// Native titlebar and menu close share view-only teardown for managed windows.
pub fn closeManagedWindow(window: *Window) bool {
    const current = if (state) |*value| value else return false;
    const executor = current.executorForWindow(window) orelse return false;
    var decision = executor.lifecycleDecision("close", null) catch return true;
    defer decision.deinit();
    if (decision.value.@"error") |message| { current.controller_model.setLocalError(message); current.renderControllers(); return true; }
    window.as(gtk.Window).destroy();
    return true;
}

fn nativeWindowRemoved(_: *gtk.Application, window: *gtk.Window, _: ?*anyopaque) callconv(.c) void {
    const native = gobject.ext.cast(Window, window.as(gobject.Object)) orelse return;
    @import("auxiliary_window.zig").parentRemoved(window);
    if (state) |*current| {
        if (current.closing) return;
        if (current.executorForWindow(native)) |executor| executor.windowRemoved(native);
        if (current.lgp) |lgp| lgp.windowRemoved(native);
        current.rememberOpenWindows();
        nativeStateChanged();
    }
}

fn nativeWindowAdded(_: *gtk.Application, window: *gtk.Window, _: ?*anyopaque) callconv(.c) void {
    observeNativeWindow(window);
    nativeStateChanged();
}

fn observeNativeWindow(gtk_window: *gtk.Window) void {
    const window = gobject.ext.cast(Window, gtk_window.as(gobject.Object)) orelse return;
    const object = window.as(gobject.Object);
    if (object.getData("polar-station-state-connected") != null) return;
    object.setData("polar-station-state-connected", window);
    _ = gobject.Object.signals.notify.connect(object, ?*anyopaque, nativeFocusChanged, null, .{ .detail = "is-active" });
    _ = gtk.Widget.signals.map.connect(window.as(gtk.Widget), ?*anyopaque, nativeWindowMapped, null, .{});
    _ = gobject.Object.signals.notify.connect(object, ?*anyopaque, nativePropertyChanged, null, .{ .detail = "active-surface" });
    _ = gobject.Object.signals.notify.connect(object, ?*anyopaque, nativeFocusWidgetChanged, null, .{ .detail = "focus-widget" });
    const tabs = window.getTabView();
    _ = adw.TabView.signals.page_reordered.connect(tabs, ?*anyopaque, nativeTabReordered, null, .{});
    _ = gobject.Object.signals.notify.connect(tabs.as(gobject.Object), ?*anyopaque, nativePropertyChanged, null, .{ .detail = "selected-page" });
    _ = gobject.Object.signals.notify.connect(tabs.as(gobject.Object), ?*anyopaque, nativePropertyChanged, null, .{ .detail = "n-pages" });
}

fn bindActiveManagedWindow(current: *State, app: *Application) void {
    const host_window = activeGhosttyWindow(app) orelse return;
    if (!managedWindowIsReady(host_window)) return;
    const executor = current.ensureWindowExecutor(host_window) catch return;
    if (current.lgp) |lgp| if (executor.nativeWorkspaceSnapshot()) |snapshot| { _ = lgp.bindWindow(host_window, snapshot.workspace_id) catch return; };
    if (!executor.setManagedWindowIfReady(host_window)) return;
    if (executor.nativeWorkspaceSnapshot() == null) {
        if (ensureWindowController(current, host_window)) |controller| controller.render();
        return;
    }
    const tab_view = host_window.getTabView();
    if (tab_view.as(gobject.Object).getData("polar-station-focus-connected") == null) {
        ipc_client.trace(2, current.frontend_instance_id, current.frontend_generation);
        tab_view.as(gobject.Object).setData("polar-station-focus-connected", tab_view);
        _ = adw.TabView.signals.indicator_activated.connect(tab_view, ?*anyopaque, tabFocusActivated, null, .{});
    }
    const projection = current.controller_model.projection() orelse return;
    current.activeExecutor().applyControlProjection(projection) catch |apply_error| {
        std.log.err(
            "Polar Station startup Workspace apply failed: {s}",
            .{@errorName(apply_error)},
        );
    };
    current.activeExecutor().syncTabFocusIndicators();
}

fn tabFocusActivated(_: *adw.TabView, page: *adw.TabPage, _: ?*anyopaque) callconv(.c) void {
    const current = if (state) |*value| value else return;
    _ = toggleNativeTabMode(current, page);
}

fn toggleNativeTabMode(current: *State, page: *adw.TabPage) bool {
    const borrowed_id = current.activeExecutor().nativeTabIdForPage(page) orelse return false;
    // The mode mutation replaces the native catalog; do not retain its slices.
    const tab_id = current.allocator.dupe(u8, borrowed_id) catch return false;
    defer current.allocator.free(tab_id);
    sendControllerRequest(current, 0, .{ .toggle_display_mode = .{ .tab_id = tab_id } });
    return true;
}

fn managedWindowIsReady(window: *Window) bool {
    if (window.getTabView().getNPages() == 0) return false;
    return window.getActiveSurface() != null;
}

fn activeGhosttyWindow(app: *Application) ?*Window {
    const gtk_app = app.as(gtk.Application);
    if (gtk_app.getActiveWindow()) |active| {
        if (liveGhosttyWindow(active)) |window| return window;
    }

    var current = @as(?*glib.List, gtk_app.getWindows());
    while (current) |node| : (current = node.f_next) {
        const data = node.f_data orelse continue;
        const gtk_window: *gtk.Window = @ptrCast(@alignCast(data));
        if (liveGhosttyWindow(gtk_window)) |window| return window;
    }
    return null;
}

fn liveGhosttyWindow(gtk_window: *gtk.Window) ?*Window {
    const window = gobject.ext.cast(
        Window,
        gtk_window.as(gobject.Object),
    ) orelse return null;
    if (gtk_window.as(gtk.Widget).getVisible() == 0) return null;
    if (gtk_window.as(gtk.Widget).getMapped() == 0) return null;
    if (gtk_window.as(gtk.Native).getSurface() == null) return null;
    return window;
}

/// A processless Pane can retire the final initialized core Surface while
/// its Workspace still owns empty Tabs or a replacement awaiting realization.
pub fn hasOpenWorkspaceWindow(app: *Application) bool {
    const current = if (state) |*value| value else return false;
    if (current.app != app or current.closing) return false;
    return activeGhosttyWindow(app) != null;
}

pub fn focusPrevious(app: *Application) void {
    if (state) |*current| {
        if (current.app != app) return;
        const pane_id = current.activeExecutor().nativeFocusCycleTarget(
            .previous,
        ) catch return orelse return;
        sendControllerRequest(current, 0, .{ .activate_pane = .{
            .pane_id = pane_id,
        } });
    }
}

pub fn focusNext(app: *Application) void {
    if (state) |*current| {
        if (current.app != app) return;
        const pane_id = current.activeExecutor().nativeFocusCycleTarget(
            .next,
        ) catch return orelse return;
        sendControllerRequest(current, 0, .{ .activate_pane = .{
            .pane_id = pane_id,
        } });
    }
}

pub fn focusSidebarSlot(app: *Application, slot: u8) void {
    if (state) |*current| {
        if (current.app != app) return;
        const pane_id = current.activeExecutor().nativeFocusSidebarTarget(slot) catch
            return orelse return;
        sendControllerRequest(current, 0, .{ .activate_pane = .{
            .pane_id = pane_id,
        } });
    }
}

pub fn focusPaneNumber(app: *Application, number: u8) void {
    const window = activeGhosttyWindow(app) orelse return;
    _ = dispatchWindowShortcut(window, .{ .focus_pane_number = number });
}

pub fn setNumberBadgesVisible(window: *Window, visible: bool) void {
    const current = if (state) |*value| value else return;
    if (visible and activeGhosttyWindow(current.app) != window) return;
    current.number_badges_visible = visible;
    current.activeExecutor().setNumberBadgesVisible(window, visible);
}

pub fn shortcutHintContext(window:*Window) shortcuts.HintContext {
    const current=if(state)|*value|value else return .{.panes=0,.tabs=0,.idea=false};
    const executor=current.executorForWindow(window) orelse return .{.panes=0,.tabs=0,.idea=false};
    const snapshot=executor.nativeWorkspaceSnapshot() orelse return .{.panes=0,.tabs=0,.idea=false};
    const tab=snapshot.tab(executor.nativeSelectedTabId() orelse snapshot.selected_tab_id) orelse return .{.panes=0,.tabs=0,.idea=false};
    var idea=false;
    for(tab.panes)|pane| if(std.mem.eql(u8,pane.id,executor.nativeSelectedPaneId(tab.id) orelse "")) {
        if(pane.content)|content|idea=content.kind==.idea_tree;
    };
    return .{.panes=@intCast(tab.panes.len),.tabs=@intCast(snapshot.tabs.len),.idea=idea};
}

/// Native menu copy and suffix come from the shared action catalog.
pub fn shortcutMenuLabel(action: u32, buffer: []u8) [:0]const u8 {
    for (0..shortcuts.catalogCount()) |index| {
        const entry = shortcuts.catalogEntry(index);
        if (entry.action == action) return std.fmt.bufPrintZ(buffer, "{s}…  Ctrl+Shift+{s}", .{
            std.mem.span(entry.label), std.mem.span(entry.key),
        }) catch "Station action";
    }
    return "Station action";
}

pub fn installNativeResizeObserver(window:*Window) void {
    const events=gtk.EventControllerLegacy.new();events.as(gtk.EventController).setPropagationPhase(.capture);
    _=gtk.EventControllerLegacy.signals.event.connect(events,*Window,resizePointerEvent,window,.{});
    window.as(gtk.Widget).addController(events.as(gtk.EventController));
}
fn resizePointerEvent(_: *gtk.EventControllerLegacy,event:*gdk.Event,window:*Window) callconv(.c) c_int {
    if(event.getEventType()==.button_release or event.getEventType()==.touch_end){
        _=window.as(gobject.Object).ref();
        _ = glib.idleAddOnce(resizePointerEnded,window);
    }
    return 0;
}
fn resizePointerEnded(raw:?*anyopaque) callconv(.c) void {
    const window:*Window=@ptrCast(@alignCast(raw orelse return));defer window.as(gobject.Object).unref();
    const current=if(state) |*value| value else return;
    if(current.executorForWindow(window)) |executor| executor.saveNativeSplitRatios() catch |err| current.resources_window.showError(@errorName(err));
}

pub fn installShortcuts(app: *Application) void {
    shortcuts.install(app);
}

/// Return false for disabled or unavailable commands so Ghostty receives the
/// original key event. The window-level HUD never invents backend actions.
pub fn dispatchWindowShortcut(window: *Window, action: shortcuts.Action) bool {
    const current = if (state) |*value| value else return false;
    const active = activeGhosttyWindow(current.app) orelse return false;
    if (active != window) return false;

    switch (action) {
        .insert_idea_node, .edit_idea_instruction => {return if(current.lgp) |lgp| lgp.ideaInstruction(window,action==.edit_idea_instruction) else false;},
        .open_file_reference => {openSelectedFileInWindow(window);return true;},
        .quote_selection => { return if(current.lgp) |lgp| lgp.quoteSelection(window) else false; },
        .open_inquiry => {return if(current.lgp) |lgp| lgp.openInquiry(window) else false;},
        .open_idea_tree => {
            showIdeaTree(current.app);
            return true;
        },
        .close_focused_pane => {
            const target = current.activeExecutor().closeFocusedPaneTarget(window) orelse
                return true;
            switch (target) {
                .managed => |pane_id| sendControllerRequest(current, 0, .{
                    .remove_pane = .{ .pane_id = pane_id },
                }),
                .native_split => |surface| surface.close(),
            }
            return true;
        },
        .control_center_toggle => {
            toggleControlCenter(current.app);
            return true;
        },
        .previous_tab, .next_tab => {
            const executor = current.activeExecutor();
            const snapshot = executor.nativeWorkspaceSnapshot() orelse return false;
            if (snapshot.tabs.len <= 1) return true;
            for (snapshot.tabs, 0..) |tab, index| {
                if (!std.mem.eql(u8, tab.id, snapshot.selected_tab_id)) continue;
                const offset: usize = if (action == .previous_tab) snapshot.tabs.len - 1 else 1;
                const target = snapshot.tabs[(index + offset) % snapshot.tabs.len].id;
                sendControllerRequest(current, 0, .{ .select_tab = .{ .tab_id = target } });
                return true;
            }
            return false;
        },
        .focus_previous, .focus_next => {
            if (!current.activeExecutor().canCycleNativeFocus()) return false;
            if (action == .focus_previous) {
                focusPrevious(current.app);
            } else {
                focusNext(current.app);
            }
            return true;
        },
        .focus_pane_number => |number| {
            const borrowed_id = current.activeExecutor().nativeNumberedPaneTarget(number) orelse return false;
            const pane_id = current.allocator.dupe(u8, borrowed_id) catch return false;
            defer current.allocator.free(pane_id);
            sendControllerRequest(current, 0, .{ .activate_pane = .{ .pane_id = pane_id } });
            return true;
        },
        .workspace_mode_toggle => {
            const page = window.getTabView().getSelectedPage() orelse return false;
            return toggleNativeTabMode(current, page);
        },
        .new_shell, .new_codex, .resume_codex, .new_workspace, .rename_workspace, .refresh_sessions => {
            const controller = ensureWindowController(current, window) orelse return false;
            return controller.performApplicationShortcut(action);
        },
    }
}

pub fn dispatchShortcut(app: *Application, action: shortcuts.Action) void {
    switch (action) {
        .previous_tab, .next_tab, .new_shell, .new_codex, .resume_codex, .open_idea_tree, .open_inquiry, .insert_idea_node, .edit_idea_instruction, .quote_selection, .open_file_reference, .close_focused_pane => {
            const window = activeGhosttyWindow(app) orelse return;
            _ = dispatchWindowShortcut(window, action);
        },
        .control_center_toggle => toggleControlCenter(app),
        .focus_previous => focusPrevious(app),
        .focus_next => focusNext(app),
        .focus_pane_number => |number| focusPaneNumber(app, number),
        .workspace_mode_toggle => {
            const window = activeGhosttyWindow(app) orelse return;
            _ = dispatchWindowShortcut(window, action);
        },
        .new_workspace,
        .rename_workspace,
        .refresh_sessions,
        => if (state) |*current| {
            if (current.app != app) return;
            const window = activeGhosttyWindow(app) orelse return;
            for (current.controllers.items) |controller| {
                if (controller.ownsHostWindow(window)) {
                    _ = controller.performApplicationShortcut(action);
                    return;
                }
            }
        },
    }
}

fn syncFocusCycleActions(current: *State) void {
    const projection = current.controller_model.projection();
    const focus_enabled = current.activeExecutor().canCycleNativeFocus();
    const action_map: *gio.ActionMap = gobject.ext.cast(
        gio.ActionMap,
        current.app,
    ) orelse return;
    for ([_][:0]const u8{
        "polar-focus-previous",
        "polar-focus-next",
    }) |name| {
        setShortcutActionEnabled(action_map, name, focus_enabled);
    }
    for (1..9) |slot| {
        var name_buffer: [32]u8 = undefined;
        const name = std.fmt.bufPrintZ(
            &name_buffer,
            "polar-focus-slot-{d}",
            .{slot},
        ) catch continue;
        const available = current.activeExecutor().nativeNumberedPaneTarget(@intCast(slot));
        setShortcutActionEnabled(action_map, name, available != null);
    }
    setShortcutActionEnabled(
        action_map,
        "polar-workspace-mode",
        current.activeExecutor().nativeSelectedTabId() != null,
    );
    setShortcutActionEnabled(
        action_map,
        "polar-workspace-new",
        projectedActionEnabled(projection, "workspace.create"),
    );
    setShortcutActionEnabled(
        action_map,
        "polar-workspace-rename",
        projectedActionEnabled(projection, "workspace.rename"),
    );
    setShortcutActionEnabled(
        action_map,
        "polar-sessions-refresh",
        if (projection) |value| value.sessions.refresh_action.enabled else false,
    );
}

fn projectedActionEnabled(
    projection: ?*const controller_model.Projection,
    id: []const u8,
) bool {
    const value = projection orelse return false;
    const action = value.action(id) orelse return false;
    return action.enabled;
}

fn setShortcutActionEnabled(
    action_map: *gio.ActionMap,
    name: [:0]const u8,
    enabled: bool,
) void {
    const action: *gio.SimpleAction = gobject.ext.cast(
        gio.SimpleAction,
        action_map.lookupAction(name) orelse return,
    ) orelse return;
    action.setEnabled(@intFromBool(enabled));
}

pub fn deinit(app: *Application) void {
    if (state) |*current| {
        if (current.app != app) return;
        current.deinit();
        state = null;
    }
}

fn wakeApplication(context: ?*anyopaque) void {
    const app: *Application = @ptrCast(@alignCast(context orelse return));
    app.wakeup();
}

fn handleHostFrame(
    current: *State,
    generation: u64,
    payload: []const u8,
    batch: *host_frame_batch.Batch,
) void {
    const kind = protocol.classifyHostFrame(
        current.allocator,
        payload,
        current.identity(),
    ) catch return;
    switch (kind) {
        .registered => {
            var registration = protocol.decodeRegisteredFrame(current.allocator, payload) catch return;
            defer registration.deinit();
            const host_id = registration.value.host_id;
            const versions: protocol.HostControlVersions = registration.value.host_control_versions orelse .{};
            current.controller_model.setConnected(host_id) catch return;
            current.backend_state = .ready;
            current.resources_window.setBackendAvailable(true);
            current.settings_window.setBackendAvailable(true);
            current.codex_window.connected(host_id, versions.codex == 2);
            current.connector_window.connected(host_id, versions.notification == 1, versions.codex == 2);
            if (current.lgp) |lgp| lgp.connected(generation, registration.value.supportsPolarAccess(), host_id, versions.codex == 2);
            current.renderControllers();
            resetControlRefresh(current);
            resetResourcesRefresh(current);
            requestControlProjection(current);
            requestResourcesProjection(current);
            requestSettingsProjection(current);
            requestInbox(current);
            sendSessionSubscription(current);
        },
        .application_response => {
            const response_api = protocol.decodeApplicationResponseApi(
                current.allocator,
                payload,
            ) catch return;
            if (response_api == .resources_v1) {
                handleResourcesResponse(current, payload);
                return;
            }
            if (response_api == .settings_v1) {
                handleSettingsResponse(current, payload);
                return;
            }
            if (response_api == .codex_v1) {
                for(current.controllers.items)|controller|{if(controller.connections)|connections|connections.receiveCodex(payload);}
                current.codex_window.receive(payload);
                current.connector_window.receive(payload);
                return;
            }
            if (response_api == .notification_v1) {
                current.connector_window.receive(payload);
                return;
            }
            if (response_api == .inbox_v1) {
                handleInboxResponse(current, payload);
                return;
            }
            var outcome =
                protocol.decodeControlCenterResponseOutcome(
                    current.allocator,
                    payload,
                ) catch return;
            defer outcome.deinit(current.allocator);
            const completes_refresh = if (current.control_refresh_request_id != null)
                std.mem.eql(
                    u8,
                    outcome.request_id,
                    current.control_refresh_request_id.?,
                )
            else
                false;
            if (completes_refresh) {
                current.allocator.free(
                    current.control_refresh_request_id.?,
                );
                current.control_refresh_request_id = null;
            }
            const session_create_response = if (current.pending_control_effects.get(outcome.request_id)) |effect| effect.kind == .session_create else false;
            if (session_create_response) ipc_client.trace(47, outcome.request_id, 0);
            var accepted_session_create_controller: ?u64 = null;
            const response_applied = if (outcome.kind == .structure_accepted)
                true
            else if (current.controller_model.applyApplicationResponse(payload)) applied: {
                if (current.controller_model.projection()) |projection| {
                    current.executor.applyControlProjection(projection) catch {};
                    for (current.window_executors.items) |executor| executor.applyControlProjection(projection) catch {};
                }
                break :applied true;
            } else |decode_error| failed: {
                const message = std.fmt.allocPrint(
                    current.allocator,
                    "Control Center response could not be decoded: {s}. The installed frontend and Host may be incompatible.",
                    .{@errorName(decode_error)},
                ) catch break :failed false;
                defer current.allocator.free(message);
                current.controller_model.setLocalError(message);
                current.renderControllers();
                break :failed false;
            };
            if (current.pending_control_effects.fetchRemove(
                outcome.request_id,
            )) |pending| {
                releaseCreationReservation(current, pending.value);
                defer current.allocator.free(pending.key);
                defer if (pending.value.selection_json) |json| current.allocator.free(json);
                if (pending.value.kind == .quick_create) {
                    _ = completeQuickCreate(current, pending.value, outcome);
                } else if (outcome.kind.accepted() and pending.value.kind == .session_create) {
                    if (completeQuickCreate(current, pending.value, outcome)) {
                        accepted_session_create_controller = pending.value.controller_id;
                    } else if (controllerById(current, pending.value.controller_id)) |controller| {
                        // Retain the exact request ID; Retry consumes the committed receipt.
                        controller.sessionCreateRejected(null);
                    }
                } else if (outcome.kind.accepted() and response_applied) {
                    presentAcceptedControlEffect(
                        current,
                        pending.value,
                        payload,
                    );
                } else {
                    presentRejectedControlEffect(
                        current,
                        pending.value,
                        outcome.error_message,
                    );
                    if (pending.value.kind == .session_create) if (controllerById(current, pending.value.controller_id)) |controller| controller.sessionCreationRecovery(payload);
                }
            }
            if (response_applied) {
                current.controller_model.applySessionRuntime(
                    &current.session_cache,
                );
                current.renderControllers();
            }
            if (accepted_session_create_controller) |controller_id| {
                if (controllerById(current, controller_id)) |controller| {
                    const revision = if (current.controller_model.projection()) |projection| projection.control_revision else 0;
                    ipc_client.trace(48, outcome.request_id, revision);
                    if (@import("../../../polar_station_trace.zig").enabled()) @import("frame_trace.zig").observeControl(controller.host_window.as(gtk.Widget), revision);
                    controller.sessionCreateAccepted();
                }
            }
            if (outcome.kind == .structure_accepted or
                outcome.kind == .structure_conflict)
            {
                requestControlProjection(current);
            }
            if (completes_refresh and
                current.control_refresh.complete() == .request_projection)
            {
                emitControlProjectionRequest(current);
            }
        },
        .application_state_changed => {
            const change = protocol.decodeApplicationStateChange(
                current.allocator,
                payload,
            ) catch return;
            if (change.api == .control_center_v1) {
                if (current.control_refresh.observe(
                    change.state_revision,
                ) == .request_projection) {
                    emitControlProjectionRequest(current);
                }
            } else if (change.api == .resources_v1 and
                current.resources_refresh.observe(
                    change.state_revision,
                ) == .request_projection)
            {
                emitResourcesProjectionRequest(current);
            } else if (change.api == .inbox_v1) {
                requestInbox(current);
            } else if (change.api == .codex_v1) {
                current.codex_window.changed();
                if (current.control_refresh.request() == .request_projection) emitControlProjectionRequest(current);
                requestInbox(current);
            } else if (change.api == .notification_v1) {
                current.connector_window.changed();
            } else if (change.api == .settings_v1) {
                requestSettingsProjection(current);
            }
        },
        .session_feed_snapshot => {
            current.session_cache.applySnapshot(payload) catch {
                sendSessionSubscription(current);
                return;
            };
            retireEndedSessionPanes(current);
            observeNativeInboxAttention(current);
            batch.requestSessionProjection();
        },
        .session_feed_event => {
            current.session_cache.applyEvent(payload) catch {
                sendSessionSubscription(current);
                return;
            };
            retireEndedSessionPanes(current);
            observeNativeInboxAttention(current);
            batch.requestSessionProjection();
        },
        .workspace_relay => handleWorkspaceRelay(current, payload),
        .response => {},
        .frontend_execute => {},
        .ping => {
            const nonce = protocol.decodePingNonce(current.allocator, payload) catch return;
            const response = protocol.encodePong(current.allocator, nonce) catch return;
            defer current.allocator.free(response);
            current.client.sendJson(response) catch {};
        },
    }
}

fn retireEndedSessionPanes(current: *State) void {
    retireEndedExecutorPanes(current, &current.executor);
    for (current.window_executors.items) |executor| retireEndedExecutorPanes(current, executor);
}

fn retireEndedExecutorPanes(current: *State, executor: *presentation_executor.Executor) void {
    const snapshot = executor.nativeWorkspaceSnapshot() orelse return;
    var arena = std.heap.ArenaAllocator.init(current.allocator);
    defer arena.deinit();
    const alloc = arena.allocator();
    var panes: std.ArrayList([]const u8) = .empty;
    for (snapshot.tabs) |tab| for (tab.panes) |pane| {
        if (current.session_cache.lifecycle(pane.execution_target_id, pane.tmux_session_id) == .ended) {
            panes.append(alloc, alloc.dupe(u8, pane.id) catch return) catch return;
        }
    };
    for (panes.items) |id| {
        _ = executor.applyNativeWorkspaceAction(.{ .remove_pane = .{ .pane_id = id } }) catch |err| {
            std.log.warn("Ended Session Pane removal failed: {}", .{err});
            continue;
        };
    }
}

fn notificationActivated(_: *gio.SimpleAction, parameter: ?*glib.Variant, app: *Application) callconv(.c) void {
    const current = if (state) |*value| value else return;
    if (current.app != app) return;
    const target = std.mem.span((parameter orelse return).getString(null));
    const executor = current.executorForWorkspace(target) orelse return;
    const window = executor.managed_window orelse return;
    const controller = ensureWindowController(current, window) orelse return;
    controller.presentInboxSummary();
    requestInbox(current);
}

fn notifyInboxAttention(current: *State) void {
    const page = current.inbox_model.page() orelse return;
    var visible: std.ArrayList([]const u8) = .empty;
    defer visible.deinit(current.allocator);
    for (current.controllers.items) |controller| {
        if (controller.isVisible() and controller.layout_map_visible and controller.host_window.as(gtk.Widget).getVisible() != 0) {
            if (current.controllerExecutor(controller.controller_id)) |executor| {
                if (executor.nativeWorkspaceSnapshot()) |workspace| visible.append(current.allocator, workspace.workspace_id) catch return;
            }
        }
    }
    const settings_projection = current.settings_model.projection();
    const output = current.attention_engine.dispatch(current.allocator, .{ .type = "messages", .messages = page.messages,
        .visible_workspaces = visible.items, .enabled = if (settings_projection) |v| v.attention.system_notifications_enabled else false }) catch return;
    defer output.deinit();
    for (output.value.notify) |notice| {
        const notification = gio.Notification.new("Polar Station");
        defer notification.unref();
        notification.setBody("A session needs your attention.");
        const icon = gio.ThemedIcon.new("com.polarealm.PolarStation");
        defer icon.unref();
        notification.setIcon(icon.as(gio.Icon));
        const workspace = current.allocator.dupeZ(u8, notice.workspace_id) catch continue;
        defer current.allocator.free(workspace);
        notification.setDefaultActionAndTargetValue("app.polar-station-inbox", glib.Variant.newString(workspace));
        const id = std.fmt.allocPrintSentinel(current.allocator, "polar-station-attention-{s}", .{notice.workspace_id}, 0) catch continue;
        defer current.allocator.free(id);
        current.app.as(gio.Application).sendNotification(id, notification);
    }
}

fn observeNativeInboxAttention(current: *State) void {
    const catalog = current.activeExecutor().nativeWorkspaceCatalog() orelse return;
    var arena = std.heap.ArenaAllocator.init(current.allocator);
    defer arena.deinit();
    const alloc = arena.allocator();
    var observations: std.ArrayList(attention_interaction.Observation) = .empty;
    for (catalog.workspaces) |workspace| {
        for (workspace.tabs) |tab| for (tab.panes) |pane| {
            const attention = current.session_cache.attention(pane.execution_target_id, pane.tmux_session_id) orelse continue;
            const token = std.fmt.allocPrint(alloc, "{s}:{d}", .{ workspace.workspace_id, workspace.workspace_revision }) catch return;
            observations.append(alloc, .{ .origin = .{
                .workspace_id = workspace.workspace_id, .tab_id = tab.id, .pane_id = pane.id,
                .execution_target_id = pane.execution_target_id, .tmux_session_id = pane.tmux_session_id,
                .consumer_generation = workspace.workspace_revision, .native_recovery_token = token,
            }, .state = attention.state, .freshness = attention.freshness, .attention_since_ms = attention.attention_since_ms }) catch return;
        };
    }
    const output = current.attention_engine.dispatch(alloc, .{ .type = "observe", .observations = observations.items }) catch return;
    defer output.deinit();
    for (output.value.observe) |origin| {
        const request_id = nextRequestId(current) orelse continue;
        defer current.allocator.free(request_id);
        const payload = inbox.encodeObserveAttention(alloc, request_id, origin) catch continue;
        current.client.sendJson(payload) catch continue;
    }
}

fn handleWorkspaceRelay(current: *State, payload: []const u8) void {
    var request = workspace_relay.decode(
        current.allocator,
        payload,
    ) catch |decode_error| {
        std.log.err(
            "Polar Station rejected invalid Workspace relay: {s}",
            .{@errorName(decode_error)},
        );
        return;
    };
    defer request.deinit();

    const requested_workspace = workspace_relay.scopeWorkspaceId(&request) catch null;
    const executor = if (requested_workspace) |id| current.executorForWorkspace(id) orelse current.activeExecutor() else current.activeExecutor();
    const initial_catalog = executor.nativeWorkspaceCatalog() orelse {
        sendWorkspaceRelayError(
            current,
            &request,
            0,
            "desktop_workspace_unavailable",
            "Polar Station has not loaded its native Workspace",
            true,
        );
        return;
    };
    const initial = initial_catalog.selected() orelse {
        sendWorkspaceRelayError(
            current,
            &request,
            initial_catalog.catalog_revision,
            "desktop_workspace_unavailable",
            "Polar Station has not selected a native Workspace",
            true,
        );
        return;
    };
    const initial_revision = initial.workspace_revision;
    const scope_workspace_id = workspace_relay.scopeWorkspaceId(
        &request,
    ) catch {
        sendWorkspaceRelayError(
            current,
            &request,
            initial_revision,
            "workspace_relay_invalid",
            "The relayed Workspace scope is invalid",
            false,
        );
        return;
    };
    const scoped_workspace = if (scope_workspace_id) |workspace_id| blk: {
        for (initial_catalog.workspaces) |workspace| {
            if (std.mem.eql(u8, workspace_id, workspace.workspace_id)) {
                break :blk workspace;
            }
        }
        sendWorkspaceRelayError(
            current,
            &request,
            initial_catalog.catalog_revision,
            "workspace_not_found",
            "The native Workspace does not exist",
            false,
        );
        return;
    } else initial;
    if (scope_workspace_id) |workspace_id| {
        if (!std.mem.eql(u8, workspace_id, initial.workspace_id) and
            !request.command.isRead())
        {
            sendWorkspaceRelayError(
                current,
                &request,
                initial_revision,
                "workspace_not_found",
                "The native Workspace does not exist",
                false,
            );
            return;
        }
    }
    if (!request.acceptsRevisions(
        initial_catalog.catalog_revision,
        scoped_workspace.workspace_revision,
    )) {
        sendWorkspaceRelayError(
            current,
            &request,
            if (scope_workspace_id == null)
                initial_catalog.catalog_revision
            else
                scoped_workspace.workspace_revision,
            "workspace_revision_conflict",
            "The native Workspace changed; refresh and retry",
            true,
        );
        return;
    }

    applyWorkspaceRelayCommand(current, executor, &request, initial) catch |apply_error| {
        const revision = if (executor.nativeWorkspaceSnapshot()) |snapshot|
            snapshot.workspace_revision
        else
            initial_revision;
        const failure = workspaceRelayFailure(apply_error);
        sendWorkspaceRelayError(
            current,
            &request,
            revision,
            failure.code,
            failure.message,
            failure.retryable,
        );
        return;
    };

    const accepted_catalog = executor.nativeWorkspaceCatalog() orelse {
        sendWorkspaceRelayError(
            current,
            &request,
            initial_revision,
            "desktop_workspace_unavailable",
            "Polar Station lost its native Workspace",
            true,
        );
        return;
    };
    const response = workspace_relay.encodeSuccess(
        current.allocator,
        &request,
        accepted_catalog,
        &current.session_cache,
    ) catch return;
    defer current.allocator.free(response);
    current.client.sendJson(response) catch {};
    current.renderControllers();
}

fn applyWorkspaceRelayCommand(
    current: *State,
    executor: *presentation_executor.Executor,
    request: *const workspace_relay.Request,
    snapshot: @import("workspace_rendering.zig").Snapshot,
) !void {
    switch (request.command) {
        .get_catalog, .get_workspace, .open => {},
        else => for (current.controllers.items) |controller| controller.prepareTerminalFocus(false),
    }
    switch (request.command) {
        .get_catalog, .get_workspace, .open => {},
        .repair => try executor.repairNativeWorkspace(),
        .select_workspace => |workspace_id| {
            _ = try executor.applyNativeWorkspaceAction(
                .{ .select_workspace = .{ .workspace_id = workspace_id } },
            );
        },
        .select_tab => |tab_id| {
            _ = snapshot.tab(tab_id) orelse return error.TabNotFound;
            if (!std.mem.eql(u8, tab_id, snapshot.selected_tab_id)) {
                _ = try executor.applyNativeWorkspaceAction(
                    .{ .select_tab = .{ .tab_id = tab_id } },
                );
            }
        },
        .set_display_mode => |value| {
            const tab = snapshot.tab(value.tab_id) orelse
                return error.TabNotFound;
            if (tab.display_mode != value.display_mode) {
                _ = try executor.applyNativeWorkspaceAction(
                    .{ .toggle_display_mode = .{ .tab_id = value.tab_id } },
                );
            }
        },
        .select_focus_pane => |value| {
            const tab = snapshot.tab(value.tab_id) orelse
                return error.TabNotFound;
            _ = tab.pane(value.pane_id) orelse return error.PaneNotFound;
            _ = try executor.applyNativeWorkspaceAction(
                .{ .activate_pane = .{ .pane_id = value.pane_id } },
            );
        },
        .focus_previous => {
            const pane_id = try executor.nativeFocusCycleTarget(
                .previous,
            ) orelse return error.PaneNotFound;
            _ = try executor.applyNativeWorkspaceAction(
                .{ .activate_pane = .{ .pane_id = pane_id } },
            );
        },
        .focus_next => {
            const pane_id = try executor.nativeFocusCycleTarget(
                .next,
            ) orelse return error.PaneNotFound;
            _ = try executor.applyNativeWorkspaceAction(
                .{ .activate_pane = .{ .pane_id = pane_id } },
            );
        },
        .focus_sidebar_slot => |slot| {
            const tab = snapshot.tab(snapshot.selected_tab_id) orelse
                return error.TabNotFound;
            if (tab.display_mode != .focus or
                slot >= tab.visual_pane_ids.len)
            {
                return error.PaneNotFound;
            }
            _ = try executor.applyNativeWorkspaceAction(
                .{ .activate_pane = .{
                    .pane_id = tab.visual_pane_ids[slot],
                } },
            );
        },
    }
}

const WorkspaceRelayFailure = struct {
    code: []const u8,
    message: []const u8,
    retryable: bool,
};

fn workspaceRelayFailure(apply_error: anyerror) WorkspaceRelayFailure {
    return switch (apply_error) {
        error.WorkspaceNotFound => .{
            .code = "workspace_not_found",
            .message = "The native Workspace does not exist",
            .retryable = false,
        },
        error.TabNotFound => .{
            .code = "tab_not_found",
            .message = "The native Tab does not exist",
            .retryable = false,
        },
        error.PaneNotFound => .{
            .code = "pane_not_found",
            .message = "The native Pane does not exist",
            .retryable = false,
        },
        else => .{
            .code = "desktop_workspace_apply_failed",
            .message = "The native Workspace change could not be applied",
            .retryable = true,
        },
    };
}

fn sendWorkspaceRelayError(
    current: *State,
    request: *const workspace_relay.Request,
    revision: u64,
    code: []const u8,
    message: []const u8,
    retryable: bool,
) void {
    const response = workspace_relay.encodeError(
        current.allocator,
        request,
        revision,
        code,
        message,
        retryable,
    ) catch return;
    defer current.allocator.free(response);
    current.client.sendJson(response) catch {};
}

fn sendSessionSubscription(current: *State) void {
    const payload = current.session_cache.encodeSubscription(
        current.allocator,
    ) catch return;
    defer current.allocator.free(payload);
    current.client.sendJson(payload) catch {};
}

fn controllerPaneDrop(
    context: ?*anyopaque,
    controller_id: u64,
    drop: @import("pane_interaction.zig").PaneDrop,
) void {
    const current: *State = @ptrCast(@alignCast(context orelse return));
    const controller = controllerById(current, controller_id) orelse return;
    for (current.controllers.items) |item| item.prepareTerminalFocus(false);
    const executor = current.controllerExecutor(controller_id) orelse return;
    const changed = executor.applyPaneDrop(drop) catch |err| {
        std.log.warn("Polar Station Pane drop rejected: {s}", .{@errorName(err)});
        current.renderControllers();
        controller.reportPaneDropError(err);
        return;
    };
    if (changed) {
        std.log.info("Polar Station native Pane drop committed", .{});
        current.renderControllers();
    }
}

fn controllerAction(
    context: ?*anyopaque,
    controller_id: u64,
    action: controller_pane.Action,
) void {
    const current: *State = @ptrCast(@alignCast(context orelse return));
    presentAuxiliaryImmediately(current, controller_id, action);
    sendControllerRequest(current, controller_id, action);
}

fn presentAuxiliaryImmediately(
    current: *State,
    controller_id: u64,
    action: controller_pane.Action,
) void {
    const controller = controllerById(current, controller_id) orelse return;
    if (action == .cancel_session_editor) {
        var pending = current.pending_control_effects.iterator();
        while (pending.next()) |entry| {
            if (entry.value_ptr.controller_id != controller_id or entry.value_ptr.kind != .session_editor) continue;
            const meta = std.json.parseFromSlice(struct { kind: []const u8, generation: u64 }, current.allocator, entry.value_ptr.selection_json orelse continue, .{}) catch continue;
            defer meta.deinit();
            if (std.mem.eql(u8, meta.value.kind, if (action.cancel_session_editor.ending) "shared_end" else "shared_editor") and meta.value.generation == action.cancel_session_editor.token) current.client.cancelRequest(entry.key_ptr.*);
        }
        return;
    }
    switch (action) {
        .invoke_action => |value| {
            if (std.mem.eql(u8, value.action_id, "resources.open")) {
                current.resources_window.present(
                    current.resources_model.projection(),
                );
                requestResourcesProjection(current);
            } else if (std.mem.eql(
                u8,
                value.action_id,
                "iphone.open",
            )) {
                controller.presentAcceptedAuxiliary(.iphone);
            }
        },
        else => {},
    }
}

fn pendingControlEffect(
    controller_id: u64,
    action: controller_pane.Action,
) ?PendingControlEffect {
    const kind: ?PendingControlEffectKind = switch (action) {
        .invoke_action => null,
        .update_connection, .check_connection => .connection_update,
        .set_workspace_ssh_target => .ssh_target,
        .create_session, .create_codex_session, .create_agent_session => .session_create,
        .rename_session, .rename_agent_session => .session_rename,
        .kill_session, .end_agent_session => .session_kill,
        .session_editor => .session_editor,
        .use_existing_session => .session_replacement,
        .retire_lost_session => .session_retirement,
        else => null,
    };
    return .{
        .kind = kind orelse return null,
        .controller_id = controller_id,
    };
}

fn controllerById(
    current: *State,
    controller_id: u64,
) ?*controller_pane.Controller {
    for (current.controllers.items) |controller| {
        if (controller.controller_id == controller_id and
            !controller.isOrphaned())
        {
            return controller;
        }
    }
    return null;
}

fn presentAcceptedControlEffect(
    current: *State,
    effect: PendingControlEffect,
    payload: []const u8,
) void {
    const controller = controllerById(
        current,
        effect.controller_id,
    ) orelse return;
    switch (effect.kind) {
        .connection_update => controller.connectionResult(null),
        .ssh_target => controller.sshTargetAccepted(),
        .quick_create => {},
        .session_create => controller.sessionCreateAccepted(),
        .session_rename => controller.renameAccepted(),
        .session_kill => controller.sessionEndResult(effect.end_token, null),
        .session_editor => controller.sessionEditorResult(effect.selection_json orelse return, payload),
        .session_retirement => {},
        .session_replacement => {
            const json = effect.selection_json orelse return;
            const parsed = std.json.parseFromSlice(control_center_intent.SessionReplacementSelection, current.allocator, json, .{}) catch return;
            defer parsed.deinit();
            const executor = current.controllerExecutor(effect.controller_id) orelse return;
            _ = executor.applyNativeWorkspaceAction(.{ .use_existing_session = parsed.value }) catch |err| {
                current.controller_model.setLocalError(if (err == error.AlreadyAssignedInWorkspace) "Already assigned in this Workspace" else if (err == error.WorkspaceChanged) "The selected Workspace changed. Choose the Session again." else "The Workspace could not be saved; the lost record was kept.");
                current.renderControllers();
                return;
            };
            sendControllerRequest(current, effect.controller_id, .{ .retire_lost_session = parsed.value });
        },
    }
}

fn presentRejectedControlEffect(
    current: *State,
    effect: PendingControlEffect,
    message: ?[]const u8,
) void {
    if (effect.kind == .quick_create) {
        reportQuickCreateError(current, message orelse "Creation result is unknown. Refresh Sessions before retrying.");
        return;
    }
    const controller = controllerById(current, effect.controller_id) orelse return;
    switch (effect.kind) {
        .quick_create => unreachable,
        .connection_update => controller.connectionResult(message orelse "Connection save failed."),
        .ssh_target => controller.sshTargetRejected(),
        .session_create => controller.sessionCreateRejected(message),
        .session_rename => controller.renameRejected(message),
        .session_kill => controller.sessionEndResult(effect.end_token, message orelse "Session end result is unknown. Refresh before retrying."),
        .session_editor => controller.sessionEditorFailure(effect.selection_json orelse return, message orelse "The Station Host connection was interrupted. Retry the request."),
        .session_replacement => current.controller_model.setLocalError(message orelse "Session replacement did not complete. Refresh Sessions before retrying."),
        .session_retirement => {
            const text = std.fmt.allocPrint(current.allocator, "Workspace saved; the lost record could not be removed. {s}", .{message orelse "Refresh Sessions before retrying."}) catch return;
            defer current.allocator.free(text);
            current.controller_model.setLocalError(text);
        },
    }
}

fn rejectPendingControlEffect(
    current: *State,
    effect: PendingControlEffect,
) void {
    releaseCreationReservation(current, effect);
    presentRejectedControlEffect(current, effect, null);
}

fn rejectAllPendingControlEffects(current: *State) void {
    var iterator = current.pending_control_effects.valueIterator();
    while (iterator.next()) |effect| {
        rejectPendingControlEffect(current, effect.*);
    }
    clearPendingControlEffects(current);
}

fn clearPendingControlEffects(current: *State) void {
    var iterator = current.pending_control_effects.iterator();
    while (iterator.next()) |entry| {
        releaseCreationReservation(current, entry.value_ptr.*);
        current.allocator.free(entry.key_ptr.*);
        if (entry.value_ptr.selection_json) |json| current.allocator.free(json);
    }
    current.pending_control_effects.clearRetainingCapacity();
}

fn codexServerStarted(raw:?*anyopaque,controller_id:u64,token:u64,message:?[]const u8) void {
    const current:*State=@ptrCast(@alignCast(raw orelse return));
    if(controllerById(current,controller_id)) |controller| controller.editorInput(.{.type="server_started",.token=token,.@"error"=message});
}
fn sendControllerRequest(
    current: *State,
    controller_id: u64,
    action: controller_pane.Action,
) void {
    if (current.quitting) return;
    if(action==.start_local_codex){const value=action.start_local_codex;current.codex_window.startLocalServer(value.owner_host_id,value.endpoint_id,controller_id,value.token);return;}
    const executor = current.controllerExecutor(controller_id) orelse return;
    if (action == .invoke_action and (std.mem.eql(u8, action.invoke_action.action_id, "workspace.window.new") or std.mem.eql(u8, action.invoke_action.action_id, "native.open_local_terminal"))) {
        var decision = executor.lifecycleDecision("new_window", null) catch return;
        defer decision.deinit();
        if (decision.value.@"error") |message| { current.controller_model.setLocalError(message); current.renderControllers(); return; }
        current.pending_unbound_windows += 1;
        _ = current.app.performAction(.app, .new_window, {}) catch |err| {
            current.pending_unbound_windows -|= 1;
            current.controller_model.setLocalError(@errorName(err));
            current.renderControllers();
            return;
        };
        return;
    }
    if (action == .invoke_action) {
        const id = action.invoke_action.action_id;
        if (std.mem.eql(u8, id, "window.close") or std.mem.eql(u8, id, "workspace.detach")) {
            if (executor.managed_window) |window| window.as(gtk.Window).close();
            return;
        }
        if (std.mem.eql(u8, id, "workspace.archive") or std.mem.eql(u8, id, "workspace.unarchive")) {
            const target = action.invoke_action.confirmation_id orelse return;
            executor.archiveOrRestore(std.mem.eql(u8, id, "workspace.archive"), target) catch |err| {
                current.controller_model.setLocalError(@errorName(err)); current.renderControllers();
            };
            return;
        }
        if (std.mem.eql(u8, id, "workspace.delete")) {
            current.controller_model.setLocalError("Workspaces cannot be deleted. Use Archive Workspace."); current.renderControllers(); return;
        }
    }
    if (action == .create_workspace or action == .select_workspace) {
        var decision = executor.lifecycleDecision(if (action == .create_workspace) "create" else "open",
            if (action == .select_workspace) action.select_workspace.workspace_id else null) catch return;
        defer decision.deinit();
        if (decision.value.@"error") |message| { current.controller_model.setLocalError(message); current.renderControllers(); return; }
    }
    if (action == .select_workspace) {
        if (current.executorForWorkspace(action.select_workspace.workspace_id)) |owner| {
            if (owner != executor) {
                if (owner.managed_window) |window| { window.as(gtk.Window).present(); return; }
            }
        }
    }
    switch (action) {
        .open_codex_thread => |value| { current.codex_window.presentOriginal(value.owner, value.binding); return; },
        .open_inbox_session => |value| {
            if (executor.nativePaneIdForSession(value.execution_target_id, value.tmux_session_id)) |pane| {
                sendControllerRequest(current, controller_id, .{ .activate_pane = .{ .pane_id = pane } });
            } else {
                sendControllerRequest(current, controller_id, .{ .toggle_session_assignment = .{
                    .execution_target_id = value.execution_target_id, .tmux_session_id = value.tmux_session_id, .focus_new = true,
                } });
            }
            return;
        },
        .open_codex_settings => {
            var machine: ?[]const u8 = null;
            for (current.controllers.items) |controller| {
                if (controller.controller_id == controller_id) {
                    if (controller.connections) |connections| machine = connections.selectedMachineId();
                }
            }
            current.codex_window.presentSettingsForMachine(machine);
            return;
        },
        .open_codex_sessions => |value| {
            current.codex_window.presentAllForTarget(value.target_id);
            return;
        },
        .open_codex => |value| {
            current.codex_window.present(.{ .execution_target = value.execution_target_id, .tmux_session_id = value.tmux_session_id });
            return;
        },
        .inbox_refresh => {
            requestInbox(current);
            return;
        },
        .inbox_reply => |value| {
            const request_id = nextRequestId(current) orelse return;
            defer current.allocator.free(request_id);
            const payload = inbox.encodeReply(current.allocator, request_id, value.message_id, value.revision, value.reply) catch return;
            defer current.allocator.free(payload);
            current.client.sendJson(payload) catch {};
            return;
        },
        .inbox_edit_draft => |value| {
            const request_id = nextRequestId(current) orelse return;
            defer current.allocator.free(request_id);
            const payload = inbox.encodeEditDraft(current.allocator, request_id, value.message_id, value.revision, value.draft) catch return;
            defer current.allocator.free(payload);
            current.client.sendJson(payload) catch {};
            return;
        },
        .inbox_approve => |value| {
            const request_id = nextRequestId(current) orelse return;
            defer current.allocator.free(request_id);
            const nonce = std.fmt.allocPrint(current.allocator, "linux-approval-{d}-{d}", .{ std.os.linux.getpid(), std.time.nanoTimestamp() }) catch return;
            defer current.allocator.free(nonce);
            const mode = if (current.settings_model.projection()) |projection|
                projection.appearance.terminal_delivery_mode
            else
                "insert";
            const payload = inbox.encodeApprove(current.allocator, request_id, value.message_id, value.revision, mode, nonce) catch return;
            defer current.allocator.free(payload);
            current.client.sendJson(payload) catch {};
            return;
        },
        else => {},
    }
    switch (action) {
        .invoke_action => |value| if (std.mem.eql(
            u8,
            value.action_id,
            "workspace.restore",
        )) {
            executor.repairNativeWorkspace() catch |restore_error| {
                std.log.err(
                    "Polar Station saved Layout restore failed: {s}",
                    .{@errorName(restore_error)},
                );
                current.controller_model.setLocalError(
                    "Polar Station could not restore the complete saved Layout.",
                );
            };
            current.renderControllers();
            return;
        },
        else => {},
    }
    _ = executor.adoptNativeTabSelection() catch false;
    var effective_action = action;
    var native_id: ?[]u8 = null;
    defer if (native_id) |value| current.allocator.free(value);
    switch (effective_action) {
        .retry_host => {
            current.client.retryNow() catch |err| {
                current.controller_model.setLocalError(@errorName(err));
                current.renderControllers();
            };
            return;
        },
        .add_tab => |*value| if (value.tab_id == null) {
            native_id = newNativeWorkspaceId(current.allocator) catch return;
            value.tab_id = native_id.?;
        },
        .toggle_session_assignment => |*value| {
            if (value.tab_id == null) {
                value.tab_id = executor.nativeSelectedTabId() orelse
                    return;
            }
            ipc_client.trace(54, value.tab_id.?, 0);
            if (value.pane_id == null) {
                value.pane_id = executor.nativePaneIdForSessionInTab(
                    value.tab_id.?,
                    value.execution_target_id,
                    value.tmux_session_id,
                ) orelse blk: {
                    native_id = newNativeWorkspaceId(current.allocator) catch return;
                    break :blk native_id.?;
                };
            }
        },
        .delete_tab => |*value| if (value.replacement_tab_id == null and
            executor.nativeTabCount() == 1)
        {
            native_id = newNativeWorkspaceId(current.allocator) catch return;
            value.replacement_tab_id = native_id.?;
        },
        else => {},
    }
    switch (effective_action) {
        .get_projection => {
            requestControlProjection(current);
            return;
        },
        else => {},
    }
    if (presentation_executor.Executor.actionMovesTerminalFocus(effective_action)) {
        for (current.controllers.items) |controller| controller.prepareTerminalFocus(effective_action == .select_pane);
    }
    const native_applied = if (effective_action == .use_existing_session) false else executor.applyNativeWorkspaceAction(
        effective_action,
    ) catch |native_error| {
        std.log.warn("Polar Station native action rejected: {s}", .{@errorName(native_error)});
        current.controller_model.setLocalError(if (native_error == error.PhysicalRepairPending)
            "Layout saved; terminal view is waiting for repair"
        else
            @errorName(native_error));
        current.renderControllers();
        if (effective_action == .rename_tab or effective_action == .rename_workspace) {
            if (controllerById(current, controller_id)) |controller| controller.renameRejected(null);
        }
        return;
    };
    if (native_applied) {
        if (effective_action == .select_workspace) retireEndedExecutorPanes(current, executor);
        observeNativeInboxAttention(current);
        current.renderControllers();
        if (effective_action == .rename_tab or effective_action == .rename_workspace) {
            if (controllerById(current, controller_id)) |controller| controller.renameAccepted();
        }
        return;
    }
    switch (effective_action) {
        .acknowledge_pane_summary => |v| {
            const pane = executor.nativePaneView(v.pane_id) orelse return;
            effective_action = .{ .acknowledge_session_summary = .{ .execution_target_id = pane.execution_target_id, .tmux_session_id = pane.tmux_session_id, .summary_revision = v.summary_revision } };
        },
        .mark_attention_notification_handled => |v| {
            const pane = executor.nativePaneView(v.pane_id) orelse return;
            effective_action = .{ .mark_session_notification_handled = .{ .execution_target_id = pane.execution_target_id, .tmux_session_id = pane.tmux_session_id, .summary_revision = v.summary_revision } };
        },
        .invoke_action => |v| {
            if (std.mem.eql(u8, v.action_id, "sessions.refresh")) {
                effective_action = .refresh_sessions;
            } else if (std.mem.startsWith(u8, v.action_id, "session.restart.")) {
                const projection = current.controller_model.projection() orelse return;
                var found = false;
                for (projection.sessions.rows) |row| {
                    const restart = row.restart_action orelse continue;
                    if (!std.mem.eql(u8, restart.id, v.action_id)) continue;
                    effective_action = .{ .restart_session = .{ .execution_target_id = row.identity.execution_target_id, .tmux_session_id = row.identity.tmux_session_id } };
                    found = true;
                    break;
                }
                if (!found) return;
            } else return; // Auxiliary presentation is entirely native.
        },
        .toggle_sessions, .select_session => return,
        else => {},
    }
    const projection = current.controller_model.projection() orelse return;
    var request_id = nextRequestId(current) orelse return;
    defer current.allocator.free(request_id);
    if (effective_action == .create_session or effective_action == .create_agent_session or effective_action == .create_codex_session) {
        if (controllerById(current, controller_id)) |controller| {
            const fingerprint = std.json.Stringify.valueAlloc(current.allocator, effective_action, .{}) catch return;
            defer current.allocator.free(fingerprint);
            if (controller.creation_request_intent != null and controller.creation_request_id != null and std.mem.eql(u8, controller.creation_request_intent.?, fingerprint)) {
                const retained = current.allocator.dupe(u8, controller.creation_request_id.?) catch return;
                current.allocator.free(request_id); request_id = retained;
            } else {
                const id_copy = current.allocator.dupe(u8, request_id) catch return;
                const intent_copy = current.allocator.dupe(u8, fingerprint) catch { current.allocator.free(id_copy); return; };
                if (controller.creation_request_id) |old| current.allocator.free(old);
                if (controller.creation_request_intent) |old| current.allocator.free(old);
                controller.creation_request_id = id_copy; controller.creation_request_intent = intent_copy;
            }
        }
    }
    const payload = control_center_intent.encodeAction(
        current.allocator,
        request_id,
        current.frontend_instance_id,
        projection.control_revision,
        effective_action,
    ) catch return;
    defer current.allocator.free(payload);
    var pending_effect = pendingControlEffect(
        controller_id,
        effective_action,
    );
    if (pending_effect) |*effect| {
        if (effect.kind == .session_kill) {
            if (controllerById(current, controller_id)) |controller| effect.end_token = controller.end_submission;
        }
        if (effective_action == .session_editor) {
            effect.selection_json = std.json.Stringify.valueAlloc(current.allocator, .{
                .kind = if (effective_action.session_editor.ending) "shared_end" else if (effective_action.session_editor.shared) "shared_editor" else effective_action.session_editor.request.type,
                .generation = effective_action.session_editor.generation,
            }, .{}) catch return;
        }
        if (effect.kind == .session_create) {
            if (controllerById(current, controller_id)) |controller| {
                const workspace_id = controller.session_create_workspace_id orelse return;
                const tab_id = controller.session_create_tab_id orelse return;
                const snapshot = executor.nativeWorkspaceSnapshot() orelse return;
                const tab = snapshot.tab(tab_id) orelse {
                    current.controller_model.setLocalError("The Tab that opened Add Session no longer exists.");
                    controller.sessionCreateRejected(null); return;
                };
                const count = if (effective_action == .create_agent_session) @max(1, effective_action.create_agent_session.session_ids.len) else 1;
                if (!std.mem.eql(u8, snapshot.workspace_id, workspace_id) or tab.panes.len + count > 9) {
                    current.controller_model.setLocalError("The original Tab is unavailable or full. Keep the draft and free a Pane before retrying.");
                    controller.sessionCreateRejected(null); return;
                }
                const pane_id = newNativeWorkspaceId(current.allocator) catch return;
                defer current.allocator.free(pane_id);
                effect.selection_json = std.json.Stringify.valueAlloc(current.allocator, QuickCreateCapture{ .workspace_id = workspace_id, .tab_id = tab_id, .pane_id = pane_id }, .{}) catch return;
                executor.reserveSessionPanes(pane_id, tab_id, count) catch {
                    current.allocator.free(effect.selection_json.?);
                    current.controller_model.setLocalError("This Tab has insufficient room for the selected Sessions (maximum nine Panes).");
                    controller.sessionCreateRejected(null); return;
                };
            }
        }
        if (effective_action == .use_existing_session) {
            effect.selection_json = std.json.Stringify.valueAlloc(current.allocator, effective_action.use_existing_session, .{}) catch return;
        }
    }
    if (effective_action == .create_session) ipc_client.trace(46, request_id, projection.control_revision);
    current.client.sendJson(payload) catch {
        current.controller_model.setLocalError("The Host did not accept this refresh request.");
        current.renderControllers();
        if (pending_effect) |effect| {
            releaseCreationReservation(current, effect);
            rejectPendingControlEffect(current, effect);
            if (effect.selection_json) |json| current.allocator.free(json);
        }
        return;
    };
    if (pending_effect) |effect| {
        const key = current.allocator.dupe(u8, request_id) catch {
            releaseCreationReservation(current, effect);
            if (effect.selection_json) |json| current.allocator.free(json);
            current.controller_model.setLocalError("The validation result could not be tracked; the lost record was kept.");
            return;
        };
        current.pending_control_effects.put(
            current.allocator,
            key,
            effect,
        ) catch {
            releaseCreationReservation(current, effect);
            current.allocator.free(key);
            if (effect.selection_json) |json| current.allocator.free(json);
            current.controller_model.setLocalError("The validation result could not be tracked; the lost record was kept.");
        };
    }
}

fn reportQuickCreateError(current: *State, message: []const u8) void {
    current.controller_model.setLocalError(message);
    current.renderControllers();
    const window = activeGhosttyWindow(current.app) orelse return;
    showNativeError(current, window, "Could not create Pane", message);
}

fn showNativeError(current: *State, window: *Window, title: [:0]const u8, message: []const u8) void {
    const text = current.allocator.dupeZ(u8, message) catch return;
    defer current.allocator.free(text);
    const dialog = adw.MessageDialog.new(window.as(gtk.Window), title, text);
    dialog.addResponse("close", "Close");
    dialog.setDefaultResponse("close");
    dialog.setCloseResponse("close");
    _ = adw.MessageDialog.signals.response.connect(dialog, ?*anyopaque, quickCreateErrorClosed, null, .{});
    dialog.as(gtk.Window).present();
}

fn quickCreateErrorClosed(dialog: *adw.MessageDialog, _: [*:0]const u8, _: ?*anyopaque) callconv(.c) void {
    dialog.as(gtk.Window).destroy();
}

const QuickCreateCapture = struct { workspace_id: []const u8, tab_id: []const u8, pane_id: []const u8 };

fn releaseCreationReservation(current: *State, effect: PendingControlEffect) void {
    if (effect.kind != .session_create and effect.kind != .quick_create) return;
    const capture = std.json.parseFromSlice(QuickCreateCapture, current.allocator, effect.selection_json orelse return, .{}) catch return;
    defer capture.deinit();
    if (current.executorForWorkspace(capture.value.workspace_id)) |executor| executor.releaseSessionPanes(capture.value.pane_id);
}

fn beginQuickCreate(current: *State, window: *Window, codex: bool) bool {
    const executor = current.executorForWindow(window) orelse return false;
    const projection = current.controller_model.projection() orelse return false;
    const enabled = projection.action(if (codex) "session.quick.create.codex" else "session.quick.create.shell") orelse return false;
    if (!enabled.enabled) return false;
    const target = executor.quickCreateTarget(window) catch |err| {
        reportQuickCreateError(current, if (err == error.TabFull) "This Tab already has nine Panes." else "The current managed terminal is unavailable. Select a Pane and try again.");
        current.renderControllers();
        return true;
    };
    var pending = current.pending_control_effects.valueIterator();
    while (pending.next()) |entry| {
        if (entry.kind != .quick_create) continue;
        const saved = std.json.parseFromSlice(QuickCreateCapture, current.allocator, entry.selection_json orelse continue, .{}) catch continue;
        defer saved.deinit();
        if (std.mem.eql(u8, saved.value.workspace_id, target.workspace_id) and std.mem.eql(u8, saved.value.tab_id, target.tab_id)) return true;
    }
    const pane_id = newNativeWorkspaceId(current.allocator) catch return true;
    defer current.allocator.free(pane_id);
    const capture = std.json.Stringify.valueAlloc(current.allocator, QuickCreateCapture{ .workspace_id = target.workspace_id, .tab_id = target.tab_id, .pane_id = pane_id }, .{}) catch return true;
    const request_id = nextRequestId(current) orelse {
        current.allocator.free(capture);
        return true;
    };
    // The existing pending-effect map owns both request identity and capture.
    const payload = control_center_intent.encodeAction(current.allocator, request_id, current.frontend_instance_id, projection.control_revision, .{ .quick_create_session = .{
        .kind = if (codex) .codex else .shell,
        .execution_target_id = if (target.source) |source| source.execution_target_id else projection.sessions.local_execution_target_id,
        .source_session_id = if (target.source) |source| source.tmux_session_id else null,
    } }) catch {
        current.allocator.free(request_id);
        current.allocator.free(capture);
        return true;
    };
    defer current.allocator.free(payload);
    executor.reserveSessionPanes(pane_id, target.tab_id, 1) catch {
        current.allocator.free(request_id); current.allocator.free(capture);
        reportQuickCreateError(current, "This Tab has insufficient room while Sessions are opening.");
        return true;
    };
    current.pending_control_effects.put(current.allocator, request_id, .{ .kind = .quick_create, .controller_id = 0, .selection_json = capture }) catch {
        executor.releaseSessionPanes(pane_id);
        current.allocator.free(request_id);
        current.allocator.free(capture);
        return true;
    };
    current.client.sendJson(payload) catch |err| {
        // All sendJson errors are local admission failures: nothing was queued.
        std.log.warn("quick_create_send_rejected request_id={s} reason={s}", .{ request_id, @errorName(err) });
        if (current.pending_control_effects.fetchRemove(request_id)) |entry| {
            executor.releaseSessionPanes(pane_id);
            current.allocator.free(entry.value.selection_json.?);
            current.allocator.free(entry.key);
        }
        reportQuickCreateError(current, switch (err) {
            error.InvalidRequest => "The frontend rejected the creation request (InvalidRequest). No Session was created. Reopen the updated Polar Station and try again.",
            error.OutboundQueueFull => "The Host request queue is full. No Session was created. Wait for pending operations before retrying.",
            error.NotConnected, error.NotStarted => "The Station Host is reconnecting or unavailable. No Session was created. Wait for connection before retrying.",
            else => "The creation request could not be sent. No Session was created.",
        });
        current.renderControllers();
    };
    return true;
}

fn completeQuickCreate(current: *State, effect: PendingControlEffect, outcome: protocol.ControlCenterResponseOutcome) bool {
    if (outcome.batch) |batch| {
        const capture = std.json.parseFromSlice(QuickCreateCapture, current.allocator, effect.selection_json orelse return false, .{}) catch return false;
        defer capture.deinit();
        const saved = capture.value;
        const executor = current.executorForWorkspace(saved.workspace_id) orelse return false;
        var sessions: std.ArrayListUnmanaged(controller_model.SessionIdentity) = .empty;
        defer sessions.deinit(current.allocator);
        var failed = false;
        for (batch.value) |item| {
            if (item.session) |session| sessions.append(current.allocator, session) catch return false;
            if (item.@"error") |failure| { failed = true; current.controller_model.setLocalError(failure.message); }
        }
        executor.appendSessions(saved.workspace_id, saved.tab_id, sessions.items) catch {
            reportQuickCreateError(current, "Sessions are available in the list, but the original Tab is unavailable or full."); return false;
        };
        if (controllerById(current, effect.controller_id)) |controller| controller.sessionBatchCompleted(batch.value);
        current.renderControllers();
        return !failed;
    }
    const session = outcome.session orelse {
        reportQuickCreateError(current, outcome.error_message orelse "Creation result is unknown. Refresh Sessions before retrying.");
        return false;
    };
    const parsed = std.json.parseFromSlice(QuickCreateCapture, current.allocator, effect.selection_json orelse return false, .{}) catch return false;
    defer parsed.deinit();
    const saved = parsed.value;
    const executor = current.executorForWorkspace(saved.workspace_id) orelse return false;
    const snapshot = executor.nativeWorkspaceSnapshot() orelse return false;
    const tab = snapshot.tab(saved.tab_id);
    if (!std.mem.eql(u8, snapshot.workspace_id, saved.workspace_id) or tab == null or tab.?.panes.len >= 9) {
        reportQuickCreateError(current, "Session created, but the original Tab is unavailable or full. Add it from Sessions.");
        return false;
    }
    if (executor.nativePaneIdForSessionInTab(saved.tab_id, session.execution_target_id, session.tmux_session_id)) |existing| {
        if (std.mem.eql(u8, snapshot.selected_tab_id, saved.tab_id)) {
            _ = executor.applyNativeWorkspaceAction(.{ .activate_pane = .{ .pane_id = existing } }) catch return false;
        }
        return true;
    }
    const focus = if (activeGhosttyWindow(current.app)) |window| blk: {
        const selected = executor.quickCreateTarget(window) catch break :blk false;
        break :blk std.mem.eql(u8, selected.tab_id, saved.tab_id);
    } else false;
    if (focus) for (current.controllers.items) |controller| {
        controller.prepareTerminalFocus(false);
    };
    _ = executor.applyNativeWorkspaceAction(.{ .toggle_session_assignment = .{
        .tab_id = saved.tab_id,
        .pane_id = saved.pane_id,
        .execution_target_id = session.execution_target_id,
        .tmux_session_id = session.tmux_session_id,
        .focus_new = focus,
        .promote_new = true,
    } }) catch {
        reportQuickCreateError(current, "Session created, but its Pane could not be attached. Add it from Sessions.");
        return false;
    };
    current.renderControllers();
    return true;
}

fn newNativeWorkspaceId(allocator: std.mem.Allocator) ![]u8 {
    var bytes: [16]u8 = undefined;
    std.crypto.random.bytes(&bytes);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    return std.fmt.allocPrint(
        allocator,
        "{x:0>2}{x:0>2}{x:0>2}{x:0>2}-" ++
            "{x:0>2}{x:0>2}-{x:0>2}{x:0>2}-" ++
            "{x:0>2}{x:0>2}-{x:0>2}{x:0>2}" ++
            "{x:0>2}{x:0>2}{x:0>2}{x:0>2}",
        .{
            bytes[0],  bytes[1],  bytes[2],  bytes[3],
            bytes[4],  bytes[5],  bytes[6],  bytes[7],
            bytes[8],  bytes[9],  bytes[10], bytes[11],
            bytes[12], bytes[13], bytes[14], bytes[15],
        },
    );
}

fn requestControlProjection(current: *State) void {
    if (current.control_refresh.request() ==
        .request_projection)
    {
        emitControlProjectionRequest(current);
    }
}

fn emitControlProjectionRequest(current: *State) void {
    const request_id = nextRequestId(current) orelse {
        current.control_refresh.failed();
        return;
    };
    defer current.allocator.free(request_id);
    const payload = control_center_intent.encodeGetProjection(
        current.allocator,
        request_id,
        current.frontend_instance_id,
    ) catch {
        current.control_refresh.failed();
        return;
    };
    defer current.allocator.free(payload);
    const tracked_request_id = current.allocator.dupe(
        u8,
        request_id,
    ) catch {
        current.control_refresh.failed();
        return;
    };
    std.debug.assert(
        current.control_refresh_request_id == null,
    );
    current.control_refresh_request_id =
        tracked_request_id;
    current.client.sendJson(payload) catch {
        current.allocator.free(tracked_request_id);
        current.control_refresh_request_id = null;
        current.control_refresh.failed();
        current.controller_model.setUnavailable();
        current.renderControllers();
    };
}

fn resetControlRefresh(current: *State) void {
    current.control_refresh.disconnected();
    if (current.control_refresh_request_id) |value| {
        current.allocator.free(value);
        current.control_refresh_request_id = null;
    }
}

fn clearResourceFile(current:*State) void {
    var result=current.resource_file_flow.dispatch(current.allocator,.{.type="cancel"}) orelse return;
    result.deinit();
}
fn resourceFileInput(current:*State,input:anytype) void {
    resourceFileTrace("dispatch");
    var result=current.resource_file_flow.dispatch(current.allocator,input) orelse return;
    defer result.deinit();
    const dict=@import("dictate.zig");const effects=dict.field(result.value,"effects");
    if(effects!=.array)return;
    for(effects.array.items)|effect|{
        const kind=dict.string(effect,"type");
        resourceFileTrace(kind);
        if(std.mem.eql(u8,kind,"resolve")){
            const origin=dict.field(effect,"origin");
            const bytes=(if(dict.field(effect,"candidates")==.array) resources.encodeResolveFileCandidates(current.allocator,dict.string(effect,"request_id"),origin,dict.field(effect,"candidates")) else resources.encodeResolveFile(current.allocator,dict.string(effect,"request_id"),dict.string(origin,"execution_target_id"),dict.string(effect,"path"),if(dict.string(origin,"cwd").len>0)dict.string(origin,"cwd")else null,if(dict.string(origin,"terminal_session_id").len>0)dict.string(origin,"terminal_session_id")else null)) catch continue;
            defer current.allocator.free(bytes);
            current.client.sendJson(bytes) catch {resourceFileInput(current,.{.type="failed",.request_id=dict.string(effect,"request_id"),.message="Could not resolve the source file."});};
        }else if(std.mem.eql(u8,kind,"choose_file")){
            presentFileChoices(current,effect);
        }else if(std.mem.eql(u8,kind,"start_resource")){
            resourceAction(current,.{.restart=dict.string(effect,"resource_id")});
        }else if(std.mem.eql(u8,kind,"create_mapping")){
            const access=dict.field(effect,"access");
            current.resources_window.present(current.resources_model.projection());
            current.resources_window.prefillFile(dict.string(access,"execution_target_id"),dict.string(access,"remote_directory"));
        }else if(std.mem.eql(u8,kind,"open_local_file")){
            const path=dict.string(effect,"path");
            const opened=if(std.mem.eql(u8,dict.string(effect,"strategy"),"reveal")) resources_window.revealLocalFile(current.allocator,path) else resources_window.openLocalPath(current.allocator,path);
            if(!opened)resourceFileInput(current,.{.type="viewer_failed",.path=path});
        }else if(std.mem.eql(u8,kind,"show_error")){
            current.resources_window.present(current.resources_model.projection());
            current.resources_window.showError(dict.string(effect,"message"));
        }
    }
}
const FileChoiceDialog = struct { request_id: []u8, picker: *gtk.DropDown };
fn presentFileChoices(current:*State,effect:std.json.Value) void {
    const dict=@import("dictate.zig");
    const window=activeGhosttyWindow(current.app) orelse return;
    const choices=dict.field(effect,"candidates");if(choices!=.array)return;
    const names=gtk.StringList.new(null);
    for(choices.array.items)|item|{
        const label=current.allocator.dupeZ(u8,dict.string(item,"path")) catch return;
        defer current.allocator.free(label);names.append(label);
    }
    const picker=gtk.DropDown.new(names.as(gio.ListModel),null);
    const data=current.allocator.create(FileChoiceDialog) catch return;
    data.*=.{.request_id=current.allocator.dupe(u8,dict.string(effect,"request_id")) catch {current.allocator.destroy(data);return;},.picker=picker};
    const dialog=adw.MessageDialog.new(window.as(gtk.Window),"Choose a file","Several matching files exist on the source machine.");
    dialog.setExtraChild(picker.as(gtk.Widget));dialog.addResponse("cancel","Cancel");dialog.addResponse("open","Open");
    dialog.setCloseResponse("cancel");dialog.setDefaultResponse("cancel");
    _=adw.MessageDialog.signals.response.connect(dialog,*FileChoiceDialog,fileChoiceResponse,data,.{});
    dialog.as(gtk.Window).present();
}
fn fileChoiceResponse(dialog:*adw.MessageDialog,response:[*:0]const u8,data:*FileChoiceDialog) callconv(.c) void {
    defer dialog.as(gtk.Window).destroy();
    const current=if(state)|*value|value else return;
    defer {current.allocator.free(data.request_id);current.allocator.destroy(data);}
    const index:?usize=if(std.mem.eql(u8,std.mem.span(response),"open")) @intCast(data.picker.getSelected()) else null;
    resourceFileInput(current,.{.type="choose",.request_id=data.request_id,.index=index});
}

pub fn openSelectedFile(app:*Application) void {
    resourceFileTrace("selected-action");
    openSelectedFileInWindow(activeGhosttyWindow(app) orelse return);
}
fn openSelectedFileInWindow(window:*Window) void {
    const current=if(state) |*v| v else return;
    const executor=current.executorForWindow(window) orelse return;
    const page=window.getTabView().getSelectedPage() orelse return;
    const Tab=@import("../class/tab.zig").Tab;const tab=gobject.ext.cast(Tab,page.getChild()) orelse return;
    const surface=tab.getActiveSurface() orelse return;const pane=executor.nativePaneIdForSurface(surface) orelse return;
    const selected=executor.terminalSelection(current.allocator,pane) orelse return;defer current.allocator.free(selected);
    resourceFileTrace("selection-read");
    _=openResourceFile(selected,surface);
}
pub fn openResourceFile(value:[]const u8,source:?*@import("../class/surface.zig").Surface) bool {
    resourceFileTrace("native-reference");
    const current=if(state) |*v| v else return false;
    const c=@cImport({@cInclude("polar_station_desktop_client.h");});
    const path=c.polar_station_resource_file_path(value.ptr,value.len);if(path==null)return false;defer c.polar_station_desktop_string_free(path);
    resourceFileTrace("reference-parsed");
    const surface=source orelse return false;
    const root=surface.as(gtk.Widget).getRoot() orelse return false;
    const window=gobject.ext.cast(Window,root.as(gobject.Object)) orelse return false;
    const executor=current.executorForWindow(window) orelse return false;
    const snapshot=executor.nativeWorkspaceSnapshot() orelse return false;
    const pane_id=executor.nativePaneIdForSurface(surface) orelse return false;
    const location=snapshot.pane(pane_id) orelse return false;const pane=location.pane;
    if(!pane.isTerminal())return false;
    resourceFileTrace("source-bound");
    const cwd: ?[]const u8=surface.getPwd();
    const id=nextRequestId(current) orelse return true;defer current.allocator.free(id);
    resourceFileInput(current,.{.type="open",.request_id=id,.text=value,.origin=.{.execution_target_id=pane.execution_target_id,.cwd=cwd,.terminal_session_id=pane.tmux_session_id}});
    requestResourcesProjection(current);return true;
}
fn resourceFileTrace(stage: []const u8) void {
    if (std.posix.getenv("POLAR_STATION_TRACE_DIR") != null)
        std.log.info("Polar Station file open: {s}", .{stage});
}
fn requestResourceFile(current:*State) void {
    const id=nextRequestId(current) orelse return;defer current.allocator.free(id);
    resourceFileInput(current,.{.type="refresh",.request_id=id});
}

fn handleResourcesResponse(current: *State, payload: []const u8) void {
    var outcome = current.resources_model.applyResponse(payload) catch return;
    defer outcome.deinit(current.allocator);
    if(outcome.kind==.file_access){
        if(current.resources_model.fileAccess())|access|resourceFileInput(current,.{.type="resolved",.request_id=outcome.request_id,.access=access});
    }else if(outcome.kind==.@"error"){
        resourceFileInput(current,.{.type="failed",.request_id=outcome.request_id,.message=outcome.error_message orelse "The file Resource is unavailable."});
    }
    if(outcome.kind==.projection or outcome.kind==.command_accepted)requestResourceFile(current);
    const completes_refresh = if (current.resources_refresh_request_id) |request_id|
        std.mem.eql(u8, outcome.request_id, request_id)
    else
        false;
    if (completes_refresh) {
        current.allocator.free(current.resources_refresh_request_id.?);
        current.resources_refresh_request_id = null;
    }
    if (current.resources_model.projection()) |projection| {
        current.resources_window.render(
            projection,
            outcome.kind == .command_accepted,
        );
    }
    if (outcome.error_message) |message| current.resources_window.showError(message);
    if (outcome.kind == .log_tail) {
        if (current.resources_model.logTail()) |tail| {
            current.resources_window.showLog(tail);
        }
    }
    if (completes_refresh and
        current.resources_refresh.complete() == .request_projection)
    {
        emitResourcesProjectionRequest(current);
    }
}

fn requestResourcesProjection(current: *State) void {
    if (current.resources_refresh.request() == .request_projection) {
        emitResourcesProjectionRequest(current);
    }
}

fn emitResourcesProjectionRequest(current: *State) void {
    const request_id = nextRequestId(current) orelse {
        current.resources_refresh.failed();
        return;
    };
    defer current.allocator.free(request_id);
    const payload = resources.encodeGetProjection(
        current.allocator,
        request_id,
    ) catch {
        current.resources_refresh.failed();
        return;
    };
    defer current.allocator.free(payload);
    const tracked = current.allocator.dupe(u8, request_id) catch {
        current.resources_refresh.failed();
        return;
    };
    current.resources_refresh_request_id = tracked;
    current.client.sendJson(payload) catch {
        current.allocator.free(tracked);
        current.resources_refresh_request_id = null;
        current.resources_refresh.failed();
    };
}

fn resetResourcesRefresh(current: *State) void {
    current.resources_refresh.disconnected();
    if (current.resources_refresh_request_id) |value| {
        current.allocator.free(value);
        current.resources_refresh_request_id = null;
    }
}

fn handleSettingsResponse(current: *State, payload: []const u8) void {
    current.settings_model.applyResponse(payload) catch return;
    if (current.settings_model.projection()) |projection| {
        current.settings_window.render(projection);
        current.theme.update(projection) catch |err| {
            current.settings_window.themeError(@errorName(err));
        };
        if (current.lgp) |lgp| {
            lgp.setSendMode(std.mem.eql(u8, projection.appearance.chat_send_mode, "modified_enter"));
            lgp.setCachePolicy(projection.appearance.lgp_cache);
        }
        for (current.controllers.items) |controller| {
            applyControllerSettings(current, controller);
        }
    }
    if (current.settings_model.codexNotifyStatus()) |status| current.settings_window.renderCodexNotifyStatus(status);
}

fn applyControllerSettings(current: *State, controller: *controller_pane.Controller) void {
    const projection = current.settings_model.projection() orelse return;
    controller.applyPresentationSettings(
        projection.appearance.layout_map_visible,
        projection.appearance.inbox_visible,
        projection.appearance.layout_map_width,
        projection.appearance.inbox_width,
        projection.appearance.terminal_delivery_mode,
        projection.appearance.interface_text_scale_percent,
    );
}

fn requestSettingsProjection(current: *State) void {
    if (current.backend_state != .ready) {
        current.settings_window.setBackendAvailable(false);
        return;
    }
    const request_id = nextRequestId(current) orelse return;
    defer current.allocator.free(request_id);
    const payload = settings.encodeGetProjection(current.allocator, request_id) catch return;
    defer current.allocator.free(payload);
    current.client.sendJson(payload) catch |err| current.settings_window.showError(@errorName(err));
}

fn requestInbox(current: *State) void {
    if (current.backend_state != .ready) return;
    if (current.inbox_refresh.request() == .request_projection) emitInboxRequest(current);
}

fn emitInboxRequest(current: *State) void {
    const request_id = nextRequestId(current) orelse return;
    defer current.allocator.free(request_id);
    const payload = inbox.encodeGetPage(current.allocator, request_id) catch { current.inbox_refresh.failed(); return; };
    defer current.allocator.free(payload);
    current.inbox_refresh_request_id = current.allocator.dupe(u8, request_id) catch { current.inbox_refresh.failed(); return; };
    current.client.sendJson(payload) catch {
        current.allocator.free(current.inbox_refresh_request_id.?); current.inbox_refresh_request_id = null;
        current.inbox_refresh.failed();
    };
}

fn handleInboxResponse(current: *State, payload: []const u8) void {
    const Header = struct { response: struct { request_id: []const u8 } };
    const header = std.json.parseFromSlice(Header, current.allocator, payload, .{ .ignore_unknown_fields = true }) catch return;
    defer header.deinit();
    const completes_read = if (current.inbox_refresh_request_id) |id| std.mem.eql(u8, id, header.value.response.request_id) else false;
    if (completes_read) { current.allocator.free(current.inbox_refresh_request_id.?); current.inbox_refresh_request_id = null; }
    defer if (completes_read and current.inbox_refresh.complete() == .request_projection) { emitInboxRequest(current); };
    const is_page = std.mem.indexOf(u8, payload, "\"kind\":\"page\"") != null;
    var prior_revision: ?u64 = null;
    if (current.inbox_model.page()) |page| {
        for (page.messages) |message| {
            if (std.mem.indexOf(u8, payload, message.id) != null) {
                prior_revision = message.revision;
                break;
            }
        }
    }
    const delivery = current.inbox_model.applyResponse(payload) catch return;
    if (is_page) notifyInboxAttention(current);
    if (delivery) |value| {
        const delivery_state: []const u8 = blk: {
            const executor = current.executorForWorkspace(value.origin.workspace_id) orelse break :blk "failed_before_write";
            executor.deliverInboxText(
                value.origin.workspace_id,
                value.origin.tab_id,
                value.origin.pane_id,
                value.origin.execution_target_id,
                value.origin.tmux_session_id,
                value.text,
                std.mem.eql(u8, value.mode, "send"),
            ) catch break :blk "failed_before_write";
            break :blk "delivered";
        };
        const request_id = nextRequestId(current) orelse return;
        defer current.allocator.free(request_id);
        const revision = (prior_revision orelse 0) + 1;
        const receipt = inbox.encodeReceipt(
            current.allocator,
            request_id,
            value.message_id,
            revision,
            delivery_state,
        ) catch return;
        defer current.allocator.free(receipt);
        current.client.sendJson(receipt) catch {};
    }
    current.renderControllers();
    if (delivery == null and !is_page) requestInbox(current);
}

fn settingsAction(context: ?*anyopaque, action: settings_window.Action) void {
    const current: *State = @ptrCast(@alignCast(context orelse return));
    switch (action) {
        .dictate_deleted => |id| {
            if (current.lgp) |lgp| lgp.dictate.input(.{ .type = "recording_deleted", .operation_id = id });
            return;
        },
        .dictate_recover => |id| {
            if (current.lgp) |lgp| lgp.dictate.input(.{ .type = "recover_recording", .operation_id = id });
            return;
        },
        .voice_changed => { if(current.lgp)|lgp|lgp.dictate.reloadVoiceSettings(); return; },
        .voice_shortcuts => { if(!current.app.configureStationDictateShortcut()) current.settings_window.showError("System shortcut settings are unavailable. Use your desktop settings to configure Polar Station; the Dictate button remains available."); return; },
        .open_codex => {
            current.codex_window.presentAll();
            return;
        },
        .open_connectors => {
            current.connector_window.present();
            return;
        },
        .open_resources => {
            current.resources_window.present(current.resources_model.projection());
            requestResourcesProjection(current);
            return;
        },
        .open_connections => {
            const host_window = activeGhosttyWindow(current.app) orelse return;
            const controller = ensureWindowController(current, host_window) orelse return;
            controller.presentConnections();
            return;
        },
        .open_workspace_font => {
            const host_window = activeGhosttyWindow(current.app) orelse return;
            for (current.controllers.items) |controller| {
                if (controller.ownsHostWindow(host_window)) {
                    controller.presentWorkspaceFontEditor();
                    return;
                }
            }
            return;
        },
        else => {},
    }
    if (current.backend_state != .ready) {
        current.settings_window.setBackendAvailable(false);
        return;
    }
    const request_id = nextRequestId(current) orelse return;
    defer current.allocator.free(request_id);
    const revision = (current.settings_model.projection() orelse return).revision;
    const payload = switch (action) {
        .reload => settings.encodeGetProjection(current.allocator, request_id),
        .appearance => |value| settings.encodeUpdateAppearance(current.allocator, request_id, revision, value),
        .attention => |value| settings.encodeUpdateAttention(current.allocator, request_id, revision, value),
        .monitoring => |value| settings.encodeUpdateMonitoring(current.allocator, request_id, revision, value),
        .session_summary => |value| settings.encodeUpdateSessionSummary(current.allocator, request_id, revision, value),
        .test_summary => |value| settings.encodeTestSessionSummary(current.allocator, request_id, value),
        .inspect_codex_notify => settings.encodeGetCodexNotifyStatus(current.allocator, request_id),
        .configure_codex_notify => settings.encodeConfigureCodexNotify(current.allocator, request_id),
        .open_workspace_font, .open_connections, .open_resources, .open_codex, .open_connectors, .voice_changed, .voice_shortcuts, .dictate_recover, .dictate_deleted => unreachable,
    } catch return;
    defer current.allocator.free(payload);
    current.client.sendJson(payload) catch |err| current.settings_window.showError(@errorName(err));
}

fn resourceAction(
    context: ?*anyopaque,
    action: resources_window.Action,
) void {
    const current: *State = @ptrCast(@alignCast(context orelse return));
    if(action==.cancel_file_open){clearResourceFile(current);return;}
    if (current.backend_state != .ready) {
        current.resources_window.setBackendAvailable(false);
        return;
    }
    const request_id = nextRequestId(current) orelse return;
    defer current.allocator.free(request_id);
    const projection = current.resources_model.projection();
    const payload = switch (action) {
        .cancel_file_open => unreachable,
        .reload => resources.encodeGetProjection(
            current.allocator,
            request_id,
        ),
        .apply => |catalog| resources.encodeApply(
            current.allocator,
            request_id,
            catalog.revision,
            catalog,
        ),
        .restart => |resource_id| resources.encodeRestart(
            current.allocator,
            request_id,
            (projection orelse return).revision,
            resource_id,
        ),
        .open_mirror => |resource_id| resources.encodeOpenMirror(
            current.allocator,
            request_id,
            (projection orelse return).revision,
            resource_id,
        ),
        .get_log => |resource_id| resources.encodeGetLogTail(
            current.allocator,
            request_id,
            resource_id,
        ),
    } catch return;
    defer current.allocator.free(payload);
    current.client.sendJson(payload) catch |err| current.resources_window.showError(@errorName(err));
}

fn backendBecameUnavailable(
    current: *State,
    next_state: BackendState,
) void {
    const was_ready = current.backend_state == .ready;
    const changed = current.backend_state != next_state;
    current.backend_state = next_state;
    if (!was_ready and !changed) return;
    resetControlRefresh(current);
    resetResourcesRefresh(current);
    current.controller_model.connection = if (next_state == .connecting) .connecting else .unavailable;
    current.resources_window.setBackendAvailable(false);
    current.settings_window.setBackendAvailable(false);
    for(current.controllers.items)|controller|{if(controller.connections)|connections|connections.codexDisconnected();}
    current.codex_window.disconnected();
    current.inbox_refresh.disconnected();
    if (current.inbox_refresh_request_id) |id| current.allocator.free(id);
    current.inbox_refresh_request_id = null;
    current.connector_window.disconnected();
    current.renderControllers();
    if (was_ready) rejectAllPendingControlEffects(current);
}

fn nextRequestId(current: *State) ?[]u8 {
    const request_id = std.fmt.allocPrint(
        current.allocator,
        "{s}-control-{d}",
        .{ current.frontend_instance_id, current.next_request_sequence },
    ) catch return null;
    current.next_request_sequence +%= 1;
    if (current.next_request_sequence == 0) current.next_request_sequence = 1;
    return request_id;
}

fn nextControllerId(current: *State) u64 {
    const controller_id = current.next_controller_sequence;
    current.next_controller_sequence +%= 1;
    if (current.next_controller_sequence == 0) {
        current.next_controller_sequence = 1;
    }
    return controller_id;
}

fn sendCodexPayload(context: ?*anyopaque, payload: []const u8) !void {
    const current: *State = @ptrCast(@alignCast(context orelse return error.NotStarted));
    try current.client.sendJson(payload);
}

/// GTK render callback: map a process-local diagnostic key to the existing
/// content channel without persisting terminal data or memory addresses.
pub fn traceSurface(surface: *@import("../class/surface.zig").Surface, key: u64) void {
    const current = if (state) |*value| value else return;
    current.activeExecutor().traceSurface(surface, key);
}

/// Initialization callbacks run on GTK's main thread, before the first bytes.
pub fn traceSurfaceStage(surface: *@import("../class/surface.zig").Surface, stage: u32) void {
    if (!@import("../../../polar_station_trace.zig").enabled()) return;
    if (state) |*current| current.activeExecutor().traceSurfaceStage(surface, stage);
}
