# Browser reader simulator

This directory contains the browser build for the PRS-T1 reader simulator. The
WASM module owns the reader, the shared renderer, and a tightly packed RGBA
framebuffer. JavaScript copies each rendered frame into an HTML canvas. The
browser uses the same parser, layout, pagination, navigation, image,
hit-testing, and rendering library as the native reader. The page has no
browser-specific reader layout implementation. It depends on the shared
`prs-markdown` core and does not add a JavaScript package manager or a
frontend framework.

Open directory uses the browser File System Access API in read-only mode.
`directory-library.js` recursively reads the selected directory into a
root-relative in-memory snapshot and passes the bytes to Rust. It selects the
root `README.md`, then root `index.md`, then the first root `.md` file in
lexicographic order. If the directory has no root Markdown file, the page
reports an error without constructing an ambiguous reader. The
`BrowserResourceProvider` implements the shared `ResourceProvider` boundary;
local Markdown links and image assets resolve relative to the containing
document, while absolute and escaping paths return structured reader errors.
Files are not uploaded or persisted by the simulator.

## One-shot cloud sync

The **Authorize & sync** button starts one reader authorization request against
the production PRSync API. Open the displayed approval link in its new tab and
complete the existing Cloudflare Access approval flow. The link contains only
the public request ID. The polling secret is sent separately in the poll API
body, and the reader bearer token is returned only by the successful claim
response; neither is rendered, logged, added to a URL, or stored in browser
storage. When approval completes, that click performs one manifest check and,
if needed, one bounded bundle download.

After authorization, **Sync now** performs one additional manifest check and
at most one bundle download per click. The page never schedules a sync or polls
for future inbox changes. Closing or reloading the page discards the reader
session and revision snapshot; authorize again after reload. A denied, expired,
or rejected session requires a new **Authorize & sync** click. An unchanged
revision leaves the active reader alone. An empty inbox successfully clears the
active reader. A new bundle replaces the active reader only after the shared
Rust bundle validator has checked archive structure, the manifest, all file
sizes, all SHA-256 values, and the entry document; a failed download or
validation keeps the active reader. The active-library label identifies the
local directory or cloud inbox currently being shown.

The browser sends API requests with `credentials: "omit"` and keeps the reader
session in page memory only. Its authorization and bearer requests use the
existing production endpoints. The Worker permits CORS only for the exact
`PRS_READER_WEB_ORIGIN` configured in `crates/prs-cloudflare/wrangler.toml`, on
the reader authorization, poll, manifest, and bundle routes. It allows only
the methods and request headers those calls need. It does not allow credential
cookies or caller-supplied Cloudflare identity headers. `/a/*` is not covered
by API CORS and remains protected by Cloudflare Access.

The checked-in production origin is `http://127.0.0.1:8000`, for the loopback
static server below. Keep the browser bound to that exact host and port. If the
static reader is hosted elsewhere, set `PRS_READER_WEB_ORIGIN` to that exact
origin and deploy the Worker configuration; do not use `*` or a reflected
request origin. The browser API base defaults to
`https://prs-reader.dstoc.workers.dev`; a host can override it with a
`<meta name="prsync-api-base" content="https://…">` value when it has a
different same-origin API deployment.

The build uses these pinned versions:

- Rust and Cargo 1.98.1.
- The wasm32-unknown-unknown Rust target.
- wasm-bindgen crate and wasm-bindgen-cli 0.2.128.
- Python 3 with the standard-library http.server for local serving.

The browser page uses plain HTML, CSS, and ES modules. It does not use npm,
Vite, Webpack, React, or another frontend build system.

## Required CI check

The required GitHub Actions check is `CI / Browser reader WASM build`. Configure
that exact check name in branch protection for pull requests and pushes to
`main`. The job starts from a clean checkout, runs the shared reader host and
WASM checks, builds the browser adapter with the pinned toolchain, assembles
the complete static site, and validates the generated files and deterministic
browser-side contracts.

The simulator uses the shared prs-markdown reader with the directory snapshot.
Rust owns hit testing, page turns, link navigation, and reader history.
`main.js` maps browser Pointer Events to the active portrait (600 by 800) or
landscape (800 by 600) surface, forwards control or keyboard commands to the
WASM adapter, and copies the Rust-rendered framebuffer.

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

Open <http://127.0.0.1:8000/> in a browser and select **Open directory** or
start **Authorize & sync**. Use
the canvas, Previous, Next, Home, Back, Switch to landscape/portrait, Reader
fullscreen, and Fullscreen controls. Reader fullscreen is session-scoped and
reflows the shared reader while preserving the current passage and navigation
history. Browser Fullscreen uses the Fullscreen API for the reader stage; its
canvas keeps the active logical aspect ratio and fits inside the available
display area. Use Exit fullscreen, Escape, or the browser's fullscreen controls
to return to the shell. The keyboard shortcuts are Left Arrow, Right Arrow,
Home, Backspace, and Alt+Left for Back. Use a static server because the browser
loads the generated ES module and WASM file through HTTP.

A lightweight contract check covers deterministic entry-point selection,
pointer mapping at 1×, ½×, and 1.5× CSS display scales, live portrait to
landscape canvas sizing and pointer mapping, fullscreen transitions, viewport
changes, and unsupported or denied Fullscreen API requests. The shared Rust
tests cover rendering, image loading, orientation reflow, page turns,
navigation history, Home, and external-link events. For manual browser
acceptance, select a Markdown library with **Open directory**, switch between
portrait and landscape, enter Browser Fullscreen, resize or rotate the viewport,
activate document and asset links, use Back and Home, turn pages, then exit with
the button or Escape.

The directory-picker workflow is read-only and keeps bytes in page memory. It
requires a browser implementing `showDirectoryPicker()` (currently Chromium-
based browsers on localhost or a secure origin). The simulator does not upload
or persist a selected directory.

The canvas backing store matches the active 600 by 800 portrait or 800 by 600
landscape surface while CSS scales its display size. Pointer coordinates use
the same active dimensions and getBoundingClientRect() before they enter Rust,
so orientation and display scaling both preserve hit testing. Rust owns the
reader's links, page turns, and history; JavaScript only translates browser
events and copies the rendered framebuffer.

## Output layout

target/reader-web/
├── index.html
├── directory-library.js
├── main.js
├── input.mjs
├── fullscreen.mjs
├── style.css
└── pkg/
    ├── prs_reader_web.js
    └── prs_reader_web_bg.wasm

The generated target/ files are not committed. Follow-on simulator issues
should call tools/reader-web-build.sh build and use target/reader-web/ as
the static site root.
