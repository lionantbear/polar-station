import Clutter from 'gi://Clutter';
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import Meta from 'gi://Meta';
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import {ChordCapture} from './capture.js';

const PATH = '/com/polarealm/PolarStation/Shortcuts';
const IFACE = 'com.polarealm.PolarStation.Shortcuts1';
const XML = `<node><interface name="${IFACE}">
<property name="Version" type="u" access="read"/>
<signal name="Edge"><arg type="u" name="event"/></signal>
</interface></node>`;

export default class StationShortcuts extends Extension {
    get Version() { return this._ready ? 1 : 0; }
    enable() {
        this._object = Gio.DBusExportedObject.wrapJSObject(XML, this);
        this._object.export(Gio.DBus.session, PATH);
        this._capture = new ChordCapture(edge => this._object.emit_signal('Edge', new GLib.Variant('(u)', [edge])));
        this._settings = new Gio.Settings({schema_id: 'org.gnome.settings-daemon.global-shortcuts.application',
            path: '/org/gnome/settings-daemon/global-shortcuts/com.polarealm.PolarStation/'});
        this._reload();
        this._changed = this._settings.connect('changed::shortcuts', () => {
            this._capture.cancel(true); this._reload();
        });
        // Mutter owns the existing Portal accelerator, so no competing grab.
        this._activated = global.display.connect('accelerator-activated', () => {
            const event = Clutter.get_current_event();
            if (!event || Main.sessionMode.isLocked) return;
            const mods = event.get_state() & this._modifierMask();
            const name = Meta.accelerator_name(mods, event.get_key_symbol());
            if (!this._bindings.includes(name.toLowerCase())) return;
            this._capture.press(event.get_key_code(), mods,
                !!(event.get_flags() & Clutter.EventFlags.FLAG_REPEATED));
        });
        this._deactivated = global.display.connect('accelerator-deactivated', () => {
            const event = Clutter.get_current_event();
            if (event) this._release(event);
        });
        // Keymap changes are emitted for depressed modifiers even while a
        // Wayland client has focus. Sample only in response to this signal.
        this._keymap = global.stage.get_context().get_backend().get_default_seat().get_keymap();
        this._modifierChanged = this._keymap.connect('state-changed', () => {
            const held = this._capture.held;
            if (held && (global.get_pointer()[2] & held.modifiers) !== held.modifiers)
                this._capture.release(held.keycode, 0);
        });
        this._ready = true;
        this._object.emit_property_changed('Version', new GLib.Variant('u', 1));
        this._session = Main.sessionMode.connect('updated', () => {
            if (Main.sessionMode.isLocked) this._capture.cancel(true);
        });
    }
    _modifierMask() {
        return Clutter.ModifierType.CONTROL_MASK | Clutter.ModifierType.SHIFT_MASK |
            Clutter.ModifierType.MOD1_MASK | Clutter.ModifierType.SUPER_MASK |
            Clutter.ModifierType.META_MASK | Clutter.ModifierType.HYPER_MASK;
    }
    _reload() {
        const rows = this._settings.get_value('shortcuts').deep_unpack();
        this._bindings = rows.filter(([id]) => id === 'polar-station-dictate')
            .flatMap(([, properties]) => properties.shortcuts?.deep_unpack() ?? []).map(value => value.replaceAll('<Primary>', '<Control>').toLowerCase());
    }
    _release(event) {
        const symbol = event.get_key_symbol();
        let modifier = 0;
        if ([Clutter.KEY_Alt_L, Clutter.KEY_Alt_R].includes(symbol)) modifier = Clutter.ModifierType.MOD1_MASK;
        if ([Clutter.KEY_Control_L, Clutter.KEY_Control_R].includes(symbol)) modifier = Clutter.ModifierType.CONTROL_MASK;
        if ([Clutter.KEY_Shift_L, Clutter.KEY_Shift_R].includes(symbol)) modifier = Clutter.ModifierType.SHIFT_MASK;
        if ([Clutter.KEY_Super_L, Clutter.KEY_Super_R].includes(symbol)) modifier = Clutter.ModifierType.SUPER_MASK;
        this._capture.release(event.get_key_code(), modifier);
    }
    disable() {
        this._ready = false;
        this._capture?.cancel();
        if (this._activated) global.display.disconnect(this._activated);
        if (this._deactivated) global.display.disconnect(this._deactivated);
        if (this._modifierChanged) this._keymap.disconnect(this._modifierChanged);
        if (this._session) Main.sessionMode.disconnect(this._session);
        if (this._changed) this._settings.disconnect(this._changed);
        this._object?.emit_property_changed('Version', new GLib.Variant('u', 0));
        this._object?.unexport();
        this._object = this._capture = this._settings = null;
    }
}
