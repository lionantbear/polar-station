//! Observe one existing GDK frame per traced Surface. No timer, redraw request,
//! payload capture or permanent frame-loop subscription is introduced.
const gdk = @import("gdk");
const gtk = @import("gtk");
const glib = @import("glib");
const gobject = @import("gobject");
const ipc = @import("ipc_client.zig");
const Observation = struct {
    timing: *gdk.FrameTimings,
    key: u64,
    handler: c_ulong = 0,
};

pub fn observe(widget: *gtk.Widget, key: u64) void {
    const clock = widget.getFrameClock() orelse return;
    const timing = clock.getCurrentTimings() orelse return;
    const observation = glib.ext.create(Observation);
    observation.* = .{ .timing = timing.ref(), .key = key };
    observation.handler = gdk.FrameClock.signals.after_paint.connect(
        clock, *Observation, afterPaint, observation, .{ .after = true, .destroyData = destroy },
    );
}

fn afterPaint(clock: *gdk.FrameClock, observation: *Observation) callconv(.c) void {
    const timing = observation.timing;
    const complete = timing.getComplete() != 0;
    // GTK 4.22 Wayland first marks an estimated frame callback time complete;
    // wp_presentation feedback can overwrite it afterwards. Observe two later
    // existing frames, and let the offline protocol trace confirm the value.
    const elapsed_frames = clock.getFrameCounter() - timing.getFrameCounter();
    if (elapsed_frames < 2 or (!complete and elapsed_frames < 120)) return;
    ipc.tracePresentation(observation.key, timing.getFrameCounter(), if (complete) timing.getPresentationTime() else 0);
    // Disconnect releases Observation. Do not touch it afterwards.
    gobject.signalHandlerDisconnect(clock.as(gobject.Object), observation.handler);
}

fn destroy(observation: *Observation) callconv(.c) void {
    observation.timing.unref();
    glib.ext.destroy(observation);
}

// One paint callback for the accepted Session-list revision; no periodic probe.
const ControlObservation = struct { revision: u64, handler: c_ulong = 0 };
pub fn observeControl(widget: *gtk.Widget, revision: u64) void {
    const clock = widget.getFrameClock() orelse return;
    const observation = glib.ext.create(ControlObservation);
    observation.* = .{ .revision = revision };
    observation.handler = gdk.FrameClock.signals.after_paint.connect(clock, *ControlObservation, controlPaint, observation, .{ .after = true, .destroyData = destroyControl });
}
fn controlPaint(clock: *gdk.FrameClock, observation: *ControlObservation) callconv(.c) void {
    ipc.trace(49, "", observation.revision);
    gobject.signalHandlerDisconnect(clock.as(gobject.Object), observation.handler);
}
fn destroyControl(observation: *ControlObservation) callconv(.c) void { glib.ext.destroy(observation); }
