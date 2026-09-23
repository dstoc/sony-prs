import { readFile, writeFile } from "node:fs/promises";
import { DEFAULT_API_BASE } from "../web/reader-web/prsync-client.mjs";

const indexPath = process.argv[2];
if (!indexPath) {
  throw new Error("Usage: node tools/configure-reader-web-api-base.mjs <index.html>");
}

const configuredBase = process.env.PRS_READER_WEB_API_BASE?.trim() || DEFAULT_API_BASE;
let apiBase;
try {
  apiBase = new URL(configuredBase);
} catch {
  throw new Error("PRS_READER_WEB_API_BASE must be an absolute URL with a scheme and host.");
}

const localHttp = apiBase.protocol === "http:"
  && (apiBase.hostname === "127.0.0.1" || apiBase.hostname === "localhost");
if ((apiBase.protocol !== "https:" && !localHttp)
  || apiBase.hostname === "api"
  || apiBase.username || apiBase.password
  || (apiBase.pathname !== "/" && apiBase.pathname !== "")
  || apiBase.search || apiBase.hash) {
  throw new Error(
    "PRS_READER_WEB_API_BASE must be an HTTPS origin (or localhost HTTP) without credentials, a path, query, or fragment.",
  );
}

const html = await readFile(indexPath, "utf8");
const meta = `<meta name="prsync-api-base" content="${apiBase.origin}">`;
const metaPattern = /\s*<meta\s+name="prsync-api-base"\s+content="[^"]*"\s*\/?>/giu;
const updated = metaPattern.test(html)
  ? html.replace(metaPattern, `\n    ${meta}`)
  : html.replace("  </head>", `    ${meta}\n  </head>`);
if (!updated.includes(meta)) {
  throw new Error(`could not add the PRSync API base to ${indexPath}`);
}
await writeFile(indexPath, updated);
