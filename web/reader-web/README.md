# Browser reader simulator

This directory contains the browser build for the PRS-T1 reader simulator. The
WASM module owns the reader, the shared renderer, and a tightly packed RGBA
framebuffer. JavaScript copies each rendered frame into an HTML `<canvas>`.
The browser uses the same parser, layout, pagination, navigation, image,
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
- The `wasm32-unknown-unknown` Rust target.
- `wasm-bindgen` crate and `wasm-bindgen-cli` 0.2.128.
- Python 3 with the standard-library `http.server` for local serving.

The browser page uses plain HTML, CSS, and an ES module. It does not use npm,
Vite, Webpack, React, or another frontend build system.

## Clean build

Install the Rust toolchain and target with rustup:

```sh
rustup toolchain install 1.98.1 \
  --profile minimal \
  --target wasm32-unknown-unknown \
  --no-self-update
```

Install the pinned binding generator with the same Rust toolchain:

```sh
cargo +1.98.1 install wasm-bindgen-cli --version 0.2.128 --locked
```

Build and assemble the page from the repository root:

```sh
RUSTUP_TOOLCHAIN=1.98.1 tools/reader-web-build.sh build
```

The script checks both tool versions. It writes the complete static page to
`target/reader-web/` and writes the intermediate Rust artifact to
`target/wasm32-unknown-unknown/release/prs_reader_web.wasm`.

## Local server

Serve the assembled page from its output directory:

```sh
python3 -m http.server 8000 --directory target/reader-web
```

Open <http://127.0.0.1:8000/> in a browser. The page renders the default
Markdown document through the shared Rust renderer. The canvas bitmap remains
600 × 800 logical pixels; CSS scales the canvas to fit the available window.
Use a static server because the browser loads the generated ES module and WASM
file through HTTP.

`ReaderSimulator::render_frame()` returns RGBA bytes in row-major order. The
JavaScript bridge creates `ImageData` from those bytes and calls
`CanvasRenderingContext2D.putImageData()` without changing the logical surface.

## Output layout

```text
target/reader-web/
├── index.html
├── directory-library.js
├── main.js
├── style.css
└── pkg/
    ├── prs_reader_web.js
    └── prs_reader_web_bg.wasm
```

The generated `target/` files are not committed. Follow-on simulator issues
should call `tools/reader-web-build.sh build` and use `target/reader-web/` as
the static site root.
