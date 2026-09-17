# PRS-T1 monospace font validation

This case verifies the device fonts used by highlighted fenced code. It
requires a rooted PRS-T1 with the native reader build staged as described in
the [T1 build guide](../../crates/prs-t1-agent/build.md).

## Stage and run

Copy this document to the default startup path, then launch the native reader:

```sh
adb push docs/prs-t1/monospace-font-validation.md /mnt/sdcard/index.md
crates/prs-t1-agent/tools/native-test.sh start
```

The default configuration loads these device faces:

```text
/system/fonts/HelveticaMonospacedW1G-Rg.otf
/system/fonts/HelveticaMonospacedW1G-Bd.otf
/system/fonts/HelveticaMonospacedW1G-It.otf
/system/fonts/HelveticaMonospacedW1G-BdIt.otf
```

To test an alternate copy, set `PRS_T1_FONT_MONOSPACE`,
`PRS_T1_FONT_MONOSPACE_BOLD`, `PRS_T1_FONT_MONOSPACE_ITALIC`, and
`PRS_T1_FONT_MONOSPACE_BOLD_ITALIC` before launching the agent. A missing
optional variant falls back to the regular face of its family.

## Checks

Capture the page after the reader opens it. Inspect the code at normal zoom and
compare it with a render made by the earlier synthetic-bold build.

- The `fn`, `let`, and `true` tokens are bold and use the true bold monospace face.
- The line comments and the documentation comment are italic and use the true italic monospace face.
- The inline `let ready` sample below is bold-italic and uses the true bold-italic monospace face.
- Character columns remain aligned across normal, bold, italic, and bold-italic lines.
- Bold strokes keep their counters open and do not look smeared or crowded.
- Italic glyphs have the device face's designed slant. They do not show a renderer-applied shear.

The validation is successful when all four style variants remain aligned and
the bold and italic glyph shapes are visibly cleaner than the synthetic-bold
comparison.

## Validation source

The fenced Rust block below supplies normal syntax, bold keywords, and italic
comments for a real device preview. The inline code sample after the block
supplies the combined bold-italic case.

```rust
/// bold-italic documentation sample: reader metrics stay aligned
fn main() {
    let ready = true;
    // italic comment sample: true face, no synthetic shear
    println!("{ready}");
}
```

The combined monospace case is ***`let ready`***.

Use the runtime's normal style configuration for this check. Do not set a
monospace variant to the regular face when comparing visual quality.
