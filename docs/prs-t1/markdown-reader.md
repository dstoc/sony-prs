# PRS-T1 Markdown reader architecture

This is the implementation guide for the reusable Markdown reader used by the
PRS-T1 native application and the host harness. It describes the boundaries
that exist in `prs-markdown`, the current content contract, and the policies
that the T1 adapter supplies. Device observations and historical experiments
belong in the [T1 analysis ledger](analysis.md); device operation belongs in
the [T1 agent guide](../../crates/prs-t1-agent/README.md).

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
   target's pixel type, such as the T1 canvas's `Rgb565`. Loaded image
   fragments carry bounded 8-bit grayscale rasters; the renderer maps those
   pixels through the same generic draw-target boundary.
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

- `Text` carries a text run, its bounds, common `TextStyle` metrics, explicit
  underline and strikethrough decorations, and an e-ink grayscale ink value.
- `Fill` and `Border` express backgrounds and framed regions.
- `Rule` expresses horizontal or vertical rules as a stroked rectangle.
- `Image` carries a bounded grayscale raster, bounds, alternative text, and
  its source identifier. `ImagePlaceholder` remains the visible fallback for
  callers that construct a display list directly without decoded image data.

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
and loaded images or alt-text fallbacks for Markdown blocks without teaching the
renderer about Markdown blocks. Conversely, renderer code depends only on
`PageLayout` and these generic styles, geometry, and raster types.

Fenced code is highlighted before layout by `highlighting::SyntectHighlighter`.
The runtime loads Syntect 5.3.0's bundled upstream definitions through
`SyntaxSet::load_defaults_newlines()`. The set contains 75 definitions and the
embedded `default_newlines.packdump` payload is 368,467 bytes in the 5.3.0
crate. The runtime uses Oniguruma; it does not enable `regex-fancy`. One
Syntect state is kept through all source lines in a block, including lines that
later land on different pages. `LayoutLine::wrapped` marks display
continuations, and the paginator uses a separate light-gray continuation fill
while retaining normal monospace text bounds and line-level page breaks.

Application fence normalization remains in place. Syntect 5.3.0 has no separate
TypeScript or TOML default definition, so TypeScript aliases use JavaScript and
TOML aliases use YAML. Both mappings select real upstream definitions and keep
the existing visible, lossless behavior.

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
- thematic rules and standalone images are atomic and are moved to the next
  page when they do not fit. Images are fitted to the content width and the
  available page height before pagination, so an oversized source has a
  deterministic, display-sized fallback rather than a retry/page-break loop;
- code splits at layout-line boundaries. Tables split only before a complete
  row when the row fits a fresh page; every continuation page synthesizes the
  current group's header above the row without adding a second logical cursor
  range. A row taller than one page is an explicit fallback: it is placed once
  and then split at its layout-line boundaries, with the header repeated on
  each continuation page.

These rules operate on `DocumentLayout` lines and display-list commands only;
pagination never allocates or depends on full-page bitmaps or E-ink refresh
behaviour. Re-running pagination with the same layout and style produces the
same page count, ranges, and page-space commands.

## T1 resource policy and host baseline

The production `Reader::with_components` path uses an explicit bounded policy
(`ReaderLimits::default()`): at most 16 history entries, two inactive
open-document states, three resident page display lists, 4 MiB of fitted image
rasters across 128 image references, and 16 MiB per encoded image source. The
filesystem provider checks the encoded size before reading it; the image
decoder additionally limits transient decoded allocation to 64 MiB. Failed,
unsupported, or over-budget images remain visible through their alt-text
fallback. `ImageResources` retains no decoded source image.

`FontdueTextEngine` keeps an LRU glyph cache bounded both by entry count and by
256 KiB of coverage bytes. The selected Syntect runtime bundle is the static
368,467-byte upstream `default_newlines.packdump` payload in Syntect 5.3.0,
and its 75-definition grammar set is initialized once. Highlighting is
performed once per code block while layout is built; it is not repeated on page
turns.

