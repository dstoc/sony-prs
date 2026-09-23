import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { DEFAULT_API_BASE } from "../web/reader-web/prsync-client.mjs";

const [htmlPath, expectedBaseArgument] = process.argv.slice(2);
if (!htmlPath) {
  throw new Error("Usage: node tools/test-reader-web-api-base.mjs <index.html> [expected-api-origin]");
}

const html = await readFile(htmlPath, "utf8");
const metas = [...html.matchAll(/<meta\s+name="prsync-api-base"\s+content="([^"]*)"\s*\/?>/giu)];
assert.equal(metas.length, 1, "static reader HTML must contain one configured PRSync API base");

const configuredBase = metas[0][1];
let apiBase;
assert.doesNotThrow(() => {
  apiBase = new URL(configuredBase);
}, "static PRSync API base must include a scheme and host");
assert.equal(apiBase.origin, configuredBase, "static PRSync API base must be an origin without a path");
assert.ok(apiBase.hostname !== "api", "static PRSync API base must not treat bare 'api' as a hostname");
assert.ok(apiBase.protocol === "https:" || (
  apiBase.protocol === "http:"
    && (apiBase.hostname === "127.0.0.1" || apiBase.hostname === "localhost")
), "static PRSync API base must use HTTPS outside localhost");
assert.equal(apiBase.username, "");
assert.equal(apiBase.password, "");
assert.equal(apiBase.search, "");
assert.equal(apiBase.hash, "");

const expectedBase = expectedBaseArgument
  || process.env.PRS_READER_WEB_API_BASE?.trim()
  || DEFAULT_API_BASE;
let expectedApiBase;
assert.doesNotThrow(() => {
  expectedApiBase = new URL(expectedBase);
}, "configured PRSync API base must include a scheme and host");
const expectedLocalHttp = expectedApiBase.protocol === "http:"
  && (expectedApiBase.hostname === "127.0.0.1" || expectedApiBase.hostname === "localhost");
assert.ok(expectedApiBase.protocol === "https:" || expectedLocalHttp);
assert.ok(expectedApiBase.hostname !== "api");
assert.equal(expectedApiBase.username, "");
assert.equal(expectedApiBase.password, "");
assert.ok(expectedApiBase.pathname === "/" || expectedApiBase.pathname === "");
assert.equal(expectedApiBase.search, "");
assert.equal(expectedApiBase.hash, "");
assert.equal(configuredBase, expectedApiBase.origin, "static PRSync API base does not match the build configuration");

const endpoints = [
  "/api/v1/authorization/reader",
  "/api/v1/authorization/poll",
  "/api/v1/reader/manifest",
  "/api/v1/reader/bundle",
];
for (const endpoint of endpoints) {
  const resolved = new URL(`${apiBase.pathname.replace(/\/$/u, "")}${endpoint}`, apiBase.origin);
  assert.equal(resolved.href, `${apiBase.origin}${endpoint}`, `endpoint resolved incorrectly: ${endpoint}`);
  assert.equal(resolved.search, "", `endpoint unexpectedly contains a query: ${endpoint}`);
  assert.equal(resolved.hash, "", `endpoint unexpectedly contains a fragment: ${endpoint}`);
}
console.log(`verified PRSync API endpoint URLs for ${apiBase.origin}`);
