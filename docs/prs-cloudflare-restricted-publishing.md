# Restricted PRSync publishing verification

This runbook is the manual follow-up for `sony-prs/114`. It verifies that a
versioned Worker publish succeeds with a deployment token that has no D1 API
permissions. It does not create resources and it does not apply migrations.

Run the local command-contract test before this procedure:

```sh
python3 tools/test-prs-cloudflare-publishing.py
```

Do not record token values, cookies, request bodies that contain secrets, or
operator credentials in the issue or in command output.

## Preconditions

Use a disposable or production-equivalent account and pre-existing resources:

- Worker: `prs-reader`.
- D1 database: `prs-reader-db`, with its checked-in UUID in
  `crates/prs-cloudflare/wrangler.toml`.
- Private R2 bucket: `prs-reader-documents`.
- Two separate tokens: an operator token for D1 migration management and a
  deployment token for Worker publishing.

Install and verify the pinned Wrangler version:

```sh
npm install --global wrangler@4.135.0
wrangler --version
```

Expected result: the version output contains `4.135.0`. Stop if Wrangler is
missing or has another version.

The deployment token must have the Worker publish permission required by the
account and no D1 or R2 management permission. Record the permission names
and the account scope, including any `Workers Scripts` authority, but not the
token value.

## Apply and verify migrations with operator authority

Set `CLOUDFLARE_ACCOUNT_ID` and `CLOUDFLARE_API_TOKEN` in the shell to the
operator values. Keep those values outside the repository and do not reuse the
shell for the restricted deployment test without replacing the token.

```sh
export CLOUDFLARE_ACCOUNT_ID="$ACCOUNT_ID"
export CLOUDFLARE_API_TOKEN="$OPERATOR_TOKEN"
```

Run the checked-in operator command:

```sh
tools/prs-cloudflare-migrate.sh --production
```

Expected result:

- Wrangler lists the existing `prs-reader-db` migration history.
- Wrangler applies only pending checked-in migrations with
  `--no-x-provision`.
- The final history has no pending migrations.
- The final migration ID and filename match
  `crates/prs-cloudflare/src/schema.rs`.

The migration apply operation is:

```text
wrangler d1 migrations apply prs-reader-db --remote --env production --no-x-provision
```

If the command is interrupted or returns an error, do not publish. Run the
operator migration command again only after inspecting the D1 state and
Wrangler history. The retry must either resume from the recorded history or
stop with a clear migration error. Do not delete history rows or edit an
applied migration.

For a code-only release, confirm that the release does not change
`crates/prs-cloudflare/migrations/`. The migration history must remain
unchanged, and the deployment still requires the release-aware `/ready` check.
Do not apply a migration because a code-only release was published.

For a schema-dependent release, compare the release requirement with the
history before publishing. Apply and verify compatible migrations first. Stop
when a required migration is missing, the history is interrupted, a migration
name does not match, or the code and schema are incompatible. An undeclared
destructive or otherwise incompatible future migration is not a valid reason
to publish an older Worker.

## Publish with restricted authority

Replace the shell token with the deployment token. Keep the account ID and
token in environment variables that are not printed by the shell:

```sh
export CLOUDFLARE_ACCOUNT_ID="$ACCOUNT_ID"
export CLOUDFLARE_API_TOKEN="$DEPLOY_TOKEN"
export PRS_READER_URL='https://reader.example.com'
tools/prs-cloudflare-deploy.sh --production
```

Expected result:

- The script derives the version tag from the checked-out commit SHA.
- The only Wrangler management operations are
  `wrangler versions upload --env production --tag <commit-sha> --no-x-provision`
  and
  `wrangler versions deploy --env production --version-tag <commit-sha>@100% --yes --no-x-provision`.
- The upload creates a version without changing live traffic.
- The promotion sends 100% of traffic to that exact uploaded version.
- The command does not create a D1 database or R2 bucket.
- The command does not list or apply D1 migrations.
- The command does not deploy routes, custom domains, or triggers.
- The post-publish readiness check returns HTTP 200 with `status` `ready` and
  the exact schema requirement from this checkout.

The same read-only check can be repeated without publishing:

```text
tools/prs-cloudflare-readiness.sh https://reader.example.com
```

If `PRS_READER_URL` is absent, or if Wrangler is missing or has the wrong
version, the command must stop before publishing. A code-only release follows
the same command and expected result.

## Prove direct management denial

With the deployment token still active, run read-only management requests. Do
not use a create command for this negative test.

```sh
wrangler d1 list
wrangler d1 migrations list prs-reader-db --remote --env production
wrangler r2 bucket list
```

Expected result: each command is denied because the deployment token has no
D1 or R2 management permission. If any command succeeds, stop the verification
and record that the token has broader authority than intended. Do not record
the token value.

## Check request paths and readiness

Set `BASE_URL` to the published Worker URL and run:

```sh
curl --fail-with-body --silent --show-error "$BASE_URL/health"
curl --fail-with-body --silent --show-error "$BASE_URL/ready"
curl --include --silent --show-error "$BASE_URL/api/v1/reader/manifest"
curl --include --silent --show-error "$BASE_URL/api/v1/reader/bundle"
curl --include --silent --show-error \
  --request PUT "$BASE_URL/api/v1/sender/bundle"
```

Expected result:

- `/health` returns HTTP 200.
- `/ready` returns HTTP 200, `status` `ready`, and this release's schema
  requirement.
- The unauthenticated reader and sender paths return HTTP 401. They must not
  run a migration.
- The readiness response is read-only. It must not apply a migration.

Before and after these requests, use the operator credential to list the D1
migration history. The two histories must be identical. The local tests also
check the public, sender, reader, and readiness handler paths for migration
calls.

## Check binding-change authority

`--no-x-provision` disables Wrangler automatic resource provisioning. It does
not by itself make Worker binding configuration immutable. Check the remaining
platform authority in a disposable Worker before using a production resource:

1. Record the deployment token's effective account permissions and scope.
2. Prepare two pre-existing disposable D1 databases and two pre-existing
   private R2 buckets.
3. Publish the Worker with resource A in its bindings and confirm readiness.
4. Change only the checked-in binding identifiers to resource B. Do not add a
   resource-creation command.
5. Run the restricted deploy command again.
6. Record whether the publish is allowed to change the Worker binding from A
   to B. Restore the original binding after the test.

Expected result: the restricted token can publish the Worker without creating
or migrating a resource. Record whether its Worker publish permission also
allows a binding change. If the binding change succeeds, that is the remaining
Cloudflare platform limitation: the repository can prevent automatic resource
provisioning and separate D1 migration authority, but the platform may not
provide a narrower permission that publishes code while making binding changes
impossible. If the binding change is denied, record the denial and its
permission name.

Do not treat a successful `/ready` response as proof that the binding is
authorized or that the database schema was changed by deployment. `/ready`
only reads migration history through the Worker `DB` binding.
