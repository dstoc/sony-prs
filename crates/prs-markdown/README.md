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
cross-file links, exact page boundaries, agent responses, table placeholders,
fenced code, and image references. `tests/harness.rs` asserts structural
outputs such as page count, logical cursor ranges, visible fragments, hit
regions, navigation, PGM encoding, and the checked-in
`tests/goldens/host-page.png` visual golden. These structural tests use the
deterministic approximate measurer; the golden uses a deterministic test glyph
backend so CI does not depend on a system font. The command-line harness uses
the supplied Fontdue font for both layout metrics and rasterization.
The CLI writes both formats from the same rendered grayscale pixels, so PNGs
can be opened directly while PGM remains convenient for simple tooling.

Run the focused harness tests with:

```sh
cargo test -p prs-markdown --test harness
```
