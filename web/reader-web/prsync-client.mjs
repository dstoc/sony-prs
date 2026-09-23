export const DEFAULT_API_BASE = "https://prs-reader.dstoc.workers.dev";
export const MAX_BUNDLE_BYTES = 16 * 1024 * 1024;
const MAX_SMALL_JSON_BYTES = 64 * 1024;
const MAX_MANIFEST_RESPONSE_BYTES = MAX_BUNDLE_BYTES + 64 * 1024;

const PROTOCOL_VERSION = Object.freeze({ major: 1, minor: 0 });
const MAX_POLL_DELAY_SECONDS = 10;

export class PrsyncError extends Error {
  constructor(message, { status, code } = {}) {
    super(message);
    this.name = "PrsyncError";
    this.status = status;
    this.code = code;
  }
}

/**
 * A page-lifetime PRSync client. It holds a reader token and inbox snapshot
 * only in this closure; it never writes either value to browser storage.
 */
export function createPrsyncClient({
  apiBase = DEFAULT_API_BASE,
  fetchImpl = globalThis.fetch,
  nowSeconds = () => Math.floor(Date.now() / 1000),
  wait = (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds)),
  onApprovalUrl = () => {},
  onProgress = () => {},
} = {}) {
  const api = new URL(apiBase);
  if (api.protocol !== "https:"
    && !(api.protocol === "http:" && (api.hostname === "127.0.0.1" || api.hostname === "localhost"))) {
    throw new Error("PRSync API must use HTTPS outside localhost.");
  }
  api.search = "";
  api.hash = "";
  const apiPathPrefix = api.pathname.replace(/\/$/u, "");

  let session;
  let snapshot;

  async function request(path, { method = "GET", token, body, headers = {} } = {}) {
    const requestHeaders = new Headers(headers);
    if (body !== undefined) {
      requestHeaders.set("Content-Type", "application/json");
    }
    if (token) {
      requestHeaders.set("Authorization", `Bearer ${token}`);
    }
    let response;
    try {
      response = await fetchImpl(new URL(`${apiPathPrefix}${path}`, api.origin), {
        method,
        headers: requestHeaders,
        body: body === undefined ? undefined : JSON.stringify(body),
        cache: "no-store",
        credentials: "omit",
        redirect: "error",
      });
    } catch (error) {
      throw new PrsyncError(`Could not reach the PRSync API: ${errorMessage(error)}`);
    }
    if (!response.ok) {
      let detail = "";
      try {
        const bytes = await readBoundedResponse(response, MAX_SMALL_JSON_BYTES);
        const payload = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes));
        detail = payload?.error?.message || payload?.message || "";
      } catch {
        // Keep non-JSON server responses out of the browser status.
      }
      throw new PrsyncError(detail || `PRSync API returned HTTP ${response.status}.`, {
        status: response.status,
        code: responseErrorCode(detail),
      });
    }
    return response;
  }

  async function jsonRequest(path, options, maxResponseBytes = MAX_SMALL_JSON_BYTES) {
    const response = await request(path, options);
    let payload;
    try {
      const bytes = await readBoundedResponse(
        response,
        maxResponseBytes,
        maxResponseBytes === MAX_MANIFEST_RESPONSE_BYTES
          ? "PRSync manifest response exceeds its configured size limit."
          : "PRSync API JSON response exceeds its configured size limit.",
      );
      payload = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes));
    } catch {
      throw new PrsyncError("PRSync API returned an invalid or oversized JSON response.");
    }
    requireCompatibleProtocol(payload?.protocol_version);
    return { response, payload };
  }

  async function authorize() {
    session = undefined;
    snapshot = undefined;
    onProgress("Requesting reader approval…");
    const { payload: start } = await jsonRequest("/api/v1/authorization/reader", {
      method: "POST",
      body: { protocol_version: PROTOCOL_VERSION },
    });
    const requestInfo = start.request;
    const pollingSecret = start.polling_secret;
    if (!requestInfo || requestInfo.kind !== "reader"
      || typeof requestInfo.request_id !== "string"
      || !pollingSecret || typeof pollingSecret !== "string"
      || !Number.isSafeInteger(requestInfo.expires_at)) {
      throw new PrsyncError("PRSync API returned an incomplete reader authorization request.");
    }
    const approvalUrl = safeApprovalUrl(requestInfo.approval_url, requestInfo.request_id, pollingSecret, api);
    onApprovalUrl(approvalUrl);

    while (nowSeconds() < requestInfo.expires_at) {
      onProgress("Waiting for approval in the new tab…");
      const { payload } = await jsonRequest("/api/v1/authorization/poll", {
        method: "POST",
        body: {
          protocol_version: PROTOCOL_VERSION,
          request_id: requestInfo.request_id,
          polling_secret: pollingSecret,
        },
      });
      const outcome = payload.outcome;
      switch (outcome?.kind) {
        case "pending": {
          const delay = Math.max(1, Math.min(
            Number(outcome.retry_after_seconds) || 1,
            MAX_POLL_DELAY_SECONDS,
          ));
          await wait(delay * 1000);
          break;
        }
        case "reader":
          session = validateSession(outcome.session, nowSeconds());
          onProgress("Reader approved. Starting one sync…");
          return session;
        case "denied":
          throw new PrsyncError("Reader authorization was denied.", { code: "denied" });
        case "expired":
          throw new PrsyncError("Reader authorization expired. Authorize again.", { code: "expired" });
        case "already_claimed":
          throw new PrsyncError("Reader authorization was already claimed. Authorize again.");
        default:
          throw new PrsyncError("PRSync API returned an unexpected authorization result.");
      }
    }
    throw new PrsyncError("Reader authorization expired. Authorize again.", { code: "expired" });
  }

  async function syncOnce(activate) {
    const activeSession = validSessionOrClear();
    if (!activeSession) {
      throw new PrsyncError("Reader access is missing or expired. Authorize again.", { code: "reauthorize" });
    }

    onProgress("Checking inbox manifest…");
    const headers = {};
    if (snapshot) {
      headers["If-Revision"] = String(snapshot.revision);
    }
    let manifestResult;
    try {
      manifestResult = await jsonRequest("/api/v1/reader/manifest", {
        token: activeSession.bearer_token,
        headers,
      }, MAX_MANIFEST_RESPONSE_BYTES);
    } catch (error) {
      throw clearSessionIfRejected(error);
    }
    const { state } = manifestResult.payload;
    if (!state || typeof state.kind !== "string") {
      throw new PrsyncError("PRSync API returned an invalid inbox manifest state.");
    }
    if (state.kind === "not_modified") {
      if (!snapshot || snapshot.kind !== "current"
        || state.revision !== snapshot.revision
        || state.etag !== snapshot.etag) {
        throw new PrsyncError("PRSync API returned not-modified without a matching local snapshot.");
      }
      snapshot = { ...snapshot, revision: state.revision };
      return { kind: "unchanged", revision: snapshot.revision };
    }
    if (state.kind === "empty") {
      const revision = requireRevision(state.revision);
      if (snapshot?.kind === "empty" && snapshot.revision === revision) {
        return { kind: "unchanged", revision };
      }
      await activate(null);
      snapshot = { kind: "empty", revision };
      return { kind: "cleared", revision };
    }
    if (state.kind !== "current") {
      throw new PrsyncError("PRSync API returned an unsupported inbox state.");
    }

    const revision = requireRevision(state.revision);
    const etag = state.etag;
    if (typeof etag !== "string" || !etag) {
      throw new PrsyncError("PRSync API returned an invalid inbox entity tag.");
    }
    if (snapshot?.kind === "current" && snapshot.revision === revision && snapshot.etag === etag) {
      return { kind: "unchanged", revision };
    }
    validateManifestBounds(state.manifest);
    onProgress("Downloading cloud bundle…");
    let bundleResponse;
    try {
      bundleResponse = await request("/api/v1/reader/bundle", {
        token: activeSession.bearer_token,
      });
    } catch (error) {
      throw clearSessionIfRejected(error);
    }
    const bytes = await readBoundedResponse(
      bundleResponse,
      MAX_BUNDLE_BYTES,
      "Downloaded bundle exceeds the 16 MiB protocol limit.",
    );
    await activate({ bytes, manifest: state.manifest });
    snapshot = { kind: "current", revision, etag };
    return { kind: "replaced", revision };
  }

  function clearSessionIfRejected(error) {
    if (error?.status === 401 || error?.status === 403) {
      session = undefined;
      return new PrsyncError("Reader access expired or was rejected. Authorize again.", {
        status: error.status,
        code: "reauthorize",
      });
    }
    return error;
  }

  function validSessionOrClear() {
    if (!session || session.expires_at <= nowSeconds()) {
      session = undefined;
      return undefined;
    }
    return session;
  }

  function hasSession() {
    return Boolean(validSessionOrClear());
  }

  function resetSnapshot() {
    snapshot = undefined;
  }

  return Object.freeze({ authorize, syncOnce, hasSession, resetSnapshot });
}

