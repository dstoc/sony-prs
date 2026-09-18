# `prs-cloudflare`

`prs-cloudflare` is the PRSync Rust Worker. It is the only component that may
access the D1 database and private R2 bucket.

## Bindings and environments

| Environment | Worker name | D1 binding | D1 database | R2 binding | R2 bucket |
| --- | --- | --- | --- | --- | --- |
| `local` | `prs-reader-local` | `DB` | `prs-reader-local` | `BUNDLES` | `prs-reader-local` |
| `production` | `prs-reader` | `DB` | `prs-reader-db` | `BUNDLES` | `prs-reader-documents` |

The binding names are part of the Worker contract:

- `DB` is the D1 metadata binding.
- `BUNDLES` is the private R2 bundle binding.

The local environment uses a non-production placeholder D1 ID. Wrangler's
default local mode stores D1 and R2 data in `.wrangler/state` and does not
contact Cloudflare. Do not add `remote = true` to the local bindings.

The production D1 ID is a deliberate placeholder until the production
resources are created. The production bucket name is fixed in the checked-in
configuration. The Worker deploy configuration references these resources; it
does not create them.

## Local development

Install a compatible `wrangler` and `worker-build` in the development
environment, then run the commands below from this directory:

```sh
cargo build -p prs-cloudflare --target wasm32-unknown-unknown --release
wrangler d1 migrations apply DB --local --env local
wrangler dev --env local
```

The local Worker listens on Wrangler's default address. Check the scaffold
health endpoint in another shell:

```sh
curl http://localhost:8787/health
```

The local bindings do not require a Cloudflare account or production
credentials. Local R2 object operations can use Wrangler's `--local` mode with
the `prs-reader-local` bucket.

## Production bootstrap and deployment

Production resource creation is a separate, deliberate operation:

```sh
../../tools/prs-cloudflare-bootstrap.sh --confirm-production
```

The script creates the named D1 database and R2 bucket. Copy the returned D1
ID into `wrangler.toml`, review the production account and resource names, and
apply the migration:

```sh
../../tools/prs-cloudflare-deploy.sh --production
```

The deploy script disables Wrangler's automatic resource provisioning. The
commands only use the resources named in the configuration. They do not create
a database or a bucket. Keep Cloudflare credentials outside the repository and
outside pull request jobs.

## Schema

`migrations/0001_initial.sql` creates metadata tables for the inbox,
authorization requests, sender credentials, and reader sessions. It does not
store bundle bytes. The Worker will add the protocol endpoints in later
issues.
