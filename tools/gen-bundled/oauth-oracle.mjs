// Sign-in and quota parity oracle (spec 005 T040), run by generate.mjs in its own process.
//
// Runs the inputs of ref/9router's sign-in, refresh and usage unit tests through 9router's
// own modules and writes what they send and return to tests/fixtures/9router/{oauth,usage}/.
// Every network call goes to a scripted fetch installed before any 9router module loads, so
// `proxyFetch.js` captures it as its `originalFetch`; nothing leaves the machine. Inputs are
// fixed (state, challenge, verifier, codes) and random values are recorded as placeholders,
// so regenerating writes the same bytes.

import { execFileSync } from "node:child_process";
import { mkdirSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { register } from "node:module";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const REF = join(ROOT, "ref", "9router");
const SHA = execFileSync("git", ["-C", REF, "rev-parse", "--short", "HEAD"]).toString().trim();
const HEADER = `Generated from ref/9router@${SHA} by tools/gen-bundled/generate.mjs — do not edit.`;

// ── Scripted fetch ──────────────────────────────────────────────────────────

const calls = [];
let route = null;
const headerObject = (h) => {
  if (!h) return {};
  const entries = h instanceof Headers ? [...h.entries()] : Object.entries(h);
  return Object.fromEntries(entries.map(([k, v]) => [k.toLowerCase(), String(v)]).sort(([a], [b]) => a.localeCompare(b)));
};
const bodyRecord = (body, headers) => {
  if (body === undefined || body === null) return null;
  if (body instanceof URLSearchParams) return { encoding: "form", fields: [...body.entries()], raw: body.toString() };
  if (body instanceof Uint8Array) return { encoding: "binary", hex: Buffer.from(body).toString("hex") };
  const text = String(body);
  const ct = headers["content-type"] || "";
  if (ct.includes("json")) return { encoding: "json", fields: Object.entries(JSON.parse(text)), raw: text };
  if (ct.includes("x-www-form-urlencoded")) return { encoding: "form", fields: [...new URLSearchParams(text).entries()], raw: text };
  return { encoding: "text", raw: text };
};
globalThis.fetch = async (url, init = {}) => {
  const headers = headerObject(init.headers);
  calls.push({ url: String(url), method: init.method || "GET", headers, body: bodyRecord(init.body, headers) });
  if (!route) throw new Error(`unscripted fetch ${url}`);
  return route(String(url), init);
};
const json = (body, status = 200) =>
  new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
const text = (body, status) => new Response(body, { status, headers: { "Content-Type": "text/plain" } });
const binary = (buf, status = 200) =>
  new Response(buf, { status, headers: { "content-type": "application/grpc-web+proto" } });
/** Answers in order; a function answer is called (it may throw, as a network failure). */
const script = (...answers) => {
  const queue = [...answers];
  route = (url, init) => {
    const a = queue.shift();
    if (a === undefined) throw new Error(`unscripted fetch ${url}`);
    return typeof a === "function" ? a(url, init) : a.clone();
  };
};
const networkDown = (msg = "network down") => () => { throw new Error(msg); };
/** Runs `fn` with `answers` scripted; returns the calls it made and its result or error. */
const run = async (answers, fn) => {
  calls.length = 0;
  script(...answers);
  let result;
  let error = null;
  try { result = await fn(); } catch (e) { error = String(e?.message || e); }
  route = null;
  return { calls: calls.map((c) => ({ ...c })), result: result === undefined ? null : result, error };
};
const quiet = { debug() {}, info() {}, warn() {}, error() {} };

// ── Module loading ──────────────────────────────────────────────────────────
//
// As in generate.mjs's translator oracle: `@/` and `open-sse/` aliases, extensionless
// paths, and stubs for npm packages ref/9router doesn't vendor. Any other package the
// import graph reaches (dashboard and CLI code these modules never call here) becomes an
// inert stub exporting the names its importer asks for.

const stubs = {
  uuid: "export const v4 = () => '00000000-0000-4000-8000-000000000000'; export const v5 = () => '00000000-0000-5000-8000-000000000000'; export default { v4, v5 };",
  undici: "export class Agent {} export class ProxyAgent {} export const fetch = globalThis.fetch; export const setGlobalDispatcher = () => {}; export default {};",
  open: "export default async () => {};",
  chalk: "const c = new Proxy((s) => s, { get: () => c }); export default c;",
  ora: "export default () => ({ start() { return this; }, succeed() {}, fail() {}, stop() {} });",
  "node-machine-id": "export const machineIdSync = () => 'oracle-machine'; export default { machineIdSync };",
};
// The hook runs in Node's loader thread: its source is this function's text, with STUBS,
// SRC and SSE defined beside it.
async function resolve(spec, ctx, next) {
  const isFile = (u) => { try { const p = fileURLToPath(u); return existsSync(p) && statSync(p).isFile(); } catch { return false; } };
  if (spec in STUBS) return { url: `data:text/javascript,${encodeURIComponent(STUBS[spec])}`, shortCircuit: true };
  let base = null;
  if (spec.startsWith("@/")) base = new URL(spec.slice(2), SRC).href;
  else if (spec.startsWith("open-sse/")) base = new URL(spec.slice("open-sse/".length), SSE).href;
  else if ((spec.startsWith(".") || spec.startsWith("/")) && ctx.parentURL?.startsWith("file:")) base = new URL(spec, ctx.parentURL).href;
  if (base) {
    const [path, query = ""] = base.split("?");
    for (const u of [path, `${path}.js`, `${path}/index.js`]) {
      if (isFile(u)) return { url: query ? `${u}?${query}` : u, format: u.endsWith(".json") ? "json" : "module", shortCircuit: true };
    }
  }
  try {
    return await next(spec, ctx);
  } catch (e) {
    if (e?.code !== "ERR_MODULE_NOT_FOUND" || spec.startsWith(".") || spec.startsWith("/") || !ctx.parentURL?.startsWith("file:")) throw e;
    const src = readFileSync(fileURLToPath(ctx.parentURL), "utf8");
    const names = new Set();
    for (const m of src.matchAll(/(?:import|export)\s*\{([^}]*)\}\s*from\s*["']([^"']+)["']/g)) {
      if (m[2] !== spec) continue;
      for (const part of m[1].split(",")) {
        const n = part.trim().split(/\s+as\s+/)[0].trim();
        if (/^[A-Za-z_$][\w$]*$/.test(n) && n !== "default") names.add(n);
      }
    }
    const body = "const s = new Proxy(function () { return s; }, { get: (t, k) => (k === Symbol.toPrimitive ? () => '' : k === 'then' ? undefined : s), construct: () => s }); export default s;"
      + [...names].map((n) => ` export const ${n} = s;`).join("");
    return { url: `data:text/javascript,${encodeURIComponent(body)}`, shortCircuit: true };
  }
}
const hooks = [
  `import { existsSync, readFileSync, statSync } from "node:fs";`,
  `import { fileURLToPath } from "node:url";`,
  `const STUBS = ${JSON.stringify(stubs)};`,
  `const SRC = ${JSON.stringify(pathToFileURL(join(REF, "src")).href + "/")};`,
  `const SSE = ${JSON.stringify(pathToFileURL(join(REF, "open-sse")).href + "/")};`,
  `export ${resolve.toString()}`,
].join("\n");
register(`data:text/javascript,${encodeURIComponent(hooks)}`);
// A fresh instance of a module (and none of its imports), for module-level caches.
let generation = 0;
const imp = (p, fresh = false) => import(pathToFileURL(join(REF, p)).href + (fresh ? `?oracle=${++generation}` : ""));

