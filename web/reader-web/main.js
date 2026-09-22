import init, {
  ReaderSimulator,
  logical_height,
  logical_width,
} from "./pkg/prs_reader_web.js";

const status = document.querySelector("#status");
const canvas = document.querySelector("#reader-canvas");
const context = canvas.getContext("2d", { alpha: false });
const markdown = `# PRS-T1 reader simulator

This page is parsed, laid out, paginated, and rendered by the shared Rust
reader. The browser only uploads its RGBA framebuffer to this canvas.

The canvas keeps a 600 × 800 logical surface while CSS fits it to the window.`;

function drawFrame(frame) {
  const pixels = new Uint8ClampedArray(frame);
  context.putImageData(new ImageData(pixels, canvas.width, canvas.height), 0, 0);
}

try {
  await init();
  canvas.width = logical_width();
  canvas.height = logical_height();
  const reader = new ReaderSimulator(markdown);
  drawFrame(reader.render_frame());
  status.textContent = `Shared Rust renderer · ${canvas.width} × ${canvas.height} logical pixels`;
} catch (error) {
  status.textContent = `WASM load failed: ${error}`;
  throw error;
}
