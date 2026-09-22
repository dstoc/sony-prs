import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";
import {
  DEMO_ENTRY_POINT,
  DEMO_FILES,
  loadDemoDirectory,
} from "../web/reader-web/demo-library.js";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const demoRoot = resolve(repoRoot, "web/reader-web/demo");

// Exercise the browser loader without a frontend test framework. The mock
// keeps the production URL construction and snapshot shape intact while
// reading the checked-in fixture from disk.
globalThis.fetch = async (request) => {
  const path = fileURLToPath(request);
  const relative = path.slice(`${demoRoot}/`.length);
  const bytes = await readFile(resolve(demoRoot, relative));
  return {
    ok: true,
    status: 200,
    async arrayBuffer() {
      return bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength);
    },
  };
};

const snapshot = await loadDemoDirectory();
assert.equal(snapshot.name, "PRS-T1 demo library");
assert.equal(DEMO_ENTRY_POINT, "README.md");
assert.deepEqual(snapshot.files.map(({ path }) => path), DEMO_FILES);
assert.equal(
  new TextDecoder().decode(snapshot.files[0].bytes).startsWith("# PRS-T1 browser demo"),
  true,
);
assert.equal(snapshot.files.filter(({ path }) => path.endsWith(".png")).length, 2);
assert.ok(snapshot.files.find(({ path }) => path.endsWith(".png")).bytes.length > 100);

console.log("reader web demo fixture contract passed");
