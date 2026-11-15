// Physical chord capture only. No Dictate lifecycle or hold/toggle policy.
export class ChordCapture {
    constructor(emit) { this.emit = emit; this.held = null; }
    press(keycode, modifiers, repeat) {
        if (repeat || this.held) return;
        this.held = {keycode, modifiers};
        this.emit(1);
    }
    release(keycode, modifier) {
        if (!this.held) return;
        if (keycode !== this.held.keycode && !(modifier & this.held.modifiers)) return;
        this.held = null;
        this.emit(2);
    }
    cancel(force = false) {
        if (!this.held && !force) return;
        this.held = null;
        this.emit(4);
    }
}
