# `prs-sync-bundle`

`prs-sync-bundle` owns the PRSync document bundle boundary. It generates and
consumes one uncompressed tar archive with a generated `manifest.json` at the
archive root.

The public API provides:

- `BundleBuilder` and `create_bundle` for one Markdown entry point plus an
  explicit list of regular files;
- common-parent stripping that preserves the remaining bundle-relative paths;
- UStar-compatible bundle paths;
- per-file manifest sizes and the shared 16 MiB encoded archive limit;
- streaming validation without retaining the archive in memory; and
- staged extraction that publishes files only after the complete archive is
  valid.

Validation rejects unsafe relative paths, duplicate paths, unsupported
manifest versions, manifest size mismatches, oversized archives, and every tar
entry type except regular files. The crate does not discover linked files and
does not calculate content hashes.

Run the focused checks from the repository root:

```sh
cargo test -p prs-sync-bundle
cargo clippy -p prs-sync-bundle --all-targets -- -D warnings
```
