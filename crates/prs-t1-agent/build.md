# Build and deploy `prs-t1-agent`

`prs-t1-agent` is the native ARM binary that runs on the PRS-T1. The device
expects an ARMv5TE, soft-float executable, and the release build is statically
linked against musl so it does not need Android or other target-side runtime
libraries.

## Host toolchain

Install or otherwise make these commands available on `PATH`:

- Rust and Cargo, for the Rust 2021 crate.
- [`cargo-zigbuild`](https://github.com/rust-cross/cargo-zigbuild), Cargo's
  Zig-backed cross-build command.
- Zig, used by `cargo-zigbuild` as the cross-linker.
- ADB, for copying and launching the binary on a rooted reader.

The agent does not require the Android SDK, Android NDK, Gradle, or a Java
toolchain. The repository currently does not pin versions for Rust,
`cargo-zigbuild`, or Zig; verify the installed tools before building:

```sh
rustc --version
cargo --version
zig version
cargo zigbuild --help >/dev/null
adb version
```

If `cargo-zigbuild` is not installed, it can be installed with Cargo. Install
Zig separately using the host's package manager or the official Zig release,
then ensure its `zig` executable is on `PATH`.

## Cross-build

From the repository root, run:

```sh
RUSTFLAGS='-C target-cpu=arm926ej-s -C link-arg=-mcpu=arm926ej-s' \
  cargo zigbuild \
  --manifest-path crates/prs-t1-agent/Cargo.toml \
  --release \
  --target armv5te-unknown-linux-musleabi
```

The deployable binary is:

```text
target/armv5te-unknown-linux-musleabi/release/prs-t1-agent
```

For a quick sanity check, confirm that the result is ARM, EABI5, soft-float,
and static:

```sh
file target/armv5te-unknown-linux-musleabi/release/prs-t1-agent
readelf -h target/armv5te-unknown-linux-musleabi/release/prs-t1-agent
readelf -d target/armv5te-unknown-linux-musleabi/release/prs-t1-agent
```

The repository-wide `cargo build --workspace --release` command is useful for
host-side compilation, but does not produce the T1 ARM artifact.

## Copy to the reader

With the reader booted into Android and ADB available:

```sh
AGENT_BINARY=target/armv5te-unknown-linux-musleabi/release/prs-t1-agent

adb wait-for-device
adb push "$AGENT_BINARY" /data/local/tmp/prs-t1-agent
adb shell chmod 755 /data/local/tmp/prs-t1-agent
```

The agent's read-only commands can then be exercised while Android is still
running:

```sh
adb shell /data/local/tmp/prs-t1-agent status
adb shell '/data/local/tmp/prs-t1-agent capture > /data/local/tmp/t1-screen.pgm'
adb pull /data/local/tmp/t1-screen.pgm ./t1-screen.pgm
```

For the native ownership test, the helper script uses the same default build
output path:

```sh
./crates/prs-t1-agent/tools/native-test.sh status
./crates/prs-t1-agent/tools/native-test.sh start
```

`start` stops zygote and `system_server` before launching the agent, so use it
only for an intentional native-mode test. The supported return path is:

```sh
./crates/prs-t1-agent/tools/native-test.sh reboot
```

The helper also accepts `PRS_T1_AGENT_BINARY` when the binary is stored at a
different host path. See [README.md](README.md) for the available runtime
commands, screenshot workflow, refresh tests, and recovery details.
