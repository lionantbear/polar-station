//! Managed Linux Polar Station shortcut catalog and modifier state.
//! Ordinary Ghostty never loads this module.

const std = @import("std");
const gdk = @import("gdk");
const c = @cImport({ @cInclude("polar_station_desktop_client.h"); });
const input = @import("../../../input.zig");

pub const Action = union(enum) {
    control_center_toggle,
    workspace_mode_toggle,
    focus_previous,
    focus_next,
    previous_tab,
    next_tab,
    focus_pane_number: u8,
    new_shell,
    new_codex,
    open_idea_tree,
    resume_codex,
    open_inquiry,
    insert_idea_node,
    edit_idea_instruction,
    quote_selection,
    open_file_reference,
    new_workspace,
    rename_workspace,
    refresh_sessions,
    close_focused_pane,
};

/// The Window owns capture; no one-shot GTK application accelerators are set.
pub fn install(_: anytype) void {}

pub const ModifierState = struct {
    control: bool = false,
    shift: bool = false,

    pub fn visible(self: ModifierState) bool {
        return self.control and self.shift;
    }

    pub fn press(self: *ModifierState, keyval: c_uint, mods: gdk.ModifierType) void {
        self.control = self.control or mods.control_mask;
        self.shift = self.shift or mods.shift_mask;
        switch (keyval) {
            gdk.KEY_Control_L, gdk.KEY_Control_R => self.control = true,
            gdk.KEY_Shift_L, gdk.KEY_Shift_R => self.shift = true,
            // A popover may receive the release instead of this Window.
            // Ordinary key events carry the current physical modifier facts.
            else => { self.control = mods.control_mask; self.shift = mods.shift_mask; },
        }
    }

    pub fn release(self: *ModifierState, keyval: c_uint) void {
        switch (keyval) {
            gdk.KEY_Control_L, gdk.KEY_Control_R => self.control = false,
            gdk.KEY_Shift_L, gdk.KEY_Shift_R => self.shift = false,
            else => {},
        }
    }
};

pub const HintContext = c.StationShortcutHintContext;
pub fn hintCount(context:c.StationShortcutHintContext) usize {return c.polar_station_shortcut_hint_count(context);}
pub fn hintEntry(context:c.StationShortcutHintContext,index:usize) c.StationShortcutView {return c.polar_station_shortcut_hint_at(context,index);}
pub fn catalogCount() usize { return c.polar_station_shortcut_count(); }
pub fn catalogEntry(index: usize) c.StationShortcutView { return c.polar_station_shortcut_at(index); }
pub fn repeatable(keyval: c_uint, keycode: c_uint) bool {
    const key = if (physicalDigit(keycode)) |number| @as(c_uint, '0') + number else keyval;
    const index = c.polar_station_shortcut_lookup(2, key);
    return index != 0 and c.polar_station_shortcut_at(index - 1).repeatable;
}
pub fn resolve(keyval: c_uint, keycode: c_uint) ?Action {
    const key = if (physicalDigit(keycode)) |number| @as(c_uint, '0') + number else keyval;
    const index = c.polar_station_shortcut_lookup(2, key);
    if (index == 0) return null;
    const entry = c.polar_station_shortcut_at(index - 1);
    return switch (entry.action) {
        1 => .control_center_toggle, 2 => .workspace_mode_toggle,
        3 => .focus_previous, 4 => .focus_next,
        5 => .{ .focus_pane_number = entry.number },
        6 => .new_shell, 7 => .new_codex,
        8 => .new_workspace, 9 => .rename_workspace,
        10 => .refresh_sessions, 11 => .close_focused_pane,
        12 => .open_idea_tree, 13 => .resume_codex, 14 => .open_inquiry,
        15 => .insert_idea_node, 16 => .edit_idea_instruction,
        17 => .quote_selection,
        18 => .previous_tab, 19 => .next_tab,
        20 => .open_file_reference,
        else => null,
    };
}

fn physicalDigit(keycode: c_uint) ?u8 {
    for (input.keycodes.entries) |entry| {
        if (entry.native != keycode) continue;
        return switch (entry.key) {
            .digit_1 => 1,
            .digit_2 => 2,
            .digit_3 => 3,
            .digit_4 => 4,
            .digit_5 => 5,
            .digit_6 => 6,
            .digit_7 => 7,
            .digit_8 => 8,
            .digit_9 => 9,
            else => null,
        };
    }
    return null;
}






