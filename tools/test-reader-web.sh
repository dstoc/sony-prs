#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
output_dir=${1:-"$repo_root/target/reader-web"}

grep -Fq 'wasm-bindgen = "=0.2.128"' "$repo_root/web/reader-web/Cargo.toml"
grep -Fq 'js-sys = "=0.3.105"' "$repo_root/web/reader-web/Cargo.toml"
grep -Fq 'type="module" src="./main.js"' "$repo_root/web/reader-web/index.html"
grep -Fq 'ReaderSimulator' "$repo_root/web/reader-web/main.js"
grep -Fq 'BrowserReader' "$repo_root/web/reader-web/src/lib.rs"
grep -Fq 'pub fn render_frame(&mut self)' "$repo_root/web/reader-web/src/lib.rs"
grep -Fq 'canvas.width = logical_width();' "$repo_root/web/reader-web/main.js"
grep -Fq 'canvas.height = logical_height();' "$repo_root/web/reader-web/main.js"
grep -Fq 'new ImageData' "$repo_root/web/reader-web/main.js"
grep -Fq 'putImageData' "$repo_root/web/reader-web/main.js"
grep -Fq 'aspect-ratio: 3 / 4' "$repo_root/web/reader-web/style.css"
grep -Fq 'max-height: 100%;' "$repo_root/web/reader-web/style.css"
grep -Fq 'import init,' "$repo_root/web/reader-web/main.js"
grep -Fq 'ReaderSimulator,' "$repo_root/web/reader-web/main.js"
grep -Fq 'load_directory,' "$repo_root/web/reader-web/main.js"
grep -Fq 'proof_of_life,' "$repo_root/web/reader-web/main.js"
grep -Fq 'from "./pkg/prs_reader_web.js"' "$repo_root/web/reader-web/main.js"
grep -Fq 'showDirectoryPicker' "$repo_root/web/reader-web/directory-library.js"
grep -Fq 'load_directory(selected.files, path)' "$repo_root/web/reader-web/main.js"
grep -Fq 'const selectedReader = load_directory(selected.files, path);' "$repo_root/web/reader-web/main.js"
grep -Fq 'reader = selectedReader;' "$repo_root/web/reader-web/main.js"
grep -Fq 'render("Loaded " + selected.name);' "$repo_root/web/reader-web/main.js"
grep -Fq 'logicalPointFromPointer' "$repo_root/web/reader-web/main.js"
grep -Fq 'pointer_up(point.x, point.y)' "$repo_root/web/reader-web/main.js"
grep -Fq 'reader.pointer_up(point.x, point.y)' "$repo_root/web/reader-web/main.js"
grep -Fq 'data-command="previous"' "$repo_root/web/reader-web/index.html"
grep -Fq 'data-command="next"' "$repo_root/web/reader-web/index.html"
grep -Fq 'data-command="home"' "$repo_root/web/reader-web/index.html"
grep -Fq 'data-command="back"' "$repo_root/web/reader-web/index.html"
grep -Fq 'width="600"' "$repo_root/web/reader-web/index.html"
grep -Fq 'height="800"' "$repo_root/web/reader-web/index.html"
grep -Fq 'getBoundingClientRect' "$repo_root/web/reader-web/input.mjs"
grep -Fq 'wasm_bindgen_version=0.2.128' <("$repo_root/tools/reader-web-build.sh" print-github-actions)
test -x "$repo_root/tools/reader-web-serve.sh"
grep -Fq 'python3 -m http.server' "$repo_root/tools/reader-web-serve.sh"
grep -Fq 'tools/reader-web-serve.sh 8000' "$repo_root/web/reader-web/README.md"

if grep -Fq 'simulator.' "$repo_root/web/reader-web/main.js"; then
  printf '%s\n' 'main.js must route all actions through the active reader' >&2
  exit 1
fi

node "$repo_root/tools/test-reader-web-input.mjs"

test -f "$output_dir/index.html"
test -f "$output_dir/directory-library.js"
test -f "$output_dir/main.js"
test -f "$output_dir/input.mjs"
test -f "$output_dir/style.css"
test -s "$output_dir/pkg/prs_reader_web.js"
test -s "$output_dir/pkg/prs_reader_web_bg.wasm"
grep -Fq 'ReaderSimulator' "$output_dir/pkg/prs_reader_web.js"

printf '%s\n' 'reader web build contract passed'