const { PROVIDERS: REG, PROVIDER_OAUTH } = await imp("open-sse/config/providers.js");
const oauthProviders = await imp("src/lib/oauth/providers/index.js");
const { CLAUDE_CONFIG, GROK_CLI_CONFIG } = await imp("src/lib/oauth/constants/oauth.js");
const { XAI_CONFIG, XAI_PKCE_VERIFIER_BYTES } = await imp("src/lib/oauth/constants/xai.js");
const xaiService = await imp("src/lib/oauth/services/xai.js");
const tokenRefresh = await imp("open-sse/services/tokenRefresh.js");
const refreshProviders = await imp("open-sse/services/tokenRefresh/providers.js");
const background = await imp("src/sse/services/backgroundTokenRefresh.js");
const { classifyOAuthProbeResult } = await imp("src/app/api/providers/[id]/test/testUtils.js");
const { getUsageForProvider } = await imp("open-sse/services/usage.js");
const { parseGrokCliBilling } = await imp("open-sse/services/usage/grok-cli.js");
const { decodeGrokCreditsFrame, probeFrameHeader } = await imp("open-sse/services/usage/grokCliQuotaFrame.js");
const { parseResetTime } = await imp("open-sse/services/usage/shared.js");
const { parseGrokCliModels, resolveGrokCliModels } = await imp("open-sse/services/grokCliModels.js");

// 9router's provider id → 0router's.
const OURS = { claude: "anthropic", xai: "xai", "grok-cli": "grok-cli", "opencode-go": "opencode-go", "opencode-zen": "opencode-zen" };

// ── Output ──────────────────────────────────────────────────────────────────

const fixDir = join(ROOT, "tests", "fixtures", "9router");
const oauthDir = join(fixDir, "oauth");
const usageDir = join(fixDir, "usage");
rmSync(oauthDir, { recursive: true, force: true });
mkdirSync(oauthDir, { recursive: true });
mkdirSync(usageDir, { recursive: true });
for (const f of readdirSync(usageDir)) if (f !== "cases.json") rmSync(join(usageDir, f));
const written = [];
const write = (dir, name, data) => {
  const out = `${JSON.stringify({ source: HEADER, data }, null, 2)}\n`;
  if (/client_secret|GOCSPX-/.test(out)) {
    console.error(`oauth-oracle.mjs: a client secret leaked into ${name}`);
    process.exit(1);
  }
  writeFileSync(join(dir, name), out);
  written.push(`${dir === oauthDir ? "oauth" : "usage"}/${name}`);
};

// Placeholders for values 9router draws at random.
const HEX16 = /^[a-f0-9]{32}$/;
const urlParts = (u) => {
  const parsed = new URL(u);
  const params = [...parsed.searchParams.entries()].map(([k, v]) => [k, k === "nonce" && HEX16.test(v) ? "{random.hex16}" : v]);
  const query = u.slice(u.indexOf("?") + 1).replace(/nonce=[a-f0-9]{32}/, "nonce={random.hex16}");
  return { base: parsed.origin + parsed.pathname, params, query };
};

// ── oauth/providers.json: each sign-in's declared data ──────────────────────

const providerData = {
  anthropic: {
    ninerouter_id: "claude",
    flow: "authorization_code_pkce",
    client_id: CLAUDE_CONFIG.clientId,
    scopes: CLAUDE_CONFIG.scopes,
    authorize_url: CLAUDE_CONFIG.authorizeUrl,
    token_url: CLAUDE_CONFIG.tokenUrl,
    redirect_uri: CLAUDE_CONFIG.redirectUri ?? null,
    token_body: "json",
    verifier_bytes: oauthProviders.PROVIDERS.claude.pkceVerifierBytes ?? 32,
    refresh_lead_ms: tokenRefresh.getRefreshLeadMs("claude"),
  },
  xai: {
    ninerouter_id: "xai",
    flow: "authorization_code_pkce",
    client_id: XAI_CONFIG.clientId,
    scopes: XAI_CONFIG.scope.split(" "),
    discovery_url: XAI_CONFIG.discoveryUrl,
    authorize_url: XAI_CONFIG.authorizeUrl,
    token_url: XAI_CONFIG.tokenUrl,
    redirect_uri: XAI_CONFIG.redirectUri,
    token_body: "form",
    verifier_bytes: XAI_PKCE_VERIFIER_BYTES,
    refresh_lead_ms: tokenRefresh.getRefreshLeadMs("xai"),
  },
  "grok-cli": {
    ninerouter_id: "grok-cli",
    flow: "device_code",
    client_id: GROK_CLI_CONFIG.clientId,
    scopes: GROK_CLI_CONFIG.scope.split(" "),
    device_url: GROK_CLI_CONFIG.deviceCodeUrl,
    token_url: GROK_CLI_CONFIG.tokenUrl,
    refresh_url: GROK_CLI_CONFIG.refreshUrl,
    params: { referrer: GROK_CLI_CONFIG.referrer },
    token_body: "form",
    refresh_lead_ms: tokenRefresh.getRefreshLeadMs("grok-cli"),
  },
};
write(oauthDir, "providers.json", {
  providers: providerData,
  default_refresh_lead_ms: tokenRefresh.TOKEN_EXPIRY_BUFFER_MS,
  background_refresh_lead_ms: background.BACKGROUND_REFRESH_LEAD_MS,
  registry_oauth_ids: Object.keys(PROVIDER_OAUTH).filter((k) => k in OURS).sort(),
});

