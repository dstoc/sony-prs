# Build and deploy `prs-t1-agent`

`prs-t1-agent` is the native ARM binary that runs on the PRS-T1. The device
expects an ARMv5TE, soft-float executable, and the release build is statically
linked against musl so it does not need Android or other target-side runtime
libraries.

## Automated GitHub releases

Release Please manages `prs-t1-agent` releases in manifest mode. Pushes to
`main` create or update a release PR when Conventional Commits affecting the
crate are ready to release. Merging that PR updates the crate version and
`CHANGELOG.md`, creates a component-prefixed tag and GitHub Release such as
`prs-t1-agent-v0.1.0`, and builds the device binary from that tagged commit.

The release workflow follows the production `cargo zigbuild` command below,
checks the result for ARM/EABI5, soft-float, and static linking, and uploads it
to the release as `prs-t1-agent-armv5te`. Download it from the matching GitHub
Release rather than from crates.io; this crate is not published there.

Use Conventional Commit messages for changes that should affect the release:

- `fix:` produces a patch release.
- `feat:` produces a minor release.
- `!` after the type/scope or a `BREAKING CHANGE:` footer produces a major
  release.

Documentation-only, test-only, and other non-release commit types do not bump
the crate unless the commit also uses an explicit release directive supported
by Release Please. Pull requests and ordinary pushes do not upload artifacts;
the ARM build and upload run only when Release Please reports that this
component's release was created.

Run the repository's configuration guard after changing the release files:

```sh
python3 tools/test-release-workflow.py
```

## Host toolchain

Install or otherwise make these commands available on `PATH`:

- Rust and Cargo, for the Rust 2021 crate.
- [`cargo-zigbuild`](https://github.com/rust-cross/cargo-zigbuild), Cargo's
  Zig-backed cross-build command.
- Zig, used by `cargo-zigbuild` as the cross-linker.
- ADB, for copying and launching the binary on a rooted reader.

The agent does not require the Android SDK, Android NDK, Gradle, or a Java
toolchain. The local development workflow does not require the exact versions
used by the release workflow. The GitHub workflow pins Rust 1.88.0, Zig 0.13.0,
and `cargo-zigbuild` 0.23.4 so published artifacts are reproducible. Verify
the tools available for a local build before building:

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
