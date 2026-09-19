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

The local Worker listens on Wrangler's default address. Check the process
health endpoint in another shell:

```sh
curl http://localhost:8787/health
```

Check schema readiness after applying local migrations:

```sh
curl http://localhost:8787/ready
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
and the explicit `local-test` feature- and environment-gated
`X-PRSync-Test-Owner` header seam. It does not contact Cloudflare. It covers
runtime Access rejection of forged assertions, concurrent claims and cleanup,
expiry and terminal retention, D1 and R2 failure injection, bundle replacement,
conditional manifest reads, bundle download, inbox clearing, credential
listing and revocation, capability rejection, and authorization rate-limit
responses with their retry delays. The test-only maintenance route and control
headers compile only with `local-test` and remain disabled unless
`PRS_ENVIRONMENT=local`.

## Production bootstrap and deployment

Production resource creation is a separate, deliberate operation:

```sh
../../tools/prs-cloudflare-bootstrap.sh --confirm-production
```

The script creates the named D1 database and R2 bucket. Copy the returned D1
ID into `wrangler.toml`, review the production account and resource names, and
apply migrations with the operator command:

```sh
../../tools/prs-cloudflare-migrate.sh --production
```

The migration command locks concurrent runs, lists pending migrations, applies
the checked-in files, and lists the history again. Do not continue when a
migration fails or when the final list still has pending work. Inspect the D1
database and Wrangler history before retrying a failed migration.

After migration verification, publish the Worker with the restricted deploy
credential and the URL for its readiness check:

```sh
PRS_READER_URL=https://reader.example.com \
  ../../tools/prs-cloudflare-deploy.sh --production
