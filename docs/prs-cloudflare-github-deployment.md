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
environment. The URL must point to the deployed Worker.

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
read-only release-aware `/ready` endpoint. The workflow then checks `/health`.
A failed build, upload, promotion, readiness check, or health check fails the
deployment job. A rerun promotes the ID from its new upload, even when an
earlier upload used the same commit tag.

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
