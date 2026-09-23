import assert from "node:assert/strict";
import {
  containedSize,
  createBrowserFullscreenController,
  syncCanvasPresentation,
} from "../web/reader-web/fullscreen.mjs";
import {
  logicalPointFromPointer,
  logicalPointFromReaderPointer,
} from "../web/reader-web/input.mjs";

assert.deepEqual(containedSize(1000, 700, 600, 800), { width: 525, height: 700 });
const fittedLandscape = containedSize(1000, 722, 800, 600);
assert.ok(Math.abs(fittedLandscape.width - 800 * 722 / 600) < 1e-9);
assert.equal(fittedLandscape.height, 722);
assert.deepEqual(containedSize(1920, 1080, 800, 600), { width: 1440, height: 1080 });
assert.equal(containedSize(0, 700, 600, 800), null);
assert.equal(containedSize(1000, 700, 0, 800), null);

const presentationFrame = {
  clientWidth: 1000,
  clientHeight: 722,
  style: {},
};
const presentationCanvas = {
  width: 600,
  height: 800,
  style: {
    removeProperty(property) {
      delete this[property];
    },
  },
  getBoundingClientRect() {
    const width = Number.parseFloat(this.style.width) || activeReader.width * 0.75;
    const height = Number.parseFloat(this.style.height) || activeReader.height * 0.75;
    return {
      left: (presentationFrame.clientWidth - width) / 2,
      top: (presentationFrame.clientHeight - height) / 2,
      width,
      height,
    };
  },
};
const activeReader = {
  width: 600,
  height: 800,
  logical_width() { return this.width; },
  logical_height() { return this.height; },
  toggle_orientation() {
    [this.width, this.height] = [this.height, this.width];
  },
};

syncCanvasPresentation({
  canvas: presentationCanvas,
  readerFrame: presentationFrame,
  reader: activeReader,
  fullscreen: true,
});
assert.equal(presentationCanvas.width, 600);
assert.equal(presentationCanvas.height, 800);
assert.equal(presentationFrame.style.aspectRatio, "600 / 800");
assert.equal(presentationCanvas.style.width, "541.5px");
assert.equal(presentationCanvas.style.height, "722px");
assert.deepEqual(
  logicalPointFromReaderPointer(
    { clientX: 500, clientY: 361 },
    presentationCanvas,
    activeReader,
  ),
  { x: 300, y: 400 },
);

activeReader.toggle_orientation();
syncCanvasPresentation({
  canvas: presentationCanvas,
  readerFrame: presentationFrame,
  reader: activeReader,
  fullscreen: true,
});
assert.equal(presentationCanvas.width, 800);
assert.equal(presentationCanvas.height, 600);
assert.equal(presentationFrame.style.aspectRatio, "800 / 600");
assert.ok(
  Math.abs(Number.parseFloat(presentationCanvas.style.width) - 800 * 722 / 600)
    < 1e-9,
);
assert.equal(presentationCanvas.style.height, "722px");
assert.deepEqual(
  logicalPointFromReaderPointer(
    { clientX: 500, clientY: 361 },
    presentationCanvas,
    activeReader,
  ),
  { x: 400, y: 300 },
);
const landscapeRect = presentationCanvas.getBoundingClientRect();
const fractionalLandscapePoint = logicalPointFromReaderPointer(
  {
    clientX: landscapeRect.left + landscapeRect.width * 0.375,
    clientY: landscapeRect.top + landscapeRect.height * 0.625,
  },
  presentationCanvas,
  activeReader,
);
assert.ok(Math.abs(fractionalLandscapePoint.x - 300) < 1e-9);
assert.ok(Math.abs(fractionalLandscapePoint.y - 375) < 1e-9);

syncCanvasPresentation({
  canvas: presentationCanvas,
  readerFrame: presentationFrame,
  reader: activeReader,
  fullscreen: false,
});
assert.equal(presentationCanvas.width, 800);
assert.equal(presentationCanvas.height, 600);
assert.equal("width" in presentationCanvas.style, false);
assert.equal("height" in presentationCanvas.style, false);
const shellRect = presentationCanvas.getBoundingClientRect();
assert.deepEqual(
  logicalPointFromReaderPointer(
    {
      clientX: shellRect.left + shellRect.width / 2,
      clientY: shellRect.top + shellRect.height / 2,
    },
    presentationCanvas,
    activeReader,
  ),
  { x: 400, y: 300 },
);

