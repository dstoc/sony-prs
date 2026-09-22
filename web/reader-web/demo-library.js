const DEMO_ROOT = new URL("./demo/", import.meta.url);

export const DEMO_ENTRY_POINT = "README.md";

// Keep this manifest explicit so a clean checkout has a deterministic demo
// library and a missing copied asset fails during the demo load, not halfway
// through rendering a page.
export const DEMO_FILES = [
  "README.md",
  "guide/chapter.md",
  "guide/notes.md",
  "assets/observatory.png",
  "assets/detail.png",
];

async function readDemoFile(path) {
  const response = await fetch(new URL(path, DEMO_ROOT));
  if (!response.ok) {
    throw new Error(`demo file ${path} could not be loaded (${response.status})`);
  }
  return {
    path,
    bytes: new Uint8Array(await response.arrayBuffer()),
  };
}

/** Load the checked-in fixture through the same snapshot shape as a picker. */
export async function loadDemoDirectory() {
  const files = await Promise.all(DEMO_FILES.map(readDemoFile));
  return { name: "PRS-T1 demo library", files };
}