The bounded reader retains the complete owned document and document-coordinate
layout needed to rebuild pages, plus a compact page directory of logical ranges
and origins. It does not retain a display list for every page. An evicted page
is rebuilt from that layout on demand, so ordinary next/previous turns do not
reparse Markdown. The document cache is LRU-like and the history cap drops the
oldest entries when the limit is reached; a cache miss reparses and relayouts
only the requested document. Full-page bitmap caching is intentionally absent;
the T1 owns its framebuffer and refresh policy.

The host regression `crates/prs-markdown/tests/performance.rs` runs a checked-in
stress corpus containing prose, headings, nested lists, highlighted Rust/JSON,
a wide table, repeated images, and links to `stress-linked.md`. One observed
Linux host run (debug test binary, 600x800 viewport) measured 29 KiB source,
312 blocks, 48 pages, parse 6.0 ms, image preparation 558.6 ms, layout 13.4
ms, page-directory construction 2.0 ms, open/time-to-first-page 568.8 ms, ten
next-page turns 0.465 ms total, and previous-page 0.0006 ms. The process RSS
sample was 35 MiB. These are regression observations, not T1 guarantees;
on-device ARMv5TE measurements remain authoritative, especially for image
decode and first-page latency.

`Reader::new` remains an eager-pagination compatibility constructor for host
callers that inspect every `Pagination` page. T1 integration and new callers
use `with_components` or `with_limits`, which expose `cache_stats()` for host
instrumentation and apply the bounded policy.

## Current crate shape

The crate exposes the module boundaries for the pipeline:

| Module | Boundary |
| --- | --- |
| `document` | Owned semantic document IR. |
| `parse` | Replaceable Markdown parser trait and parse errors. |
| `resources` | Host-provided image/include/resource loading. |
| `image` | Bounded PNG/JPEG/WebP decoding, grayscale conversion, and display-sized image retention. |
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

The resource boundary permits alternate storage providers. The current
`ImageResources` implementation resolves Markdown image references from the
containing document through `ResourceProvider`, supports PNG, JPEG, and WebP,
reads dimensions before decoding, and converts the result to opaque 8-bit
grayscale. The decoder enforces a transient source allocation limit and
retained display rasters have a bounded byte budget; the decoded source is not
retained. Alpha is composited against white. A device or archive provider can
use the same interface without introducing T1 framebuffer policy into parsing
or layout.

Missing, unsupported, corrupt, external, or over-budget images do not abort a
document. Their alt text is laid out as visible `[image: …]` fallback text (or
`[image unavailable]` when no alt text exists). Standalone images are atomic
pagination units; inline images participate in their containing line, and all
loaded imagery is scaled proportionally to the content width and available
page area.

The layout stage now handles the core reader structures: it
recursively lays out paragraphs, headings, inline emphasis/strong/
strikethrough/code, soft and hard breaks, ordered and unordered (including
task) lists, nested lists, block quotes, rules, links, and readable
images and fully styled table cells. All line widths come from the configured
`TextMeasurer`; a `FontdueTextEngine` therefore supplies real font metrics.

Layout returns the complete document in document coordinates. It exposes each
positioned line as a legal pagination split and never decides page boundaries.
Links are retained on each wrapped fragment so pagination can create one hit
region per visible line portion. Linked text also receives the explicit thin
underline decoration, while ordinary text remains unchanged.

