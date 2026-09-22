import init, {
  ReaderSimulator,
  load_directory,
  logical_height,
  logical_width,
  proof_of_life,
} from "./pkg/prs_reader_web.js";
import { chooseDirectory, ENTRY_POINT } from "./directory-library.js";
import {
  commandForKeyboardEvent,
  logicalPointFromPointer,
} from "./input.mjs";

const status = document.querySelector("#status");
const canvas = document.querySelector("#reader-canvas");
const context = canvas.getContext("2d", { alpha: false });
const chooseButton = document.querySelector("#choose-directory");
const entryPoint = document.querySelector("#entry-point");
const reference = document.querySelector("#reference");
const resolveButton = document.querySelector("#resolve-reference");
const commands = new Map(
  [...document.querySelectorAll("[data-command]")].map((button) => [
    button.dataset.command,
    button,
  ]),
);
let simulator;
let reader;

function drawFrame(frame) {
  const pixels = new Uint8ClampedArray(frame);
  context.putImageData(new ImageData(pixels, canvas.width, canvas.height), 0, 0);
}

function render(feedback) {
  drawFrame(simulator.render_frame());
  status.textContent = feedback + " · page " + simulator.current_page()
    + " of " + simulator.page_count();
}

function apply(command) {
  render(simulator[command]());
}

function errorMessage(error) {
  return error && typeof error.message === "string" ? error.message : String(error);
}

async function chooseLibrary() {
  chooseButton.disabled = true;
  resolveButton.disabled = true;
  status.textContent = "Reading the selected directory…";
  try {
    const selected = await chooseDirectory();
    const path = entryPoint.value.trim() || ENTRY_POINT;
    reader = load_directory(selected.files, path);
    resolveButton.disabled = false;
    status.textContent = "Loaded " + reader.current_document() + " from "
      + selected.name + " (" + reader.page_count() + " page"
      + (reader.page_count() === 1 ? "" : "s") + ").";
  } catch (error) {
    status.textContent = "Reader error: " + errorMessage(error);
  } finally {
    chooseButton.disabled = false;
  }
}

function resolveReference() {
  if (!reader) {
    return;
  }
  try {
    const target = reader.resolve_reference(reference.value);
    status.textContent = "Resolved " + reference.value + " as " + target.kind
      + (target.path ? " (" + target.path + ")" : "") + ".";
  } catch (error) {
    status.textContent = "Reader error: " + errorMessage(error);
  }
}

function isEditableTarget(target) {
  return target instanceof HTMLInputElement
    || target instanceof HTMLTextAreaElement
    || target instanceof HTMLSelectElement
    || (target instanceof HTMLElement && target.isContentEditable);
}

chooseButton.addEventListener("click", chooseLibrary);
resolveButton.addEventListener("click", resolveReference);

try {
  await init();
  canvas.width = logical_width();
  canvas.height = logical_height();
  simulator = new ReaderSimulator();
  render(proof_of_life() + " " + simulator.feedback());
  chooseButton.disabled = false;

  canvas.addEventListener("pointerdown", (event) => {
    canvas.setPointerCapture?.(event.pointerId);
  });

  canvas.addEventListener("pointerup", (event) => {
    canvas.releasePointerCapture?.(event.pointerId);
    const point = logicalPointFromPointer(
      event,
      canvas,
      logical_width(),
      logical_height(),
    );
    if (point) {
      render(simulator.pointer_up(point.x, point.y));
    }
  });

  for (const [command, button] of commands) {
    button.addEventListener("click", () => apply(command));
  }

  document.addEventListener("keydown", (event) => {
    if (isEditableTarget(event.target)) {
      return;
    }
    const command = commandForKeyboardEvent(event);
    if (command) {
      event.preventDefault();
      apply(command);
    }
  });
} catch (error) {
  status.textContent = "WASM load failed: " + errorMessage(error);
  chooseButton.disabled = true;
}
