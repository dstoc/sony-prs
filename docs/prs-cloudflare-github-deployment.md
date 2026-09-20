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

That command publishes the Worker with the existing D1 and R2 bindings. It
passes `--no-x-provision` to Wrangler and does not run D1 migrations. The
command also checks the read-only release-aware `/ready` endpoint. The
workflow then checks `/health`. A failed build, publish, readiness check, or
health check fails the deployment job.

Apply schema migrations separately with the operator procedure before the
protected deployment. Ordinary publishing must not list, query, or mutate D1
through Wrangler, and it must not create or replace production resources.

The first protected deployment and the live token rotation test require
operator access. Do not record token values in workflow logs, artifacts,
summaries, comments, or repository files.
