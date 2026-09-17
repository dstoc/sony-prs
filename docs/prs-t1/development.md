# PRS-T1 development reader

This is the small development document used to smoke-test the native Markdown
reading surface. It is intentionally plain so it can be replaced with another
file while the document browser is still out of scope.

Tap the right half of a page to advance and the left half to go back. Links are
handled by the shared reader before those blank-page tap zones are applied.

On the Home reading surface, the physical left key (`KEY_LEFT`, code 105) moves
to the previous reader page and the physical right key (`KEY_RIGHT`, code 106)
moves to the next page. Each key press moves exactly one page; key-repeat events
are ignored. A short press of the physical menu key goes Back through the
Markdown reader's internal-link history. Holding menu for at least one second
still requests the existing full GC16 refresh. These controls are inactive on
the Details / Settings page.

Reader links support `#anchor`, `other.md`, and `other.md#anchor`. External URLs
are not opened by the native runtime; after activation, the URL is shown in a
bottom-of-screen **External URL** notice for the user to copy or act on
elsewhere.

The status bar remains available at the top of the screen; tapping it opens the
existing details/settings screen. Tap the status bar again, or use **Back to
reading**, to return here.