function validateSession(session, now) {
  const expiresAt = session?.expires_at;
  const capabilities = session?.scope?.capabilities;
  if (typeof session?.bearer_token !== "string"
    || !Number.isSafeInteger(expiresAt)
    || expiresAt <= now
    || !Array.isArray(capabilities)
    || !capabilities.includes("read_manifest")
    || !capabilities.includes("download_bundle")) {
    throw new PrsyncError("Approved reader session is incomplete or expired.");
  }
  return session;
}

function safeApprovalUrl(value, requestId, pollingSecret, api) {
  let url;
  try {
    url = new URL(value);
  } catch {
    throw new PrsyncError("PRSync API returned an invalid approval URL.");
  }
  if (url.origin !== api.origin || url.pathname !== `/a/${encodeURIComponent(requestId)}`
    || url.search || url.hash || url.username || url.password
    || value.includes(pollingSecret)) {
    throw new PrsyncError("Approval URL failed the public request URL checks.");
  }
  return url.href;
}

function validateManifestBounds(manifest) {
  requireCompatibleProtocol(manifest?.protocol_version);
  if (!manifest || manifest.bundle_format_version !== 1
    || typeof manifest.entry_point !== "string"
    || !manifest.entry_point.toLowerCase().endsWith(".md")
    || !Array.isArray(manifest.files)) {
    throw new PrsyncError("PRSync API returned an invalid bundle manifest.");
  }
  let total = 0;
  for (const file of manifest.files) {
    if (typeof file?.path !== "string" || !Number.isSafeInteger(file.size) || file.size < 0
      || typeof file.sha256 !== "string" || !/^[0-9a-f]{64}$/u.test(file.sha256)) {
      throw new PrsyncError("PRSync manifest is missing a valid file size or SHA-256.");
    }
    total += file.size;
    if (total > MAX_BUNDLE_BYTES) {
      throw new PrsyncError("PRSync manifest exceeds the 16 MiB protocol limit.");
    }
  }
}