// ── oauth/authorize.json: authorize URLs and xai discovery ──────────────────
//
// xai-oauth-service.test.js. Inputs are the test's fixed state and challenge; the nonce is
// drawn per sign-in and recorded as {random.hex16}.

const STATE = "state-1";
const CHALLENGE = "challenge-1";
const XAI_REDIRECT = "http://127.0.0.1:56121/callback";
const CLAUDE_REDIRECT = "http://localhost:54545/callback";
const authorize = [];
const discovery = [];

{
  const viaService = new xaiService.XaiService().buildXaiAuthUrl(XAI_REDIRECT, STATE, CHALLENGE, XAI_CONFIG.authorizeUrl);
  authorize.push({ name: "xai-service", provider: "xai", input: { redirect_uri: XAI_REDIRECT, state: STATE, challenge: CHALLENGE }, ...urlParts(viaService) });

  const DISCOVERED = { authorization_endpoint: "https://auth.x.ai/oauth2/authorize-from-discovery", token_endpoint: "https://auth.x.ai/oauth2/token-from-discovery" };
  const xaiProvider = (await imp("src/lib/oauth/providers/xai.js", true)).default;
  const r = await run([json(DISCOVERED)], async () => {
    const config = await xaiProvider.prepareConfig(xaiProvider.config);
    return { config: { authorizeUrl: config.authorizeUrl, tokenUrl: config.tokenUrl }, url: xaiProvider.buildAuthUrl(config, XAI_REDIRECT, STATE, CHALLENGE) };
  });
  authorize.push({ name: "xai-dashboard-discovered", provider: "xai", input: { redirect_uri: XAI_REDIRECT, state: STATE, challenge: CHALLENGE, discovery: DISCOVERED }, discovery_request: r.calls[0], ...urlParts(r.result.url) });

  const claude = oauthProviders.PROVIDERS.claude;
  authorize.push({
    name: "anthropic-dashboard",
    provider: "anthropic",
    input: { redirect_uri: CLAUDE_REDIRECT, state: STATE, challenge: CHALLENGE },
    ...urlParts(claude.buildAuthUrl(claude.config, CLAUDE_REDIRECT, STATE, CHALLENGE)),
  });

  // generateAuthData: verifier length and challenge method, with discovery.
  const g = await run([json(DISCOVERED)], async () => {
    const fresh = await imp("src/lib/oauth/providers/index.js", true);
    const d = await fresh.generateAuthData("xai", XAI_REDIRECT);
    return { verifier_len: d.codeVerifier.length, state_len: d.state.length, base: urlParts(d.authUrl).base };
  });
  authorize.push({ name: "xai-generate-auth-data", provider: "xai", ...g.result });
  const gc = await run([], async () => {
    const d = await oauthProviders.generateAuthData("claude", CLAUDE_REDIRECT);
    return { verifier_len: d.codeVerifier.length, state_len: d.state.length, base: urlParts(d.authUrl).base };
  });
  authorize.push({ name: "anthropic-generate-auth-data", provider: "anthropic", ...gc.result });

  // Discovery: endpoint validation, then which URLs a sign-in ends up with.
  for (const [value, field] of [
    ["https://auth.x.ai/oauth2/authorize", "authorization_endpoint"],
    ["https://x.ai/oauth2/token", "token_endpoint"],
    ["http://auth.x.ai/oauth2/authorize", "authorization_endpoint"],
    ["https://example.com/oauth2/authorize", "authorization_endpoint"],
    ["https://auth.x.ai.example.com/oauth2/token", "token_endpoint"],
    ["", "token_endpoint"],
    ["not a url", "token_endpoint"],
  ]) {
    let accepted = null;
    let error = null;
    try { accepted = xaiService.validateOAuthEndpoint(value, field); } catch (e) { error = e.message; }
    discovery.push({ kind: "validate", input: value, field, accepted, error });
  }
  for (const [name, answer] of [
    ["discovered", json(DISCOVERED)],
    ["discovery-404", json({ error: "not found" }, 404)],
    ["discovery-off-host", json({ ...DISCOVERED, token_endpoint: "https://evil.example.com/token" })],
    ["discovery-http", json({ ...DISCOVERED, authorization_endpoint: "http://auth.x.ai/oauth2/authorize" })],
    ["discovery-network-down", networkDown()],
  ]) {
    const fresh = await imp("src/lib/oauth/services/xai.js", true);
    const r2 = await run([answer], () => fresh.discoverEndpoints());
    discovery.push({ kind: "discover", name, response: answer instanceof Response ? await answer.clone().json() : "network failure", request: r2.calls[0], result: r2.result });
  }
}
write(oauthDir, "authorize.json", { cases: authorize, discovery });

// ── oauth/token-requests.json: code exchange and device grant ───────────────

