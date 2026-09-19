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

The production D1 ID and bucket name are fixed in the checked-in
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

Run the complete local workflow test from the repository root after installing
the two local development tools:

```sh
cargo install worker-build --version 0.8.6 --locked
npm install --global wrangler@4
python3 tools/test-prs-cloudflare-local.py
```

The test uses `wrangler.local.toml`, a temporary local D1/R2 state directory,
and a test-only owner identity. It does not contact Cloudflare. It covers
sender and reader approval, bundle replacement, failed replacement clearing,
conditional manifest reads, bundle download, inbox clearing, credential
listing and revocation, and capability rejection.

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
- sender and reader authorization requests, including the requested sender
  credential name;
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

## Human approval application

The Worker exposes the following human-facing routes:

- `GET /a/<request-id>` renders the request context and the available action.
- `POST /a/<request-id>/approve` approves the request.
- `POST /a/<request-id>/deny` denies the request.

The page shows the request kind, sender credential name when the request is a
sender request, request ID, creation time, expiry time, and current state. It
does not show a polling secret, a polling-secret hash, or a bearer credential.
Approval and denial use separate POST actions. A request in a terminal state
has no action buttons.

## Protocol API

The machine API is under `/api/v1`. It uses the JSON types from
`prs-sync-protocol`. Every JSON request includes a compatible
`protocol_version`. Protocol API errors use the versioned `ApiError` envelope.

Authorization requests do not require a bearer token. The client keeps the
polling secret from the approval browser and submits it only to the polling
route.

| Method | Route | Access | Purpose |
| --- | --- | --- | --- |
| `POST` | `/api/v1/authorization/sender` | none | Create a named sender request. |
| `POST` | `/api/v1/authorization/reader` | none | Create a reader request. |
| `POST` | `/api/v1/authorization/poll` | polling secret | Claim the approved sender credential or reader session. |
| `GET` | `/api/v1/authorization/<request-id>` | none | Read public authorization status. |
| `PUT` | `/api/v1/sender/bundle` | sender bearer | Replace the current bundle. The request body is the validated tar archive. |
| `DELETE` | `/api/v1/sender/bundle` | sender bearer | Clear the current bundle. |
| `GET` | `/api/v1/sender/credentials` | sender bearer | List credential metadata only. |
| `DELETE` | `/api/v1/sender/credentials/<name>` | sender bearer | Revoke the active credential with this name. |
| `GET` | `/api/v1/reader/manifest` | reader bearer | Read the current manifest and revision. |
| `GET` | `/api/v1/reader/bundle` | reader bearer | Stream the current bundle. |

The reader manifest route accepts `If-Revision` and `If-None-Match` headers.
When either condition matches, the response state is `not_modified`. The
response also includes `X-PRSync-Revision` and, when a bundle exists, `ETag`.
The sender routes never return the manifest or bundle. The reader routes do
not accept sender mutations. Production protocol routes use bearer
capabilities and do not require an interactive Access login.

A successful sender push returns only publication metadata: revision, ETag,
and encoded size. It does not return the manifest.

### Trusted owner identity boundary

The approval routes accept only a platform-authenticated principal. In a
production build, the input is the `Cf-Access-Jwt-Assertion` header supplied by
Cloudflare Access. Access must validate the JWT signature before the request
reaches the Worker. The Worker then validates the JWT shape, requires the
`RS256` algorithm, requires an HTTPS issuer and a subject, and compares the
issuer and subject with the singleton row in `owner_identity`. If that row has
an email, the JWT must contain the same email. The Worker does not accept an
identity from the approval URL, query string, form body, cookie, or ordinary
browser-provided identity header.

Local integration tests can be built with the `local-test` Cargo feature. When
`PRS_ENVIRONMENT=local`, the same boundary can parse
`X-PRSync-Test-Owner: <issuer>|<subject>|<email>`. The production fetch path
does not select this source, and a default production build does not compile
the local test constructor.

The approval URL contains only the public request ID. It is a lookup key, not
an authentication factor. Owner authentication occurs before the Worker reads
or changes the authorization request.

## Authorization boundary

`src/authorization.rs` implements the D1-backed authorization state machine.
Request creation generates 32-byte polling secrets and request identifiers with
the Workers Web Crypto API. D1 stores only SHA-256 hashes. Approval URLs contain
the public request identifier but never the polling secret.

The service exposes separate typed paths for pending polling, sender
write/credential management, owner approval, and reader read-only access. The
sender and reader authentication queries are separate, so a sender token cannot
resolve to a reader session and a reader token cannot resolve to sender
operations. A successful claim uses an atomic D1 batch to create exactly one
child credential or session and consume the approved request. Approval methods
return no bearer credential; only the corresponding pending polling capability
receives the claim result.

Run the dependency-free local migration regression test from the repository
root with:

```sh
python3 tools/test-prs-cloudflare-migrations.py
python3 tools/test-prs-cloudflare-authorization.py
```

The second test runs the authorization transitions and D1 claim transaction
shapes against Python's SQLite library. It does not require Wrangler or live
Cloudflare bindings.
