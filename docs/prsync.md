# PRSync

PRSync is a document-delivery system for the Sony PRS-T1 Markdown reader.

Its purpose is to make the PRS-T1 behave like a distraction-free, offline-first Markdown appliance:

1. Send Markdown documents from a trusted computer.
2. Store them in a small hosted inbox.
3. Boot the PRS-T1.
4. Authorize that boot from another trusted device by scanning a QR code.
5. Download the current document library.
6. Read entirely offline.
7. Lose all downloaded documents and reader credentials when the PRS-T1 reboots.

PRSync is intentionally not a general filesystem synchronization system, cloud drive, feed reader, or continuously connected application.

## Goals

PRSync should provide:

- a simple CLI for publishing Markdown documents and their images;
- a small hosted inbox;
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
- multi-user sharing;
- general cloud storage;
- real-time synchronization;
- remote deletion of files from an offline reader;
- protection of documents while an already-authorized reader remains powered on.

These can be reconsidered later without being requirements for the initial system.

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
    |   tmpfs library          |
    |   Markdown reader        |
    +--------------------------+

The Cloudflare Worker is the only component that directly accesses D1 or R2.

Neither client receives Cloudflare account credentials or storage credentials.

## Components

### `prs-sync-protocol`

A shared Rust crate defines the wire protocol and common identifiers.

It should contain protocol-level types such as:

- document identifiers;
- document versions;
- manifests;
- hashes;
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

    prs-send login
    prs-send logout
    prs-send push document.md
    prs-send list

A sender credential may persist on this computer.

That credential is specific to PRSync. It must not be a Cloudflare API token, R2 credential, D1 credential, or other infrastructure credential.

### Cloudflare Worker

The Worker implements the hosted service.

Responsibilities include:

- sender authorization;
- reader authorization;
- human approval;
- document upload;
- inbox manifests;
- document download;
- authentication and authorization;
- metadata management;
- storage access.

The Worker should be written in Rust using the Cloudflare Workers Rust environment.

### D1

D1 stores small structured state.

Expected state includes:

- document metadata;
- inbox revisions;
- pending authorization requests;
- hashed authorization secrets;
- reader sessions;
- sender credentials;
- sender credential revocation state.

D1 does not store document bundles.

### R2

R2 stores immutable document bundles.

The R2 bucket remains private.

All object access occurs through the Worker.

A reader must only be able to retrieve objects referenced by the inbox visible to its current session.

### PRS-T1 synchronization client

The synchronization client runs on the PRS-T1.

Its responsibilities include:

- HTTPS communication;
- boot-scoped authorization;
- QR generation;
- waiting for authorization;
- fetching the inbox manifest;
- downloading bundles;
- validating hashes and limits;
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
- document identifier;
- document version;
- title;
- Markdown entry point;
- contained files;
- content hashes;
- sizes.

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
- duplicate output paths;
- writing outside the destination directory;
- oversized individual files;
- oversized archives;
- excessive extracted size;
- hash mismatches;
- unsupported bundle versions.

Extraction should be bounded and preferably streaming.

The PRS-T1 must not need to hold an entire bundle in RAM before extracting it.

## Inbox model

The hosted service exposes a logical inbox.

The inbox is a set of currently published document versions plus a monotonically changing revision identifier.

A reader can request the manifest and determine whether anything has changed.

The protocol should support conditional requests using a revision or HTTP ETag so an unchanged inbox is inexpensive to check.

Publishing a new version of a document creates a new immutable bundle and changes the manifest.

Storage objects are immutable even when a logical document is replaced.

## Sender authorization

The sender CLI uses a human-approved authorization flow.

The CLI creates a pending sender authorization request and receives an approval URL.

The user opens the URL in a normal browser and authenticates to the human-facing service.

After approval, the CLI receives a sender-scoped credential.

The sender credential:

- may be persisted on the trusted computer;
- may upload documents;
- may list the inbox;
- may replace documents;
- may remove documents from the inbox;
- may be revoked.

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
         | receive short-lived read-only session
         v
    synchronize documents

The reader stores the resulting session credential only in RAM.

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

Approval requires authentication as an authorized human.

The PRS-T1 separately retains a high-entropy polling secret that is required to claim the approved session.

## Human authorization

Human-facing approval should be protected independently from the machine API.

