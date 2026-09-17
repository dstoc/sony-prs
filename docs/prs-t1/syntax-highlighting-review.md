# Syntax-highlighting rendering review

This review supports sony-prs/48. It compares the source fixture, the
highlighting and rendering code, and the checked-in PNG goldens at the public
T1 viewport of 600 by 800 pixels.

The baseline command passed:

```sh
cargo test -p prs-markdown --test harness
```

The suite passed 11 tests. Passing tests do not prove visual quality. The
current structural test checks that code is lossless, that some spans have
non-black ink, and that some spans are bold. It does not check scope boundaries,
contrast ratios, italic rendering, diff semantics, or code-block geometry.

## Findings

### Multiline Python scope does not end

The Python fixture uses a triple-quoted docstring at
`tests/fixtures/syntax-highlight.md:13-18`. The resulting page golden renders
the following `return` statement with the docstring's secondary gray styling.
The generated grammar uses the same `'''` form for the block-comment start and
end, but the loaded scope remains active after the closing delimiter.

Follow-up: sony-prs/49.

### Escaped string delimiters can change the rest of a line

The generated string context in `crates/prs-markdown/build.rs` needs a reliable
rule for escaped quotes. A Rust input such as let value = "a \" quote"; return
value; keeps the text after the closing literal in string styling. The
checked-in fixture has no escaped-delimiter case, so this failure is not
covered by the current PNG suite.

Follow-up: sony-prs/50.

### Fence attributes disable highlighting for supported languages

`normalize_language` accepts a leading dot and whitespace-delimited aliases,
but not a common attribute form such as `rust,ignore`. The highlighter then
uses the plain-code fallback. The fallback is lossless, but it silently removes
syntax styling from a supported language.

Follow-up: sony-prs/51.

### Token contrast is weak and several roles collapse

The theme assigns gray values from 32 through 128, then `e_ink_gray` maps
64, 96, and 128 to the same output value, 96. The code surface uses 248 on a
255 background. The syntax goldens show comments and strings as pale gray
strips that are much less legible than body text, and several token classes no
longer differ after quantization.

Follow-up: sony-prs/52.

### Code italic styling is discarded

Layout preserves Syntect's italic flag. The renderer selects
`FontFace::Monospace` for every code style, however, and only has a synthetic
bold pass. The multiline C comment in the fixture therefore has no visible
italic treatment. Its only distinction is the weak gray ink described above.

Follow-up: sony-prs/53.

### Diff highlighting does not represent patch structure

The fixture includes hunk, removed, and added lines at
`tests/fixtures/syntax-highlight.md:28-32`. The generated Diff grammar marks
the literal words `old` and `new` as keywords, but does not style the `+`, `-`,
or `@@` line prefixes. The golden therefore gives no reliable visual cue for
additions, removals, or hunk headers, and the current emphasis is misleading
when those words occur in ordinary code.

Follow-up: sony-prs/54.

### Code blocks do not have a stable surface or inset

Pagination fills each code `LayoutLine`, and each line stores only its used
text width. The gray surface stops at the last visible glyph on each line, and
code starts at the normal content margin. The first syntax golden consequently
shows short gray strips with different widths rather than one clear code-block
surface. Wrapped-line shading can also be mistaken for token highlighting.

Follow-up: sony-prs/55.

## Scope and dependencies

The findings are independent. No blocking links are required between the
follow-up issues. #49, #50, #51, and #54 change grammar behavior. #52 and #53
change how styles survive an e-ink renderer. #55 changes code-block geometry.
Each issue includes its own regression and golden-refresh requirement so the
changes can be reviewed separately.

The inspected goldens did not provide enough evidence to file a separate
confirmed clipped-glyph issue. The review did identify the style and geometry
problems above, which should be resolved before judging final syntax-rendering
sharpness on hardware.
