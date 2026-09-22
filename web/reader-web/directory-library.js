function childPath(parent, name) {
  // File System Access handles cannot escape their selected parent, but keep
  // the logical namespace explicit before it crosses into Rust.
  if (!name || name === "." || name === ".." || /[\\/]/u.test(name)) {
    throw new Error(`selected directory contains an invalid entry name: ${name}`);
  }
  return parent ? `${parent}/${name}` : name;
}

function markDirectorySelected(error) {
  if (error && typeof error === "object") {
    error.directorySelected = true;
    return error;
  }

  const wrapped = new Error(String(error));
  wrapped.directorySelected = true;
  return wrapped;
}

/**
 * Select the root Markdown document that opens when a directory is loaded.
 * The explicit preferences keep common libraries predictable; the code-unit
 * comparison makes the fallback independent of picker enumeration order and
 * locale.
 */
export function selectEntryPoint(files) {
  const paths = files
    .map(({ path }) => path)
    .filter((path) => typeof path === "string"
      && !path.includes("/")
      && path.toLowerCase().endsWith(".md"));

  if (paths.includes("README.md")) {
    return "README.md";
  }
  if (paths.includes("index.md")) {
    return "index.md";
  }
  if (paths.length > 0) {
    return [...new Set(paths)].sort((left, right) => (
      left < right ? -1 : left > right ? 1 : 0
    ))[0];
  }

  throw new Error("No root Markdown file found in the selected directory.");
}

/**
 * Keep an existing reader only when the directory picker was cancelled before
 * a new directory was selected.
 */
export function readerAfterDirectoryError(reader, error, directorySelected) {
  return directorySelected || error?.directorySelected === true
    ? undefined
    : reader;
}

async function readDirectory(directory, parent, files) {
  for await (const handle of directory.values()) {
    const path = childPath(parent, handle.name);
    if (handle.kind === "directory") {
      await readDirectory(handle, path, files);
      continue;
    }

    const file = await handle.getFile();
    files.push({
      path,
      bytes: new Uint8Array(await file.arrayBuffer()),
    });
  }
}

/**
 * Prompt for a directory and return a root-relative in-memory snapshot.
 * Nothing is uploaded: bytes stay in this page and are copied into WASM.
 */
export async function chooseDirectory() {
  if (typeof window.showDirectoryPicker !== "function") {
    throw new Error("this browser does not support the File System Access API");
  }

  const directory = await window.showDirectoryPicker({ mode: "read" });
  try {
    const files = [];
    await readDirectory(directory, "", files);
    return {
      name: directory.name,
      files,
      entryPoint: selectEntryPoint(files),
    };
  } catch (error) {
    throw markDirectorySelected(error);
  }
}
