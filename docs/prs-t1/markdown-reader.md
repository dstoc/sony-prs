# PRS-T1 Markdown reader architecture

This document defines the reusable reader boundary for the PRS-T1 work and
for future host-side tools. It is the design reference for the follow-up
reader milestones; device-specific behavior belongs in the T1 agent guide and
the T1 analysis ledger instead.

## Components and dependency direction

The workspace dependency is deliberately one-way:

```text
prs-t1-agent
    |
    v
prs-markdown
    |
    +-- Markdown parser
    +-- typography and font metrics
    +-- syntax highlighting
    +-- image decoding
    +-- embedded-graphics display-target boundary
```

`crates/prs-markdown` is a library crate. It may be used by the T1 native
application, host-side tests, and later tools. It must never depend on
`prs-t1-agent`; the reusable crate must not acquire a reverse dependency on a
binary crate or on T1 hardware code.

The crate owns these concerns:

- parsing modern Markdown and GFM through a replaceable parser boundary;
- converting parser events into an owned document representation;
- proportional text measurement, typography, wrapping, and style selection;
- block layout for headings, lists, block quotes, tables, code, and images;
- pagination into discrete page layouts and display-list commands;
- semantic link hit regions and document/anchor target resolution;
- navigation between Markdown files and anchors, including reader history;
- rendering a page through a generic `embedded-graphics::DrawTarget`.

The crate does not own any of the following:

- Linux framebuffer mapping or framebuffer ioctls;
- Sony/i.MX EPDC ioctls, waveform selection, or refresh scheduling;
- framebuffer damage calculation or pixel-refresh policy;
- evdev input handling or PRS-T1 coordinate normalization;
- suspend/wake handling, Android lifecycle integration, or process handoff;
- battery, Wi-Fi, USB, power, or other device-status collection;
- the PRS-T1 status bar, settings UI, or other application chrome;
- assumptions about a particular display size, status-bar height, or panel.

The T1 application provides a viewport and a drawing target, turns physical
input into reader operations, chooses the document root/current document for a
resource provider, and decides how and when resulting pixel changes are
refreshed on the e-ink panel. It does not parse Markdown links or own their
path-resolution rules.

## Processing pipeline

```text
Markdown source
    -> owned document IR
    -> document layout
    -> pagination
    -> PageLayout / display list
    -> generic embedded-graphics renderer
```

Each stage has a stable handoff:

1. A parser consumes source text and returns owned `Document`, `Block`, and
   `Inline` values. Parser event lifetimes and parser-specific types stop at
   this boundary.
2. Layout receives the owned document, host-supplied style/font metrics, and
   a caller-supplied viewport. It produces positioned lines and fragments,
   retaining link semantics rather than converting links into pixel-only
   state.
3. Pagination turns the document layout into a `Pagination` of independent
   `PageLayout` values. A page contains display-list commands and semantic hit
   regions in its own viewport coordinates, so a caller can redraw or cache a
   page without knowing how the panel is refreshed.
4. The renderer accepts a page, a caller-owned `TextEngine`, and an arbitrary
   compatible `embedded-graphics` `DrawTarget`. It translates page coordinates
   to a caller-selected origin, clips to the translated viewport, and submits
   fills, borders, rules, and antialiased glyph coverage. `Rgb888` is the
   renderer's intermediate color type; `embedded-graphics` converts it to the
   target's pixel type, such as the T1 canvas's `Rgb565`. Image fragments are
   currently rendered as deterministic placeholders until an image decoder
   supplies image pixels.
5. The T1 agent compares or refreshes pixels using its existing framebuffer
   and EPDC policy. That final step is outside `prs-markdown`.

## Layout/display-list boundary

`geometry::Viewport` describes the width and height supplied by the reader
application. `geometry::Rect` is the shared, hardware-independent rectangle
type, and `geometry::translate` plus `Viewport::clip` provide the only
coordinate operations needed at this boundary. Page-space `(0, 0)` is always
the top-left of the reader viewport. A T1 application may translate a page to
the framebuffer origin later; no status-bar offset is stored in a page.

