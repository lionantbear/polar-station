// Physical chord capture only. No Dictate lifecycle or hold/toggle policy.
export class ChordCapture {
    constructor(emit) { this.emit = emit; this.held = null; }
    press(keycode, modifiers, repeat, time) {
        if (repeat || this.held) return;
        this.held = {keycode, modifiers};
        this.emit(1, time);
    }
    release(keycode, modifier, time) {
        if (!this.held) return;
        if (keycode !== this.held.keycode && !(modifier & this.held.modifiers)) return;
        this.held = null;
        this.emit(2, time);
    }
    cancel(force = false) {
        if (!this.held && !force) return;
        this.held = null;
        this.emit(4);
    }
}

// Expand the compositor's wrapping millisecond clock near current monotonic time.
export function eventTime(eventMs, nowMs) {
    return eventMs ? nowMs - (((nowMs >>> 0) - eventMs) >>> 0) : nowMs;
}
