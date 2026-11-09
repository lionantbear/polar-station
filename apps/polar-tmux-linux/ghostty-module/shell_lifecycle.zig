//! Pure lifecycle state for the terminal-first Polar Station shell.

pub const ControlCenter = enum {
    absent,
    hidden,
    visible,
};

pub const Effect = enum {
    create_and_show,
    show_existing,
    hide_existing,
};

pub const State = struct {
    control_center: ControlCenter = .absent,

    pub fn toggleControlCenter(self: *State) Effect {
        return switch (self.control_center) {
            .absent => effect: {
                self.control_center = .visible;
                break :effect .create_and_show;
            },
            .hidden => effect: {
                self.control_center = .visible;
                break :effect .show_existing;
            },
            .visible => effect: {
                self.control_center = .hidden;
                break :effect .hide_existing;
            },
        };
    }

    pub fn controlCenterHidden(self: *State) void {
        if (self.control_center != .absent) self.control_center = .hidden;
    }

    pub fn controlCenterCreationFailed(self: *State) void {
        self.control_center = .absent;
    }
};