`pagination::PageLayout` is the renderer input. Its ordered `DisplayList`
contains positioned, non-semantic primitives:

- `Text` carries a text run, its bounds, common `TextStyle` metrics, and an
  e-ink grayscale ink value.
- `Fill` and `Border` express backgrounds and framed regions.
- `Rule` expresses horizontal or vertical rules as a stroked rectangle.
- `ImagePlaceholder` carries bounds, alternative text, and an optional source
  identifier until an image renderer is supplied.

`FillStyle`, `BorderStyle`, and `Color` are document presentation values, not
device UI styling. The display list is ordered back-to-front, so a later
command is drawn over an earlier one. `PageLayout::push_command` clips a
command to its viewport; renderers clip again after applying their target
origin. `render::EmbeddedGraphicsRenderer` owns no layout policy: it maps
reader text styles to the supplied typography face, rasterizes each positioned
glyph, and draws only the resulting coverage pixels. Because a generic
`DrawTarget` cannot read a background pixel, RGBA values and glyph coverage
are flattened against white before color conversion.

Semantic links are separate from the display list. Each `HitRegion` associates
one page-space rectangle with a `NavigationTarget`; a wrapped logical link is
represented by several entries with the same target. `PageLayout::hit_test`
does not inspect rendered pixels and searches regions in reverse insertion
order, making the topmost overlapping region win deterministically. Regions
are clipped when added through `PageLayout::add_hit_region`.

Layout may therefore produce any combination of text, fills, borders, rules,
and image placeholders for tables, quotes, or code blocks without teaching the
renderer about Markdown blocks. Conversely, renderer code depends only on
`PageLayout` and these generic styles and geometry types.

Fenced code is highlighted before layout by `highlighting::SyntectHighlighter`.
The build script serializes 16 selected language grammars plus plain text into
a 7,551-byte packdump in the current build; the runtime does not parse grammar
source files or load Syntect's unrestricted defaults. One Syntect state is kept
through all source lines in a block, including lines that later land on
different pages. `LayoutLine::wrapped` marks display continuations, and the
paginator uses a separate light-gray continuation fill while retaining normal
monospace text bounds and line-level page breaks.

## Deterministic pagination rules

`Paginator` scans the positioned layout once and returns a `Pagination`. Each
`PageLayout` contains a half-open `DocumentRange` of `DocumentCursor` values;
the cursor identifies a top-level block and a line within that block. The
canonical position between blocks is `(next_block, 0)`, and the document end is
`(block_count, 0)`. Page navigation can therefore use `page_for_cursor`,
`next_page`, and `previous_page` without retaining rendered pixels.

The paginator is a greedy line scanner with deterministic local lookahead; it
does not optimise a document globally. A line whose bottom exactly reaches the
usable page bottom fits. A page break is made before a line that would
overflow, and the first line on a new page is translated to the configured
top padding. The bottom padding is reserved when deciding whether a line fits.

The split policy is:

- paragraphs split only between layout lines. A short paragraph that fits a
  fresh page is moved as a unit when the current page cannot hold it; a
  two-line tail is kept together when this avoids a one-line orphan;
- headings use the same line boundaries, but a heading that would otherwise be
  the last content on a page is moved with the next line when the heading fits
  a fresh page;
- lists and block quotes may continue on another page at any line boundary.
  This permits an item or nested child to split when its lines cannot fit,
  while ordinary item boundaries remain natural layout-line boundaries;
- thematic rules are atomic and are moved to the next page when they do not
  fit. The current image placeholder is also kept atomic when it fits a fresh
  page. An atomic block taller than a viewport uses a deterministic clipped
  single-page fallback until a size-aware image layout is available;
