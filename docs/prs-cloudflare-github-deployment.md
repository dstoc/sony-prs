# Protected GitHub deployment environment

This runbook defines the repository-side controls for the production PRSync
deployment environment. It does not contain secret values.

## Environment controls

Configure a GitHub Actions environment with these settings:

- Name: `production`.
- Protected deployment branch: `main`.
- Required reviewers: enabled.
- Environment secrets: `CLOUDFLARE_ACCOUNT_ID` and
  `CLOUDFLARE_API_TOKEN`.

Keep both Cloudflare values in the `production` environment. Do not add them
as repository secrets. Do not expose them to pull-request jobs.

The production deployment job must be the only job that references these
secrets. That job must declare `environment: production`. Pull-request jobs
must not declare the production environment or reference either secret. The
workflow must not print the values or include them in artifacts, summaries, or
comments.

Expose the two secrets only to the Worker deploy steps. Do not set them on the
whole job. The health and browser smoke tests execute without them.

The local contract test is:

```sh
python3 tools/test-prs-cloudflare-deployment-credentials.py
```

The test permits Cloudflare secret references only in one job that uses the
`production` environment. It rejects those references in pull-request
workflows.

## Deployment token record

Keep the deployment token record in a protected operator system. Record these
fields, but never record the token value:

- token name;
- owner account;
- exact permission categories;
- resource restrictions;
- creation date;
- rotation owner.

The token must have only the Worker publishing permission required by the
verified restricted publishing path. It must not have D1 management, R2
management, or broader infrastructure permissions. The operator migration
credential remains separate and must not be installed in GitHub deployment
secrets.

## Rotation and revocation

Use this sequence for each token rotation:

1. Create a replacement token with the same account scope and permission
   categories.
2. Record the replacement token metadata in the protected operator record.
3. Update `CLOUDFLARE_API_TOKEN` in the protected `production` environment.
4. Run the protected `main` deployment.
5. Verify `/health` and the release-aware `/ready` response.
6. Confirm that the deployment log and artifacts contain no token text.
7. Revoke the old token.
8. Confirm that the old token is denied and that the replacement token still
   supports the restricted publish operation.

Do not test rotation with a pull-request workflow. Do not place either token
in a shell command, repository file, issue comment, or build artifact. Use the
restricted publishing runbook for direct D1/R2 management-denial checks and
binding-authority verification.

## Production deployment workflow

The production workflow is `.github/workflows/prs-cloudflare-deploy.yml`.
It listens for a completed `CI` workflow run on `main`. The deployment job
starts only when that run succeeds, represents a push to `main`, and belongs to
this repository. It checks out the exact commit tested by that run.

The job uses the protected `production` environment and its two Cloudflare
secrets. Configure `PRS_READER_URL` as a non-secret variable in that
environment. Set it to `https://prs-reader.dstoc.workers.dev`, the
`workers.dev` endpoint configured for the production `prs-reader` Worker.
The checked-in production Worker configuration uses this same URL for
`PRS_APPROVAL_BASE_URL`. Access protects only the `/a/*` approval routes;
`/health`, `/ready`, and `/api/v1/*` remain outside interactive Access.
Configure the Worker secret `PRS_CSRF_SECRET` before the first deployment
with `tools/prs-cloudflare-configure-approval.sh --production`. Keep this
secret in Cloudflare Worker secret storage. Do not add it to the GitHub
environment, repository variables, Wrangler configuration, logs, or
artifacts. The deployment readiness check fails until the binding exists.

Before the protected deployment job, the workflow checks the exact
`workflow_run.head_sha` against the nearest earlier successful `CI` run on
`main`. It checks out the tested commit with full history and compares the
range between those two commits. This includes all commits in a normal push
and the complete result of a merge commit. The gate derives local package
paths by walking the path dependencies of `crates/prs-cloudflare/Cargo.toml`.
It also watches the root Cargo manifests, Cloudflare configuration and
migration files, deployment scripts, build-pin script, deployment workflow,
CI workflow, and related Cloudflare contract documentation and tests.

The gate logs every changed path and its `deploy=true|false` decision. A
commit with no deploy-relevant changes writes `No Cloudflare deploy-relevant
changes for <sha>` to the Actions summary and skips the protected job. If the
previous successful run cannot be found or the history cannot be proven to be
ancestral, the gate deploys conservatively.

The job installs the pinned `worker-build` and Wrangler versions, builds the
Worker, and runs:

```sh
tools/prs-cloudflare-deploy.sh --production
```

The script derives a tag from the checked-out release commit. It uploads a
version with the existing D1 and R2 bindings, reads the returned Worker
Version ID, and promotes that ID to 100% of traffic with
`--no-x-provision` and no interactive prompt. It does not run D1 migrations or
deploy routes, custom domains, or triggers. The command also checks the
read-only release-aware `/ready` endpoint. That check polls for up to 120
seconds with bounded exponential backoff, requires the exact promoted Version
ID from `CF_VERSION_METADATA`, and retries propagation responses such as the
previous Worker's plain-text response or invalid JSON. Only after that check
succeeds does the workflow check `/health`. A failed build, upload, promotion,
readiness timeout, or health check fails the deployment job. A rerun promotes
the ID from its new upload, even when an earlier upload used the same commit
tag.

## Browser reader static Worker

