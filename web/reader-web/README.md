# Browser reader simulator

This directory contains the browser build for the PRS-T1 reader simulator. The
WASM module owns the reader, the shared renderer, and a tightly packed RGBA
framebuffer. JavaScript copies each rendered frame into an HTML canvas. The
browser uses the same parser, layout, pagination, navigation, image,
hit-testing, and rendering library as the native reader. The page has no
browser-specific reader layout implementation. It depends on the shared
`prs-markdown` core and does not add a JavaScript package manager or a
frontend framework.

Choose directory opens the browser File System Access API in read-only mode.
`directory-library.js` recursively reads that selected directory into a
root-relative in-memory snapshot and passes the bytes to Rust. The
`BrowserResourceProvider` implements the shared `ResourceProvider` boundary;
local Markdown links and image assets resolve relative to the containing
document, while absolute and escaping paths return structured reader errors.
The default entry point is `README.md`, and the entry-point field accepts
another Markdown path when a library uses a different root document. Files
are not uploaded or persisted by the simulator.

The build uses these pinned versions:

- Rust and Cargo 1.98.1.
- The wasm32-unknown-unknown Rust target.
- wasm-bindgen crate and wasm-bindgen-cli 0.2.128.
- Python 3 with the standard-library http.server for local serving.

The browser page uses plain HTML, CSS, and ES modules. It does not use npm,
Vite, Webpack, React, or another frontend build system.

The simulator uses the shared prs-markdown reader with a checked-in demo
directory under `demo/`. **Load demo library** fetches those files into the
same root-relative snapshot shape produced by **Open directory**, so the demo
exercises the real browser resource provider. Rust owns hit testing, page
turns, link navigation, and reader history. `main.js` only maps browser Pointer
Events to the logical 600 by 800 surface, forwards control or keyboard
commands to the WASM adapter, and copies the Rust-rendered framebuffer.

## Clean build

Install the Rust toolchain and target with rustup:

rustup toolchain install 1.98.1 \
  --profile minimal \
  --target wasm32-unknown-unknown \
  --no-self-update

Install the pinned binding generator with the same Rust toolchain:

cargo +1.98.1 install wasm-bindgen-cli --version 0.2.128 --locked

Build and assemble the page from the repository root:

RUSTUP_TOOLCHAIN=1.98.1 tools/reader-web-build.sh build

The script checks both tool versions. It writes the complete static page to
target/reader-web/ and writes the intermediate Rust artifact to
target/wasm32-unknown-unknown/release/prs_reader_web.wasm.

This is the one assembly command. It removes the previous assembled output,
builds the Rust/WASM module, runs wasm-bindgen, and copies the hand-written
HTML, CSS, and JavaScript files. The generated binding files stay under
`target/reader-web/pkg/`; the copied static shell stays at the output root.

## Local server

Serve the assembled page with the repository helper:

tools/reader-web-serve.sh 8000

Open <http://127.0.0.1:8000/> in a browser. The checked-in demo loads on
startup; use **Load demo library** to reset it. Use the canvas, Previous, Next,
Home, and Back controls. The keyboard shortcuts are Left Arrow, Right Arrow,
Home, Backspace, and Alt+Left for Back. Select **Open directory** to load a
root-relative Markdown library through the browser File System Access API. Use
a static server because the browser loads the generated ES module, fixture,
and WASM file through HTTP.

The demo covers an entry-point README, two linked Markdown documents, a PNG
asset, internal document and fragment links, and external URLs. A lightweight
contract check runs the fixture snapshot loader and pointer mapping at 1×, ½×, and 1.5× CSS display scales; the shared Rust tests cover rendering, image
loading, page turns, navigation history, Home, and external-link events. For
manual browser acceptance, open the demo, resize the window, activate the
chapter and notes links, use Back and Home, turn pages, and activate an
external link. Then choose the fixture directory itself with **Open directory**
to repeat the same flow through the File System Access API.

The directory-picker workflow is read-only and keeps bytes in page memory. It
requires a browser implementing `showDirectoryPicker()` (currently Chromium-
based browsers on localhost or a secure origin); browsers without that API can
still run the deterministic demo. The simulator does not upload or persist a
selected directory.

The canvas backing store stays at 600 by 800 while CSS may scale its display
size. Pointer coordinates are mapped through getBoundingClientRect() before
they enter Rust, so display scaling does not change hit testing. Rust owns the
reader's links, page turns, and history; JavaScript only translates browser
events and copies the rendered framebuffer.

## Output layout

target/reader-web/
├── index.html
├── directory-library.js
├── demo-library.js
├── demo/
│   ├── README.md
│   ├── guide/
│   └── assets/
├── main.js
├── input.mjs
├── style.css
└── pkg/
    ├── prs_reader_web.js
    └── prs_reader_web_bg.wasm

The generated target/ files are not committed. Follow-on simulator issues
should call tools/reader-web-build.sh build and use target/reader-web/ as
the static site root.
