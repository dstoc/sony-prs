# PRSync

PRSync is a document-delivery system for the Sony PRS-T1 Markdown reader.

Its purpose is to make the PRS-T1 behave like a distraction-free, offline-first Markdown appliance:

1. Send a Markdown bundle from a trusted computer.
2. Store the current bundle in a small hosted inbox.
3. Boot the PRS-T1.
4. Authorize that boot from another trusted device by scanning a QR code.
5. Download the current bundle.
6. Read entirely offline.
7. Lose the downloaded bundle and reader credentials when the PRS-T1 reboots.

PRSync is intentionally not a general filesystem synchronization system, cloud drive, feed reader, or continuously connected application.

## Goals

PRSync should provide:

- a simple CLI for publishing a Markdown bundle and its images;
- a small hosted inbox containing one current bundle;
- secure human authorization of a PRS-T1 after each boot;
- no persistent reader credential;
- no persistent plaintext synchronized documents on the reader;
- outbound-only networking from the PRS-T1;
- offline reading after synchronization;
- a simple filesystem boundary between synchronization and the Markdown reader;
- reproducible CI and Cloudflare deployment;
- a protocol that can be shared by the CLI, Worker, and device client.

The normal reader experience should remain quiet.

Network activity exists to get documents onto the reader. It should not turn the reader into a continuously connected application.

## Non-goals

The first version does not need:

- bidirectional filesystem synchronization;
- editing documents on the PRS-T1;
- uploading content from the PRS-T1;
- cross-device reading-position synchronization;
- push notifications;
- permanent reader registration;
- permanent reader credentials;
- persistent plaintext document storage on the reader;
- account management on the PRS-T1;
- multi-user sharing or delegated approval;
- general cloud storage;
- real-time synchronization;
- remote deletion of files from an offline reader;
- Wi-Fi credential provisioning or network configuration on the PRS-T1;
- protection of documents while an already-authorized reader remains powered on.

These can be reconsidered later without being requirements for the initial system.

## User model

The initial PRSync deployment is single-user.

One human owner controls one hosted inbox and is the only human authorized to
approve sender and reader authorization requests. The owner may use multiple
trusted sender computers and may authorize multiple reader boots, but the
inbox is not shared with other people and approval authority is not delegated.

## System overview

PRSync consists of five main parts:

    +------------------+
    |    prs-send      |
    |    Rust CLI      |
    +--------+---------+
             |
             | authenticated upload
             v
    +--------------------------+
    |   Cloudflare Worker      |
    |                          |
    |   protocol API           |
    |   authorization API      |
    |   approval UI            |
    +----------+---------------+
               |
          +----+----+
          |         |
          v         v
         D1         R2
      metadata    bundles

               ^
               |
               | boot-scoped authorized download
               |
    +----------+---------------+
    |       PRS-T1             |
    |                          |
    |   authorization client   |
    |   QR UI                  |
    |   synchronization        |
    |   tmpfs bundle           |
    |   Markdown reader        |
    +--------------------------+

The Cloudflare Worker is the only component that directly accesses D1 or R2.

Neither client receives Cloudflare account credentials or storage credentials.

The human approval application is part of the Worker. It displays the
non-secret context for sender and reader requests and provides separate approve
and deny actions. The approval URL contains only the public request ID. It does
not authenticate the human.

Approval and denial require the authenticated Access context, a same-origin
`Origin` header, and a CSRF token bound to the request and Access context.
Approval responses prohibit framing and restrict form submissions to the same
origin.

The approval hostname is protected by Cloudflare Access. The Worker requires
the runtime `ctx.access` context for direct approval requests. Cloudflare
Access applies the human-owner policy before the Worker runs. The Worker does
not read an Access assertion header, parse JWTs, fetch JWKS documents, or keep a
second D1 owner-identity policy. Local integration tests may use the explicit
`local-test` feature and a test-only owner header when
`PRS_ENVIRONMENT=local`.

## Components

### `prs-sync-protocol`

A shared Rust crate defines the wire protocol and common identifiers.

It should contain protocol-level types such as:

- manifests;
- authorization requests;
- authorization status;
- session information;
- API errors;
- protocol versions.

It must not contain Cloudflare-, filesystem-, UI-, or PRS-T1-specific behaviour.