const fittedPortrait = containedSize(1000, 722, 600, 800);
const portraitCanvas = {
  getBoundingClientRect() {
    return {
      left: (1000 - fittedPortrait.width) / 2,
      top: (722 - fittedPortrait.height) / 2,
      width: fittedPortrait.width,
      height: fittedPortrait.height,
    };
  },
};
assert.deepEqual(
  logicalPointFromPointer(
    { clientX: 500, clientY: 361 },
    portraitCanvas,
    600,
    800,
  ),
  { x: 300, y: 400 },
);

class FakeEvents {
  listeners = new Map();

  addEventListener(type, listener) {
    const listeners = this.listeners.get(type) ?? new Set();
    listeners.add(listener);
    this.listeners.set(type, listeners);
  }

  removeEventListener(type, listener) {
    this.listeners.get(type)?.delete(listener);
  }

  dispatch(type) {
    for (const listener of this.listeners.get(type) ?? []) {
      listener({ type });
    }
  }
}

function fullscreenFixture({ requestError, exitError, supported = true } = {}) {
  const documentObject = new FakeEvents();
  documentObject.fullscreenElement = null;
  documentObject.exitFullscreen = async () => {
    if (exitError) {
      throw exitError;
    }
    documentObject.fullscreenElement = null;
    documentObject.dispatch("fullscreenchange");
  };
  const windowObject = new FakeEvents();
  const target = {};
  if (supported) {
    target.requestFullscreen = async () => {
      if (requestError) {
        throw requestError;
      }
      documentObject.fullscreenElement = target;
      documentObject.dispatch("fullscreenchange");
    };
  }

  const changes = [];
  const statuses = [];
  let resizeCount = 0;
  const controller = createBrowserFullscreenController({
    target,
    documentObject,
    windowObject,
    onFullscreenChange: (fullscreen) => changes.push(fullscreen),
    onViewportResize: () => { resizeCount += 1; },
    setStatus: (message) => statuses.push(message),
  });
  return {
    changes,
    controller,
    documentObject,
    statuses,
    target,
    windowObject,
    resizeCount: () => resizeCount,
  };
}

const transitions = fullscreenFixture();
assert.deepEqual(transitions.changes, [false]);
assert.equal(await transitions.controller.toggle(), true);
assert.equal(transitions.controller.isFullscreen(), true);
assert.equal(transitions.changes.at(-1), true);
assert.equal(await transitions.controller.toggle(), true);
assert.equal(transitions.controller.isFullscreen(), false);
assert.equal(transitions.changes.at(-1), false);

transitions.documentObject.fullscreenElement = transitions.target;
transitions.documentObject.dispatch("fullscreenchange");
transitions.documentObject.fullscreenElement = null;
transitions.documentObject.dispatch("fullscreenchange");
assert.equal(transitions.changes.at(-1), false);
transitions.windowObject.dispatch("resize");
transitions.windowObject.dispatch("orientationchange");
assert.equal(transitions.resizeCount(), 2);
transitions.controller.dispose();
transitions.windowObject.dispatch("resize");
assert.equal(transitions.resizeCount(), 2);

const denied = fullscreenFixture({ requestError: new Error("request denied") });
assert.equal(await denied.controller.toggle(), false);
assert.match(denied.statuses.at(-1), /Could not enter browser fullscreen: request denied/);

const unsupported = fullscreenFixture({ supported: false });
assert.equal(await unsupported.controller.toggle(), false);
assert.match(unsupported.statuses.at(-1), /not supported/);

const deniedExit = fullscreenFixture({ exitError: new Error("exit denied") });
deniedExit.documentObject.fullscreenElement = deniedExit.target;
assert.equal(await deniedExit.controller.toggle(), false);
assert.match(deniedExit.statuses.at(-1), /Could not exit browser fullscreen: exit denied/);

console.log("reader web fullscreen contract passed");
