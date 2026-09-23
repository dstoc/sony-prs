import assert from "node:assert/strict";
import {
  createPrsyncClient,
  MAX_BUNDLE_BYTES,
} from "../web/reader-web/prsync-client.mjs";

const protocol = { major: 1, minor: 0 };
const secret = "private-polling-capability";
const bearer = "private-reader-bearer";
const manifest = {
  protocol_version: protocol,
  bundle_format_version: 1,
  entry_point: "README.md",
  files: [{
    path: "README.md",
    size: 5,
    sha256: "a".repeat(64),
  }],
};

function jsonResponse(payload, status = 200) {
  return new Response(JSON.stringify(payload), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

function currentManifest(revision = 3, body = manifest) {
  return jsonResponse({
    protocol_version: protocol,
    state: {
      kind: "current",
      revision,
      etag: `etag-${revision}`,
      manifest: body,
    },
  });
}

function authorizationStart(approvalUrl = "https://prs-reader.dstoc.workers.dev/a/auth-123", expiresAt = 200) {
  return jsonResponse({
    protocol_version: protocol,
    request: {
      protocol_version: protocol,
      request_id: "auth-123",
      kind: "reader",
      approval_url: approvalUrl,
      created_at: 100,
      expires_at: expiresAt,
    },
    polling_secret: secret,
  });
}

function approved() {
  return jsonResponse({
    protocol_version: protocol,
    outcome: {
      kind: "reader",
      session: {
        session_id: "session-123",
        bearer_token: bearer,
        issued_at: 101,
        expires_at: 180,
        scope: { capabilities: ["read_manifest", "download_bundle"] },
      },
    },
  });
}

function bundleResponse(bytes = new Uint8Array([1, 2, 3])) {
  return new Response(bytes, {
    headers: {
      "Content-Type": "application/x-tar",
      "Content-Length": String(bytes.byteLength),
    },
  });
}

const calls = [];
const progress = [];
const approvalLinks = [];
let now = 100;
let pollCount = 0;
let manifestRequests = 0;
let bundleRequests = 0;
let manifests = [currentManifest(), jsonResponse({
  protocol_version: protocol,
  state: { kind: "not_modified", revision: 3, etag: "etag-3" },
}), currentManifest(4), jsonResponse({
  protocol_version: protocol,
  state: { kind: "empty", revision: 5 },
}), jsonResponse({
  protocol_version: protocol,
  state: { kind: "empty", revision: 5 },
})];
const fetchImpl = async (url, options) => {
  const parsed = new URL(url);
  calls.push({ url: parsed, options });
  assert.equal(options.credentials, "omit");
  assert.equal(options.cache, "no-store");
  assert.equal(options.redirect, "error");
  if (parsed.pathname.endsWith("/authorization/reader")) {
    return authorizationStart();
  }
  if (parsed.pathname.endsWith("/authorization/poll")) {
    const body = JSON.parse(options.body);
    assert.equal(body.polling_secret, secret);
    assert.equal(parsed.search, "");
    pollCount += 1;
    return pollCount === 1
      ? jsonResponse({ protocol_version: protocol, outcome: { kind: "pending", retry_after_seconds: 0 } })
      : approved();
  }
  if (parsed.pathname.endsWith("/reader/manifest")) {
    manifestRequests += 1;
    assert.equal(options.headers.get("Authorization"), `Bearer ${bearer}`);
    return manifests.shift();
  }
  if (parsed.pathname.endsWith("/reader/bundle")) {
    bundleRequests += 1;
    assert.equal(options.headers.get("Authorization"), `Bearer ${bearer}`);
    return bundleResponse();
  }
  throw new Error(`unexpected request ${parsed.pathname}`);
};

const waits = [];
const client = createPrsyncClient({
  apiBase: "https://prs-reader.dstoc.workers.dev",
  fetchImpl,
  nowSeconds: () => now,
  wait: async (milliseconds) => waits.push(milliseconds),
  onApprovalUrl: (url) => approvalLinks.push(url),
  onProgress: (value) => progress.push(value),
});

assert.equal(client.hasSession(), false);
client.resetSnapshot();
assert.equal(client.hasSession(), false);
await client.authorize();
assert.equal(client.hasSession(), true);
assert.equal(pollCount, 2);
assert.deepEqual(waits, [1000]);
assert.equal(approvalLinks.length, 1);
assert.equal(approvalLinks[0], "https://prs-reader.dstoc.workers.dev/a/auth-123");
assert.equal(approvalLinks[0].includes(secret), false);
assert.equal(approvalLinks[0].includes(bearer), false);
assert.ok(progress.some((value) => value.includes("Waiting for approval")));

const activated = [];
const first = await client.syncOnce(async (value) => activated.push(value));
assert.deepEqual(first, { kind: "replaced", revision: 3 });
assert.equal(activated.length, 1);
assert.deepEqual([...activated[0].bytes], [1, 2, 3]);
assert.deepEqual(activated[0].manifest, manifest);
assert.equal(bundleRequests, 1);
const second = await client.syncOnce(async (value) => activated.push(value));
assert.deepEqual(second, { kind: "unchanged", revision: 3 });
assert.equal(bundleRequests, 1);
assert.equal(manifestRequests, 2);
assert.equal(calls.filter(({ url }) => url.pathname.endsWith("/reader/manifest"))[1]
  .options.headers.get("If-Revision"), "3");
assert.equal(calls.some(({ options }) => (
  options.headers.get("Authorization") === `Bearer ${bearer}`
)), true);
assert.equal(JSON.stringify(approvalLinks).includes(secret), false);
assert.equal(client.hasSession(), true);

// A local directory activation invalidates the remote snapshot, so a later
// explicit Sync click asks for the current inbox instead of trusting stale UI.
client.resetSnapshot();
assert.equal(calls.some(({ options }) => options.body?.includes(secret)), true);
const afterLocalLibrary = await client.syncOnce(async (value) => activated.push(value));
assert.deepEqual(afterLocalLibrary, { kind: "replaced", revision: 4 });
assert.equal(bundleRequests, 2);
const cleared = await client.syncOnce(async (value) => activated.push(value));
assert.deepEqual(cleared, { kind: "cleared", revision: 5 });
assert.equal(activated.at(-1), null);
const activationsAtClear = activated.length;
const emptyUnchanged = await client.syncOnce(async (value) => activated.push(value));
assert.deepEqual(emptyUnchanged, { kind: "unchanged", revision: 5 });
assert.equal(activated.length, activationsAtClear);

const deniedClient = createPrsyncClient({
  fetchImpl: async (url) => new URL(url).pathname.endsWith("/authorization/reader")
    ? authorizationStart()
    : jsonResponse({ protocol_version: protocol, outcome: { kind: "denied" } }),
  nowSeconds: () => now,
  onApprovalUrl: (url) => assert.equal(url.includes(secret), false),
});
await assert.rejects(deniedClient.authorize(), /authorization was denied/u);
assert.equal(deniedClient.hasSession(), false);

let leakedApprovalUrlShown = false;
const hostileUrlClient = createPrsyncClient({
  fetchImpl: async () => authorizationStart(
    `https://prs-reader.dstoc.workers.dev/a/auth-123?poll=${secret}`,
  ),
  nowSeconds: () => now,
  onApprovalUrl: () => { leakedApprovalUrlShown = true; },
});
await assert.rejects(hostileUrlClient.authorize(), /URL checks/u);
assert.equal(leakedApprovalUrlShown, false);

const httpRemoteClient = () => createPrsyncClient({
  apiBase: "http://reader.example.com",
  fetchImpl,
});
assert.throws(httpRemoteClient, /must use HTTPS/u);

const futureProtocolClient = createPrsyncClient({
  fetchImpl: async () => {
    const start = await authorizationStart().json();
    start.protocol_version.minor = 1;
    return jsonResponse(start);
  },
  nowSeconds: () => now,
});
await assert.rejects(futureProtocolClient.authorize(), /not compatible/u);

const expiredAuthorizationClient = createPrsyncClient({
  fetchImpl: async () => authorizationStart(undefined, now),
  nowSeconds: () => now,
});
await assert.rejects(expiredAuthorizationClient.authorize(), /authorization expired/u);

const expiringClient = createPrsyncClient({
  fetchImpl,
  nowSeconds: () => now,
});
assert.equal(expiringClient.hasSession(), false);
await assert.rejects(
  expiringClient.syncOnce(async () => assert.fail("expired session must not activate content")),
  /missing or expired/u,
);

const badHash = structuredClone(manifest);
badHash.files[0].sha256 = "invalid";
const boundedCalls = [];
const boundedClient = createPrsyncClient({
  fetchImpl: async (url) => {
    const path = new URL(url).pathname;
    boundedCalls.push(path);
    if (path.endsWith("/authorization/reader")) return authorizationStart();
    if (path.endsWith("/authorization/poll")) return approved();
    if (path.endsWith("/reader/manifest")) return currentManifest(4, badHash);
    if (path.endsWith("/reader/bundle")) return bundleResponse();
    throw new Error(`unexpected request ${path}`);
  },
  nowSeconds: () => now,
});
await boundedClient.authorize();
let activatedOnInvalidManifest = false;
await assert.rejects(
  boundedClient.syncOnce(async () => { activatedOnInvalidManifest = true; }),
  /valid file size or SHA-256/u,
);
assert.equal(activatedOnInvalidManifest, false);
assert.equal(boundedCalls.some((path) => path.endsWith("/reader/bundle")), false);

const oversizedClient = createPrsyncClient({
  fetchImpl: async (url) => {
    const path = new URL(url).pathname;
    if (path.endsWith("/authorization/reader")) return authorizationStart();
    if (path.endsWith("/authorization/poll")) return approved();
    if (path.endsWith("/reader/manifest")) return currentManifest(5);
    if (path.endsWith("/reader/bundle")) {
      return new Response(null, { headers: { "Content-Length": String(MAX_BUNDLE_BYTES + 1) } });
    }
    throw new Error(`unexpected request ${path}`);
  },
  nowSeconds: () => now,
});
await oversizedClient.authorize();
let activatedOnOversize = false;
await assert.rejects(
  oversizedClient.syncOnce(async () => { activatedOnOversize = true; }),
  /exceeds the 16 MiB protocol limit/u,
);
assert.equal(activatedOnOversize, false);

const callCountBeforeExpiry = calls.length;
now = 180;
assert.equal(client.hasSession(), false);
await assert.rejects(
  client.syncOnce(async () => assert.fail("expired session must not activate content")),
  /missing or expired/u,
);
assert.equal(calls.length, callCountBeforeExpiry);

console.log("reader web sync contract passed");