The crate must compile for both native Rust targets and the Cloudflare Worker WebAssembly target.

### `prs-send`

`prs-send` is the trusted sender.

It runs on a normal computer and is allowed to modify the hosted inbox.

Expected commands include:

    prs-send credentials create --name NAME
    prs-send push ENTRYPOINT [FILE ...]
    prs-send clear
    prs-send credentials list
    prs-send credentials revoke NAME

`prs-send` does not persist sender credentials automatically. The user may
store the printed credential externally, such as in the
`PRSYNC_SENDER_TOKEN` environment variable, for later commands.

The CLI reads the Worker base URL from `PRSYNC_URL`. It reads the sender
bearer token from `PRSYNC_SENDER_TOKEN` for push, clear, list, and revoke.
The create command does not require a sender token because it uses the human
approval flow.

For example:

    export PRSYNC_URL=https://sync.example.com
    prs-send credentials create --name laptop
    export PRSYNC_SENDER_TOKEN='the-token-printed-by-create'
    prs-send push docs/index.md docs/images/diagram.png

The create command writes the bearer token only to standard output. It writes
the approval URL and progress messages to standard error. The other commands
write metadata or status messages and never print a bearer token.

The owner may use multiple trusted sender computers. Each installation may hold
its own named sender credential, but all sender credentials belong to the same
owner. `--name` is mandatory for `prs-send credentials create`; the owner
supplies a name for each credential, such as `laptop` or `desktop`. Names must
be unique among active credentials, but may be reused after a credential is
revoked. The command must fail if the name is omitted or already belongs to an
active credential.

That credential is specific to PRSync. It must not be a Cloudflare API token, R2 credential, D1 credential, or other infrastructure credential.

`prs-send push` accepts an explicit list of files. The first path must be a
Markdown file and becomes the bundle's default entry point. Every subsequent
path is included in the bundle as an additional Markdown file or resource.
The CLI computes the common parent directory of the supplied files and strips
that directory from each archive path while preserving the remaining directory
structure. For example, supplying `docs/index.md`, `docs/chapters/one.md`, and
`docs/images/diagram.png` produces `index.md`, `chapters/one.md`, and
`images/diagram.png` in the bundle. The first entry point is recorded using its
resulting bundle path.

The CLI generates `manifest.json`; it is not supplied as one of the input files.

The CLI does not discover or include unlisted files, and does not validate
whether links resolve to supplied files. Absolute archive paths, `..`
traversal, duplicate bundle paths, and other unsafe paths must be rejected.

### Cloudflare Worker

The Worker implements the hosted service.

Responsibilities include:

- sender authorization;
- reader authorization;
- human approval;
- bundle upload;
- inbox manifests;
- document download;
- authentication and authorization;
- metadata management;
- storage access.

The Worker should be written in Rust using the Cloudflare Workers Rust environment.

### D1

D1 stores small structured state.

Expected state includes:

- current bundle metadata;
- inbox revisions;
- pending authorization requests;
- hashed authorization secrets;
- reader sessions;
- sender credentials;
- sender credential revocation state.

D1 does not store bundle contents.

### R2

R2 stores immutable bundle objects.

The R2 bucket remains private.

All object access occurs through the Worker.

A reader must only be able to retrieve the bundle object referenced by the inbox
visible to its current session.

### PRS-T1 synchronization client

The synchronization client runs on the PRS-T1.

Its responsibilities include:

- HTTPS communication;
- boot-scoped authorization;
- QR generation;
- waiting for authorization;
- fetching the inbox manifest;
- downloading the current bundle;
- validating paths, sizes, and limits;
- extracting documents into temporary storage.

It should remain separate from Markdown parsing and rendering.

### Markdown reader

The Markdown reader consumes an ordinary filesystem tree.

It does not know:

- how authorization works;
- where documents came from;
- whether Cloudflare is involved;
- how bundles are transported.

Its input is simply a directory containing documents and resources.

This boundary is intentional:

    network + authorization + synchronization
                    |
                    v
               filesystem
                    |
                    v
             Markdown reader

## Document model

A synchronized document is an immutable bundle.

The bundle is represented as an uncompressed tar archive. It contains a
`manifest.json` generated by `prs-send` at its root, followed by the entry
point and explicitly supplied files. The tar archive is extracted as a stream
so the PRS-T1 does not need random access to the complete bundle.

