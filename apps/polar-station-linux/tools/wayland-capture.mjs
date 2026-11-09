#!/usr/bin/gjs -m

import Gio from "gi://Gio";
import GioUnix from "gi://GioUnix";
import GdkPixbuf from "gi://GdkPixbuf";
import GLib from "gi://GLib";
import System from "system";

import {
  CaptureGate,
  buildGstLaunchArgv,
  frameHasVisualVariation,
  parseCaptureCommand,
  parseCaptureSourceArgs,
} from "./wayland-capture-protocol.mjs";

const PORTAL_BUS = "org.freedesktop.portal.Desktop";
const PORTAL_PATH = "/org/freedesktop/portal/desktop";
const SCREENCAST_INTERFACE = "org.freedesktop.portal.ScreenCast";
const REQUEST_INTERFACE = "org.freedesktop.portal.Request";
const SESSION_INTERFACE = "org.freedesktop.portal.Session";
const HIDDEN_CURSOR_MODE = 1;
const CHILD_PIPEWIRE_FD = 3;

const loop = GLib.MainLoop.new(null, false);
const connection = Gio.bus_get_sync(Gio.BusType.SESSION, null);
const captureGate = new CaptureGate();
let requestSequence = 0;
let sessionHandle = null;
let nodeId = null;
let captureSource = null;

function usage() {
  print(`Usage:
  gjs -m apps/polar-station-linux/tools/wayland-capture.mjs [--source=window|monitor]

The default source is one Polar Station window. Use --source=monitor when a
GPU-rendered window stream is black. After READY, send:
  capture /absolute/path/to/frame.png
  status
  quit`);
}

function token(prefix) {
  requestSequence += 1;
  return `${prefix}_${GLib.get_monotonic_time()}_${requestSequence}`;
}

function senderPathComponent() {
  return connection
    .get_unique_name()
    .replace(/^:/, "")
    .replaceAll(".", "_");
}

function requestPath(handleToken) {
  return `/org/freedesktop/portal/desktop/request/${senderPathComponent()}/${handleToken}`;
}

function recursivelyUnpack(value) {
  if (value instanceof GLib.Variant) {
    return recursivelyUnpack(value.deepUnpack());
  }
  if (Array.isArray(value)) {
    return value.map(recursivelyUnpack);
  }
  if (value !== null && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value).map(([key, item]) => [
        key,
        recursivelyUnpack(item),
      ]),
    );
  }
  return value;
}

function portalRequest(method, signature, values, options) {
  const handleToken = token(method.toLocaleLowerCase());
  const expectedPath = requestPath(handleToken);
  const callOptions = {
    ...options,
    handle_token: new GLib.Variant("s", handleToken),
  };
  const parameters = new GLib.Variant(signature, [...values, callOptions]);

  return new Promise((resolve, reject) => {
    let settled = false;
    const subscriptionId = connection.signal_subscribe(
      null,
      REQUEST_INTERFACE,
      "Response",
      expectedPath,
      null,
      Gio.DBusSignalFlags.NONE,
      (
        _connection,
        _sender,
        _path,
        _interface,
        _signal,
        responseParameters,
      ) => {
        if (settled) {
          return;
        }
        settled = true;
        connection.signal_unsubscribe(subscriptionId);
        const [responseCode, responseResults] =
          recursivelyUnpack(responseParameters);
        if (responseCode !== 0) {
          reject(
            new Error(`${method} portal response code ${responseCode}`),
          );
          return;
        }
        resolve(responseResults);
      },
    );

    connection.call(
      PORTAL_BUS,
      PORTAL_PATH,
      SCREENCAST_INTERFACE,
      method,
      parameters,
      new GLib.VariantType("(o)"),
      Gio.DBusCallFlags.NONE,
      -1,
      null,
      (source, result) => {
        try {
          const [returnedPath] = source.call_finish(result).deepUnpack();
          if (returnedPath !== expectedPath) {
            throw new Error(
              `${method} returned ${returnedPath}, expected ${expectedPath}`,
            );
          }
        } catch (error) {
          if (!settled) {
            settled = true;
            connection.signal_unsubscribe(subscriptionId);
            reject(error);
          }
        }
      },
    );
  });
}