The deliberate visual deviations from browser/GitHub rendering are compact
reader choices: soft breaks collapse to ordinary whitespace, long unbreakable
words and URLs split at character boundaries, headings use one configured
style with a compact level-size reduction, images use decoded grayscale rasters
when available and alt-text fallbacks otherwise,
and block quotes use a configured vertical rule with e-reader indentation.
Tight lists use `ReaderStyle::tight_list_item_spacing`, which defaults to zero
page-space units. Loose lists use `ReaderStyle::list_item_spacing`, which
defaults to four page-space units between item blocks. Additional paragraph
blocks inside loose items use the normal `paragraph_spacing` value, which
defaults to twelve page-space units. Each nested list applies its own tight or
loose policy, and task-list items use the same spacing as their parent list.
Tables use a deterministic sizing pass: short tokens retain useful minimum
widths, while oversized tokens use the same character-wrap fallback as cell
layout. Preferred widths come from normally wrapped cell content. The
available width is allocated in
source-column order, with GFM left/center/right alignment, 3 page-space units
of horizontal and row-edge vertical cell padding by default, borders, and a
bold/heavier-rule header treatment. Normal body typography is retained for
ordinary tables when their columns fit. Compact and grouped fallback modes use
a minimum 12-pixel font and 16-pixel line height. Wrapped rows use continuous
vertical edges and logical top/bottom rules, which keeps glyphs away from
internal rules. If the frame still cannot hold all columns, the table is
continued vertically in deterministic column groups; the first key column is
repeated in each group where the viewport can hold it. Wider diagnostic tables
use the same sizing policy: compact and aggressive modes are used only when
the normal pass cannot fit readable column widths. These choices favor
legibility and bounded host/device layout over HTML/CSS compatibility.

## Parser and owned document IR

The parser stage is implemented by `parse::ComrakParser`. It enables the GFM
extensions used by agent output (tables, task lists, strikethrough, autolinks,
footnotes, inline footnotes, and GitHub-style alerts), then copies Comrak's
arena-backed tree into the owned IR. The IR retains list tightness, fenced-code
info strings, link/image destinations, task state, footnote definitions and
references,
alert titles and kinds, table alignment, heading anchors, source text, and
source spans for top-level blocks. Comrak nodes, arenas, and their lifetimes
stop at the parser module; layout and later stages consume only
`prs-markdown` types.

## Markdown support matrix

The reader targets readable, bounded output rather than browser compatibility.
The default `ComrakParser` enables the GFM extensions listed below; callers
that use `ComrakParser::with_options` can change the parser extensions, but the
owned IR and layout still apply the same reader-oriented fallbacks.

| Construct | Implemented behaviour and fallback |
| --- | --- |
| CommonMark basics | Paragraphs, escaped/literal text, soft and hard breaks, inline code, links, images, and thematic rules are parsed into owned IR and laid out as readable page content. Soft breaks collapse to ordinary whitespace; hard breaks remain line boundaries. |
| Headings, lists, and quotes | ATX/setext headings (levels 1--6), ordered/unordered lists, nested list children, and block quotes are laid out with reader spacing and indentation. Tight and loose lists retain distinct item spacing, including for nested and task lists. Heading text receives deterministic slug anchors; long lists and quotes split at layout-line boundaries. |
| Emphasis and strikethrough | Emphasis and strong text select the configured italic/bold faces. GFM strikethrough remains visible and adds a strike decoration; it does not remove or hide the content. |
| GFM task lists | Checked and unchecked items render a fixed-size, high-contrast visual checkbox before readable task text. Checked items use a filled box with a white check mark; unchecked items use an outlined box. There is no task toggle action. |
| Autolinks | URL, `www`, and email autolinks become semantic links. Local targets navigate through the reader; external targets are returned to the host as `ReaderEvent::ExternalUrl`. |
| Internal links and anchors | Fragment links (`#anchor`), root-relative `.md`/`.markdown` documents, and document-plus-anchor references are resolved relative to the containing document and root boundary. Back restores the prior document, page, and logical cursor. Missing documents or anchors return a reader error. |
| GFM tables | Headers, body rows, left/center/right alignment, readable normal/compact/aggressive font fallback, horizontal and row-edge vertical cell padding, regular line spacing within wrapped cells, continuous wrapped-row rules, repeated continuation headers, and deterministic vertical column groups are implemented. Rows normally paginate atomically; an oversized row splits at its displayed lines. |
| Fenced code and syntax | Fenced source is preserved, highlighted in one stateful pass, and paginated at displayed-line boundaries. Syntect's upstream definitions cover shell/bash, Rust, Python, JavaScript, JSON, YAML, C, C++, Go, HTML, CSS, SQL, diff/patch, and Markdown. TypeScript aliases use JavaScript, and TOML aliases use YAML because Syntect 5.3.0 does not bundle separate definitions for them. Unknown or absent languages remain lossless plain monospace. |
| Images | Local PNG, JPEG, and WebP references are decoded, alpha-composited onto white, proportionally fitted to the content/page bounds, converted to bounded grayscale, and rendered as display-list rasters. Standalone images are atomic pagination units; inline images participate in their line. |
| Missing or unsupported images | Missing, external, corrupt, over-budget, and unsupported image formats do not abort the document. The image's alt text is rendered as `[image: ...]`, or `[image unavailable]` when no alt text exists. Encoded reads, decoder allocation, retained bytes, and image-entry count are bounded. |
| Footnotes | GFM footnote and inline-footnote references render as `[1]` (or `[^name]` when no number is supplied). Definitions render as ordinary splittable `[^name]: ...` content. There is no browser-style back-link action. |
| GitHub alerts | `NOTE`, `TIP`, `IMPORTANT`, `WARNING`, and `CAUTION` alerts retain their kind, custom/default title, and body. Layout renders a bold title and quote-style body; alerts do not add a separate interactive panel. |
| Raw HTML | Inline and block HTML is never executed, interpreted, or styled. Its source is rendered as ordinary readable text, so HTML tags cannot hide following Markdown. |
| Mermaid and diagram DSLs | Mermaid and other specialist fences are treated as fenced code. Their literal source is readable in monospace; there is no JavaScript, SVG, or diagram renderer. |
| Math | A `math` fenced block is ordinary fenced source. When a caller enables Comrak's dollar-math extension, inline/display expressions retain their `$...$` or `$$...$$` delimiters and render as text. There is no TeX, MathML, or equation layout engine. |
| Directives and other unsupported content | An enabled Comrak block directive becomes a labelled `[unsupported block directive: ...]` marker plus converted child content inside the quote primitive. Other parser nodes are converted to child content where possible; no HTML/CSS/browser layout engine or arbitrary embedded-content execution is present. |

