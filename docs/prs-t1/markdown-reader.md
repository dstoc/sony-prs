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
3. Pagination turns the document layout into independent `PageLayout` values.
   A page contains display-list commands and semantic hit regions in its own
   viewport coordinates, so a caller can redraw or cache a page without
   knowing how the panel is refreshed.
4. The renderer accepts a page and an arbitrary `embedded-graphics`
   `DrawTarget`. Font rasterization, image decoding, and actual pixel commands
   can be supplied in later milestones without changing device ownership.
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

- `Text` carries a text run, its bounds, and common `TextStyle` metrics.
- `Fill` and `Border` express backgrounds and framed regions.
- `Rule` expresses horizontal or vertical rules as a stroked rectangle.
- `ImagePlaceholder` carries bounds, alternative text, and an optional source
  identifier until an image renderer is supplied.

`FillStyle`, `BorderStyle`, and `Color` are document presentation values, not
device UI styling. The display list is ordered back-to-front, so a later
command is drawn over an earlier one. `PageLayout::push_command` clips a
command to its viewport; renderers may clip again after applying their target
origin.

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

## Current crate shape

The initial crate exposes skeletal module boundaries for the pipeline:

| Module | Boundary |
| --- | --- |
| `document` | Owned semantic document IR. |
| `parse` | Replaceable Markdown parser trait and parse errors. |
| `resources` | Host-provided image/include/resource loading. |
| `style` | Caller-supplied style and font-independent metrics. |
| `geometry` | Viewport-relative rectangles, translation, and clipping. |
| `layout` | Viewport-relative blocks, lines, fragments, and link semantics. |
| `pagination` | Pages, display-list commands, and semantic hit regions. |
| `navigation` | Document paths, anchors, link targets, and history. |
| `reader` | Page position and navigation state. |
| `render` | Generic `DrawTarget` adapter. |

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

The resource, layout, pagination, and rendering boundaries remain intentionally
skeletal where their production implementations are follow-up work. Their
integration points should extend these boundaries instead of moving T1 hardware
policy into the library.

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
