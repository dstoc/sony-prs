import assert from "node:assert/strict";
import {
  containedSize,
  createBrowserFullscreenController,
} from "../web/reader-web/fullscreen.mjs";
import { logicalPointFromPointer } from "../web/reader-web/input.mjs";

assert.deepEqual(containedSize(1000, 700, 600, 800), { width: 525, height: 700 });
const fittedLandscape = containedSize(1000, 722, 800, 600);
assert.ok(Math.abs(fittedLandscape.width - 800 * 722 / 600) < 1e-9);
assert.equal(fittedLandscape.height, 722);
assert.deepEqual(containedSize(1920, 1080, 800, 600), { width: 1440, height: 1080 });
assert.equal(containedSize(0, 700, 600, 800), null);
assert.equal(containedSize(1000, 700, 0, 800), null);

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