const tokenRequests = [];
const TOKENS = { access_token: "access-token", refresh_token: "refresh-token", expires_in: 3600, scope: "openid offline_access", token_type: "Bearer" };
{
  const DISCOVERED = { authorization_endpoint: "https://auth.x.ai/oauth2/authorize", token_endpoint: "https://auth.x.ai/oauth2/token-from-discovery" };
  // exchangeTokens("xai", ...) step by step on a fresh xai module, so its discovery cache is
  // empty and the discovery answer is the one scripted here.
  const xp = (await imp("src/lib/oauth/providers/xai.js", true)).default;
  const xaiTokens = { ...TOKENS, id_token: `h.${Buffer.from(JSON.stringify({ email: "xai@example.com", sub: "s-2" })).toString("base64url")}.s` };
  const r = await run([json(DISCOVERED), json(xaiTokens)], async () => {
    const config = await xp.prepareConfig(xp.config);
    return xp.mapTokens(await xp.exchangeToken(config, "auth-code", XAI_REDIRECT, "verifier-1", STATE));
  });
  tokenRequests.push({ name: "xai-code-exchange", provider: "xai", grant: "authorization_code", input: { code: "auth-code", redirect_uri: XAI_REDIRECT, verifier: "verifier-1", state: STATE }, response: xaiTokens, ...r });

  for (const [name, code] of [["anthropic-code-exchange", "auth-code"], ["anthropic-code-hash-state", "auth-code#state-from-page"]]) {
    const rc = await run([json(TOKENS)], () => oauthProviders.exchangeTokens("claude", code, CLAUDE_REDIRECT, "verifier-1", STATE));
    tokenRequests.push({ name, provider: "anthropic", grant: "authorization_code", input: { code, redirect_uri: CLAUDE_REDIRECT, verifier: "verifier-1", state: STATE }, response: TOKENS, ...rc });
  }

  const DEVICE = { device_code: "dev-code-1", user_code: "ABCD-EFGH", verification_uri: "https://accounts.x.ai/device", verification_uri_complete: "https://accounts.x.ai/device?user_code=ABCD-EFGH", expires_in: 600, interval: 5 };
  const rd = await run([json(DEVICE)], () => oauthProviders.requestDeviceCode("grok-cli", "challenge-ignored"));
  tokenRequests.push({ name: "grok-cli-device-request", provider: "grok-cli", grant: "device_request", response: DEVICE, ...rd });
  for (const [name, answer] of [
    ["grok-cli-device-pending", json({ error: "authorization_pending" }, 400)],
    ["grok-cli-device-slow-down", json({ error: "slow_down" }, 400)],
    ["grok-cli-device-expired", json({ error: "expired_token", error_description: "expired" }, 400)],
    ["grok-cli-device-denied", json({ error: "access_denied" }, 400)],
  ]) {
    const rp = await run([answer], () => oauthProviders.pollForToken("grok-cli", "dev-code-1", "verifier-ignored", {}));
    tokenRequests.push({ name, provider: "grok-cli", grant: "device_poll", response: await answer.clone().json(), ...rp });
  }
  const idToken = `h.${Buffer.from(JSON.stringify({ email: "grok@example.com", sub: "s-1" })).toString("base64url")}.s`;
  const USER = { userId: "user-0001", email: "profile@example.com", firstName: "Ada", lastName: "L", hasGrokCodeAccess: true, subscriptionTier: "SuperGrok" };
  const granted = { ...TOKENS, id_token: idToken };
  const rg = await run([json(granted), json(USER)], () => oauthProviders.pollForToken("grok-cli", "dev-code-1", "verifier-ignored", {}));
  if (rg.result?.tokens?.expiresAt) rg.result.tokens.expiresAt = "{now + expires_in}";
  tokenRequests.push({ name: "grok-cli-device-granted", provider: "grok-cli", grant: "device_poll", response: granted, profile: USER, ...rg });
}
write(oauthDir, "token-requests.json", { cases: tokenRequests });

// ── oauth/refresh.json: refresh requests, rotation and dispatch ─────────────
//
// xai-tokenRefresh, token-refresh-dispatch and token-refresh-generic (claude only: the
// other providers there aren't in 0router's bundle, and iflow's body carries a secret).

const refresh = [];
const REFRESHED = { access_token: "new-access", refresh_token: "new-refresh", expires_in: 900, id_token: "id-token" };
const { refresh_token: _drop, ...NOT_ROTATED } = REFRESHED;
// xai and grok-cli discover their token URL once per process: warm it, then script only
// token answers.
const warm = await run([json({ authorization_endpoint: XAI_CONFIG.authorizeUrl, token_endpoint: XAI_CONFIG.tokenUrl })], () => xaiService.discoverEndpoints());
const discoveryRequest = warm.calls[0];
for (const provider of ["xai", "grok-cli", "claude"]) {
  for (const [name, response] of [["rotated", REFRESHED], ["not-rotated", NOT_ROTATED]]) {
    const rt = `old-refresh-${provider}-${name}`;
    const r = await run([json(response)], () => tokenRefresh.refreshTokenByProvider(provider, { refreshToken: rt }, quiet));
    refresh.push({ name: `${OURS[provider]}-${name}`, provider: OURS[provider], input: { refresh_token: rt }, response, calls: r.calls, result: r.result, error: r.error });
  }
}
const guards = [];
for (const [provider, creds] of [["claude", {}], ["claude", { refreshToken: 123 }], ["claude", { refreshToken: "" }], ["xai", { refreshToken: "" }], ["totally-unknown", { refreshToken: "x" }]]) {
  const a = await run([], () => tokenRefresh.getAccessToken(provider, creds, quiet));
  guards.push({ fn: "getAccessToken", provider, credentials: creds, result: a.result, calls: a.calls.length });
  if (provider !== "totally-unknown" && typeof creds.refreshToken !== "number") {
    const b = await run([], () => tokenRefresh.refreshTokenByProvider(provider, creds, quiet));
    guards.push({ fn: "refreshTokenByProvider", provider, credentials: creds, result: b.result, calls: b.calls.length });
  }
}
write(oauthDir, "refresh.json", { discovery_request: discoveryRequest, cases: refresh, guards });

// ── oauth/refresh-errors.json: what a failed refresh means ──────────────────
//
// classifyOAuthRefreshError (codex's classifier, 9router's only one) and isUnrecoverable-
// RefreshError, beside what the refresh call itself returns for each provider: xai and
// grok-cli map invalid_grant/invalid_request to {error: "invalid_grant"}; claude returns
// null for every failure.

