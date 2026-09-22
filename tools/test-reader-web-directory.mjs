import assert from "node:assert/strict";
import {
  chooseDirectory,
  readerAfterDirectoryError,
  selectEntryPoint,
} from "../web/reader-web/directory-library.js";

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

const loadedReader = { currentDocument: "README.md" };
const previousWindow = globalThis.window;
globalThis.window = {
  showDirectoryPicker: async () => ({
    name: "empty-library",
    async *values() {
      yield {
        kind: "file",
        name: "cover.png",
        async getFile() {
          return { arrayBuffer: async () => new ArrayBuffer(0) };
        },
      };
    },
  }),
};

let noMarkdownError;
try {
  await chooseDirectory();
  assert.fail("an empty library must be rejected");
} catch (error) {
  noMarkdownError = error;
}
globalThis.window = previousWindow;

assert.match(noMarkdownError.message, /No root Markdown file found/);
assert.equal(noMarkdownError.directorySelected, true);
assert.equal(
  readerAfterDirectoryError(loadedReader, noMarkdownError, false),
  undefined,
  "a failed replacement must clear the previously loaded reader",
);
assert.equal(
  readerAfterDirectoryError(loadedReader, new Error("picker cancelled"), false),
  loadedReader,
  "picker cancellation before selection must preserve the loaded reader",
);

console.log("reader web directory entry-point contract passed");
