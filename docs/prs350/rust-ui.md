# Rust UI milestone

The first custom UI is a deliberately small Rust framebuffer application. It
uses the reader's 8-bit grayscale `/dev/fb0` mapping, draws a fixed 600x800
home screen with a built-in 5x7 font, sets E-Ink power mode normal (`0x46f6`),
and refreshes the full panel through the firmware-proven `0x4700` update ioctl.

The application is the `ui` mode of `prs350-agent`. The development gadget
hook stops the stock `tinyhttp` UI and starts `/tmp/prs350-agent ui` after the
signed package has been installed. It intentionally remains alive after the
first refresh so the stock UI cannot immediately reclaim the framebuffer.

The next increment now includes a small input loop. The PRS-350 does not expose
Linux `evdev` nodes for these controls; the firmware uses `/dev/subcpu`. The
agent enables touch scanning, decodes the firmware's 8-byte packed packets,
and maps the touch coordinates with the calibration points from
`deviceConfig.xml`. The screen reports the last touch position and raw key
code/state. Navigation actions are intentionally not assigned yet while the
physical key mapping is being confirmed.

The observed packet families are category `6`, commands `4/5/6` for touch
samples, and category `3`, command `1` for keys. The scan-enable packet is
encoded in the agent rather than relying on the stock application to
initialize the controller.

The packet decoder is covered by host-side tests using captured device traffic,
including checksum rejection and the full 600x800 calibration range. The ARM
binary also cross-compiles successfully. Device-side framebuffer and physical
input validation remain pending; the next deployment will report whether a
failure occurs while opening the framebuffer, setting E-Ink power, or refreshing
the picture.

Build the ARM binary with:

```text
PATH=/home/agent/.local/bin:$PATH \
RUSTFLAGS='-C target-cpu=arm926ej-s -C link-arg=-mcpu=arm926ej-s' \
cargo zigbuild --manifest-path crates/prs350-agent/Cargo.toml --release \
  --target armv5te-unknown-linux-musleabi
```

Then use `tools/prs350/build-dev-package.sh` and the normal mass-storage /
selector workflow. The separate `prs350-devctl` tool retains the serial
controls for probing, shell access, and screenshots; the UI binary is deployed
only as part of the signed package.
