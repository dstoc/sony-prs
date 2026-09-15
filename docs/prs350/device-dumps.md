# Device dump archive

Device-derived captures are kept locally under
[`device-dumps/prs350/`](../../device-dumps/prs350/). The archive is
intentionally ignored by Git because it contains firmware images, extracted
filesystems, framebuffer captures, and device identity material.

The archive currently contains:

- `raw/`: `Info.img`, MTD captures, and the tested partial `mtdblock1` read;
- `extracted/`: the normal root filesystem and the recovery/Id CramFS trees;
- `captures/`: kernel messages, USB-mon captures, and the live model read;
- `screenshots/`: framebuffer captures from the Rust UI and input tests.

The raw captures currently have these SHA-256 digests:

```text
7daa6242e045fa0271471773d80b15a683a8e5613203d592b1510c5f35b2745c  raw/Info.img
53c9b0abbd18569e51930fbb6276eda654ab92e99733bce6ea81f52b76390ecb  raw/mtdblock1-test2.bin
9f32a6acf68a1475d36a01832ab1dac23cb63eec94d8e6a3d9f7ac1ea27ee255  raw/mtdblock10.img
c9eaca3950efff60f259791b2c7c2c94ea56ffe7bbec94891db2ce528b012714  raw/mtdblock15.img
bb2d6826d1f50b429abe8e35ffd4a4bd030e355d95510f9ba2178a9b40b2bdf9  raw/mtdblock2.img
6e68710bb0d245a91a0d0013a10187eab6c3523e0660c9db7591b47ec0393a1a  raw/mtdblock3.img
11f99e47b709a0c3db041ac6726b490bfa8dfb8df2c473640518884dc38da44c  raw/mtdblock4.img
6e68710bb0d245a91a0d0013a10187eab6c3523e0660c9db7591b47ec0393a1a  raw/mtdblock9.img
```

`raw/Info.img` contains device signing/identity material used by the
historical updater verifier. Keep the ignored archive private and do not add
it to a commit or share it externally.