The browser reader is a separate static-assets-only Worker named
`prs-reader-web`. Its existing `workers.dev` address is
`https://prs-reader-web.dstoc.workers.dev`. Keep this Worker separate from
`prs-reader`; do not create another Worker or add the site assets to the API
Worker. The static Worker configuration is
`web/reader-web/wrangler.toml`. It points to `target/reader-web/`, the output
from `tools/reader-web-build.sh build`, and has no Worker entry point, API
binding, custom route, or Access policy. The reader uses the root page and has
no client-side URL routes, so missing assets return 404. Wrangler assigns
`Content-Type` from each asset's extension, including `application/wasm` for
the generated WebAssembly file.

The same successful-CI `workflow_run` checks out the exact tested `main` SHA.
Its deployment gate decides separately whether to publish the API Worker and
the browser reader. Browser changes include its checked-in assets and config,
reachable local Cargo dependencies, browser build and test scripts, shared
Cargo files, and deployment scripts and config. Reader README and demo fixture
changes do not deploy the production site. Unrelated changes skip deployment.
If the previous successful CI commit or its ancestry cannot be proven, the
gate deploys both Workers conservatively. Browser deployment runs in the
existing protected `production` environment and reuses
`CLOUDFLARE_ACCOUNT_ID` and `CLOUDFLARE_API_TOKEN`. It builds and validates
`target/reader-web/` with `tools/test-reader-web.sh target/reader-web` before
deploying it to the existing Worker. The workflow then checks the deployed
HTML, JavaScript, CSS, generated WASM and production API preflight.

The browser asset probe accepts one canonical 307: `/index.html` to `/` on
`prs-reader-web.dstoc.workers.dev`. It requests `/` directly after that response
and still requires a final HTTP 200 with the expected MIME type. It does not
follow other redirects. This rejects Access login redirects, cross-origin
destinations, redirects to other paths, and redirects from non-HTML assets.
When an asset check fails, the probe prints the asset path, request URL, HTTP
status, `Location`, effective URL, `Content-Type`, curl exit code, and selected
response headers such as `CF-Ray` and `CF-Cache-Status`. It omits response
bodies, cookies, and request headers. It removes Location query strings,
fragments, and user information so the diagnostic output does not include
credentials. The production smoke tests run without the Cloudflare deploy
secrets.

The API CORS binding is `PRS_READER_WEB_ORIGIN`. Production sets it to
`https://prs-reader-web.dstoc.workers.dev` in
`crates/prs-cloudflare/wrangler.toml` under `[env.production.vars]`. The API
deployment reads that Wrangler variable and installs it in the API Worker's
runtime configuration. A GitHub Actions variable alone does not change the
Worker runtime value. The production API deployment gate watches this file,
so a source change deploys the API Worker. CORS remains exact-origin and is
limited to reader authorization, polling, manifest, and bundle routes.
`/a/*` approval routes remain on `prs-reader` behind the existing Cloudflare
Access policy and stay outside API CORS.

The deployment token must be able to publish both existing Workers. Keep the
current token in the protected `production` environment. Do not add a new
secret or put a token value in Wrangler configuration. The existing
`PRS_READER_URL` GitHub variable still identifies the API Worker for health
checks; no `PRS_READER_WEB_ORIGIN` GitHub variable is required.

To redeploy, rerun the successful `CI` workflow run for the intended `main`
commit. The deployment workflow uses that run's exact SHA. To roll back the
browser site, install the pinned Wrangler version and run
`wrangler rollback --config web/reader-web/wrangler.toml <VERSION_ID>` from the
repository root, or use the Worker's Deployments page in Cloudflare. To
re-publish the current source, rerun the successful CI deployment run for its
commit. A rollback restores the previous static asset version and does not
change the API Worker or its bindings.

The workflow verifies asset responses and the exact-origin authorization
preflight. A complete authorization and sync still needs an interactive
browser session: sign in through the existing Access approval page and approve
a request for an authorized reader account. CI does not have that user session
or the account data needed to complete this manual end-to-end step.

## Debug a production deployment

The deployment workflow uses the GitHub Actions `runner.debug` context. A
normal run does not enable Wrangler debug logging. A run with GitHub Actions
debug logging enabled sets `WRANGLER_LOG=debug` only for the deployment job.
The workflow also sets `WRANGLER_LOG_SANITIZE=true` for the deploy command.

To collect sanitized Wrangler details from a failed production deployment:

1. Open the failed run for **PRSync Cloudflare production deployment** in the
   GitHub Actions tab.
2. Select **Re-run jobs** and enable **Enable debug logging**. Re-run the
   deployment job. Enable debug logging on this production workflow run, not
   only on the upstream `CI` run.
3. Open the **Upload and promote production version, then verify schema readiness**
   step in the new run.
4. Review the Wrangler `debug` entries for the request endpoint, HTTP status,
   and sanitized Cloudflare API error. Wrangler omits request headers and
   other sensitive request data while sanitization is enabled.

Do not set `WRANGLER_LOG_SANITIZE=false`. Do not copy authorization headers,
API tokens, cookies, or other secret values into an issue, artifact, summary,
or comment. Record only the sanitized endpoint, status, error code, and
request reference that identify the failing bindings, routes, or services
metadata request.

Apply schema migrations separately with the operator procedure before the
protected deployment. Version publishing must not list, query, or mutate D1
through Wrangler, and it must not create or replace production resources. It
must not deploy routes, custom domains, or triggers.

The first protected deployment and the live token rotation test require
operator access. Do not record token values in workflow logs, artifacts,
summaries, comments, or repository files.
