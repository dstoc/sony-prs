import init, { proof_of_life } from "./pkg/prs_reader_web.js";

const status = document.querySelector("#status");

try {
  await init();
  status.textContent = proof_of_life();
} catch (error) {
  status.textContent = `WASM load failed: ${error}`;
  throw error;
}