const ERRORS = [
  ["invalid-grant", 400, { error: "invalid_grant", error_description: "Refresh token expired" }],
  ["invalid-request", 400, { error: "invalid_request" }],
  ["unauthorized-client", 401, { error: "unauthorized_client" }],
  ["refresh-token-reused", 400, { error: "refresh_token_reused" }],
  ["refresh-token-expired-nested", 400, { error: { code: "refresh_token_expired", message: "expired" } }],
  ["refresh-token-invalidated", 401, { error: "refresh_token_invalidated" }],
  ["invalid-scope", 400, { error: "invalid_scope" }],
  ["access-denied-403", 403, { error: "access_denied" }],
  ["error-code-field", 400, { error_code: "invalid_grant" }],
  ["unauthorized-text", 401, "unauthorized"],
  ["rate-limited", 429, { error: "rate_limited" }],
  ["server-error", 500, { error: "server_error" }],
  ["unavailable-html", 503, "<html>Service Unavailable</html>"],
  ["not-found", 404, { error: "not_found" }],
  ["network-down", null, null],
];
const refreshErrors = [];
for (const [name, status, body] of ERRORS) {
  const raw = body === null ? "" : typeof body === "string" ? body : JSON.stringify(body);
  const answer = () => (status === null ? networkDown()() : typeof body === "string" ? text(body, status) : json(body, status));
  const classified = status === null ? null : refreshProviders.classifyOAuthRefreshError(raw, status);
  const results = {};
  for (const provider of ["xai", "grok-cli", "claude"]) {
    const r = await run([answer], () => tokenRefresh.refreshTokenByProvider(provider, { refreshToken: `rt-${name}-${provider}` }, quiet));
    results[OURS[provider]] = { result: r.result, unrecoverable: Boolean(tokenRefresh.isUnrecoverableRefreshError(r.result)), calls: r.calls.length };
  }
  refreshErrors.push({ name, status, body, classify: classified, refresh: results });
}
write(oauthDir, "refresh-errors.json", { cases: refreshErrors });

// ── oauth/refresh-schedule.json: which accounts the background task refreshes ─

const NOW = Date.parse("2026-08-01T12:00:00.000Z");
const conn = (o = {}) => ({ id: "c1", provider: "grok-cli", authType: "oauth", refreshToken: "rt-1", expiresAt: new Date(NOW + 10 * 60 * 1000).toISOString(), isActive: true, ...o });
const schedule = [];
for (const provider of ["grok-cli", "xai", "claude"]) {
  for (const [name, minutes, o] of [
    ["expires-in-10m", 10, {}],
    ["expires-in-29m", 29, {}],
    ["expires-in-31m", 31, {}],
    ["expires-in-2h", 120, {}],
    ["expires-in-3h59m", 239, {}],
    ["expires-in-4h1m", 241, {}],
    ["expired-1m-ago", -1, {}],
    ["apikey", 1, { authType: "apikey" }],
    ["api_key", 1, { authType: "api_key" }],
    ["no-refresh-token", 1, { refreshToken: null }],
    ["no-expiry", null, { expiresAt: undefined }],
  ]) {
    const c = conn({ provider, ...(minutes === null ? {} : { expiresAt: new Date(NOW + minutes * 60 * 1000).toISOString() }), ...o });
    if (minutes === null) delete c.expiresAt;
    schedule.push({ name, provider: OURS[provider], ninerouter_provider: provider, auth_type: c.authType, has_refresh_token: Boolean(c.refreshToken), expires_in_minutes: minutes, selected: background.selectConnectionsNeedingRefresh([c], NOW).length === 1 });
  }
}
write(oauthDir, "refresh-schedule.json", { now: new Date(NOW).toISOString(), cases: schedule });

// ── oauth/probe.json: the grok-cli connection test (grok-cli-oauth-probe) ───

const GROK_CLI_PROBE = {
  url: REG["grok-cli"]?.userUrl || "https://cli-chat-proxy.grok.com/v1/user",
  method: "GET",
  acceptStatuses: [402],
  softFailMessage: { 402: "Connected, but Grok Build credits are exhausted (spending limit). Add credits or upgrade SuperGrok." },
};
const probe = [];
for (const [name, res, body, config] of [
  ["ok-200", { ok: true, status: 200 }, "", GROK_CLI_PROBE],
  ["spending-limit-402", { ok: false, status: 402 }, JSON.stringify({ code: "personal-team-blocked:spending-limit", error: "You have run out of credits" }), GROK_CLI_PROBE],
  ["unauthorized-401", { ok: false, status: 401 }, "unauthorized", GROK_CLI_PROBE],
  ["forbidden-403", { ok: false, status: 403 }, "", GROK_CLI_PROBE],
  ["server-500", { ok: false, status: 500 }, "", GROK_CLI_PROBE],
  ["codex-accept-400", { ok: false, status: 400 }, "bad request", { acceptStatuses: [400] }],
]) {
  probe.push({ name, status: res.status, body, accept_statuses: config.acceptStatuses, result: classifyOAuthProbeResult(res, config, body) });
}
write(oauthDir, "probe.json", { cases: probe });

// ── usage/quota-*.json: quota responses in, windows out ─────────────────────

const quotaCase = (name, provider, input, r) => ({ name, provider, input, requests: r.calls, result: r.result, error: r.error });

