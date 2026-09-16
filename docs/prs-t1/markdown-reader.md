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
input into reader operations, resolves application/resource paths, and decides
how and when resulting pixel changes are refreshed on the e-ink panel.

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

## Current crate shape

The initial crate exposes skeletal module boundaries for the pipeline:

| Module | Boundary |
| --- | --- |
| `document` | Owned semantic document IR. |
| `parse` | Replaceable Markdown parser trait and parse errors. |
| `resources` | Host-provided image/include/resource loading. |
| `style` | Caller-supplied style and font-independent metrics. |
| `typography` | Fontdue-backed font loading, proportional measurement, styled wrapping, glyph positions, line metrics, and bounded raster caching. |
| `layout` | Viewport-relative blocks, lines, fragments, and link semantics. |
| `pagination` | Pages, display-list commands, and semantic hit regions. |
| `navigation` | Document paths, anchors, link targets, and history. |
| `reader` | Page position and navigation state. |
| `render` | Generic `DrawTarget` adapter. |

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
