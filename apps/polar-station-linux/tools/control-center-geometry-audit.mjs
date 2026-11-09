#!/usr/bin/gjs -m

import Atspi from "gi://Atspi";
import System from "system";

const EXPECTED = Object.freeze({
  controllerWidth: 611,
  sessionsWidth: 220,
  layoutMapWidth: 390,
  layoutHorizontalOffset: 0,
  maximumBoundedPaneHeight: 200,
});

function roleName(accessible) {
  try {
    return accessible.get_role_name() || "";
  } catch {
    return "";
  }
}

function name(accessible) {
  try {
    return accessible.get_name() || "";
  } catch {
    return "";
  }
}

function children(accessible) {
  let count = 0;
  try {
    count = accessible.get_child_count();
  } catch {
    return [];
  }
  const result = [];
  for (let index = 0; index < count; index += 1) {
    result.push(accessible.get_child_at_index(index));
  }
  return result;
}

function descendants(accessible) {
  const result = [];
  const pending = [...children(accessible)];
  while (pending.length > 0) {
    const current = pending.shift();
    result.push(current);
    pending.push(...children(current));
  }
  return result;
}

function contains(accessible, predicate) {
  return descendants(accessible).some(predicate);
}

function ancestor(accessible, predicate) {
  let current = accessible;
  while (current) {
    if (predicate(current)) {
      return current;
    }
    try {
      current = current.get_parent();
    } catch {
      current = null;
    }
  }
  return null;
}

function extent(accessible) {
  const component = accessible.get_component_iface();
  if (!component) {
    return null;
  }
  const value = component.get_extents(Atspi.CoordType.WINDOW);
  return {
    x: value.x,
    y: value.y,
    width: value.width,
    height: value.height,
  };
}

function fail(message) {
  printerr(`GEOMETRY_AUDIT_FAILED ${message}`);
  System.exit(1);
}

function exactControl(candidate, expectedName) {
  return (
    ["button", "toggle button"].includes(roleName(candidate)) &&
    name(candidate) === expectedName
  );
}

Atspi.init();
const desktop = Atspi.get_desktop(0);
const all = [desktop, ...descendants(desktop)];
const workspaceControl = all.find((candidate) =>
  exactControl(candidate, "Workspace"),
);
if (!workspaceControl) {
  fail("no visible Polar Station Workspace control was found");
}

const frame = ancestor(
  workspaceControl,
  (candidate) => roleName(candidate) === "frame",
);
if (!frame) {
  fail("the Polar Station controls have no native frame");
}

const stableControlPredicates = [
  (candidate) => exactControl(candidate, "Workspace"),
  (candidate) => exactControl(candidate, "Rename Workspace"),
  (candidate) =>
    roleName(candidate) === "button" &&
    name(candidate).startsWith("SSH target status"),
  (candidate) => exactControl(candidate, "Add Tab"),
  (candidate) => exactControl(candidate, "Refresh"),
  (candidate) => exactControl(candidate, "Sessions"),
  (candidate) => exactControl(candidate, "Resources"),
  (candidate) => exactControl(candidate, "iPhone"),
  (candidate) => exactControl(candidate, "More"),
  (candidate) =>
    roleName(candidate) === "label" && name(candidate) === "Layout Map",
];

const controller = ancestor(
  workspaceControl,
  (candidate) =>
    roleName(candidate) === "panel" &&
    stableControlPredicates.every((predicate) =>
      contains(candidate, predicate),
    ),
);
if (!controller) {
  fail("could not resolve the stable Control Center root");
}

const controllerDescendants = descendants(controller);
const requiredControlNames = [
  "Workspace",
  "Rename Workspace",
  "Add Tab",
  "Refresh",
  "Sessions",
  "Resources",
  "iPhone",
  "More",
];
const requiredControls = requiredControlNames.map((expectedName) => {
  const control = controllerDescendants.find((candidate) =>
    exactControl(candidate, expectedName),
  );
  if (!control) {
    fail(`required control ${expectedName} is missing`);
  }
  return [expectedName, control];
});
const sshControl = controllerDescendants.find(
  (candidate) =>
    roleName(candidate) === "button" &&
    name(candidate).startsWith("SSH target status"),
);
if (!sshControl) {
  fail("required control SSH status is missing");
}
requiredControls.push(["SSH status", sshControl]);