// claude: OAuth usage endpoint. Distinct tokens: getClaudeUsage caches per token.
const claudeQuota = [];
{
  const bodies = [
    ["windows-and-models", {
      five_hour: { utilization: 87, resets_at: "2026-10-02T18:00:00Z" },
      seven_day: { utilization: 40, resets_at: "2026-10-07T00:00:00Z" },
      seven_day_opus: { utilization: 12.5, resets_at: "2026-10-07T00:00:00Z" },
      seven_day_sonnet: { utilization: 3, resets_at: 1791331200 },
      seven_day_oauth_apps: null,
      extra_usage: null,
      limits: [
        { kind: "weekly_scoped", percent: 30, resets_at: "2026-10-07T00:00:00Z", scope: { model: { display_name: "Fable" } } },
        { kind: "other", percent: 99 },
      ],
    }],
    ["limits-clamped-and-skipped", {
      five_hour: { utilization: 0, resets_at: null },
      limits: [
        { kind: "weekly_scoped", percent: 140, resets_at: "2026-10-07T00:00:00Z", scope: { model: { display_name: " Opus 4 " } } },
        { kind: "weekly_scoped", percent: -5, resets_at: "2026-10-07T00:00:00Z", scope: { model: { display_name: "Haiku" } } },
        { kind: "weekly_scoped", percent: "50", scope: { model: { display_name: "Sonnet" } } },
        { kind: "weekly_scoped", percent: 10, scope: { model: {} } },
      ],
    }],
    ["utilization-not-a-number", {
      five_hour: { utilization: "87", resets_at: "2026-10-02T18:00:00Z" },
      seven_day: { resets_at: "2026-10-07T00:00:00Z" },
      seven_day_opus: { utilization: 101, resets_at: "1791331200000" },
      extra_usage: { is_enabled: true, monthly_limit: 50, used_credits: 12 },
    }],
    ["empty", {}],
  ];
  let n = 0;
  for (const [name, body] of bodies) {
    const token = `claude-token-${++n}`;
    const r = await run([json(body)], () => getUsageForProvider({ provider: "claude", accessToken: token }));
    claudeQuota.push(quotaCase(name, "anthropic", { response: { status: 200, body } }, r));
  }
}
write(usageDir, "quota-claude.json", { cases: claudeQuota });

// grok-cli: billing JSON (+ user profile) and the gRPC-web fallback.
function encodeVarint(value) {
  const bytes = [];
  let v = BigInt(value);
  do {
    let byte = Number(v & 0x7fn);
    v >>= 7n;
    if (v !== 0n) byte |= 0x80;
    bytes.push(byte);
  } while (v !== 0n);
  return Buffer.from(bytes);
}
const tag = (field, wire) => encodeVarint((field << 3) | wire);
const fixed32 = (field, value) => { const b = Buffer.alloc(4); b.writeFloatLE(value, 0); return Buffer.concat([tag(field, 5), b]); };
const fixed64 = (field, value) => { const b = Buffer.alloc(8); b.writeDoubleLE(value, 0); return Buffer.concat([tag(field, 1), b]); };
const lengthDelimited = (field, body) => Buffer.concat([tag(field, 2), encodeVarint(body.length), body]);
const varintField = (field, value) => Buffer.concat([tag(field, 0), encodeVarint(value)]);
const timestamp = (field, seconds, nanos) => lengthDelimited(field, Buffer.concat([...(seconds ? [varintField(1, seconds)] : []), ...(nanos ? [varintField(2, nanos)] : [])]));
const creditsInfo = (s) => Buffer.concat([
  ...(s.ratio !== undefined ? [fixed32(1, s.ratio)] : []),
  ...(s.ratio64 !== undefined ? [fixed64(1, s.ratio64)] : []),
  ...(s.asOf !== undefined ? [timestamp(4, s.asOf, s.asOfNanos ?? 0)] : []),
  ...(s.reset !== undefined ? [timestamp(5, s.reset, s.resetNanos ?? 0)] : []),
]);
const top = (info) => lengthDelimited(1, info);
const frame = (payload, flag = 0x00) => { const h = Buffer.alloc(5); h[0] = flag; h.writeUInt32BE(payload.length, 1); return Buffer.concat([h, payload]); };
const trailer = (t = "grpc-status:0\r\n") => frame(Buffer.from(t, "utf8"), 0x80);
const RESET = 1784825940;
const RESET_NANOS = 867850000;

const EXHAUSTED = { config: { currentPeriod: { type: "USAGE_PERIOD_TYPE_WEEKLY", start: "2026-07-08T00:00:00+00:00", end: "2026-07-15T00:00:00+00:00" }, onDemandCap: { val: 0 }, onDemandUsed: { val: 0 }, isUnifiedBillingUser: true, prepaidBalance: { val: 0 }, topUpMethod: "TOP_UP_METHOD_SAVED_PAYMENT_METHOD", billingPeriodStart: "2026-07-08T00:00:00+00:00", billingPeriodEnd: "2026-07-15T00:00:00+00:00" } };
const ACTIVE = { config: { currentPeriod: { type: "USAGE_PERIOD_TYPE_WEEKLY", start: "2026-07-08T00:00:00+00:00", end: "2026-07-15T00:00:00+00:00" }, onDemandCap: { val: 100 }, onDemandUsed: { val: 35 }, isUnifiedBillingUser: true, prepaidBalance: { val: 12.5 }, billingPeriodStart: "2026-07-08T00:00:00+00:00", billingPeriodEnd: "2026-07-15T00:00:00+00:00" } };
const USER = { userId: "d84768dd-224d-4052-ba49-0d336fa9160c", email: "user@example.com", hasGrokCodeAccess: true, subscriptionTier: null };
const SUPERGROK = { config: { currentPeriod: { type: "USAGE_PERIOD_TYPE_WEEKLY", start: "2026-07-17T12:42:26.494595+00:00", end: "2026-07-24T12:42:26.494595+00:00" }, creditUsagePercent: 99.0, onDemandCap: { val: 0 }, onDemandUsed: { val: 0 }, productUsage: [{ product: "GrokBuild", usagePercent: 97.0 }, { product: "GrokImagine", usagePercent: 2.0 }], isUnifiedBillingUser: true, prepaidBalance: { val: 0 }, billingPeriodStart: "2026-07-17T12:42:26.494595+00:00", billingPeriodEnd: "2026-07-24T12:42:26.494595+00:00" } };
const MONTHLY = { monthlyLimit: { val: 1000 }, includedUsed: { val: 275 }, totalUsed: { val: 300 }, resetAt: "2026-08-01T00:00:00Z" };
// 0router's testkit sample (crates/nullrouter-engine/src/testkit/mock_quota.rs), so later
// tests built on it have 9router's answer.
const TESTKIT_BILLING = { config: { currentPeriod: { type: "USAGE_PERIOD_TYPE_WEEKLY", start: "2026-09-28T00:00:00Z", end: "2026-10-05T00:00:00Z" }, monthlyLimit: { val: 2500 }, monthlyUsed: { val: 1200 }, onDemandCap: { val: 1000 }, onDemandUsed: { val: 250 }, prepaidBalance: { val: 500 }, isUnifiedBillingUser: true, billingPeriodStart: "2026-10-01T00:00:00Z", billingPeriodEnd: "2026-11-01T00:00:00Z" } };
const TESTKIT_USER = { userId: "user-0001", email: "user@example.com", subscriptionTier: "SuperGrok", hasGrokCodeAccess: true };