A bundle has one Markdown entry point and may contain additional resources.

For example:

    manifest.json
    index.md
    chapters/
        details.md
    images/
        diagram.png
        photo.jpg

The bundle manifest identifies:

- bundle format version;
- Markdown entry point;
- contained files;
- sizes; and
- optional per-file lowercase SHA-256 digests (required by browser sync).

Relative links within the bundle should continue to work using normal filesystem semantics.

This includes:

- Markdown-to-Markdown links;
- images;
- other supported local resources.

## Bundle safety

A bundle is untrusted input when received by either the service or device.

Bundle validation must prevent:

- absolute paths;
- `..` path traversal;
- symlinks, hard links, device files, FIFOs, and other special entries;
- duplicate output paths;
- writing outside the destination directory;
- oversized archives;
- excessive extracted size;
- unsupported bundle versions.

Extraction should be bounded and preferably streaming.

The PRS-T1 must not need to hold an entire bundle in RAM before extracting it.

Production HTTPS provides transport integrity. New bundles include per-file
SHA-256 digests, and the shared validator checks any digest that is present.
Browser sync requires a digest for every file and fails closed on legacy
manifests without hashes. Existing native readers continue to accept legacy
bundles that omit the optional hash field.

## Inbox model

The hosted service exposes a logical inbox containing at most one currently
published bundle plus a monotonically changing revision identifier.

A reader can request the manifest and determine whether anything has changed.

The protocol should support conditional requests using a revision or HTTP ETag so an unchanged inbox is inexpensive to check.

Publishing a new bundle first deletes the current R2 object and clears the
current inbox reference. The new bundle is then stored and becomes the current
inbox object. If storing the replacement fails, the inbox remains empty and a
later push is required to repopulate it.

The sender CLI validates the bundle before upload, but the Worker validates the
bundle only after the current object has been deleted. An invalid upload is
therefore rejected and leaves the inbox empty.

Concurrent pushes require no conflict handling. The last push to successfully
publish a replacement wins.

The sender may also clear the current bundle, leaving the inbox empty.

Storage objects are immutable while they are current. A replaced object is
deleted before the replacement is stored. Cleanup of abandoned replacement
objects is a separate storage-retention concern.

A reader holding a manifest for a deleted object may fail to retrieve it. The
reader treats this as a failed synchronization, keeps its existing local
bundle, and fetches a fresh manifest on the next attempt.

## Sender authorization

The sender CLI uses a human-approved authorization flow.

The CLI creates a pending sender authorization request and receives an approval URL.

The user opens the URL in a normal browser and authenticates to the human-facing service.

After approval, the CLI receives a sender-scoped credential.

The CLI retains a high-entropy polling secret in memory while waiting for
approval. After the human approves the request, the CLI polls with that secret
and claims the sender credential. The service returns the credential only to a
successful polling request and marks the authorization request consumed.

The CLI prints the resulting sender bearer credential once to standard output
and does not persist it. Progress and other non-secret status messages must be
written to standard error so the credential can be captured separately.

The sender credential:

- may upload a bundle;
- may replace the current bundle;
- may clear the inbox;
- may list named sender credentials and their metadata;
- may revoke a named sender credential;
- may be revoked.

Sender-credential management is metadata-only. A sender credential is
write-only with respect to bundle content: it must not be able to fetch the
current manifest, current bundle, or historical bundle objects. An upload
response may acknowledge success and return upload metadata, but must not
return hosted content.

Any active sender credential may list and revoke any sender credential. This is
an owner-level management capability within the single-user deployment, but it
does not grant access to bundle content.

Sender credentials do not expire automatically. They remain valid until
explicitly revoked.

Credential listing returns names and management metadata, such as credential
identifier, creation time, last-use time, and revocation state. It never
returns bearer credentials. A credential name is immutable after creation, and
the stable credential identifier distinguishes historical credentials when a
name is reused. Revocation takes effect for subsequent requests,
including requests made with the revoked credential itself.

The sender credential must not grant reader authorization or infrastructure access.

The browser involved in approval must never receive the resulting sender bearer credential.

## Reader trust model

The PRS-T1 must be treated as an untrusted endpoint.

The device is rooted and may be physically lost.