```

The deploy script does not run D1 management commands. It disables Wrangler's
automatic resource provisioning and only publishes the Worker that uses the
existing bindings. The command fails unless the response is ready and reports
this checkout's exact release schema requirement.

To inspect an already-published Worker without publishing, run the same check
explicitly:

```sh
../../tools/prs-cloudflare-readiness.sh https://reader.example.com
```

The check reads `d1_migrations` through the Worker `DB` binding. It does not
use D1 API credentials and it never applies a migration. It rejects a ready
response from an older Worker because that response contains a different
release schema requirement.

Keep operator and deployment credentials outside the repository and outside
pull-request jobs.

Before the production Worker can serve approval requests, configure
`PRS_APPROVAL_BASE_URL` with the HTTPS base URL for the human approval
application. Protect that hostname with a Cloudflare Access application whose
policy allows only the human owner. The Worker checks `ctx.access` and accepts
approval requests only when the request hostname matches the hostname in
`PRS_APPROVAL_BASE_URL`. Configure a secret named `PRS_CSRF_SECRET` with at
least 32 bytes. For example, run `wrangler secret put PRS_CSRF_SECRET
--env production`. Keep the public protocol routes on their public hostname;
those routes use PRSync bearer capabilities and do not require an interactive
Access login.

## Schema

`migrations/0001_initial.sql` creates the original metadata tables. The
numbered `0002_metadata_schema_upgrade.sql` migration upgrades those tables
for existing local and production databases. Migration
`0004_approved_authorization_expiry.sql` applies the authorization expiry
rules. Migration `0005_bundle_cleanup_lifecycle.sql` adds lifecycle
coordination for bundle publication and cleanup. Migration
`0006_authorization_maintenance.sql` adds bearer-token lookup indexes and the
rate-limit state table. Migration `0007_remove_owner_identity.sql` removes the
obsolete D1 owner-identity policy. A fresh database applies all migrations in
order.

The Worker release declares its schema requirement in
`src/schema.rs` as `RELEASE_SCHEMA_REQUIREMENT`. The current release requires
migration `0007_remove_owner_identity.sql`. The `/ready` endpoint reads the
Wrangler `d1_migrations` history through `DB` and returns HTTP 200 only when
the required migration prefix is contiguous and has the expected names. It
returns HTTP 503 for missing, unavailable, incomplete, outdated, or mismatched
history. Readiness is read-only and never runs migrations.

`/health` reports only that the Worker process responds. It is not a schema
check. An old Worker can report readiness for its own requirement after a
compatible additive migration. That response is not proof that a newer
release is ready. Always inspect `/ready` after the new Worker is published
and confirm the response contains that release's requirement.

An older Worker may accept a later migration only when its
`COMPATIBLE_FUTURE_MIGRATIONS` declaration contains the exact migration ID
and filename. The numeric filename prefix does not establish compatibility.
Review and test the migration before adding it to that declaration. An
undeclared future migration, including a correctly numbered destructive
migration, makes the older Worker not ready.

Schema changes must preserve old Worker behavior during the migration and
deployment window. Prefer additive columns, indexes, tables, and nullable
fields. Defer drops, renames, and other destructive cleanup until old Worker
releases no longer need the schema. If a migration changes the required
schema, update `RELEASE_SCHEMA_REQUIREMENT` in the same release and apply and
verify the migration before publishing that release.

The resulting schema contains:

- one current inbox revision and an optional current bundle reference;
- immutable bundle metadata and its private R2 object key;
- sender and reader authorization requests, including the requested sender
  credential name;
- 32-byte polling-secret, sender-token, and reader-session hashes;
- boot-scoped reader sessions with a finite server-side expiry;
- named sender credentials with revocation timestamps.

Each bundle metadata row has a lifecycle row. Legacy metadata is backfilled as
`published`. New uploads start as `uploading`, change to `published` in the
same D1 batch that updates the inbox, and can enter `cleanup_claimed` only
after the retention period expires.

The migrations do not store bundle bytes or bearer credentials. Authorization
requests use the states `pending`, `approved`, `denied`, `expired`, and
`consumed`. The `expires_at` value is an absolute deadline for both pending
and approved requests. Approval does not extend that deadline. The state
trigger prevents rewinds. The Worker must insert the credential or session
and mark the request `consumed` in one D1 transaction.

## Bundle publication

The Worker owns the BUNDLES binding. Clients never receive an R2 binding or
an R2 credential. A push clears the D1 inbox reference before it deletes the
old object, validates the received archive, and stores a new immutable object.
The Worker publishes the new bundles row and the inbox reference in one D1
batch. After the clear step, validation, R2, or final D1 failure leaves the
inbox empty.

New objects use the `bundles/candidates/<random-id>.tar` prefix. The Worker
creates the lifecycle row before it writes the R2 object. Publication is
conditional on that row still being `uploading`. Cleanup first claims an
expired row in D1 and then deletes its R2 object. A publication that races the
claim fails its D1 batch, so cleanup cannot delete an object that the
publication can still make current.

An hourly Worker schedule scans at most 100 lifecycle rows and 100 R2 objects
per invocation. R2 list cursors are followed until that bound is reached.
Each abandoned object and unreferenced metadata row is retained for 24 hours.
All D1 lifecycle timestamps use Unix seconds. The scheduled event timestamp and
R2 upload timestamp are converted to Unix seconds before cleanup compares them.
Current inbox references are protected by the D1 foreign key relationship.
Legacy objects under `bundles/` that have no lifecycle row are deleted only
after the same R2 upload-age retention period. A failed R2 delete leaves the
lifecycle claim in D1 for the next scheduled run. Metadata is deleted only
after the R2 delete succeeds, in dependency order, so retries handle partial
failures without removing current metadata.

## Human approval application

The Worker exposes the following human-facing routes:

- `GET /a/<request-id>` renders the request context and the available action.
- `POST /a/<request-id>/approve` approves the request.
- `POST /a/<request-id>/deny` denies the request.

The page shows the request kind, sender credential name when the request is a
sender request, request ID, creation time, expiry time, and current state. It
does not show a polling secret, a polling-secret hash, or a bearer credential.
Approval and denial use separate POST actions. Each action requires a matching
same-origin `Origin` header and a CSRF token. The token is signed with
`PRS_CSRF_SECRET` and bound to the request ID and authenticated Access context.
The response uses `frame-ancestors 'none'` and `form-action 'self'` CSP
directives. A request in a terminal state has no action buttons.

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

### Authorization abuse controls

The Worker applies fixed-window limits by the trusted `CF-Connecting-IP`
value:

- Authorization creation allows 10 requests per 60-second window. Sender and
  reader creation share this limit.
- Status and claim polling allow 60 requests per 60-second window. The status
  and claim routes share this limit.

When a limit is exceeded, the Worker returns HTTP 429 with the versioned
`rate_limited` error code and a `Retry-After` header. The header gives the
number of seconds until the next request can proceed. Clients should wait for
that delay before retrying.

The hourly scheduled event also runs authorization maintenance. Each pass:

- expires pending and approved requests whose deadlines have passed;
- retains terminal authorization requests for 24 hours;
- deletes at most 100 expired reader sessions, terminal requests, and stale
  rate-limit buckets in each category; and
- preserves reader sessions that an authorization request still references.

Active sender credentials are not part of the cleanup set and remain valid
until a sender revokes them. The cleanup queries use the terminal-request
index and the sender and reader bearer-token indexes from
`0006_authorization_maintenance.sql`.

A successful sender push returns only publication metadata: revision, ETag,
and encoded size. It does not return the manifest.

Request bodies are bounded before parsing. Authorization JSON bodies are limited
to 4 KiB. Credential-management JSON bodies are limited to 4 KiB. Bundle bodies
are limited to the protocol maximum of 16 MiB. The Worker checks
`Content-Length` when it is present, then enforces the same limit while it reads
the body. A missing or inaccurate `Content-Length` cannot bypass the limit.

An oversized bundle returns the versioned `payload_too_large` error with HTTP
status 413. An authorized oversized bundle is a failed replacement: the Worker
clears the inbox before it returns the error. This matches malformed bundle
uploads, which also clear the inbox before validation. An unauthorized upload
fails at the bearer check and cannot change the inbox.

### Cloudflare Access boundary

The approval routes require the configured approval hostname and a
Cloudflare-authenticated `ctx.access` context. Cloudflare Access applies the
human-owner policy before the direct Worker invocation. The Worker does not
read `Cf-Access-Jwt-Assertion`, parse JWTs, fetch JWKS documents, or compare a
caller identity with a second D1 owner policy. It calls
`ctx.access.getIdentity()` only if the page later needs identity data for
display or audit.

Local integration tests can be built with the `local-test` Cargo feature. When
`PRS_ENVIRONMENT=local`, the explicit local seam can parse
`X-PRSync-Test-Owner: <issuer>|<subject>|<email>`. The production fetch path
does not select this source, and a default production build does not compile
the local seam. The local end-to-end test uses this feature-gated seam and
`http://127.0.0.1` as its approval hostname.