const grokParse = [];
for (const [name, billing, user] of [
  ["active", ACTIVE, USER],
  ["exhausted-free", EXHAUSTED, USER],
  ["tier-super-grok", ACTIVE, { ...USER, subscriptionTier: "super_grok" }],
  ["paid-subscription-no-quota", EXHAUSTED, { ...USER, subscriptionTier: "XPremiumPlus" }],
  ["credit-usage-percent", SUPERGROK, { subscriptionTier: "XPremiumPlus", hasGrokCodeAccess: true }],
  ["monthly-included", MONTHLY, { subscription_tier: "premium_plus" }],
  ["monthly-total-used-only", { monthlyLimit: 400, totalUsed: "120", billing_period_end: 1790000000 }, null],
  ["credits-bag-remaining", { credits: { total: { val: 50 }, remaining: { val: 20 }, resetAt: "2026-09-01T00:00:00Z" } }, null],
  ["credits-bag-balance-only", { config: { includedCredits: { balance: 0 } } }, null],
  ["testkit-sample", TESTKIT_BILLING, TESTKIT_USER],
]) {
  const { rawConfig: _raw, ...parsed } = parseGrokCliBilling(billing, user);
  grokParse.push({ name, billing, user, result: parsed });
}

const accessTokenWithTier = (tier) => `header.${Buffer.from(JSON.stringify({ tier })).toString("base64url")}.signature`;
const grokFlow = [];
for (const [name, token, psd, answers] of [
  ["billing-and-user", "test-token", { email: "user@example.com", userId: USER.userId }, [json(ACTIVE), json(USER)]],
  ["billing-401", "expired", null, [json({ error: "unauthorized" }, 401), json(USER)]],
  ["billing-403", "expired", null, [json({ error: "forbidden" }, 403), json(USER)]],
  ["billing-500", "test-token", null, [text("upstream exploded", 500), json(USER)]],
  ["exhausted-free", "test-token", null, [json(EXHAUSTED), json(USER)]],
  ["grpc-fallback", accessTokenWithTier(5), null, [json(EXHAUSTED), json({ ...USER, subscriptionTier: "XPremiumPlus" }), binary(frame(top(creditsInfo({ ratio: 0.35, reset: RESET, resetNanos: RESET_NANOS }))))]],
  ["grpc-fallback-500", "test-token", null, [json(EXHAUSTED), json({ ...USER, subscriptionTier: "XPremiumPlus" }), binary(Buffer.alloc(0), 500)]],
  ["grpc-fallback-network-down", "test-token", null, [json(EXHAUSTED), json({ ...USER, subscriptionTier: "XPremiumPlus" }), networkDown()]],
  ["user-network-down", "test-token", null, [json(ACTIVE), networkDown()]],
]) {
  const r = await run(answers, () => getUsageForProvider({ provider: "grok-cli", accessToken: token, ...(psd ? { providerSpecificData: psd } : {}) }));
  const responses = [];
  for (const a of answers) {
    if (typeof a === "function") responses.push({ network_failure: true });
    else if ((a.headers.get("content-type") || "").includes("grpc")) responses.push({ status: a.status, hex: Buffer.from(await a.clone().arrayBuffer()).toString("hex") });
    else { const t = await a.clone().text(); let body = t; try { body = JSON.parse(t); } catch { /* text */ } responses.push({ status: a.status, body }); }
  }
  grokFlow.push(quotaCase(name, "grok-cli", { access_token: token, provider_specific_data: psd, responses }, r));
}
write(usageDir, "quota-grok-cli.json", { parse: grokParse, flow: grokFlow });

// gRPC-web frames (grok-cli-quota-frame): bytes in, {percentUsed, resetAt} or null out.
const frames = [];
const real = creditsInfo({ ratio: 1.0, asOf: 1784221140, asOfNanos: RESET_NANOS, reset: RESET, resetNanos: RESET_NANOS });
const half = top(creditsInfo({ ratio: 0.5, reset: RESET }));
const truncated = frame(top(creditsInfo({ ratio: 0.5, reset: RESET, resetNanos: RESET_NANOS })));
for (const [name, buf] of [
  ["real-shape-with-trailer", Buffer.concat([frame(top(real)), trailer()])],
  ["half-without-trailer", frame(half)],
  ["half-with-trailer", Buffer.concat([frame(half), trailer()])],
  ["raw-unframed", top(creditsInfo({ ratio: 0.75, reset: RESET, resetNanos: RESET_NANOS }))],
  ["ratio-omitted", frame(top(creditsInfo({ reset: RESET, resetNanos: RESET_NANOS })))],
  ["ratio-above-one", frame(top(creditsInfo({ ratio: 1.5 })))],
  ["ratio-negative", frame(top(creditsInfo({ ratio: -0.1 })))],
  ["ratio-fixed64", frame(top(creditsInfo({ ratio64: 0.35, reset: RESET })))],
  ["ratio-0.35-fixed32", frame(top(creditsInfo({ ratio: 0.35, reset: RESET, resetNanos: RESET_NANOS })))],
  ["top-field-not-length-delimited", frame(varintField(1, 42))],
  ["nested-ratio-wrong-wire-type", frame(top(lengthDelimited(1, Buffer.from("not-a-float", "utf8"))))],
  ["no-top-field-1", frame(varintField(9, 1))],
  ["truncated", truncated.subarray(0, truncated.length - 3)],
  ["trailer-only", trailer()],
  ["empty", Buffer.alloc(0)],
  ["testkit-sample", Buffer.concat([frame(top(creditsInfo({ ratio: 0.25, reset: 1759622400 }))), trailer()])],
]) {
  frames.push({ name, hex: buf.toString("hex"), result: decodeGrokCreditsFrame(buf) });
}
const headers = [];
{
  const over = Buffer.alloc(5); over.writeUInt32BE(9999, 1);
  const badFlag = Buffer.alloc(5); badFlag[0] = 0x07;
  const two = Buffer.concat([frame(top(creditsInfo({ ratio: 0.5 }))), trailer()]);
  const first = probeFrameHeader(two);
  for (const [name, buf, offset] of [
    ["length-exceeds-body", Buffer.concat([over, Buffer.from([1, 2])]), 0],
    ["invalid-flag", badFlag, 0],
    ["trailer", trailer(), 0],
    ["data-then-trailer-first", two, 0],
    ["data-then-trailer-second", two, first.payloadStart + first.payloadLength],
    ["compressed-flag", frame(Buffer.from([1]), 0x01), 0],
    ["short", Buffer.from([0, 0, 0]), 0],
  ]) headers.push({ name, hex: buf.toString("hex"), offset, result: probeFrameHeader(buf, offset) });
}
write(usageDir, "grpc-web.json", { empty_request_frame_hex: "0000000000", frames, headers });