function openPipeWireRemote() {
  return new Promise((resolve, reject) => {
    connection.call_with_unix_fd_list(
      PORTAL_BUS,
      PORTAL_PATH,
      SCREENCAST_INTERFACE,
      "OpenPipeWireRemote",
      new GLib.Variant("(oa{sv})", [sessionHandle, {}]),
      new GLib.VariantType("(h)"),
      Gio.DBusCallFlags.NONE,
      -1,
      null,
      null,
      (source, result) => {
        try {
          const [reply, fdList] =
            source.call_with_unix_fd_list_finish(result);
          const [fdIndex] = reply.deepUnpack();
          resolve(fdList.get(fdIndex));
        } catch (error) {
          reject(error);
        }
      },
    );
  });
}

function communicateUtf8(subprocess) {
  return new Promise((resolve, reject) => {
    subprocess.communicate_utf8_async(
      null,
      null,
      (source, result) => {
        try {
          const [_completed, stdout, stderr] =
            source.communicate_utf8_finish(result);
          resolve({ stdout, stderr });
        } catch (error) {
          reject(error);
        }
      },
    );
  });
}

function readLine(input) {
  return new Promise((resolve, reject) => {
    input.read_line_async(
      GLib.PRIORITY_DEFAULT,
      null,
      (source, result) => {
        try {
          const [line] = source.read_line_finish_utf8(result);
          resolve(line);
        } catch (error) {
          reject(error);
        }
      },
    );
  });
}

function fileSize(outputPath) {
  const file = Gio.File.new_for_path(outputPath);
  const info = file.query_info(
    Gio.FILE_ATTRIBUTE_STANDARD_SIZE,
    Gio.FileQueryInfoFlags.NONE,
    null,
  );
  return info.get_size();
}

function validateCapturedFrame(outputPath) {
  const pixbuf = GdkPixbuf.Pixbuf.new_from_file(outputPath);
  const hasVariation = frameHasVisualVariation({
    pixels: pixbuf.get_pixels(),
    width: pixbuf.get_width(),
    height: pixbuf.get_height(),
    rowstride: pixbuf.get_rowstride(),
    nChannels: pixbuf.get_n_channels(),
  });
  if (hasVariation) {
    return;
  }

  const output = Gio.File.new_for_path(outputPath);
  output.delete(null);
  throw new Error(
    `captured frame is visually flat and was removed: ${outputPath}`,
  );
}

async function captureFrame(outputPath) {
  captureGate.begin(outputPath);
  try {
    if (GLib.file_test(outputPath, GLib.FileTest.EXISTS)) {
      throw new Error(`refusing to overwrite existing capture: ${outputPath}`);
    }
    const parent = GLib.path_get_dirname(outputPath);
    if (GLib.mkdir_with_parents(parent, 0o755) !== 0) {
      throw new Error(`cannot create capture directory: ${parent}`);
    }

    const pipewireFd = await openPipeWireRemote();
    const launcher = new Gio.SubprocessLauncher({
      flags:
        Gio.SubprocessFlags.STDOUT_PIPE |
        Gio.SubprocessFlags.STDERR_PIPE,
    });
    launcher.take_fd(pipewireFd, CHILD_PIPEWIRE_FD);
    const subprocess = launcher.spawnv(
      buildGstLaunchArgv({
        pipewireFd: CHILD_PIPEWIRE_FD,
        nodeId,
        outputPath,
      }),
    );
    const { stderr } = await communicateUtf8(subprocess);
    if (!subprocess.get_successful()) {
      throw new Error(
        `GStreamer capture failed: ${(stderr || "").trim() || "unknown error"}`,
      );
    }
    if (!GLib.file_test(outputPath, GLib.FileTest.IS_REGULAR)) {
      throw new Error(`GStreamer produced no capture: ${outputPath}`);
    }
    validateCapturedFrame(outputPath);
    const bytes = fileSize(outputPath);
    if (bytes <= 0) {
      throw new Error(`GStreamer produced an empty capture: ${outputPath}`);
    }
    print(`CAPTURED path=${outputPath} bytes=${bytes}`);
  } finally {
    captureGate.finish(outputPath);
  }
}