The approval URL contains only the public request ID. It is a lookup key, not
an authentication factor. Owner authentication occurs before the Worker reads
or changes the authorization request.

## Authorization boundary

`src/authorization.rs` implements the D1-backed authorization state machine.
Request creation generates 32-byte polling secrets and request identifiers with
the Workers Web Crypto API. D1 stores only SHA-256 hashes. Approval URLs contain
the public request identifier but never the polling secret.

The service exposes separate typed paths for pending polling, sender
write/credential management, Access approval, and reader read-only access. The
sender and reader authentication queries are separate, so a sender token cannot
resolve to a reader session and a reader token cannot resolve to sender
operations. A successful claim uses an atomic D1 batch to create exactly one
child credential or session and consume the approved request. A sender-name
conflict uses an insert-if-absent operation, so the losing request remains
approved and receives a typed conflict. Approval methods return no bearer
credential; only the corresponding pending polling capability receives the
claim result.

Run the dependency-free local migration regression test from the repository
root with:

```sh
python3 tools/test-prs-cloudflare-migrations.py
python3 tools/test-prs-cloudflare-authorization.py
```

The second test runs the authorization transitions and D1 claim transaction
shapes against Python's SQLite library. It also checks the fixed-window rate
limits, retry delay, bounded maintenance, reference preservation, and the
bearer-token indexes. It does not require Wrangler or live Cloudflare
bindings.