function requireRevision(revision) {
  if (!Number.isSafeInteger(revision) || revision < 0) {
    throw new PrsyncError("PRSync API returned an invalid inbox revision.");
  }
  return revision;
}

function requireCompatibleProtocol(version) {
  if (version?.major !== PROTOCOL_VERSION.major
    || !Number.isInteger(version.minor)
    || version.minor < 0
    || version.minor > PROTOCOL_VERSION.minor) {
    throw new PrsyncError("PRSync API protocol version is not compatible with this reader.");
  }
}

function responseErrorCode(detail) {
  if (/expired|expiry/iu.test(detail)) {
    return "expired";
  }
  if (/denied/iu.test(detail)) {
    return "denied";
  }
  return undefined;
}

function errorMessage(error) {
  return error && typeof error.message === "string" ? error.message : String(error);
}

async function readBoundedResponse(response, limit, limitMessage = "API response exceeds its configured size limit.") {
  const contentLength = Number(response.headers.get("Content-Length"));
  if (Number.isFinite(contentLength) && contentLength > limit) {
    throw new PrsyncError(limitMessage);
  }
  if (!response.body?.getReader) {
    const bytes = new Uint8Array(await response.arrayBuffer());
    if (bytes.byteLength > limit) {
      throw new PrsyncError(limitMessage);
    }
    return bytes;
  }

  const reader = response.body.getReader();
  const chunks = [];
  let size = 0;
  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) {
        break;
      }
      size += value.byteLength;
      if (size > limit) {
        await reader.cancel().catch(() => {});
        throw new PrsyncError(limitMessage);
      }
      chunks.push(value);
    }
  } finally {
    reader.releaseLock();
  }
  const bytes = new Uint8Array(size);
  let offset = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return bytes;
}