- code and the current pipe-separated table placeholder split at layout-line
  boundaries. Future code/table/image layout should preserve this paginator
  boundary and add display-line, row, or image-fragment metadata without
  changing cursor semantics.

These rules operate on `DocumentLayout` lines and display-list commands only;
pagination never allocates or depends on full-page bitmaps or E-ink refresh
behaviour. Re-running pagination with the same layout and style produces the
same page count, ranges, and page-space commands.

## Current crate shape

The crate exposes the module boundaries for the pipeline:

| Module | Boundary |
| --- | --- |
| `document` | Owned semantic document IR. |
| `parse` | Replaceable Markdown parser trait and parse errors. |
| `resources` | Host-provided image/include/resource loading. |
| `style` | Caller-supplied style and font-independent metrics. |
| `geometry` | Viewport-relative rectangles, translation, and clipping. |
| `typography` | Fontdue-backed font loading, proportional measurement, styled wrapping, glyph positions, line metrics, and bounded raster caching. |
| `layout` | Viewport-relative blocks, lines, fragments, and link semantics. |
| `pagination` | Logical page ranges, pages, display-list commands, and semantic hit regions. |
| `navigation` | Document paths, anchors, link targets, and history. |
| `reader` | High-level reading session, page position, document loading, rendering, and navigation state. |
| `render` | Generic `DrawTarget` adapter with origin/viewport clipping, decoration primitives, and caller-supplied glyph rasterization. |

## High-level reader API

`reader::Reader` coordinates the complete host-side pipeline. It is generic over
the `ResourceProvider`, Markdown parser, and layout `TextMeasurer`, so a T1
application can use the filesystem provider today and replace it with a device
or archive-backed provider later:

```rust
let mut reader = Reader::new(provider, style, Viewport::new(600, 760));
reader.open_document("book/index.md")?;

let page = reader.current_page();
let page_count = reader.page_count();
reader.next_page()?;
reader.previous_page()?;
reader.render_current_page(&mut renderer, &mut draw_target)?;

match reader.activate_at(point)? {
    ReaderEvent::ExternalUrl(url) => hand_to_application(url),
    ReaderEvent::Navigated { .. } => refresh_page(),
    ReaderEvent::NoAction => {},
    _ => {}
}
```

`open_document` starts a new session and clears history. `follow_document`,
`follow_document_anchor`, `follow_reference`, and `navigate_to_anchor` resolve
through the provider and add an internal navigation entry. `back()` restores
the document, page, and canonical `DocumentCursor` saved when the link was
followed; `forward()` restores the corresponding forward entry. The public
`history()` slice exposes those cursor-aware `ReadingLocation` entries when an
application needs to persist or inspect them.

Page indices returned by `Reader` are zero-based. `PageLayout::number` remains
the one-based display number. `hit_test` and `activate_at` use page-space
coordinates, so the application can translate a physical input coordinate
before calling them. External URLs produce `ReaderEvent::ExternalUrl` and are
never opened by `prs-markdown`; the application chooses whether to hand them
to Android, a browser, a QR-code view, or another UI.

## Resource and path model

`ResourceProvider` is the device-independent storage boundary used by the
reader. It exposes four operations: read a UTF-8 text resource, read opaque
binary bytes, identify the current document path, and resolve a reference from
the current or an explicitly supplied containing document. A filesystem-backed
`FileSystemResourceProvider` is provided for host execution and the T1; a
different provider can use an archive or a device service without changing the
reader.

The filesystem provider is configured with a root directory and a current
document. Paths in the provider namespace are normalized, root-relative paths.
For a local relative reference, the containing document's directory is joined
first and then `.` and `..` are normalized. The configured root is a hard
boundary: a reference whose normalized path escapes it is rejected, and reads
also reject symlinks that lead outside the root. Missing paths can still be
resolved to a target, but reading one returns a resource error.

Resolution produces `ResourceTarget` values without navigation state:

