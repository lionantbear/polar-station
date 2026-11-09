#!/usr/bin/gjs -m
// Read-only installed-window geometry, not a screenshot or human acceptance.
import Atspi from 'gi://Atspi';
import System from 'system';
import GLib from 'gi://GLib';

Atspi.init();
const pid = Number(ARGV[0]);
if (!Number.isInteger(pid) || pid <= 0) throw Error('Pass the exact managed Station PID');
function children(x) {
  const out = [];
  for (let i = 0; i < x.get_child_count(); i++) out.push(x.get_child_at_index(i));
  return out;
}
function all(x) {
  const out = [], pending = children(x);
  while (pending.length) {
    const n = pending.shift(); out.push(n);
    if (n.get_role_name() !== 'terminal') pending.push(...children(n));
  }
  return out;
}
function rect(x) {
  const e = x.get_component_iface().get_extents(Atspi.CoordType.WINDOW);
  return {x: e.x, y: e.y, width: e.width, height: e.height};
}
function audit() {
const app = children(Atspi.get_desktop(0)).find(x => x.get_process_id() === pid);
if (!app) throw Error('Managed process is not accessible');
const frame = children(app).find(x => all(x).some(n => n.get_role_name() === 'page tab list'));
if (!frame) throw Error('Managed terminal window is missing');
const nodes = all(frame);
const scrollers = nodes.filter(n => n.get_role_name() === 'scroll pane');
const terminal = scrollers.find(n => all(n).some(c => c.get_role_name() === 'page tab list'));
const controller = scrollers.find(n => all(n).some(c =>
  c.get_role_name() === 'toggle button' && ['Layout', 'Show or hide Layout'].includes(c.get_name())));
if (!terminal || !controller) throw Error('Open Control Center before running the audit');
const t = rect(terminal), c = rect(controller);
const violations = [];
if (c.x + c.width > t.x) violations.push('controller overlaps terminal');
for (const n of all(controller)) {
  if (!n.get_state_set().contains(Atspi.StateType.SHOWING)) continue;
  if (!['button', 'toggle button'].includes(n.get_role_name())) continue;
  if (!['Layout', 'Inbox', 'Resources', 'More'].includes(n.get_name())) continue;
  const r = rect(n);
  if (r.x < c.x || r.x + r.width > c.x + c.width) violations.push(`${n.get_name()} escapes controller`);
}
const maps = scrollers.filter(n => all(n).some(x => x.get_name() === 'Tab actions'));
const map = maps.at(-1);
if (map && map.get_state_set().contains(Atspi.StateType.SHOWING)) {
  const m = rect(map);
  for (const n of all(map)) {
    if (n.get_role_name() !== 'button' || !n.get_state_set().contains(Atspi.StateType.SHOWING)) continue;
    if (!(n.get_name() === 'Tab actions' || / Local(?: |$)| SSH(?: |$)| WSL(?: |$)/.test(n.get_name()))) continue;
    const r = rect(n);
    if (r.x < m.x || r.x + r.width > m.x + m.width) violations.push('Map control escapes its viewport');
  }
}
return {pid, controller: c, terminal: t, violations};
}
let result, transientError;
for (let attempt = 0; attempt < 5; attempt++) {
  try { result = audit(); break; }
  catch (error) { transientError = error; GLib.usleep(100000); }
}
if (!result) throw transientError;
print(JSON.stringify(result));
System.exit(result.violations.length ? 1 : 0);
