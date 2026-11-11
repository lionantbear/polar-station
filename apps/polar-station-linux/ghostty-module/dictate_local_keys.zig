//! Window-local physical Ctrl+Super edges; recording policy stays in Rust.
pub const Key = enum { control, super, other_modifier, other };
pub const Edge = enum { none, press, release, cancel };
pub const Chord = struct {
    control: bool = false,
    super: bool = false,
    held: bool = false,
    blocked: bool = false,
    pub fn press(self: *Chord, key: Key, control: bool, super: bool, extra: bool) Edge {
        self.control = control or key == .control;
        self.super = super or key == .super;
        if (key == .other or extra) {
            self.blocked = true;
            if (self.held) { self.held = false; return .cancel; }
            return .none;
        }
        if (!self.blocked and !self.held and self.control and self.super) {
            self.held = true; return .press;
        }
        return .none;
    }
    pub fn release(self: *Chord, key: Key) Edge {
        if (key == .control) self.control = false;
        if (key == .super) self.super = false;
        if (!self.control and !self.super) self.blocked = false;
        if (self.held and (key == .control or key == .super)) {
            self.held = false; return .release;
        }
        return .none;
    }
    pub fn reset(self: *Chord) Edge {
        const held = self.held; self.* = .{};
        return if (held) .release else .none;
    }
};
const std = @import("std");