- `Anchor` is a fragment-only reference such as `#installation`;
- `Document` is another Markdown document;
- `DocumentAnchor` is a Markdown document plus a fragment;
- `Asset` is a local non-Markdown resource such as `images/chart.png`;
- `External` is a URL/URI and is never opened by the filesystem provider.

Markdown documents are identified by a case-insensitive `.md` or `.markdown`
extension. Spaces and Unicode characters remain part of the path; references
are not interpreted relative to the process working directory. Resolving a
target does not open it or update history. The high-level reader chooses what
to do with a document, anchor, asset, or external URL after resolution.

The resource boundary remains intentionally extensible for future storage and
image-decoding implementations. Its integration points should extend these
boundaries instead of moving T1 hardware policy into the library.

Resource decoding, syntax highlighting, and pixel rasterization remain
follow-up work. The layout stage now handles the core reader structures: it
recursively lays out paragraphs, headings, inline emphasis/strong/
strikethrough/code, soft and hard breaks, ordered and unordered (including
task) lists, nested lists, block quotes, rules, links, and readable
placeholders for tables and images. All line widths come from the configured
`TextMeasurer`; a `FontdueTextEngine` therefore supplies real font metrics.

Layout returns the complete document in document coordinates. It exposes each
positioned line as a legal pagination split and never decides page boundaries.
Links are retained on each wrapped fragment so pagination can create one hit
region per visible line portion.

The deliberate visual deviations from browser/GitHub rendering are compact
reader choices: soft breaks collapse to ordinary whitespace, long unbreakable
words and URLs split at character boundaries, headings use one configured
style with a compact level-size reduction, tables are pipe-separated rows,
images are alt-text placeholders, and block quotes use a configured vertical
rule with e-reader indentation. These choices favor legibility and bounded
host/device layout over HTML/CSS compatibility.

## Parser and owned document IR

The parser stage is implemented by `parse::ComrakParser`. It enables the GFM
extensions used by agent output (tables, task lists, strikethrough, and
autolinks), then copies Comrak's arena-backed tree into the owned IR. The IR
retains fenced-code info strings, link/image destinations, task state, table
alignment, heading anchors, source text, and source spans for top-level blocks.
Comrak nodes, arenas, and their lifetimes stop at the parser module; layout and
later stages consume only `prs-markdown` types.

The production typography/font backend, syntax highlighting, image decoding,
and pixel rasterization remain follow-up work. Their integration points should
extend these boundaries instead of moving T1 hardware policy into the library.

## Typography boundary

`prs-markdown::typography` exposes the `TextEngine` trait and the backend-neutral
types used by it. `FontdueTextEngine` loads caller-supplied bytes through
`FontConfig` for regular, bold, italic, bold-italic, and monospace faces. A
harness can use one family for every face with `FontConfig::from_regular`, or
provide independent bytes with `FontConfig::from_faces`; the reader never
hard-codes a licensed font family.

`TextEngine::measure` returns proportional run metrics. `TextEngine::wrap`
returns line metrics and positioned glyphs, retaining an optional application
`SpanId` on each glyph for links or other semantic spans. `rasterize_glyph`
returns an 8-bit coverage bitmap. `FontdueTextEngine` also implements the
existing `layout::TextMeasurer` boundary, so a caller can pass it to
`LayoutEngine::with_measurer` and make document wrapping use the same font
advances.

Rasterized glyphs are held in an explicitly bounded least-recently-used cache;
`cache_capacity`, `cached_glyphs`, and the rasterization counters are exposed
for host instrumentation. The cache is bounded by entry count, and a capacity
of zero disables reuse.

The initial backend is deliberately scoped to Latin and code-heavy documents.
It does not yet perform complex-script shaping, bidirectional layout,
grapheme-aware cursoring, or broad fallback-font selection. A future shaping
engine can implement `TextEngine` without exposing Fontdue types to parsing,
pagination, or the device renderer.
