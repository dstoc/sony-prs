const ENTRY_POINT = "README.md";

function childPath(parent, name) {
  // File System Access handles cannot escape their selected parent, but keep
  // the logical namespace explicit before it crosses into Rust.
  if (!name || name === "." || name === ".." || /[\\/]/u.test(name)) {
    throw new Error(`selected directory contains an invalid entry name: ${name}`);
  }
  return parent ? `${parent}/${name}` : name;
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
  const files = [];
  await readDirectory(directory, "", files);
  return { name: directory.name, files };
}

export { ENTRY_POINT };