Anything stored persistently on it must therefore be assumed recoverable by an attacker.

The first version solves this by storing neither synchronized plaintext documents nor reusable reader credentials persistently.

The security boundary is a boot session.

## Reader authorization

Every reader boot requires fresh authorization.

The intended flow is:

    PRS-T1 boots
         |
         | generate fresh polling secret
         v
    create pending reader authorization
         |
         | receive approval URL
         v
    render approval URL as QR code
         |
         | scan with trusted phone/computer
         v
    human authenticates and approves
         |
         v
    PRS-T1 polling request succeeds
         |
         | receive read-only session for this boot
         v
    synchronize the current bundle

The reader stores the resulting session credential only in RAM. The claim
response includes the server expiry time.

Rebooting the reader destroys the credential and requires another approval.

## QR-code security

The QR code is a convenience mechanism for locating a pending authorization request.

It is not itself an authorization credential.

The QR must not contain:

- the reader polling secret;
- the resulting reader bearer token;
- a private key;
- a decryption key;
- any persistent credential.

It should contain only an approval URL or non-secret pending-request identifier.

For example:

    https://reader.example/a/Ab3X9k

Anyone may be able to see or photograph this URL.

That must not be sufficient to approve the request.

Approval requires authentication as the configured human owner.

The PRS-T1 separately retains a high-entropy polling secret that is required to claim the approved session.

## Human authorization

Human-facing approval should be protected independently from the machine API.

A useful deployment split is:

    sync.example.com

for protocol endpoints used by `prs-send` and the PRS-T1, and:

    reader.example.com

for authenticated human approval. The human-facing hostname is protected by
Cloudflare Access for both sender and reader authorization.

The protocol hostname cannot require interactive Cloudflare Access authentication because unauthenticated clients must be able to initiate authorization.

Cloudflare Access authenticates the human owner and its application policy
restricts that application to the owner. The Worker must require the
authenticated Access context before it approves or denies a request. Access
authentication does not replace the PRSync authorization request,
polling-secret, or client-credential checks.

## Reader session scope

A reader session is read-only.

It may:

- fetch the current inbox manifest;
- retrieve the current bundle referenced by that manifest.

It may not:

- upload a bundle;
- replace the current bundle;
- clear the inbox;
- create sender credentials;
- administer the service.

The reader session is boot-scoped and remains valid while the reader is powered
on, until the server-side session deadline. The default server deadline is 30
days from issuance. It does not require periodic reauthorization before that
deadline. Rebooting the reader destroys the session credential in RAM and
requires fresh authorization.

The reader client must check the expiry time before a synchronization attempt.
At the deadline, or after an authorization failure from the service, it must
discard the session token and restart reader authorization. It must not retry
an expired token or persist it for recovery after reboot.

## Temporary document storage

The current bundle is stored in a RAM-backed filesystem on the PRS-T1.

For example:

    /mnt/prs-reader/
        library/
            index.md
            chapters/
                details.md
            images/
                diagram.png

The exact mount path is an implementation detail.

The temporary filesystem must have an explicit device-specific size limit.

Synchronization must also enforce:

- a maximum downloaded bundle size of 16 MiB;

The 16 MiB bundle limit is a fixed protocol constant enforced by the Worker and
PRS-T1 client. The reader independently enforces its configured tmpfs capacity.

The bundle disappears when the reader reboots.

If the reader application itself restarts without an OS reboot, the mounted library may remain available.

That is desirable.

## Offline behaviour

After successful authorization and synchronization, normal reading must not depend on the network.

The expected flow is:

    authorize
        |
        v
    synchronize
        |
        v
    disable Wi-Fi
        |
        v
    read locally

Wi-Fi is enabled only for a synchronization attempt, including any authorization
needed for that attempt, then disabled again when the attempt finishes,
including after a failure. Normal reading and waiting between synchronization
triggers occur with Wi-Fi off.

The reader should not maintain an idle WebSocket, MQTT connection, or other persistent network channel.

The service is a document-delivery mechanism, not part of the active reading path.

## Synchronization triggers

The reader checks for the current bundle at explicit times:

- during boot, after reader authorization;
- asynchronously after the device has been idle for a configured period
  (15 minutes by default);
- when the user selects `Sync now` from the details/settings page.

