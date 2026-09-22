import init, {
  load_directory,
  logical_height,
  logical_width,
} from "./pkg/prs_reader_web.js";
import {
  chooseDirectory,
  readerAfterDirectoryError,
} from "./directory-library.js";
import {
  commandForKeyboardEvent,
  logicalPointFromPointer,
} from "./input.mjs";

const status = document.querySelector("#status");
const canvas = document.querySelector("#reader-canvas");
const context = canvas.getContext("2d", { alpha: false });
const readerFrame = document.querySelector(".reader-frame");
const controlsNavigation = document.querySelector(".reader-controls");
const chooseButton = document.querySelector("#choose-directory");
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

function render() {
  drawFrame(reader.render_frame());
  status.textContent = reader.current_document()
    + " · page " + reader.current_page() + " of " + reader.page_count();
}

function apply(command) {
  if (!reader) {
    return;
  }
  reader[command]();
  render();
}

function setControlsDisabled(disabled) {
  for (const button of controlButtons) {
    button.disabled = disabled;
  }
}

function setReaderLoaded(loaded) {
  readerFrame.hidden = !loaded;
  controlsNavigation.hidden = !loaded;
}

function errorMessage(error) {
  return error && typeof error.message === "string" ? error.message : String(error);
}

async function chooseLibrary() {
  chooseButton.disabled = true;
  setControlsDisabled(true);
  status.textContent = "Opening directory…";
  let directorySelected = false;
  try {
    const selected = await chooseDirectory();
    directorySelected = true;
    const selectedReader = load_directory(selected.files, selected.entryPoint);
    reader = selectedReader;
    setReaderLoaded(true);
    render();
  } catch (error) {
    reader = readerAfterDirectoryError(reader, error, directorySelected);
    status.textContent = errorMessage(error);
  } finally {
    chooseButton.disabled = false;
    setControlsDisabled(!reader);
    setReaderLoaded(Boolean(reader));
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
  status.textContent = "Open a directory to begin.";
  setReaderLoaded(false);
  setControlsDisabled(true);
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
      reader.pointer_up(point.x, point.y);
      render();
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
  setReaderLoaded(false);
  chooseButton.disabled = true;
}
