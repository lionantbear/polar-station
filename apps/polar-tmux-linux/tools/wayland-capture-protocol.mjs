const ABSOLUTE_PNG_MESSAGE = "capture requires an absolute PNG path";
const CAPTURE_SOURCES = Object.freeze({
  window: Object.freeze({ id: "window", portalType: 2 }),
  monitor: Object.freeze({ id: "monitor", portalType: 1 }),
});

export function parseCaptureSourceArgs(args) {
  const options = Array.from(args ?? []);
  if (options.length === 0) {
    return CAPTURE_SOURCES.window;
  }
  if (options.length !== 1) {
    throw new Error("capture accepts exactly one source option");
  }

  const match = /^--source=(.+)$/.exec(options[0]);
  const source = match?.[1];
  if (source === undefined || !(source in CAPTURE_SOURCES)) {
    throw new Error(`unknown capture source: ${source ?? options[0]}`);
  }
  return CAPTURE_SOURCES[source];
}

export function frameHasVisualVariation({
  pixels,
  width,
  height,
  rowstride,
  nChannels,
}) {
  if (
    pixels === null ||
    pixels === undefined ||
    !Number.isInteger(width) ||
    width <= 0 ||
    !Number.isInteger(height) ||
    height <= 0 ||
    !Number.isInteger(nChannels) ||
    nChannels < 3 ||
    !Number.isInteger(rowstride) ||
    rowstride < width * nChannels ||
    pixels.length < rowstride * height
  ) {
    throw new Error("invalid decoded frame geometry");
  }

  const baseline = [pixels[0], pixels[1], pixels[2]];
  const maximumSamples = 250_000;
  const sampleStep = Math.max(
    1,
    Math.floor(Math.sqrt((width * height) / maximumSamples)),
  );
  let sampledPixels = 0;
  let changedPixels = 0;

  for (let y = 0; y < height; y += sampleStep) {
    const row = y * rowstride;
    for (let x = 0; x < width; x += sampleStep) {
      const offset = row + x * nChannels;
      sampledPixels += 1;
      if (
        Math.abs(pixels[offset] - baseline[0]) >= 16 ||
        Math.abs(pixels[offset + 1] - baseline[1]) >= 16 ||
        Math.abs(pixels[offset + 2] - baseline[2]) >= 16
      ) {
        changedPixels += 1;
      }
    }
  }

  return changedPixels / sampledPixels >= 0.01;
}

export function validateOutputPath(outputPath) {
  if (
    typeof outputPath !== "string" ||
    !outputPath.startsWith("/") ||
    !outputPath.endsWith(".png") ||
    outputPath.includes("\0")
  ) {
    throw new Error(ABSOLUTE_PNG_MESSAGE);
  }
  return outputPath;
}

export function parseCaptureCommand(line) {
  const command = String(line ?? "").trim();
  if (command === "status") {
    return { kind: "status" };
  }
  if (command === "quit") {
    return { kind: "quit" };
  }
  if (command === "capture" || command.startsWith("capture ")) {
    const outputPath = command.slice("capture".length).trim();
    return {
      kind: "capture",
      outputPath: validateOutputPath(outputPath),
    };
  }
  throw new Error(`unknown command: ${command || "<empty>"}`);
}

export class CaptureGate {
  #activeOutputPath = null;

  get activeOutputPath() {
    return this.#activeOutputPath;
  }

  begin(outputPath) {
    validateOutputPath(outputPath);
    if (this.#activeOutputPath !== null) {
      throw new Error(
        `capture already in progress: ${this.#activeOutputPath}`,
      );
    }
    this.#activeOutputPath = outputPath;
  }

  finish(outputPath) {
    if (this.#activeOutputPath !== outputPath) {
      throw new Error(
        `capture completion ${outputPath} does not match ${this.#activeOutputPath}`,
      );
    }
    this.#activeOutputPath = null;
  }
}

export function buildGstLaunchArgv({ pipewireFd, nodeId, outputPath }) {
  if (!Number.isInteger(pipewireFd) || pipewireFd < 0) {
    throw new Error(`invalid PipeWire file descriptor: ${pipewireFd}`);
  }
  if (!Number.isInteger(nodeId) || nodeId <= 0) {
    throw new Error(`invalid PipeWire node: ${nodeId}`);
  }
  validateOutputPath(outputPath);
  return [
    "gst-launch-1.0",
    "-q",
    "pipewiresrc",
    `fd=${pipewireFd}`,
    `path=${nodeId}`,
    "num-buffers=1",
    "do-timestamp=true",
    "!",
    "videoconvert",
    "!",
    "pngenc",
    "snapshot=true",
    "!",
    "filesink",
    `location=${outputPath}`,
  ];
}
