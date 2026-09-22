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

The simulator uses the shared prs-markdown reader with an in-memory demo
document. Rust owns hit testing, page turns, link navigation, and reader
history. main.js only maps browser Pointer Events to the logical 600 by 800
surface, forwards control or keyboard commands to the WASM adapter, and copies
the Rust-rendered framebuffer.

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

## Local server

Serve the assembled page from its output directory:

python3 -m http.server 8000 --directory target/reader-web

Open <http://127.0.0.1:8000/> in a browser. Use the canvas, Previous, Next,
Home, and Back controls. The keyboard shortcuts are Left Arrow, Right Arrow,
Home, Backspace, and Alt+Left for Back. Use a static server because the browser
loads the generated ES module and WASM file through HTTP.

The canvas backing store stays at 600 by 800 while CSS may scale its display
size. Pointer coordinates are mapped through getBoundingClientRect() before
they enter Rust, so display scaling does not change hit testing. Rust owns the
reader's links, page turns, and history; JavaScript only translates browser
events and copies the rendered framebuffer.

## Output layout

target/reader-web/
├── index.html
├── directory-library.js
├── main.js
├── input.mjs
├── style.css
└── pkg/
    ├── prs_reader_web.js
    └── prs_reader_web_bg.wasm

The generated target/ files are not committed. Follow-on simulator issues
should call tools/reader-web-build.sh build and use target/reader-web/ as
the static site root.