A useful deployment split is:

    sync.example.com

for protocol endpoints used by `prs-send` and the PRS-T1, and:

    reader.example.com

for authenticated human approval.

The protocol hostname cannot require interactive Cloudflare Access authentication because unauthenticated clients must be able to initiate authorization.

The human-facing hostname can be protected by Cloudflare Access.

Only explicitly allowed identities should be permitted to approve authorization requests.

## Reader session scope

A reader session is read-only.

It may:

- fetch the current inbox manifest;
- retrieve document bundles referenced by that manifest.

It may not:

- upload documents;
- replace documents;
- delete documents;
- create sender credentials;
- administer the service.

The session should expire after a bounded period.

Expiration does not need to stop offline reading of documents already present in tmpfs.

## Temporary document storage

Synchronized documents are stored in a RAM-backed filesystem on the PRS-T1.

For example:

    /mnt/prs-reader/
        library/
            <document-id>/
                index.md
                images/
                    ...

The exact mount path is an implementation detail.

The temporary filesystem must have an explicit size limit.

Synchronization must also enforce:

- a maximum downloaded bundle size;
- a maximum extracted bundle size;
- a maximum total library size.

Documents disappear when the reader reboots.

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
    optionally disable Wi-Fi
        |
        v
    read locally

The reader should not maintain an idle WebSocket, MQTT connection, or other persistent network channel.

The service is a document-delivery mechanism, not part of the active reading path.

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

The sender may persist:

- its PRSync sender bearer credential.

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

Pending authorization requests must expire.

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
- R2 for immutable document bundles;
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

Production Worker deployment should occur from GitHub Actions after changes reach `main` and required checks pass.

Production deployment credentials are stored in protected GitHub secrets or an equivalent protected deployment environment.

Expected secrets include:

    CLOUDFLARE_ACCOUNT_ID
    CLOUDFLARE_API_TOKEN

The Cloudflare API token should have only the permissions needed to deploy this application and manage the resources required by its deployment workflow.

Deployment should:

1. build the Worker;
2. apply pending D1 migrations;
3. deploy the Worker;
4. retain the configured D1 and R2 bindings;
5. perform a small non-destructive health check.

Production deployment must not expose Cloudflare credentials to pull-request jobs.

## Local development

The hosted service should be runnable locally without a production Cloudflare account.

Local development should support:

- a local Worker;
- local D1;
- local R2-compatible bindings;
- schema migrations;
- complete protocol integration tests.

Human authentication should be isolated behind a clear trusted-principal boundary so tests can supply an authenticated test identity without weakening production authentication.

## Failure handling

Network failures are expected.

The device client must handle:

- inability to create an authorization request;
- network loss while waiting for approval;
- authorization expiry;
- session expiry;
- interrupted downloads;
- corrupt downloads;
- insufficient tmpfs capacity;
- invalid bundles;
- server errors.

A partially downloaded or partially extracted bundle must never become visible as a valid library document.

Download into staging storage, verify it, and expose it atomically.

## Synchronization semantics

Synchronization occurs at explicit boundaries rather than continuously changing the library while the user reads.

A synchronization operation should conceptually:

1. fetch one manifest revision;
2. determine the desired library for that revision;
3. obtain missing bundles;
4. verify and extract them;
5. commit the resulting library state;
6. notify the reader that a new library state is available.

A document that is currently open should not silently change underneath the reader.

The exact UI behaviour for a newly available version can be decided by the reader integration.

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

    Synchronizing…
    3 documents

         ↓

    last document / library

The normal reading interface should not contain persistent connectivity indicators, notification feeds, badges, recommendations, or other attention-oriented UI.

A small status indication is appropriate while authorization or synchronization is actively occurring.

## Library model

The initial library can remain deliberately small.

At minimum the device should support:

- list synchronized documents;
- open a document;
- navigate linked Markdown files within its bundle;
- display bundled images;
- return to the library;
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
11. synchronize documents;
12. integrate the library UI;
13. complete CI and local integration tests;
14. bootstrap production Cloudflare resources;
15. configure human authentication and deployment credentials;
16. deploy automatically;
17. run the complete real-device acceptance test.

This sequence is a guide, not a requirement that independent branches of work occur serially.

The `depends-on:` graph in the implementation issues defines the actual scheduling constraints.
