# `prs-markdown`

Hardware-independent Markdown reader primitives for the Sony PRS work. The
crate's host harness runs the production pipeline:

```text
Markdown source -> ComrakParser -> LayoutEngine -> Paginator
               -> PageLayout -> EmbeddedGraphicsRenderer -> PGM + PNG
```

## Host rendering harness

Use the checked-in corpus to render every page into host-inspectable grayscale
PGM and PNG images:

```sh
cargo run -p prs-markdown --bin prs-markdown-harness -- \
  --fixture regression \
  --font /usr/share/fonts/truetype/dejavu/DejaVuSans.ttf \
  --output target/reader-pages
```

The harness also accepts a Markdown file directly:

```sh
cargo run -p prs-markdown --bin prs-markdown-harness -- \
  path/to/document.md --page 1 --page 3 --output target/reader-pages
```

With no `--page`, all pages are rendered. Selected page numbers are 1-based;
the output directory contains matching names such as `page-001.pgm` and
`page-001.png`. The default font search checks `PRS_MARKDOWN_FONT` and common
host font locations. Pass `--font` for a reproducible font choice. Width,
height, padding, body, heading, and code metrics are configurable with the
corresponding options shown by `--help`.

The fixture corpus in `tests/fixtures/` covers prose, headings, inline styles,
nested lists, task lists, quotes, alerts, footnotes, autolinks, strikethrough,
long paragraphs, links and anchors, cross-file links, exact page boundaries,
agent responses, readable GFM tables, fenced specialist source, raw HTML
fallbacks, image references, and explicit font-face selection. `tests/harness.rs`
asserts structural outputs such as page count, logical cursor ranges, visible
fragments, hit regions, navigation, and PGM encoding. Those structural tests use the
deterministic approximate measurer and remain independent of font files. The
checked-in corpus PNG goldens use the production Fontdue pipeline with the
Noto Sans faces supplied by the test-only `notosans` crate, so they are readable,
deterministic across CI hosts, and sensitive to glyph geometry and
bold/italic selection. The crate does not publish a monospace face, so its
regular face fills the golden harness's monospace slot while code styling and
face selection remain covered by the structural and Fontdue tests. The
command-line harness uses the supplied Fontdue font for both layout metrics and
rasterization.
The CLI writes both formats from the same rendered grayscale pixels, so PNGs
can be opened directly while PGM remains convenient for simple tooling.

## Embedded raster images

When the input is a file, the harness configures a filesystem resource provider
for its containing directory. Markdown image references are resolved relative
to that file. PNG, JPEG, and WebP images are fitted proportionally to the
content width and available page area, converted to bounded grayscale rasters,
and rendered through the normal display list. Standalone images are atomic
pagination units. Missing, unsupported, corrupt, external, or over-budget
images remain visible through their alt text (or an unavailable-image label)
and do not abort the run.

The complete Markdown decision record, including deterministic fallbacks for raw
HTML, Mermaid/diagram source, and optional math/directive extensions, is in the
[Markdown support matrix](../../docs/prs-t1/markdown-reader.md#markdown-support-matrix).

## Fenced code highlighting

Fenced code is highlighted by the isolated `highlighting` component. Syntect
is used with a build-generated packdump containing 17 deliberately selected
small grammars: shell/bash, Rust, Python, JavaScript, TypeScript, JSON, YAML,
TOML, C, C++, Go, HTML, CSS, SQL, diff/patch, Markdown, and plain text. The
packdump is currently 7,551 bytes in this build (the build script reports its
exact size), compared with loading Syntect's unrestricted default package.
The runtime enables only parsing, fancy-regex, and dump loading; grammar source
files are not parsed on the reader.

The theme maps token colors into four grayscale ink levels and uses bold or
italic where useful. Unknown or absent language tags are lossless plain
monospace. Source lines are highlighted in one stateful pass before layout,
so multiline strings/comments continue across wrapped display lines and page
boundaries. Long source lines wrap at character boundaries when necessary;
continuation lines receive a slightly darker code fill to distinguish them
from source-newline lines. Code blocks paginate at displayed line boundaries.

The corpus regression test compares every page of every checked-in fixture
against a deterministic PNG golden in `tests/goldens/`. If a comparison fails,
the rendered page is written to `target/prs-markdown-golden-failures/` and the
test output includes the command to promote all current renders after they
have been inspected:

```sh
PRS_MARKDOWN_UPDATE_GOLDENS=1 \
  cargo test -p prs-markdown --test harness
```

Run the focused harness tests with:

```sh
cargo test -p prs-markdown --test harness
```
