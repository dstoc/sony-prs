# Rust UI milestone

The first custom UI is a deliberately small Rust framebuffer application. It
uses the reader's 8-bit grayscale `/dev/fb0` mapping, draws a fixed 600x800
home screen with a built-in 5x7 font, sets E-Ink power mode normal (`0x46f6`),
and refreshes the full panel through the firmware-proven `0x4700` update ioctl.

The application is the `ui` mode of `device-agent`. The development gadget
hook stops the stock `tinyhttp` UI and starts `/tmp/prs350-agent ui` after the
signed package has been installed. It intentionally remains alive after the
first refresh so the stock UI cannot immediately reclaim the framebuffer.

This milestone is display-only. Touch and button input are not yet dispatched;
the next UI increment should identify the input devices exposed during a normal
non-USB boot and add a small event loop before replacing the static screen.

Build the ARM binary with:

```text
PATH=/home/agent/.local/bin:$PATH \
RUSTFLAGS='-C target-cpu=arm926ej-s -C link-arg=-mcpu=arm926ej-s' \
cargo zigbuild --manifest-path device-agent/Cargo.toml --release \
  --target armv5te-unknown-linux-musleabi
```

Then use `tools/build-prs350-dev-package.sh` and the normal mass-storage /
selector workflow. Serial is retained for `serial-ping`, `serial-probe`,
`serial-shell`, and `serial-screenshot`; the UI binary is deployed only as
part of the signed package.
