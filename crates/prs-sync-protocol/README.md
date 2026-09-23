# `prs-sync-protocol`

`prs-sync-protocol` contains the versioned JSON wire types shared by the
PRSync sender, Cloudflare Worker, and reader client.

The crate contains protocol data only. It does not access Cloudflare bindings,
the filesystem, the reader UI, or Markdown rendering. It does not generate,
persist, hash, or validate production credentials. Opaque polling secrets and
service-issued bearer tokens exist only as validated wire values.

The public types cover:

- bundle manifests with per-file sizes and the 16 MiB bundle limit;
- inbox revisions, entity tags, and conditional manifest responses;
- sender and reader authorization requests, statuses, and claim results;
- separate sender capabilities and boot-scoped reader session scopes, including
  the server expiry metadata needed for client recovery;
- sender credential metadata; and
- versioned API errors.

Every top-level wire message contains `protocol_version`. Minor versions are
compatible when the received major version matches and the received minor
version is no greater than the supported minor version. Serde ignores unknown
fields so compatible readers can consume additive messages.

Run the workspace checks from the repository root:

```sh
cargo test -p prs-sync-protocol
cargo clippy -p prs-sync-protocol --all-targets -- -D warnings
```