function closeSession() {
  if (sessionHandle === null) {
    return;
  }
  try {
    connection.call_sync(
      PORTAL_BUS,
      sessionHandle,
      SESSION_INTERFACE,
      "Close",
      null,
      null,
      Gio.DBusCallFlags.NONE,
      -1,
      null,
    );
  } catch (error) {
    printerr(`SESSION_CLOSE_ERROR=${error.message}`);
  }
  sessionHandle = null;
}

async function commandLoop() {
  const stdin = new GioUnix.InputStream({
    fd: 0,
    close_fd: false,
  });
  const input = new Gio.DataInputStream({ base_stream: stdin });
  while (true) {
    const line = await readLine(input);
    if (line === null) {
      closeSession();
      return;
    }
    let command;
    try {
      command = parseCaptureCommand(line);
    } catch (error) {
      printerr(`COMMAND_ERROR=${error.message}`);
      continue;
    }

    if (command.kind === "quit") {
      closeSession();
      print("CLOSED");
      return;
    }
    if (command.kind === "status") {
      print(
        `STATUS session=${sessionHandle} node=${nodeId} capture=${captureGate.activeOutputPath ?? "idle"}`,
      );
      continue;
    }
    try {
      await captureFrame(command.outputPath);
    } catch (error) {
      printerr(`CAPTURE_ERROR=${error.message}`);
    }
  }
}

async function main() {
  if (globalThis.ARGV.includes("--help")) {
    usage();
    return;
  }
  captureSource = parseCaptureSourceArgs(globalThis.ARGV);

  const sessionToken = token("session");
  const createResults = await portalRequest(
    "CreateSession",
    "(a{sv})",
    [],
    {
      session_handle_token: new GLib.Variant("s", sessionToken),
    },
  );
  sessionHandle = createResults.session_handle;
  if (
    typeof sessionHandle !== "string" ||
    !sessionHandle.startsWith(
      `/org/freedesktop/portal/desktop/session/${senderPathComponent()}/`,
    )
  ) {
    throw new Error(`invalid Portal session handle: ${sessionHandle}`);
  }

  connection.signal_subscribe(
    null,
    SESSION_INTERFACE,
    "Closed",
    sessionHandle,
    null,
    Gio.DBusSignalFlags.NONE,
    () => {
      sessionHandle = null;
      printerr("SESSION_CLOSED");
      loop.quit();
    },
  );

  await portalRequest(
    "SelectSources",
    "(oa{sv})",
    [sessionHandle],
    {
      types: new GLib.Variant("u", captureSource.portalType),
      multiple: new GLib.Variant("b", false),
      cursor_mode: new GLib.Variant("u", HIDDEN_CURSOR_MODE),
    },
  );
  const startResults = await portalRequest(
    "Start",
    "(osa{sv})",
    [sessionHandle, ""],
    {},
  );
  const streams = recursivelyUnpack(startResults.streams);
  if (!Array.isArray(streams) || streams.length !== 1) {
    throw new Error(
      `expected exactly one approved window stream, got ${streams?.length ?? 0}`,
    );
  }
  const [selectedNodeId, properties] = streams[0];
  if (!Number.isInteger(selectedNodeId) || selectedNodeId <= 0) {
    throw new Error(`invalid selected PipeWire node: ${selectedNodeId}`);
  }
  if (
    properties?.source_type !== undefined &&
    properties.source_type !== captureSource.portalType
  ) {
    throw new Error(
      `approved stream is not a ${captureSource.id}: source_type=${properties.source_type}`,
    );
  }
  nodeId = selectedNodeId;
  print(
    `READY session=${sessionHandle} node=${nodeId} source=${captureSource.id}`,
  );
  print("COMMANDS capture <absolute-png-path> | status | quit");
  await commandLoop();
}

main()
  .catch((error) => {
    printerr(`FATAL=${error.message}`);
    closeSession();
    globalThis.__polarCaptureExitCode = 1;
  })
  .finally(() => loop.quit());

loop.run();

if (globalThis.__polarCaptureExitCode === 1) {
  System.exit(1);
}