Unsupported specialist content is therefore visible rather than silently
discarded. Every fallback uses the same owned blocks, lines, hit regions, and
pagination boundaries as ordinary content.

The production typography/font backend and pixel rasterization remain
independent concerns. New implementations extend these boundaries instead of
moving T1 hardware policy into the library.

## Typography boundary

`prs-markdown::typography` exposes the `TextEngine` trait and the backend-neutral
types used by it. `FontdueTextEngine` loads caller-supplied bytes through
`FontConfig` for regular, bold, italic, bold-italic, monospace,
monospace-bold, monospace-italic, and monospace-bold-italic faces. A harness
can use one family for every face with `FontConfig::from_regular`, use
`FontConfig::from_faces` for proportional faces plus one regular monospace
fallback, or provide a complete set with
`FontConfig::from_faces_with_monospace`; the reader never hard-codes a licensed
font family.

`TextEngine::measure` returns proportional run metrics. `TextEngine::wrap`
returns line metrics and positioned glyphs, retaining an optional application
`SpanId` on each glyph for links or other semantic spans. `rasterize_glyph`
returns an 8-bit coverage bitmap. `FontdueTextEngine` also implements the
existing `layout::TextMeasurer` boundary, so a caller can pass it to
`LayoutEngine::with_measurer` and make document wrapping use the same font
advances.

Fenced code selects a face from the complete monospace family. Normal, bold,
italic, and bold-italic code select `Monospace`, `MonospaceBold`,
`MonospaceItalic`, and `MonospaceBoldItalic`. The selected face supplies the
metrics and glyph raster. The renderer draws each glyph once and does not
apply synthetic bold or italic transforms, so code keeps the configured
monospace metrics while highlighted styles retain their intended shape.

Rasterized glyphs are held in an explicitly bounded least-recently-used cache;
`cache_capacity`, `cached_glyphs`, and the rasterization counters are exposed
for host instrumentation. The cache is bounded by entry count, and a capacity
of zero disables reuse.

The initial backend is deliberately scoped to Latin and code-heavy documents.
The current backend does not perform complex-script shaping, bidirectional
layout, grapheme-aware cursoring, or broad fallback-font selection. An alternate
shaping backend can implement `TextEngine` without exposing Fontdue types to
parsing, pagination, or the device renderer.
