#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
output_dir=${1:-"$repo_root/target/reader-web"}

grep -Fq 'wasm-bindgen = "=0.2.128"' "$repo_root/web/reader-web/Cargo.toml"
grep -Fq 'js-sys = "=0.3.105"' "$repo_root/web/reader-web/Cargo.toml"
grep -Fq 'type="module" src="./main.js"' "$repo_root/web/reader-web/index.html"
if grep -Fq 'ReaderSimulator' "$repo_root/web/reader-web/main.js"; then
  printf '%s\n' 'main.js must use the directory-backed BrowserReader' >&2
  exit 1
fi
grep -Fq 'BrowserReader' "$repo_root/web/reader-web/src/lib.rs"
grep -Fq 'pub fn render_frame(&mut self)' "$repo_root/web/reader-web/src/lib.rs"
grep -Fq 'canvas.width = logical_width();' "$repo_root/web/reader-web/main.js"
grep -Fq 'canvas.height = logical_height();' "$repo_root/web/reader-web/main.js"
grep -Fq 'new ImageData' "$repo_root/web/reader-web/main.js"
grep -Fq 'putImageData' "$repo_root/web/reader-web/main.js"
grep -Fq 'aspect-ratio: 3 / 4' "$repo_root/web/reader-web/style.css"
grep -Fq 'max-height: 100%;' "$repo_root/web/reader-web/style.css"
grep -Fq 'import init,' "$repo_root/web/reader-web/main.js"
grep -Fq 'load_directory,' "$repo_root/web/reader-web/main.js"
grep -Fq 'proof_of_life,' "$repo_root/web/reader-web/main.js"
grep -Fq 'from "./pkg/prs_reader_web.js"' "$repo_root/web/reader-web/main.js"
grep -Fq 'loadDemoDirectory' "$repo_root/web/reader-web/main.js"
grep -Fq 'load_directory(demo.files, "README.md")' "$repo_root/web/reader-web/main.js"
grep -Fq 'id="load-demo"' "$repo_root/web/reader-web/index.html"
grep -Fq 'demo-library.js' "$repo_root/tools/reader-web-build.sh"
grep -Fq 'cp -R -- web/reader-web/demo' "$repo_root/tools/reader-web-build.sh"
grep -Fq 'DEMO_FILES' "$repo_root/web/reader-web/demo-library.js"
grep -Fq 'new URL(path, DEMO_ROOT)' "$repo_root/web/reader-web/demo-library.js"
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
grep -Fq 'File System Access API' "$repo_root/web/reader-web/README.md"
grep -Fq '1×, ½×, and 1.5×' "$repo_root/web/reader-web/README.md"
test -f "$repo_root/web/reader-web/demo/README.md"
test -f "$repo_root/web/reader-web/demo/guide/chapter.md"
test -f "$repo_root/web/reader-web/demo/guide/notes.md"
test -s "$repo_root/web/reader-web/demo/assets/observatory.png"
test -s "$repo_root/web/reader-web/demo/assets/detail.png"

if grep -Fq 'simulator.' "$repo_root/web/reader-web/main.js"; then
  printf '%s\n' 'main.js must route all actions through the active reader' >&2
  exit 1
fi

node "$repo_root/tools/test-reader-web-input.mjs"
node "$repo_root/tools/test-reader-web-demo.mjs"

test -f "$output_dir/index.html"
test -f "$output_dir/directory-library.js"
test -f "$output_dir/demo-library.js"
test -f "$output_dir/main.js"
test -f "$output_dir/input.mjs"
test -f "$output_dir/style.css"
test -f "$output_dir/demo/README.md"
test -f "$output_dir/demo/guide/chapter.md"
test -f "$output_dir/demo/guide/notes.md"
test -s "$output_dir/demo/assets/observatory.png"
test -s "$output_dir/demo/assets/detail.png"
test -s "$output_dir/pkg/prs_reader_web.js"
test -s "$output_dir/pkg/prs_reader_web_bg.wasm"
grep -Fq 'BrowserReader' "$output_dir/pkg/prs_reader_web.js"

printf '%s\n' 'reader web build contract passed'
