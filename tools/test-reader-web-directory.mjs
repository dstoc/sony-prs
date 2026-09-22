import assert from "node:assert/strict";
import { selectEntryPoint } from "../web/reader-web/directory-library.js";

const files = (...paths) => paths.map((path) => ({ path }));

assert.equal(
  selectEntryPoint(files("guide/README.md", "index.md", "README.md", "z.md")),
  "README.md",
);
assert.equal(
  selectEntryPoint(files("guide/README.md", "index.md", "z.md")),
  "index.md",
);
assert.equal(
  selectEntryPoint(files("z.md", "guide/first.md", "chapter.md", "a.md")),
  "a.md",
);
assert.throws(
  () => selectEntryPoint(files("guide/README.md", "cover.png", "notes.txt")),
  /No root Markdown file found in the selected directory\./,
);

console.log("reader web directory entry-point contract passed");
