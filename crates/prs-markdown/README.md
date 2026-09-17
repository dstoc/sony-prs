# `prs-markdown`

`prs-markdown` is the hardware-independent Markdown reader library used by
the PRS-T1 native application and the host preview harness. It turns a
root-relative Markdown document and caller-supplied typography into bounded,
viewport-relative page layouts, semantic link regions, and a generic
`embedded-graphics` display list. It contains no framebuffer, EPDC, evdev,
Android, or e-ink refresh policy.

The complete content contract, including fallbacks for specialist Markdown,
is the [Markdown support matrix](../../docs/prs-t1/markdown-reader.md#markdown-support-matrix).

## Pipeline and architecture

The production pipeline is:

```text
Markdown source
    -> ComrakParser -> owned Document IR
    -> LayoutEngine -> document-coordinate lines/fragments
    -> Paginator -> PageLayout/display list + hit regions
    -> EmbeddedGraphicsRenderer -> caller's DrawTarget
```

The parser copies Comrak's arena-backed tree into owned `Document`, `Block`,
and `Inline` values. Layout uses a caller-supplied `TextMeasurer`; the
production Fontdue backend supplies the same font metrics later used for glyph
rasterization. Pagination applies deterministic line, heading, table, rule,
and image split policies. The renderer translates and clips page coordinates
at a caller-selected origin and supports any compatible
`embedded_graphics::DrawTarget`.

`ResourceProvider` is the storage boundary. The supplied
`FileSystemResourceProvider` keeps paths root-relative, resolves references
from the containing document, rejects lexical and symlink escapes, and exposes
bounded binary reads for image loading. `ImageResources` supports PNG, JPEG,
and WebP, retaining only fitted 8-bit grayscale rasters. The high-level
`Reader` adds document loading, page movement, anchor/document navigation,
cursor-aware Back/Forward history, and bounded page/document caches.

The T1-specific adapter in `crates/prs-t1-agent/src/reader.rs` constructs this
pipeline with `ComrakParser`, `FontdueTextEngine`, `ReaderStyle::default()`,
and a filesystem provider. It maps whole-screen taps into page coordinates and
lets the surrounding T1 runtime own status UI, input, framebuffer damage, and
EPDC refresh planning.

## Public reader API

Most applications use `reader::Reader` with explicit components:

```rust
use prs_markdown::geometry::Viewport;
use prs_markdown::parse::ComrakParser;
use prs_markdown::reader::Reader;
use prs_markdown::resources::FileSystemResourceProvider;
use prs_markdown::style::ReaderStyle;
use prs_markdown::typography::{FontConfig, FontdueTextEngine};
use embedded_graphics::geometry::Point;

let provider = FileSystemResourceProvider::new("book", "index.md")?;
let regular = std::fs::read("DejaVuSans.ttf")?;
let fonts = FontConfig::from_regular(regular);
let engine = FontdueTextEngine::new(fonts, 256)?;
let mut reader = Reader::with_components(
    provider,
    ComrakParser::default(),
    engine,
    ReaderStyle::default(),
    Viewport::new(600, 708),
);
let point = Point::new(20, 20);

reader.open()?;
reader.next_page_event()?;
let page = reader.current_page();
let event = reader.activate_at(point)?;
reader.back()?;
```

`Reader::with_components` uses the bounded production policy. Use
`Reader::with_limits` to set `ReaderLimits` explicitly and inspect
`ReaderCacheStats`. `Reader::new` remains an eager-pagination compatibility
constructor for callers that need a complete `Pagination`; new device and
host integrations should use the component constructor.

`FontConfig::from_regular` supplies one family for all faces. Keep
`FontConfig::from_faces` for a separate proportional family and one regular
monospace fallback. Use `FontConfig::from_faces_with_monospace` when the
monospace regular, bold, italic, and bold-italic files are available.

The important operations are:

- `open` or `open_document` starts a reading session;
- `current_page`, `page_count`, `next_page_event`, and `previous_page_event`
  expose page state and movement;
- `render_current_page` submits the current page through a caller-owned
  `EmbeddedGraphicsRenderer` and draw target;
- `hit_test` and `activate_at` use page-space coordinates and return
  `ReaderEvent::Navigated`, `Back`, `Forward`, `ExternalUrl`, `Asset`, or
  `NoAction` as appropriate;
- `follow_reference`, `navigate_to_anchor`, `back`, and `forward` implement
  root-relative document/anchor navigation and cursor-aware history;
- `document`, `layout`, `pagination_index`, `history`, and `cache_stats` are
  available for inspection and instrumentation.

External URLs are events, not side effects: the library never launches a
browser or chooses a device handler. Page indices are zero-based; the page
display number stored on `PageLayout` is one-based.

## Host harness and preview

The host harness runs the same parser/layout/pagination/renderer path as the
T1 integration. It can render a checked-in fixture or a Markdown file into
inspectable 8-bit grayscale PGM and PNG files:

```sh
cargo run -p prs-markdown --bin prs-markdown-harness -- \
  --fixture regression \
  --font /usr/share/fonts/truetype/dejavu/DejaVuSans.ttf \
  --output target/reader-pages

cargo run -p prs-markdown --bin prs-markdown-harness -- \
  path/to/document.md --page 1 --page 3 --output target/reader-pages
```

With no `--page`, all pages are rendered. Page selections are 1-based and
produce names such as `page-001.pgm` and `page-001.png`. The CLI defaults to
the public `T1_VIEWPORT` (600x800) and `ReaderStyle::default()`; use `--help`
for viewport, style, and glyph-cache overrides. `--font` or
`PRS_MARKDOWN_FONT` selects the font used for both layout metrics and
rasterization.

The CLI output is a host preview, not a T1 deployment artifact. To build the
device binary, follow the [T1 build guide](../prs-t1-agent/build.md) and use
the ARMv5TE/musl cross-build.

## Content model

The default Comrak parser handles CommonMark plus GFM tables, task lists,
strikethrough, autolinks, footnotes, inline footnotes, and GitHub alerts.
The owned IR and layout cover headings, paragraphs, emphasis/strong/code,
lists, nested lists, quotes, links, anchors, tables, fenced code, rules,
footnotes, alerts, and images. Supported local image formats are PNG, JPEG,
and WebP; missing, corrupt, external, unsupported, or over-budget images use
visible alt-text fallback.

GFM task lists render fixed-size, high-contrast checkbox controls before their
task text. Checked controls use a filled box with a white check mark; unchecked
controls use an outlined box. The controls are visual only and do not toggle.

Fenced code uses Syntect 5.3.0's bundled upstream syntax definitions with the
Oniguruma runtime backend. The set contains 75 definitions and embeds the
368,467-byte `default_newlines.packdump` payload. The application keeps its
fence aliases: TypeScript aliases use Syntect's JavaScript definition, and
TOML aliases use its YAML definition because those two definitions are not in
Syntect's default set. Shell/bash, Rust, Python, JavaScript, TypeScript, JSON,
YAML, TOML, C, C++, Go, HTML, CSS, SQL, diff/patch, and Markdown remain
recognized. Unknown languages remain lossless plain monospace.
Mermaid and other diagram fences therefore show their source. Raw HTML shows
its source and is never executed. Dollar math, when enabled by caller-supplied
Comrak options, preserves delimiters as text; there is no equation or browser
layout engine. See the support matrix for the exact fallback contract.

Syntax-highlighted code selects `Monospace`, `MonospaceBold`,
`MonospaceItalic`, or `MonospaceBoldItalic` from the code span's normal, bold,
italic, and bold-italic flags. The selected face supplies both the metrics and
the rasterized glyph. The renderer performs one draw pass per glyph, so real
bold and italic faces do not smear or close counters with synthetic transforms.

## Tests, fixtures, and goldens

Run the focused library tests while changing the reader:

```sh
cargo test -p prs-markdown
cargo test -p prs-markdown --test harness
```

The fixtures under `tests/fixtures/` cover agent Markdown, headings, inline
styles, nested/task lists, quotes, alerts, footnotes, autolinks,
strikethrough, links and anchors, cross-file navigation, tables, fenced
syntax, raw HTML, images, malformed input, and font-face selection. The
typography tests load four independent monospace faces and verify their
metrics and raster output. The image fixture derives deterministic PNG, JPEG,
and WebP inputs from
`tests/fixtures/assets/observatory.png` and checks readable image goldens plus
missing/corrupt fallbacks.

Structural tests use the deterministic approximate measurer and do not require
font files. PNG corpus goldens use the test-only Noto Sans faces, including
bold/italic variants, at the T1 600x800 viewport. The separate `host-page.png`
is a small 120x80 renderer smoke golden. Every page is compared against a
checked-in `tests/goldens/*.png`; an inspected mismatch is written under
`target/prs-markdown-golden-failures/`.

After intentionally changing layout or rendering, inspect the generated pages
and then promote them explicitly:

```sh
PRS_MARKDOWN_UPDATE_GOLDENS=1 \
  cargo test -p prs-markdown --test harness
```

For the repository-level verification used by the project:

```sh
cargo test --workspace
cargo build --workspace --release
```

The workspace release build is host-only. It verifies the crates but does not
produce an ARM executable for the T1.
