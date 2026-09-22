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
const commands = new Map(
  [...document.querySelectorAll("[data-command]")].map((button) => [
    button.dataset.command,
    button,
  ]),
);
const controlButtons = [...commands.values()];
let reader;

function drawFrame(frame) {
  const pixels = new Uint8ClampedArray(frame);
  context.putImageData(new ImageData(pixels, canvas.width, canvas.height), 0, 0);
}

function render(feedback) {
  drawFrame(reader.render_frame());
  status.textContent = feedback + " · " + reader.current_document()
    + " · page " + reader.current_page() + " of " + reader.page_count();
}

function apply(command) {
  if (!reader) {
    return;
  }
  render(reader[command]());
}

function setControlsDisabled(disabled) {
  for (const button of controlButtons) {
    button.disabled = disabled;
  }
}

function errorMessage(error) {
  return error && typeof error.message === "string" ? error.message : String(error);
}

async function chooseLibrary() {
  chooseButton.disabled = true;
  status.textContent = "Reading the selected directory…";
  try {
    const selected = await chooseDirectory();
    const path = entryPoint.value.trim() || ENTRY_POINT;
    const selectedReader = load_directory(selected.files, path);
    reader = selectedReader;
    render("Loaded " + selected.name);
  } catch (error) {
    status.textContent = "Reader error: " + errorMessage(error);
  } finally {
    chooseButton.disabled = false;
  }
}

function isEditableTarget(target) {
  return target instanceof HTMLInputElement
    || target instanceof HTMLTextAreaElement
    || target instanceof HTMLSelectElement
    || (target instanceof HTMLElement && target.isContentEditable);
}

chooseButton.addEventListener("click", chooseLibrary);

try {
  await init();
  canvas.width = logical_width();
  canvas.height = logical_height();
  reader = new ReaderSimulator();
  render(proof_of_life() + " " + reader.feedback());
  setControlsDisabled(false);
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
      render(reader.pointer_up(point.x, point.y));
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
  setControlsDisabled(true);
  chooseButton.disabled = true;
}
