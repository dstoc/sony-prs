# Documentation index

## PRS-350

- [Protocol ledger](prs350/protocol.md)
- [First-device session](prs350/device-session.md)
- [Firmware analysis](prs350/firmware-analysis.md)
- [Recovery startup](prs350/recovery-startup.md)
- [Native UI milestone](prs350/rust-ui.md)
- [HTTP-over-USB analysis](prs350/http-patch-usb-analysis.md)
- [Filesystem analysis](prs350/mtdblock15-analysis.md)
- [Historical artifacts](prs350/artifacts.md)
- [Ignored dump archive](prs350/device-dumps.md)

## PRS-T1

- [T1 analysis](prs-t1/analysis.md)
- [Markdown reader architecture and support matrix](prs-t1/markdown-reader.md)
- [Development reader smoke-test document](prs-t1/development.md)
- [`prs-markdown` library and host harness](../crates/prs-markdown/README.md)
- [Native UI and Markdown-reader integration guide](../crates/prs-t1-agent/README.md)
- [Build and deployment guide](../crates/prs-t1-agent/build.md)
- [Tap-to-launch APK guide](../tools/prs-t1-launcher/README.md)

The Markdown-reader documents describe the implementation in layers: the
library README covers host use and tests, the architecture guide covers the
shared pipeline and content contract, the development document is the staged
T1 smoke-test input, and the native UI guide covers device controls, refresh,
power, and recovery. Hardware observations and historical experiments remain
in [T1 analysis](prs-t1/analysis.md).
