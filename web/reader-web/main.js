import init, {
  ReaderSimulator,
  load_directory,
  logical_height,
  logical_width,
  proof_of_life,
} from "./pkg/prs_reader_web.js";
import { chooseDirectory, ENTRY_POINT } from "./directory-library.js";

const status = document.querySelector("#status");
const canvas = document.querySelector("#reader-canvas");
const context = canvas.getContext("2d", { alpha: false });
const chooseButton = document.querySelector("#choose-directory");
const entryPoint = document.querySelector("#entry-point");
const reference = document.querySelector("#reference");
const resolveButton = document.querySelector("#resolve-reference");
const markdown = `# PRS-T1 reader simulator

This page is parsed, laid out, paginated, and rendered by the shared Rust
reader. The browser only uploads its RGBA framebuffer to this canvas.

The canvas keeps a 600 × 800 logical surface while CSS fits it to the window.`;
let reader;

function drawFrame(frame) {
  const pixels = new Uint8ClampedArray(frame);
  context.putImageData(new ImageData(pixels, canvas.width, canvas.height), 0, 0);
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
    status.textContent = `Loaded ${reader.current_document()} from ${selected.name} (${reader.page_count()} page${reader.page_count() === 1 ? "" : "s"}).`;
  } catch (error) {
    status.textContent = `Reader error: ${errorMessage(error)}`;
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
    status.textContent = `Resolved ${reference.value} as ${target.kind}${target.path ? ` (${target.path})` : ""}.`;
  } catch (error) {
    status.textContent = `Reader error: ${errorMessage(error)}`;
  }
}

chooseButton.addEventListener("click", chooseLibrary);
resolveButton.addEventListener("click", resolveReference);

try {
  await init();
  canvas.width = logical_width();
  canvas.height = logical_height();
  const simulator = new ReaderSimulator(markdown);
  drawFrame(simulator.render_frame());
  status.textContent = `${proof_of_life()} Shared Rust renderer · ${canvas.width} × ${canvas.height} logical pixels`;
  chooseButton.disabled = false;
} catch (error) {
  status.textContent = `WASM load failed: ${errorMessage(error)}`;
  chooseButton.disabled = true;
}