// opencode-go / opencode-zen.
const opencode = [];
const OC_OK = { usage: { rolling: { status: "ok", percent: 13, resetsAt: "2026-09-04T14:28:02.617Z" }, weekly: { status: "ok", percent: 5, resetsAt: "2026-09-07T00:00:00.617Z" }, monthly: { status: "ok", percent: 2, resetsAt: "2026-10-02T12:14:24.617Z" } } };
for (const provider of ["opencode-go", "opencode-zen"]) {
  for (const [name, key, answers] of [
    ["subscription-usage", "sk-test", [json(OC_OK)]],
    ["string-percent-and-clamp", "sk-test", [json({ usage: { rolling: { percent: "40.5", resetsAt: 1790000000 }, weekly: { percent: 130 }, monthly: { percent: -3, resetsAt: "1790000000000" } } })]],
    ["no-key", null, []],
    ["unauthorized-401", "bad", [json({ error: "unauthorized" }, 401)]],
    ["entitlement-403", "sk-without", [json({ error: { type: "EntitlementError" } }, 403)]],
    ["forbidden-403", "sk-without", [json({ error: { type: "Other" } }, 403)]],
    ["no-valid-percent", "sk-test", [json({ usage: { rolling: { status: "ok" }, future: { percent: 10 } } })]],
    ["no-usage-object", "sk-test", [json({ data: [] })]],
    ["server-500", "sk-test", [json({ error: "unavailable" }, 500)]],
    ["network-down", "sk-test", [networkDown("socket closed")]],
  ]) {
    const r = await run(answers, () => getUsageForProvider({ provider, ...(key ? { apiKey: key } : {}) }));
    const responses = [];
    for (const a of answers) responses.push(typeof a === "function" ? { network_failure: true } : { status: a.status, body: await a.clone().json() });
    opencode.push(quotaCase(name, provider, { api_key: key, responses }, r));
  }
}
write(usageDir, "quota-opencode.json", { cases: opencode });

// parseResetTime: the `auto` reset format.
const resets = [];
for (const v of [1790000000, 1790000000000, 999999999999, 1000000000000, "1790000000", "1790000000000", "2026-10-07T00:00:00Z", "2026-07-15T00:00:00+00:00", "2026-07-24T12:42:26.494595+00:00", "", null, 0]) {
  resets.push({ input: v, result: parseResetTime(v) });
}
write(usageDir, "reset-time.json", { cases: resets });

// usage-dispatch: which providers 9router reads quota for.
const dispatch = [];
for (const provider of ["claude", "xai", "grok-cli", "opencode-go", "opencode-zen", "openrouter", "elevenlabs", "totally-unknown"]) {
  const r = await run([() => json({})], () => getUsageForProvider({ provider }));
  dispatch.push({ provider, ours: OURS[provider] ?? (provider === "totally-unknown" ? null : provider), supported: r.result?.message !== `Usage API not implemented for ${provider}`, result_if_unsupported: r.result?.message === `Usage API not implemented for ${provider}` ? r.result : null });
}
write(usageDir, "dispatch.json", { cases: dispatch });

// grok-cli live models (grok-cli-models): parsing, and refresh-and-retry on 401.
const modelsParse = [];
for (const [name, body] of [
  ["models-metadata", { models: [{ model_id: "grok-build", display_name: "Grok Build", context_window: 500000, max_output_tokens: 64000, supported_in_api: false }] }],
  ["data-ids", { data: [{ id: "grok-build" }, { id: "grok-4" }] }],
  ["empty", {}],
]) modelsParse.push({ name, body, result: parseGrokCliModels(body) });
const modelsFlow = [];
{
  // The 401 refreshes through refreshProviderCredentials (xai's token endpoint), then retries.
  const answers = [json({ error: "expired" }, 401), json({ access_token: "new-token", refresh_token: "new-refresh", expires_in: 900 }), json({ data: [{ id: "grok-build" }] })];
  const refreshed = [];
  const r = await run(answers, () => resolveGrokCliModels(
    { accessToken: "old-token", refreshToken: "refresh-token-models", providerSpecificData: { email: "user@example.com" } },
    { fetchFn: (url, init) => globalThis.fetch(url, init), proxyOptions: null, onCredentialsRefreshed: (t) => refreshed.push(t) },
  ));
  for (const t of refreshed) {
    if (t?.expiresAt) t.expiresAt = "{now + expires_in}";
    if (t?.lastRefreshAt) t.lastRefreshAt = "{now}";
  }
  modelsFlow.push({ name: "refresh-and-retry", responses: [{ status: 401, body: { error: "expired" } }, { status: 200, body: { access_token: "new-token", refresh_token: "new-refresh", expires_in: 900 } }, { status: 200, body: { data: [{ id: "grok-build" }] } }], requests: r.calls, result: r.result, error: r.error, refreshed });
}
write(usageDir, "models-grok-cli.json", { parse: modelsParse, flow: modelsFlow });

console.log(`  oauth/usage oracle: ${written.join(", ")}`);
