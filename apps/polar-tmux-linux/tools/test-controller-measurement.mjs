#!/usr/bin/gjs -m
import Gtk from 'gi://Gtk?version=4.0';
import Gio from 'gi://Gio';
import Adw from 'gi://Adw';
import Gdk from 'gi://Gdk?version=4.0';
import Pango from 'gi://Pango';

Gtk.init();
Adw.init();
const file = Gio.File.new_for_path('apps/polar-tmux-linux/ghostty-module/controller_pane.zig');
const [, contents] = file.load_contents(null);
const source = new TextDecoder().decode(contents);
const [, overlayContents] = Gio.File.new_for_path(
    'apps/polar-tmux-linux/ghostty-overlay/ghostty-v1.3.1.patch',
).load_contents(null);
const overlaySource = new TextDecoder().decode(overlayContents);
const terminalTemplate = overlaySource.split('child: Gtk.ScrolledWindow polar_station_terminal {')[1]
    .split('Adw.TabOverview tab_overview {')[0];
const horizontal = terminalTemplate.match(/hscrollbar-policy: (\w+);/)[1];
const vertical = terminalTemplate.match(/vscrollbar-policy: (\w+);/)[1];
// A previous monitor's terminal minimum must not inflate the new outer frame.
const oversizedTerminal = new Gtk.Box();
oversizedTerminal.set_size_request(2072, 1800);
const terminalViewport = new Gtk.ScrolledWindow();
terminalViewport.set_policy(Gtk.PolicyType[horizontal.toUpperCase()], Gtk.PolicyType[vertical.toUpperCase()]);
terminalViewport.set_propagate_natural_width(false);
terminalViewport.set_propagate_natural_height(false);
terminalViewport.set_child(oversizedTerminal);
terminalViewport.set_margin_start(421);
if (terminalViewport.measure(Gtk.Orientation.HORIZONTAL, -1)[0] > 421 ||
    terminalViewport.measure(Gtk.Orientation.VERTICAL, -1)[0] > 0)
    throw Error('previous terminal minimum escapes its bounded viewport');
if (!overlaySource.includes('gtk.Overlay.signals.get_child_position.connect'))
    throw Error('controller has no native allocation-time position handler');
// Reuse GTK's real per-allocation signal, including its fallback for the HUD.
const shell = new Gtk.Overlay();
shell.set_child(terminalViewport);
const pane = new Gtk.ScrolledWindow({hscrollbar_policy: Gtk.PolicyType.EXTERNAL,
    vscrollbar_policy: Gtk.PolicyType.EXTERNAL, halign: Gtk.Align.FILL});
pane.set_child(new Gtk.Label({label: 'Controller'}));
shell.add_overlay(pane);
shell.set_measure_overlay(pane, false);
const hud = new Gtk.Label({label: 'HUD', halign: Gtk.Align.CENTER});
shell.add_overlay(hud);
let paneWidth = 421;
shell.connect('get-child-position', (overlay, child, allocation) => {
    if (child !== pane) return false;
    allocation.x = 0; allocation.y = 0;
    allocation.width = Math.min(paneWidth, overlay.get_width());
    allocation.height = overlay.get_height();
    return true;
});
for (const selectedWidth of [200, 421, 761]) {
    paneWidth = selectedWidth;
    pane.set_size_request(paneWidth, -1);
    terminalViewport.set_margin_start(paneWidth);
    for (const width of [1536, 2493, 1536]) {
        shell.allocate(width, 800, -1, null);
        if (pane.get_width() !== paneWidth || terminalViewport.get_width() !== width - paneWidth)
            throw Error(`monitor allocation overlaps: ${width}, controller ${pane.get_width()}, terminal ${terminalViewport.get_width()}`);
        if (hud.get_width() >= width) throw Error('controller handler overrides HUD placement');
    }
}
print(JSON.stringify({monitor_allocation_checks: 9, viewport_minimum_isolated: true}));
const results = [];
for (const name of ['controller_viewport', 'layout_scroller', 'inbox_scroller', 'sessions_scroller']) {
    const policy = source.match(new RegExp(`${name}\\.setPolicy\\(\\.(\\w+),`))?.[1];
    if (!policy) throw Error(`missing policy for ${name}`);
    for (const width of [200, 220, 340, 390, 540, 951]) {
        const child = new Gtk.Label({label: 'Wide content '.repeat(100), wrap: false});
        const scroller = new Gtk.ScrolledWindow();
        scroller.set_policy(Gtk.PolicyType[policy.toUpperCase()], Gtk.PolicyType.EXTERNAL);
        scroller.set_propagate_natural_width(false);
        scroller.set_size_request(width, -1);
        scroller.set_child(child);
        const [minimum] = scroller.measure(Gtk.Orientation.HORIZONTAL, -1);
        if (minimum > width) throw Error(`${name}: content forces ${minimum} > allocated ${width}`);
        results.push({name, width, minimum});
        scroller.set_child(null);
    }
}
print(JSON.stringify({passed: results.length, results}));

// Exercise the actual GTK homogeneous-grid minimum, not only its viewport.
const css = new Gtk.CssProvider();
css.load_from_string('.probe button { min-width:0; padding:0; }');
Gtk.StyleContext.add_provider_for_display(Gdk.Display.get_default(), css, 604);
for (const width of [200, 250, 320, 390]) {
    const body = width - 48;
    const gap = Math.max(1, Math.floor((8 * body + 171) / 342));
    const grid = new Gtk.Grid({column_homogeneous:true, column_spacing:gap});
    grid.add_css_class('probe');
    for (const [column,span] of [[0,4],[4,1]]) {
        const slot = Math.floor((body - gap * 4) * span / 5);
        const compact = slot < 64 && source.includes('const compact = slot_width < 64;');
        const pad = compact ? 2 : 8;
        const box = new Gtk.Box({orientation:Gtk.Orientation.VERTICAL, spacing:2,
            margin_start:pad,margin_end:pad,margin_top:2,margin_bottom:2});
        const row = new Gtk.Box({spacing:2});
        row.append(new Gtk.Label({label:'●',visible:!compact}));
        const title = new Gtk.Label({label:'long-session-name',width_chars:1,ellipsize:Pango.EllipsizeMode.END});
        title.add_css_class('heading');row.append(title);box.append(row);
        box.append(new Gtk.Label({label:'Local',width_chars:1,ellipsize:Pango.EllipsizeMode.END}));
        const button = new Gtk.Button({child:box,can_shrink:true});
        grid.attach(button,column,0,span,1);
    }
    const [minimum] = grid.measure(Gtk.Orientation.HORIZONTAL,-1);
    if (minimum > body) throw Error(`Focus grid forces ${minimum} > ${body} at Map ${width}`);
    print(JSON.stringify({map_width:width,body_width:body,grid_minimum:minimum}));
}
