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

`migrations/0001_initial.sql` creates the original metadata tables. The
numbered `0002_metadata_schema_upgrade.sql` migration upgrades those tables
for existing local and production databases. A fresh database applies both
migrations in order.

The resulting schema contains:

- one current inbox revision and an optional current bundle reference;
- immutable bundle metadata and its private R2 object key;
- sender and reader authorization requests;
- 32-byte polling-secret, sender-token, and reader-session hashes;
- boot-scoped reader sessions;
- named sender credentials with revocation timestamps; and
- one configured Cloudflare Access owner identity.

The migrations do not store bundle bytes or bearer credentials. Authorization
requests use the states `pending`, `approved`, `denied`, `expired`, and
`consumed`. The state trigger prevents rewinds. The Worker must insert the
credential or session and mark the request `consumed` in one D1 transaction.

## Bundle publication

The Worker owns the BUNDLES binding. Clients never receive an R2 binding or
an R2 credential. A push clears the D1 inbox reference before it deletes the
old object, validates the received archive, and stores a new immutable object.
The Worker publishes the new bundles row and the inbox reference in one D1
batch. After the clear step, validation, R2, or final D1 failure leaves the
inbox empty.

New objects use the bundles/candidates/<random-id>.tar prefix. The prefix
identifies objects that cleanup may inspect. A cleanup operation resolves
inbox.current_bundle_id through bundles.object_key and keeps that object. All
other objects under the prefix are abandoned replacement objects. The prefix
remains on a current object because R2 has no rename operation.

Run the dependency-free local migration regression test from the repository
root with:

```sh
python3 tools/test-prs-cloudflare-migrations.py
```
