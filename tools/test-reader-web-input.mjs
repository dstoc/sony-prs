import assert from "node:assert/strict";
import {
  commandForKeyboardEvent,
  logicalPointFromPointer,
} from "../web/reader-web/input.mjs";

const canvas = {
  getBoundingClientRect() {
    return { left: 10, top: 20, width: 300, height: 400 };
  },
};
const oneXCanvas = {
  getBoundingClientRect() {
    return { left: 10, top: 20, width: 600, height: 800 };
  },
};

assert.deepEqual(
  logicalPointFromPointer(
    { clientX: 160, clientY: 220 },
    canvas,
    600,
    800,
  ),
  { x: 300, y: 400 },
);
assert.deepEqual(
  logicalPointFromPointer(
    { clientX: 10.5, clientY: 20.25 },
    canvas,
    600,
    800,
  ),
  { x: 1, y: 0.5 },
);
assert.deepEqual(
  logicalPointFromPointer(
    { clientX: 310, clientY: 420 },
    oneXCanvas,
    600,
    800,
  ),
  { x: 300, y: 400 },
);
assert.deepEqual(
  logicalPointFromPointer(
    { clientX: 460, clientY: 620 },
    {
      getBoundingClientRect() {
        return { left: 10, top: 20, width: 900, height: 1200 };
      },
    },
    600,
    800,
  ),
  { x: 300, y: 400 },
);
assert.equal(
  logicalPointFromPointer({ clientX: 310, clientY: 220 }, canvas, 600, 800),
  null,
);
assert.equal(
  logicalPointFromPointer({ clientX: 160, clientY: 420 }, canvas, 600, 800),
  null,
);

assert.equal(commandForKeyboardEvent({ key: "ArrowLeft" }), "previous");
assert.equal(commandForKeyboardEvent({ key: "ArrowRight" }), "next");
assert.equal(commandForKeyboardEvent({ key: "Home" }), "home");
assert.equal(commandForKeyboardEvent({ key: "Backspace" }), "back");
assert.equal(
  commandForKeyboardEvent({ key: "ArrowLeft", altKey: true }),
  "back",
);
assert.equal(commandForKeyboardEvent({ key: "Escape" }), null);

console.log("reader web input contract passed");