For each check, the reader enables Wi-Fi and attempts to connect using its
existing configuration. Idle and manual synchronization occur only when the
reader has an authorized session. Wi-Fi is disabled after the attempt.
Synchronization must not interrupt active reading. If the reader already has
the current bundle, the check performs no download.

## Lost-device security

### Device lost while powered off

The expected state is:

- no reusable reader credential;
- no synchronized plaintext document library;
- no authorization secret from the previous boot.

The attacker must not be able to authorize a new boot without access to the human approval identity.

### Device lost while powered on and authorized

The attacker may be able to read documents currently present in tmpfs.

This is accepted in the initial threat model.

Remote revocation cannot reliably remove information already present in RAM while the device is offline.

The security guarantee is therefore:

**authorization protects a boot session, not physical possession of an already-unlocked reader.**

This is similar to the distinction between a locked and already-unlocked computer.

## Secrets

### Allowed on the sender computer

The user may persist the PRSync sender bearer credential outside the CLI, such
as in an environment variable or operating-system secret store.

### Allowed in PRS-T1 RAM

The reader may temporarily hold:

- authorization polling secrets;
- reader session credentials;
- downloaded plaintext documents.

### Not allowed in persistent PRS-T1 storage

Do not persist:

- sender credentials;
- reader credentials;
- Cloudflare credentials;
- authorization polling secrets;
- synchronized plaintext document bundles.

### Not allowed in the repository

Do not commit:

- Cloudflare API tokens;
- production bearer credentials;
- Access secrets;
- other private deployment credentials.

## Server credential storage

Where practical, bearer credentials should be stored server-side as hashes rather than plaintext.

Authentication must distinguish at least:

- pending authorization polling capability;
- sender capability;
- reader capability;
- authenticated human approval.

Possession of one capability must not imply another.

## Authorization request lifetime

Authorization requests have an absolute expiry deadline. Both pending and
approved-but-unclaimed requests expire at that deadline. Approval does not
extend the request lifetime. The deadline is exclusive: a request is expired
when the service time is equal to or later than `expires_at`.

A request must have explicit states such as:

- pending;
- approved;
- denied;
- expired;
- consumed.

An approved session or sender credential may only be claimed by the client that possesses the corresponding high-entropy polling secret.

The service must reject:

- expired requests;
- invalid polling credentials;
- replay after consumption;
- attempts to exchange one authorization kind for another.

## Network model

The PRS-T1 is not assumed to be directly reachable from the sender or service.

All device networking is outbound.

This allows operation through:

- NAT;
- consumer Wi-Fi;
- guest networks;
- firewalls that permit normal HTTPS traffic.

The architecture does not require inbound connections to the device.

PRSync assumes that the PRS-T1 already has usable Wi-Fi configuration and
stored network credentials. Provisioning and managing those credentials are
outside the scope of PRSync.

## HTTPS

Production communication must use HTTPS.

Before implementing the complete PRS-T1 client, the project must validate a modern TLS stack on the actual ARMv5 device and existing static Rust build target.

The selected implementation must support:

- DNS;
- certificate validation;
- modern Cloudflare HTTPS;
- bounded HTTP requests and responses;
- Wi-Fi reconnect.

The protocol must not depend on obsolete TLS functionality from the original Sony userspace.

## Cloudflare deployment

The production hosted service is expected to use:

- a Rust Cloudflare Worker;
- D1 for metadata and authorization state;
- R2 for immutable bundle objects;
- Cloudflare Access for the human approval application.

These resources should use consistent names such as:

    prs-reader
    prs-reader-db
    prs-reader-documents

The exact names and domains are deployment configuration rather than protocol requirements.

The Worker configuration in the repository should define the relationships between these resources.

Production R2 must remain private.

## Cloudflare resource ownership

The Worker is the logical application root.

D1 and R2 remain separate Cloudflare resources but are associated with the service through Worker bindings and repository configuration.

The repository configuration should be treated as the source of truth for those relationships.

Production resource creation is a deliberate bootstrap operation.

Normal application deployment must not create new D1 databases or R2 buckets.

## CI

Pull-request CI must not require production credentials.

CI should validate:

