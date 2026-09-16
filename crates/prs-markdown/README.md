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
nested lists, task lists, quotes, long paragraphs, links and anchors,
cross-file links, exact page boundaries, agent responses, readable GFM tables,
fenced code, image references, and explicit font-face selection. `tests/harness.rs`
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
