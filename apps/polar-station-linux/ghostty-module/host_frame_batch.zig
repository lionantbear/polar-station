//! Bounded GTK-turn policy for ordered Host frames.
//!
//! Host events remain ordered and are all applied to their native caches. The
//! adapter limits how many it consumes in one GTK turn and coalesces any number
//! of Session projection changes into one native render at the batch boundary.

const std = @import("std");

pub const max_frames_per_tick: usize = 128;

pub const Batch = struct {
    processed: usize = 0,
    session_projection_pending: bool = false,
    render_pending: bool = false,

    pub fn canPop(self: *const Batch) bool {
        return self.processed < max_frames_per_tick;
    }

    pub fn observedFrame(self: *Batch) void {
        std.debug.assert(self.canPop());
        self.processed += 1;
    }

    pub fn requestRender(self: *Batch) void {
        self.render_pending = true;
    }

    pub fn requestSessionProjection(self: *Batch) void {
        self.session_projection_pending = true;
    }

    pub fn takeSessionProjection(self: *Batch) bool {
        const requested = self.session_projection_pending;
        self.session_projection_pending = false;
        return requested;
    }

    pub fn takeRender(self: *Batch) bool {
        const requested = self.render_pending;
        self.render_pending = false;
        return requested;
    }
};