- Rust formatting;
- host unit tests;
- protocol serialization tests;
- document bundle tests;
- sender CLI tests;
- Worker WebAssembly builds;
- Worker configuration;
- local D1 migrations;
- local Worker/D1/R2 integration tests;
- PRS-T1 cross-builds where applicable.

Cloudflare integration tests should use local/emulated resources rather than production infrastructure.

## Deployment

Production Worker deployment should occur after changes reach `main` and required checks pass.

Production deployment credentials are stored in protected GitHub secrets or an equivalent protected deployment environment.

The repository pins Wrangler `4.135.0` for these operations. Bootstrap and
migration require an operator credential with D1 and R2 management permission.
Worker publishing requires a separate credential with Worker publish
permission only. The publishing credential must not have D1 or R2 management
permission.

Expected deployment secrets include:

    CLOUDFLARE_ACCOUNT_ID
    CLOUDFLARE_API_TOKEN

The deployment API token should have only the permission needed to publish the
Worker. It must not have D1 or R2 management permission. The operator
migration command uses a separate protected credential.

Deployment should follow this order:

1. review each checked-in migration for compatibility with the old Worker;
2. apply compatible migrations with the operator credential;
3. verify the complete Wrangler history and database integrity;
4. upload a Worker version with the restricted deployment credential. The
   version uses the existing `DB` and `BUNDLES` bindings and
   `--no-x-provision`;
5. promote that exact version to 100% of traffic. The versioned release path
   does not deploy routes, custom domains, or triggers;
6. inspect `/ready` through the Worker D1 binding and compare its schema
   requirement with the release being published;
7. serve traffic only when the new release reports `ready`.

The readiness endpoint is read-only. It must not use D1 API credentials or run
migrations. After promotion, the deployment polls `/ready` for a finite
rollout window. It retries transport failures, invalid JSON, and responses from
the previous release. When version metadata is available, it requires the
exact promoted Version ID before it accepts readiness. `/health` only confirms
that the Worker responds and runs after this release-aware readiness check.

Prefer additive schema changes. Defer destructive cleanup until old Worker
releases no longer need the affected schema. If publication fails after a
compatible migration, restore the last Worker release and keep the additive
schema. If a migration is incomplete or destructive, stop traffic, inspect
D1 history and foreign-key integrity, and recover with the operator credential
before publishing the matching Worker. Never delete migration-history rows or
edit an applied migration.

An older Worker accepts a future migration only when its release explicitly
lists the exact migration ID and filename in `COMPATIBLE_FUTURE_MIGRATIONS`.
The numeric filename prefix does not establish compatibility. A correctly
numbered but undeclared destructive migration makes the older Worker not
ready. Deploy the compatibility declaration before applying an additive
migration. Apply and verify the migration before deploying the release that
requires it. If a destructive migration is necessary, drain old Workers and
complete the recovery plan before applying it.

Production deployment must not expose Cloudflare credentials to pull-request jobs.

The restricted-token deployment verification uses the reproducible procedure in
[`docs/prs-cloudflare-restricted-publishing.md`](prs-cloudflare-restricted-publishing.md).

## Local development

The hosted service should be runnable locally without a production Cloudflare account.

Local development should support:

- a local Worker;
- local D1;
- local R2-compatible bindings;
- schema migrations;
- complete protocol integration tests.

Human authentication should be isolated behind a clear Access-context
boundary so tests can supply an authenticated test identity without weakening
production authentication.

In production, Cloudflare Access supplies the authenticated context on the
human-facing approval hostname. Local tests may inject an authenticated test
identity at the same boundary without weakening the production path.

## Failure handling

Network failures are expected.

The device client must handle:

- inability to create an authorization request;
- network loss while waiting for approval;
- authorization expiry;
- invalidated reader sessions;
- interrupted downloads;
- corrupt downloads;
- insufficient tmpfs capacity;
- invalid bundles;
- server errors.

A partially downloaded or partially extracted bundle must never become visible as
the current library state.

Download into staging storage, verify it, and expose it atomically.

## Synchronization semantics

Synchronization occurs at explicit boundaries rather than continuously changing the library while the user reads.

A synchronization operation should conceptually:

1. fetch one manifest revision;
2. determine the current bundle for that revision;
3. do nothing if the reader already has that bundle;
4. otherwise obtain the current bundle, or prepare an empty state if the inbox is empty;
5. verify and extract the replacement bundle into staging storage;
6. atomically replace the reader's current bundle state;
7. notify the reader that a new bundle state is available.

