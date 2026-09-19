# Build and deploy `prs-t1-agent`

`prs-t1-agent` is the native ARM binary that runs on the PRS-T1. The device
expects an ARMv7-A/Cortex-A8, soft-float executable, and the release build is statically
linked against musl so it does not need Android or other target-side runtime
libraries.

## Automated GitHub releases

Release Please manages `prs-t1-agent` releases in manifest mode. Pushes to
`main` create or update a release PR when Conventional Commits affecting the
crate are ready to release. Merging that PR updates the crate version and
`CHANGELOG.md`, creates a component-prefixed tag and GitHub Release such as
`prs-t1-agent-v0.1.0`, and builds the device binary from that tagged commit.

The CI and release workflows call the shared
[`tools/prs-t1-agent-build.sh`](../../tools/prs-t1-agent-build.sh) production
build and ELF-validation script. CI stops after validation; the release
workflow additionally uploads the resulting `dist/prs-t1-agent-armv7` file
to the matching GitHub Release. Download it from that release rather than from
crates.io; this crate is not published there.

Use Conventional Commit messages for changes that should affect the release:

- `fix:` produces a patch release.
- `feat:` produces a minor release.
- `!` after the type/scope or a `BREAKING CHANGE:` footer produces a major
  release.

Documentation-only, test-only, and other non-release commit types do not bump
the crate unless the commit also uses an explicit release directive supported
by Release Please. Pull requests and ordinary pushes do not upload artifacts;
the ARM build and upload run when Release Please reports that this component's
release was created. If a release was created but its artifact job failed, the
same workflow can be manually dispatched with the existing release tag:

```sh
gh workflow run release-please.yml --ref main \
  --field tag=prs-t1-agent-v0.2.0
```

The recovery run checks out that tag, rebuilds the binary with the pinned
toolchain, and uploads it to the existing GitHub Release with `--clobber`; it
does not create a new release.

## Host toolchain

Install or otherwise make these commands available on `PATH`:

- Rust and Cargo, for the Rust 2021 crate.
- [`cargo-zigbuild`](https://github.com/rust-cross/cargo-zigbuild), Cargo's
  Zig-backed cross-build command.
- Zig, used by `cargo-zigbuild` as the cross-linker.
- ADB, for copying and launching the binary on a rooted reader.

The agent does not require the Android SDK, Android NDK, Gradle, or a Java
toolchain. The local development workflow does not require the exact versions
used by the release workflow. The shared GitHub build pins Rust 1.98.1, Zig
0.16.0, and `cargo-zigbuild` 0.23.4 so validation and published artifacts use
the same toolchain. Verify
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

Install the Rust target before the cross-build:

```sh
rustup target add armv7-unknown-linux-musleabi
```

## Cross-build

From the repository root, run:

```sh
RUSTFLAGS='-C target-cpu=cortex-a8 -C link-arg=-mcpu=cortex-a8' \
  cargo zigbuild \
  --manifest-path crates/prs-t1-agent/Cargo.toml \
  --release \
  --target armv7-unknown-linux-musleabi
```

The deployable binary is:

```text
target/armv7-unknown-linux-musleabi/release/prs-t1-agent
```

The pinned CI-equivalent command builds and verifies the same artifact and
copies a validated release binary to `dist/prs-t1-agent-armv7`:

```sh
tools/prs-t1-agent-build.sh build-and-verify
```

The ELF verification checks ARMv7, EABI5 soft-float attributes, static
linking, and the absence of a dynamic loader. The network probe is part of
this `prs-t1-agent` executable, so the same check covers the probe artifact.

For a quick sanity check, confirm that the result is ARM, EABI5, soft-float,
and static:

```sh
file target/armv7-unknown-linux-musleabi/release/prs-t1-agent
readelf -h target/armv7-unknown-linux-musleabi/release/prs-t1-agent
readelf -A target/armv7-unknown-linux-musleabi/release/prs-t1-agent
readelf -d target/armv7-unknown-linux-musleabi/release/prs-t1-agent
```

The repository-wide `cargo build --workspace --release` command is useful for
host-side compilation, but does not produce the T1 ARM artifact.

## Copy to the reader

With the reader booted into Android and ADB available:

```sh
AGENT_BINARY=target/armv7-unknown-linux-musleabi/release/prs-t1-agent

adb wait-for-device
adb push "$AGENT_BINARY" /data/local/tmp/prs-t1-agent
adb shell chmod 755 /data/local/tmp/prs-t1-agent
```

Run the network capability probe while Android remains active. Replace the
placeholder with the production protocol hostname selected for #99:

```sh
PROBE_HOST='your-production-host.example'
adb shell /data/local/tmp/prs-t1-agent network-probe "$PROBE_HOST"
adb shell /data/local/tmp/prs-t1-agent network-probe "$PROBE_HOST" \
  --invalid-hostname
```

The first command must report `result=success` and
`tls_validation=passed`. The second command is a safe negative test. It must
report `tls_validation=failed_as_expected` and `result=success`. The probe
does not require an authorization request, bearer token, Cloudflare secret, or
Wi-Fi credential. It does not change the device network configuration.

The normal probe performs only `GET /health`. It resolves the hostname before
the request, uses the bundled Mozilla root set with rustls chain and hostname
validation, rejects redirects, limits connection setup to 10 seconds, limits
the request and response reads to 20 seconds, and accepts at most 64 KiB of
response data. Capture stdout and the exit status for the #99 hardware record.

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
