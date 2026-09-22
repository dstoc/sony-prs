# PRS-T1 development reader

This is the small development document used to smoke-test the native Markdown
reading surface. It is intentionally plain so it can be replaced with another
file while the document browser is still out of scope.

Tap the right half of a page to advance and the left half to go back. Links are
handled by the shared reader before those blank-page tap zones are applied.

On the reader, the physical left key (`KEY_LEFT`, code 105) moves to the
previous reader page and the physical right key (`KEY_RIGHT`, code 106) moves
to the next page. Each key press moves exactly one page; key-repeat events are
ignored. Outside the reader, these keys do nothing. The Home key (`KEY_HOME`,
code 102) returns from native UI screens to the current reading position. On
the reader, it opens the bundle entry point. The Back key (`KEY_BACK`, code
158) walks native UI history before delegating to the Markdown reader's
internal-link history. A short press of the physical menu key opens Details /
Settings only from the reader. Holding menu for at least one second still
requests the existing full GC16 refresh, and consumes the release.

Reader links support `#anchor`, `other.md`, and `other.md#anchor`. External URLs
are not opened by the native runtime; after activation, the URL is shown in a
bottom-of-screen **External URL** notice for the user to copy or act on
elsewhere.

The status bar remains available at the top of the screen; tapping it opens the
existing details/settings screen. Tap the status bar again, or use **Back to
reading**, to return here. Details / Settings and Display Test use a separate
small view-history stack, so returning to the reader does not add UI screens to
reader navigation history.