The current bundle should not silently change underneath the reader while a
document is open. The exact UI behaviour for a newly available bundle can be
decided by the reader integration.

An idle synchronization may commit a verified replacement immediately because
the reader is not being used. Replacement remains atomic and must not interrupt
an active reading session.

A failed synchronization leaves the current bundle unchanged. The reader does
not retry automatically; another boot, idle period, or manual `Sync now` action
starts the next attempt.

Synchronization failures are recorded on the details/settings page. The reader
does not interrupt reading with a failure notification; status is shown while
an attempt is actively running.

## Reader user experience

The synchronization system should remain subordinate to reading.

A normal boot may look like:

    Connect to Wi-Fi

         ↓

    +----------------------+
    |                      |
    |      [ QR CODE ]     |
    |                      |
    |   Scan to unlock     |
    |                      |
    +----------------------+

         ↓

    approve on phone

         ↓

    Synchronizing current bundle…

         ↓

    current document

The normal reading interface should not contain persistent connectivity indicators, notification feeds, badges, recommendations, or other attention-oriented UI.

A small status indication is appropriate while authorization or synchronization is actively occurring.

## Library model

The synchronized library contains one current bundle.

At minimum the device should support:

- open the current bundle's Markdown entry point;
- navigate linked Markdown files within its bundle;
- display bundled images;
- return to the bundle entry point;
- reopen the last document during the current boot.

The reader does not need cloud-specific concepts such as R2 object IDs or synchronization revisions in its normal UI.

## Manual operations

Some parts of PRSync cannot be completely automated because they establish external trust or operate physical hardware.

Expected manual operations include:

- creating/selecting the production Cloudflare resources;
- selecting production hostnames;
- configuring Cloudflare Access;
- selecting the allowed human identity;
- creating a least-privilege Cloudflare deployment API token;
- storing deployment credentials in GitHub repository/environment secrets;
- validating HTTPS on a physical PRS-T1;
- validating tmpfs behaviour on the physical device;
- testing QR scanning with a phone;
- validating the complete production workflow.

These steps should be documented, repeatable, and minimized.

They must not be hidden inside otherwise automated development tasks.

## Repository structure

The intended implementation may eventually resemble:

    crates/
        prs-sync-protocol/
        prs-sync-bundle/
        prs-send/
        prs-cloudflare/
        prs-t1-sync/
        prs-markdown/
        prs-t1-agent/

The exact split may change when implementation experience shows that two small crates should be combined or a component deserves further separation.

The architectural boundaries are more important than the exact crate count.

In particular:

- Cloudflare code must not leak into protocol code;
- synchronization must not leak into Markdown parsing/rendering;
- the PRS-T1 reader should consume files rather than cloud objects;
- the sender must not know how the reader renders documents.

## Work tracking

Implementation work is tracked with `PRSYNC-*` identifiers.

Issues should refer to this document for system-level background instead of redefining the architecture independently.

An implementation issue may refine details within its component, but changes to these system-level guarantees should update this document.

Important guarantees include:

- no reusable persistent reader credential;
- no synchronized plaintext library after reboot;
- human approval for each reader boot;
- QR codes contain no reader credential;
- reader sessions are read-only;
- the Worker mediates D1 and R2;
- R2 is private;
- normal reading works offline;
- the Markdown renderer is independent of synchronization transport;
- production credentials are never required by pull-request CI.

## Initial implementation sequence

The intended development order is:

1. agree on this architecture and security model;
2. implement the shared protocol;
3. define the document bundle;
4. establish the Rust Worker and local Cloudflare environment;
5. establish D1 and R2 persistence;
6. implement authorization;
7. implement the inbox API;
8. implement the sender CLI;
9. validate HTTPS on the real PRS-T1;
10. implement temporary storage and reader authorization;
11. synchronize the current bundle;
12. integrate the library UI;
13. complete CI and local integration tests;
14. bootstrap production Cloudflare resources;
15. configure human authentication and deployment credentials;
16. deploy automatically;
17. run the complete real-device acceptance test.

This sequence is a guide, not a requirement that independent branches of work occur serially.

The `depends-on:` graph in the implementation issues defines the actual scheduling constraints.