const requiredControlExtents = Object.fromEntries(
  requiredControls.map(([controlName, control]) => [
    controlName,
    extent(control),
  ]),
);
for (const [controlName, controlExtent] of Object.entries(
  requiredControlExtents,
)) {
  if (
    !controlExtent ||
    controlExtent.width <= 0 ||
    controlExtent.height <= 0
  ) {
    fail(`required control ${controlName} has no visible allocation`);
  }
}

const sessions = controllerDescendants.find(
  (candidate) =>
    roleName(candidate) === "grouping" &&
    contains(candidate, (child) =>
      exactControl(child, "Refresh sessions"),
    ),
);
if (!sessions) {
  fail("Sessions is closed or its grouping is missing");
}
const layoutMap = controllerDescendants.find(
  (candidate) =>
    roleName(candidate) === "scroll pane" &&
    contains(
      candidate,
      (child) =>
        roleName(child) === "label" &&
        name(child) === "Layout Map",
    ),
);
if (!layoutMap) {
  fail("Layout Map scroll pane is missing");
}
const layoutHeading = controllerDescendants.find(
  (candidate) =>
    roleName(candidate) === "label" &&
    name(candidate) === "Layout Map",
);
if (!layoutHeading) {
  fail("Layout Map heading is missing");
}
const layoutMapExtent = extent(layoutMap);
const layoutHeadingExtent = extent(layoutHeading);
const layoutHorizontalOffset = Math.max(
  0,
  (layoutMapExtent?.x ?? 0) + 10 - (layoutHeadingExtent?.x ?? 0),
);

const paneButtons = controllerDescendants.filter(
  (candidate) =>
    roleName(candidate) === "button" &&
    /^●\s+\S+/.test(name(candidate)),
);
if (paneButtons.length === 0) {
  fail("no projected Pane buttons were found");
}

const observed = {
  controllerWidth: extent(controller)?.width,
  sessionsWidth: extent(sessions)?.width,
  layoutMapWidth: layoutMapExtent?.width,
  layoutHorizontalOffset,
  requiredControlExtents,
  paneHeights: paneButtons
    .map((button) => extent(button))
    .filter((value) => value !== null)
    .map((value) => value.height),
};
if (observed.paneHeights.length !== paneButtons.length) {
  fail("one or more projected Pane buttons has no component allocation");
}
print(JSON.stringify({ expected: EXPECTED, observed }));

if (observed.controllerWidth !== EXPECTED.controllerWidth) {
  fail(
    `controller width ${observed.controllerWidth} != ${EXPECTED.controllerWidth}`,
  );
}
if (observed.sessionsWidth !== EXPECTED.sessionsWidth) {
  fail(
    `Sessions width ${observed.sessionsWidth} != ${EXPECTED.sessionsWidth}`,
  );
}
if (observed.layoutMapWidth !== EXPECTED.layoutMapWidth) {
  fail(
    `Layout Map width ${observed.layoutMapWidth} != ${EXPECTED.layoutMapWidth}`,
  );
}
if (
  observed.layoutHorizontalOffset !==
  EXPECTED.layoutHorizontalOffset
) {
  fail(
    `Layout Map horizontal offset ${observed.layoutHorizontalOffset} != ${EXPECTED.layoutHorizontalOffset}`,
  );
}
if (
  Math.max(...observed.paneHeights) >
  EXPECTED.maximumBoundedPaneHeight
) {
  fail(
    `bounded Pane height ${Math.max(...observed.paneHeights)} > ${EXPECTED.maximumBoundedPaneHeight}`,
  );
}

print("GEOMETRY_AUDIT_OK");
