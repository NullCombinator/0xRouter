#!/usr/bin/env node
// Generates the bundled provider set, the core credential table, and the 9router
// parity oracle from the *evaluated* ref/9router registry (research R8, R9).
//
//   node tools/gen-bundled/generate.mjs
//
// Outputs (all committed; this script is their only writer):
//   plugins/bundled/<id>.toml
//   crates/zerorouter-registry/src/schema/oauth_params.rs
//   crates/zerorouter-registry/src/schema/section_formats.rs
//   crates/zerorouter-registry/src/credentials/bundled.rs
//   tests/fixtures/9router/*.json
//
// Any registry key this script does not know how to map is a hard error: keys are
// never dropped silently (R4).

import { execFileSync } from "node:child_process";
import { mkdirSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const REF = join(ROOT, "ref", "9router");
const imp = (p) => import(join(REF, p));

const { default: REGISTRY } = await imp("open-sse/providers/registry/index.js");
const { PROVIDERS, PROVIDER_MODELS } = await imp("open-sse/providers/index.js");
const { TTS_MODELS_CONFIG } = await imp("open-sse/config/ttsModels.js");
const { OAUTH_ENDPOINTS } = await imp("open-sse/config/appConstants.js");
const { resolveProviderAlias } = await imp("open-sse/services/model.js");
const PM = await imp("open-sse/config/providerModels.js");
const { CODEX_REVIEW_SUFFIX, isMuseSparkModel } = await imp("open-sse/providers/models/helpers.js");

const SHA = execFileSync("git", ["-C", REF, "rev-parse", "--short", "HEAD"]).toString().trim();
const HEADER = `Generated from ref/9router@${SHA} by tools/gen-bundled/generate.mjs — do not edit.`;

const errors = [];
const fail = (msg) => errors.push(msg);
const notes = [];

// ── Key tables ─────────────────────────────────────────────────────────────

// 9router DOT_VERSION_PROVIDERS (config/providerModels.js) → plugin flag (FR-020).
const DOT_VERSION_PROVIDERS = new Set(["kr", "kiro"]);

// 9router MEDIA_ONLY_ALIASES (services/model.js) fold into the owning plugin's aliases.
const MEDIA_ONLY_ALIASES = { el: "elevenlabs", jina: "jina-ai", "jina-ai": "jina-ai", polly: "aws-polly", "aws-polly": "aws-polly" };

const QUIRKS = {
  preserveCacheControl: "preserve_cache_control",
  dropClientMetadata: "drop_client_metadata",
  clineEnvelope: "cline_envelope",
  dropOutputConfig: "drop_output_config",
  requireClaudeToolType: "require_claude_tool_type",
  cloakToolsOnOAuth: "cloak_tools_on_oauth",
};
const QUIRK_LISTS = {
  claudeSupportedToolTypes: "claude_supported_tool_types",
  forceAutoToolChoiceModels: "force_auto_tool_choice_models",
};
const AUTH_HOOKS = { clineHeaders: "cline_headers", kimiHeaders: "kimi_headers", kilocodeOrg: "kilocode_org" };

// Transport keys the core interprets, JS → TOML. Values are copied verbatim.
const TRANSPORT_SCALARS = {
  baseUrl: "base_url", baseUrls: "base_urls", format: "format", forceStream: "force_stream",
  urlSuffix: "url_suffix", timeoutMs: "timeout_ms", stallTimeoutMs: "stall_timeout_ms",
  thinkingFormat: "thinking_format", defaultRegion: "default_region", validateUrl: "validate_url",
  modelsUrl: "models_url", responsesUrl: "responses_url", messagesUrl: "messages_url",
  chatPath: "chat_path", userUrl: "user_url", billingUrl: "billing_url", refreshUrl: "refresh_url",
  tokenUrl: "token_url", authUrl: "auth_url", clientId: "client_id",
};
// Open maps: keys kept verbatim (headers are HTTP names; the rest are 9router-internal names).
const TRANSPORT_MAPS = { headers: "headers", reasoningInject: "reasoning_inject", usage: "usage", regions: "regions" };
// Closed long-tail struct (data model ExecutorParams).
const EXECUTOR_PARAMS = {
  cliVersion: "cli_version", clientVersion: "client_version", apiClient: "api_client",
  clientIdentifier: "client_identifier", tokenAuth: "token_auth", noAuth: "no_auth", authType: "auth_type",
};
const COPILOT_PARAMS = { vscodeVersion: "vscode_version", chatVersion: "chat_version", userAgent: "user_agent", apiVersion: "api_version" };
const TRANSPORT_AUTH = { combined: "combined", header: "header", scheme: "scheme", anthropicVersion: "anthropic_version" };

const OAUTH_TYPED = {
  clientId: "client_id", authorizeUrl: "authorize_url", tokenUrl: "token_url", refreshUrl: "refresh_url",
  deviceCodeUrl: "device_code_url", userInfoUrl: "user_info_url", codeChallengeMethod: "code_challenge_method",
  refreshLeadMs: "refresh_lead_ms",
};

const MODEL_KEYS = {
  id: "id", name: "name", kind: "kind", type: "kind", upstreamModelId: "upstream_id",
  targetFormat: "target_format", supportedFormats: "supported_formats", quotaFamily: "quota_family",
  strip: "strip", contextLength: "context_length", maxOutputTokens: "max_output_tokens",
  dimensions: "dimensions", rateMultiplier: "rate_multiplier", capabilities: "capabilities",
  params: "params", description: "description", thinking: "thinking", imageGen: "image_gen",
};

const KINDS = {
  llm: "llm", image: "image", imageToText: "image_to_text", video: "video", tts: "tts", stt: "stt",
  embedding: "embedding", webSearch: "web_search", webFetch: "web_fetch", systemone: "systemone",
};
const CONFIG_KINDS = {
  ttsConfig: "tts", sttConfig: "stt", embeddingConfig: "embedding", imageConfig: "image",
  imageToTextConfig: "image_to_text", videoConfig: "video", searchConfig: "web_search",
  fetchConfig: "web_fetch", systemoneConfig: "systemone",
};
const SECTION_ENDPOINT = {
  baseUrl: "base_url", authType: "auth_type", authHeader: "auth_header", format: "format",
  headers: "headers", method: "method", timeoutMs: "timeout_ms", defaultModel: "default_model",
  pollUrl: "poll_url", bodyFields: "body_fields", validateUrl: "validate_url",
  searchTypes: "search_types", formats: "formats",
};
const SECTION_LIMITS = {
  costPerQuery: "cost_per_query", freeMonthlyQuota: "free_monthly_quota", maxMaxResults: "max_max_results",
  defaultMaxResults: "default_max_results", cacheTTLMs: "cache_ttl_ms", creditsPerResult: "credits_per_result",
  maxCharacters: "max_characters",
};

const DISPLAY = {
  name: "name", icon: "icon", color: "color", textIcon: "text_icon", website: "website",
  deprecated: "deprecated", deprecationNotice: "deprecation_notice",
};
const NOTICE = { apiKeyUrl: "api_key_url", signupUrl: "signup_url", text: "text" };

const snake = (k) =>
  k.replace(/([A-Z]+)([A-Z][a-z])/g, "$1_$2").replace(/([a-z0-9])([A-Z])/g, "$1_$2").toLowerCase();

function mapKeys(obj, table, where) {
  const out = {};
  for (const [k, v] of Object.entries(obj)) {
    if (v === undefined) continue;
    if (!(k in table)) { fail(`${where}.${k}: unmapped key`); continue; }
    out[table[k]] = v;
  }
  return out;
}

function onlyKeys(obj, allowed, where) {
  for (const k of Object.keys(obj)) if (!allowed.includes(k)) fail(`${where}.${k}: unmapped key`);
}

// ── Per-entry mapping ──────────────────────────────────────────────────────

const secrets = []; // { provider_id, client_secret }

function mapTransportAuth(a, where) {
  const out = {};
  for (const [k, v] of Object.entries(a)) {
    if (k in TRANSPORT_AUTH) out[TRANSPORT_AUTH[k]] = v;
    else if (k === "hooks") out.hooks = v.map((h) => AUTH_HOOKS[h] ?? (fail(`${where}.hooks: unknown hook ${h}`), h));
    else if (k === "apiKey" || k === "oauth") {
      onlyKeys(v, ["header", "scheme"], `${where}.${k}`);
      out[k === "apiKey" ? "api_key" : "oauth"] = { ...v };
    } else fail(`${where}.${k}: unmapped key`);
  }
  return out;
}

function mapTransport(t, id, where) {
  const out = {};
  const exec = {};
  for (const [k, v] of Object.entries(t)) {
    if (v === undefined) continue;
    if (k in TRANSPORT_SCALARS) out[TRANSPORT_SCALARS[k]] = v;
    else if (k in TRANSPORT_MAPS) out[TRANSPORT_MAPS[k]] = v;
    else if (k in EXECUTOR_PARAMS) exec[EXECUTOR_PARAMS[k]] = v;
    else if (k === "copilot") exec.copilot = mapKeys(v, COPILOT_PARAMS, `${where}.copilot`);
    else if (k === "clientSecret") secrets.push({ provider_id: id, client_secret: v, from: where });
    else if (k === "auth") out.auth = mapTransportAuth(v, `${where}.auth`);
    else if (k === "quirks") {
      const list = [];
      for (const [q, qv] of Object.entries(v)) {
        if (q in QUIRKS) {
          if (qv !== true) fail(`${where}.quirks.${q}: expected true, got ${JSON.stringify(qv)}`);
          list.push(QUIRKS[q]);
        } else if (q in QUIRK_LISTS) out[QUIRK_LISTS[q]] = qv;
        else fail(`${where}.quirks.${q}: unknown quirk`);
      }
      if (list.length) out.quirks = list;
    } else if (k === "retry") {
      out.retry = Object.fromEntries(
        Object.entries(v).map(([code, p]) => [code, typeof p === "number" ? p : mapKeys(p, { attempts: "attempts", delayMs: "delay_ms" }, `${where}.retry.${code}`)]),
      );
    } else fail(`${where}.${k}: unmapped key`);
  }
  if (Object.keys(exec).length) out.executor_params = exec;
  return out;
}

function mapOAuth(o, id) {
  const out = {};
  const endpoints = {};
  const params = {};
  for (const [k, v] of Object.entries(o)) {
    if (v === undefined) continue;
    if (k in OAUTH_TYPED) out[OAUTH_TYPED[k]] = v;
    else if (k === "scope" || k === "scopes") {
      const list = Array.isArray(v) ? v : v.split(/\s+/).filter(Boolean);
      out.scopes = [...(out.scopes ?? []), ...list];
    } else if (k === "clientSecret") secrets.push({ provider_id: id, client_secret: v, from: `${id}.oauth` });
    else if (typeof v === "string" && /^https?:\/\//.test(v)) endpoints[snake(k)] = v;
    else params[snake(k)] = v;
  }
  if (Object.keys(endpoints).length) out.endpoints = endpoints;
  if (Object.keys(params).length) out.params = params;
  return out;
}

function mapModel(m, where) {
  if (typeof m === "string") return m; // bare ID strings stay bare (normalizeModel)
  return mapKeys(m, MODEL_KEYS, where);
}

function mapSection(kind, cfg, where) {
  const endpoint = {};
  const limits = {};
  const sec = {};
  for (const [k, v] of Object.entries(cfg)) {
    if (v === undefined) continue;
    if (k in SECTION_ENDPOINT) endpoint[SECTION_ENDPOINT[k]] = v;
    else if (k in SECTION_LIMITS) limits[SECTION_LIMITS[k]] = v;
    else if (k === "modelMap") endpoint.model_map = v;
    else if (k === "models") sec.models = v.map((m, i) => mapKeys(m, { id: "id", name: "name", dimensions: "dimensions" }, `${where}.models[${i}]`));
    else fail(`${where}.${k}: unmapped key`);
  }
  // A non-URL baseUrl equal to the format ("edge-tts", "local-device", …) is 9router's
  // sentinel for an in-process handler; `format` already names that core handler.
  if (endpoint.base_url === endpoint.format && !/^https?:\/\//.test(endpoint.base_url ?? "")) delete endpoint.base_url;
  if (Object.keys(endpoint).length) sec.endpoint = endpoint;
  if (Object.keys(limits).length) sec.limits = limits;
  return sec;
}

function mapSearchViaChat(cfg, where) {
  const endpoint = { via_chat: true };
  for (const [k, v] of Object.entries(cfg)) {
    if (k === "endpoint") endpoint.base_url = v;
    else if (k === "defaultModel") endpoint.default_model = v;
    else if (k === "pricingUrl") endpoint.pricing_url = v;
    else if (k === "freeTier") endpoint.free_tier_note = v;
    else fail(`${where}.${k}: unmapped key`);
  }
  return { endpoint };
}

// TTS tables (config/ttsModels.js): synthetic keys → owning provider's tts section;
// `defaults` keys are provider ids and replace that provider's PROVIDER_MODELS entry.
const ttsSynthetic = {}; // key → { provider, kind: "models"|"voices", models }
const ttsDefaults = {}; // provider id → models
for (const [provider, cfg] of Object.entries(TTS_MODELS_CONFIG)) {
  onlyKeys(cfg, ["models", "voices", "allVoices", "defaults"], `ttsModels.${provider}`);
  if (cfg.models) ttsSynthetic[`${provider}-tts-models`] = { provider, kind: "models", models: cfg.models };
  if (cfg.allVoices) ttsSynthetic[`${provider}-tts-voices`] = { provider, kind: "voices", models: cfg.allVoices };
  if (cfg.defaults) ttsDefaults[provider] = cfg.defaults;
}
const ttsEntry = (m, where) => {
  onlyKeys(m, ["id", "name", "type"], where);
  if (m.type !== undefined && m.type !== "tts") fail(`${where}.type: expected tts`);
  return { id: m.id, name: m.name };
};

function mapEntry(e) {
  const id = e.id;
  const p = { schema: 1, id, category: e.category };
  const auth = {};
  const display = {};
  const kinds = new Set();
  const sections = {};
  const handled = new Set();
  const take = (k) => { handled.add(k); return e[k]; };

  take("id"); take("category");
  if (e.alias !== undefined) {
    const alias = take("alias");
    // An alias that is another provider's id is shadowed in 9router too (PROVIDER_MODELS
    // keys collide and the id's entry wins). 0router rejects such clashes, so drop it.
    if (alias !== id && REGISTRY.some((r) => r.id === alias)) notes.push(`${id}: alias ${alias} dropped (it is the id of another provider)`);
    else p.alias = alias;
  }
  const aliases = [...(take("aliases") ?? [])];
  for (const [tok, owner] of Object.entries(MEDIA_ONLY_ALIASES)) {
    if (owner === id && tok !== id && tok !== e.alias && !aliases.includes(tok)) aliases.push(tok);
  }
  if (aliases.length) p.aliases = aliases;
  if (e.uiAlias !== undefined) p.ui_alias = take("uiAlias");
  if (take("passthroughModels")) p.passthrough_models = true;
  if (DOT_VERSION_PROVIDERS.has(id) || DOT_VERSION_PROVIDERS.has(e.alias)) p.version_separator_tolerance = true;

  if (e.authType !== undefined) auth.kind = take("authType");
  if (e.authModes !== undefined) auth.modes = take("authModes");
  if (e.noAuth !== undefined) auth.no_auth = take("noAuth");
  if (e.hasOAuth !== undefined) auth.has_oauth = take("hasOAuth");
  if (e.credentialFallback !== undefined) auth.credential_fallback = take("credentialFallback");
  if (e.auth !== undefined) {
    const a = take("auth");
    onlyKeys(a, ["apiKey"], `${id}.auth`);
    if (a.apiKey) { onlyKeys(a.apiKey, ["text"], `${id}.auth.apiKey`); auth.api_key_hint = a.apiKey.text; }
  }

  if (e.display !== undefined) {
    const d = take("display");
    for (const [k, v] of Object.entries(d)) {
      if (k in DISPLAY) display[DISPLAY[k]] = v;
      else if (k === "notice") display.notice = mapKeys(v, NOTICE, `${id}.display.notice`);
      else if (k === "kindNotice") display.kind_notice = v;
      else fail(`${id}.display.${k}: unmapped key`);
    }
  }
  const DISPLAY_TOP = { priority: "priority", hidden: "hidden", hasFree: "has_free", authHint: "auth_hint", hasProviderSpecificData: "has_provider_specific_data", defaultRegion: "default_region", mediaPriority: "media_priority" };
  for (const [k, t] of Object.entries(DISPLAY_TOP)) if (e[k] !== undefined) display[t] = take(k);
  if (e.features !== undefined) display.features = mapKeys(take("features"), { usage: "usage", usageApikey: "usage_apikey" }, `${id}.features`);
  if (e.thinkingConfig !== undefined) display.thinking = mapKeys(take("thinkingConfig"), { options: "options", defaultMode: "default_mode" }, `${id}.thinkingConfig`);
  if (e.regions !== undefined) display.regions = take("regions").map((r, i) => mapKeys(r, { id: "id", label: "label" }, `${id}.regions[${i}]`));

  handled.add("transport");
  if (e.transport) p.transport = mapTransport(e.transport, id, `${id}.transport`);
  if (e.transports !== undefined) p.transports = take("transports").map((t, i) => mapTransport(t, id, `${id}.transports[${i}]`));
  if (e.oauth !== undefined) p.oauth = mapOAuth(take("oauth"), id);

  if (e.modelsFetcher !== undefined) {
    p.models_fetcher = mapKeys(take("modelsFetcher"), { type: "kind", url: "url" }, `${id}.modelsFetcher`);
  }

  // Models: registry list, or the TTS `defaults` table that replaces it in PROVIDER_MODELS.
  if (e.models !== undefined) p.models = take("models").map((m, i) => mapModel(m, `${id}.models[${i}]`));
  if (ttsDefaults[id]) {
    if (e.models !== undefined) fail(`${id}: both registry models and a ttsModels defaults table`);
    p.models = ttsDefaults[id].map((m, i) => ({ ...ttsEntry(m, `ttsModels.${id}.defaults[${i}]`), kind: "tts" }));
  }

  // Capabilities.
  for (const k of take("serviceKinds") ?? []) {
    if (!(k in KINDS)) fail(`${id}.serviceKinds: unknown kind ${k}`);
    else kinds.add(KINDS[k]);
  }
  for (const [cfgKey, kind] of Object.entries(CONFIG_KINDS)) {
    if (e[cfgKey] === undefined) continue;
    kinds.add(kind);
    sections[kind] = mapSection(kind, take(cfgKey), `${id}.${cfgKey}`);
  }
  if (e.searchViaChat !== undefined) {
    if (sections.web_search) fail(`${id}: both searchConfig and searchViaChat`);
    kinds.add("web_search");
    sections.web_search = mapSearchViaChat(take("searchViaChat"), `${id}.searchViaChat`);
  }
  if (e.serviceKinds === undefined && e.transport) kinds.add("llm");
  for (const k of take("hiddenKinds") ?? []) {
    const kind = KINDS[k];
    if (!kind) { fail(`${id}.hiddenKinds: unknown kind ${k}`); continue; }
    kinds.add(kind);
    (sections[kind] ??= {}).hidden = true;
  }
  for (const [key, t] of Object.entries(ttsSynthetic)) {
    if (t.provider !== id) continue;
    kinds.add("tts");
    const sec = (sections.tts ??= {});
    const list = t.models.map((m, i) => ttsEntry(m, `ttsModels.${key}[${i}]`));
    if (t.kind === "voices") sec.voices = list;
    else {
      if (sec.models) {
        // The registry's own ttsConfig.models must be covered by the canonical table.
        const ids = new Set(list.map((m) => m.id));
        const missing = sec.models.filter((m) => !ids.has(m.id));
        if (missing.length) fail(`${id}: ttsConfig.models not covered by ${key}: ${missing.map((m) => m.id)}`);
        notes.push(`${id}: capabilities.tts.models taken from ${key} (superset of registry ttsConfig.models)`);
      }
      sec.models = list;
    }
  }
  const caps = {};
  const omitted = [];
  for (const kind of [...kinds].sort()) {
    const sec = sections[kind] ?? {};
    if (!sec.endpoint && !p.transport) { omitted.push(kind); continue; }
    caps[kind] = sec;
  }
  if (omitted.length) notes.push(`${id}: omitted unreachable capability section(s) ${omitted.join(", ")}`);
  if (Object.keys(caps).length) p.capabilities = caps;

  if (Object.keys(auth).length) p.auth = auth;
  if (Object.keys(display).length) p.display = display;

  for (const k of Object.keys(e)) if (!handled.has(k)) fail(`${id}.${k}: unmapped top-level key`);
  return { plugin: p, omitted };
}

// ── TOML emitter ───────────────────────────────────────────────────────────

const bareKey = (k) => (/^[A-Za-z0-9_-]+$/.test(k) ? k : JSON.stringify(k));
const isTable = (v) => v !== null && typeof v === "object" && !Array.isArray(v);
const isTableArray = (v) => Array.isArray(v) && v.length > 0 && v.every(isTable);

function tomlValue(v, where) {
  if (typeof v === "string") return JSON.stringify(v);
  if (typeof v === "boolean") return String(v);
  if (typeof v === "number") {
    if (!Number.isFinite(v)) fail(`${where}: non-finite number`);
    if (Number.isInteger(v)) return String(v);
    const s = String(v);
    return /[.e]/.test(s) ? s : `${s}.0`;
  }
  if (Array.isArray(v)) return `[${v.map((x, i) => tomlValue(x, `${where}[${i}]`)).join(", ")}]`;
  if (isTable(v) && !Object.keys(v).length) return "{}";
  if (isTable(v)) return `{ ${Object.entries(v).map(([k, x]) => `${bareKey(k)} = ${tomlValue(x, `${where}.${k}`)}`).join(", ")} }`;
  fail(`${where}: cannot emit ${v === null ? "null" : typeof v}`);
  return '""';
}

// Maps whose values are all scalars (or small) are written inline so short tables stay compact.
const INLINE_TABLES = new Set(["headers", "retry", "api_key", "oauth.auth", "notice", "features", "thinking", "model_map"]);

// `header` is omitted for a table holding only sub-tables (TOML defines it implicitly).
function emitTable(lines, path, obj, header = null) {
  const scalars = [];
  const nested = [];
  for (const [k, v] of Object.entries(obj)) {
    if (v === undefined) continue;
    const inline = INLINE_TABLES.has(k) || (isTable(v) && Object.values(v).every((x) => !isTable(x) && !Array.isArray(x)) && Object.keys(v).length <= 3 && !["endpoint", "auth", "limits", "executor_params", "endpoints", "params", "copilot", "usage", "regions", "reasoning_inject", "kind_notice"].includes(k));
    if ((isTable(v) && !inline && Object.keys(v).length) || isTableArray(v)) nested.push([k, v]);
    else scalars.push(`${bareKey(k)} = ${tomlValue(v, `${path}.${k}`)}`);
  }
  if (header && (scalars.length || !nested.length || header.startsWith("[["))) lines.push("", header);
  lines.push(...scalars);
  for (const [k, v] of nested) {
    const sub = path ? `${path}.${bareKey(k)}` : bareKey(k);
    if (isTableArray(v)) for (const item of v) emitTable(lines, sub, item, `[[${sub}]]`);
    else emitTable(lines, sub, v, `[${sub}]`);
  }
}

function toToml(plugin) {
  const lines = [`# ${HEADER}`];
  emitTable(lines, "", plugin);
  return `${lines.join("\n")}\n`;
}

// ── Part 1: plugin files ───────────────────────────────────────────────────

const plugins = REGISTRY.map(mapEntry).map((r) => r.plugin);
const byId = Object.fromEntries(plugins.map((p) => [p.id, p]));

const oauthParamKeys = new Set();
const sectionFormats = new Set();
for (const p of plugins) {
  for (const k of Object.keys(p.oauth?.params ?? {})) oauthParamKeys.add(k);
  for (const sec of Object.values(p.capabilities ?? {})) if (sec.endpoint?.format) sectionFormats.add(sec.endpoint.format);
}

// ── Part 2: credential table ───────────────────────────────────────────────

const hostOf = (u) => { try { return new URL(u).hostname; } catch { return null; } };
function oauthHostSet(p) {
  const urls = [p.oauth?.authorize_url, p.oauth?.token_url, p.oauth?.refresh_url];
  for (const t of [p.transport, ...(p.transports ?? [])]) if (t) urls.push(t.token_url, t.refresh_url, t.auth_url);
  return [...new Set(urls.filter(Boolean).map(hostOf).filter(Boolean))].sort();
}
const credentials = [];
for (const s of secrets) {
  if (credentials.some((c) => c.provider_id === s.provider_id)) { fail(`${s.provider_id}: two client secrets`); continue; }
  let hosts = oauthHostSet(byId[s.provider_id]);
  if (hosts.length === 0 && s.provider_id === "gemini") {
    hosts = [...new Set([OAUTH_ENDPOINTS.google.token, OAUTH_ENDPOINTS.google.auth].map(hostOf))].sort();
  }
  if (hosts.length === 0) fail(`${s.provider_id}: credential would have no bound hosts`);
  credentials.push({ provider_id: s.provider_id, client_secret: s.client_secret, bound_hosts: hosts });
}
if (credentials.length !== 4) fail(`expected 4 bundled client secrets, found ${credentials.length}: ${credentials.map((c) => c.provider_id)}`);

// ── Part 3: parity oracle ──────────────────────────────────────────────────

const ALIAS_TOKENS = [
  "cc","cx","gc","qw","if","ag","gh","kr","cu","kc","kmc","cl","oc","ocg","qd","qoder",
  "el","openai","vercel","vercel-ai-gateway","anthropic","gemini","openrouter","glm","kimi",
  "minimax","minimax-cn","hf","huggingface","ds","deepseek","cmc","commandcode","groq","xai",
  "mistral","pplx","perplexity","together","fireworks","cerebras","cohere","nvidia","nebius",
  "siliconflow","hyp","hyperbolic","dg","deepgram","aai","assemblyai","nb","nanobanana","ch",
  "chutes","ark","volcengine-ark","byteplus","bpm","cursor","vx","vertex","vxp","vertex-partner",
  "gw","grok-web","gcli","gb","grok-build","grok-cli","pw","perplexity-web","mimo","xiaomi-mimo",
  "xmtp","xiaomi-tokenplan","cf",
  "cloudflare-ai","fal","fal-ai","stability","stability-ai","bfl","black-forest-labs","recraft",
  "topaz","runway","runwayml","jina","jina-ai","polly","aws-polly","bb","blackbox",
  "af","airforce","api-airforce","llm7","llm-7","samba","sambanova","bm","bluesminds",
  "bzl","bazaarlink","kgw","kilo-gateway","hunyuan","tencent","qianfan","baidu","ernie",
  "dv","devin","devin-cli","morph","morphllm",
]; // verbatim from ref/9router/tests/__baseline__/verify-alias.mjs

const aliasFixture = {
  tokens: ALIAS_TOKENS,
  aliasToId: Object.fromEntries(ALIAS_TOKENS.map((a) => [a, resolveProviderAlias(a)])),
  idToAlias: Object.fromEntries(Object.keys(PM.PROVIDER_ID_TO_ALIAS).sort().map((k) => [k, PM.PROVIDER_ID_TO_ALIAS[k]])),
  modelKeys: Object.keys(PROVIDER_MODELS).sort(),
};

const P = PROVIDERS;
const oauthFixture = JSON.parse(JSON.stringify({ // shape of verify-oauth-urls.mjs
  oauthEndpoints: OAUTH_ENDPOINTS,
  tokenUrls: { claude: P.claude?.tokenUrl, codex: P.codex?.tokenUrl, iflow: P.iflow?.tokenUrl, kiro: P.kiro?.tokenUrl, xai: P.xai?.tokenUrl, "grok-cli": P["grok-cli"]?.tokenUrl, cline: P.cline?.tokenUrl, kimi: P.kimi?.tokenUrl },
  authUrls: { iflow: P.iflow?.authUrl, kiro: P.kiro?.authUrl },
  refreshUrls: { cline: P.cline?.refreshUrl, kimi: P.kimi?.refreshUrl, xai: P.xai?.refreshUrl, "grok-cli": P["grok-cli"]?.tokenUrl },
  clientIds: { claude: P.claude?.clientId, codex: P.codex?.clientId, iflow: P.iflow?.clientId, kimi: P.kimi?.clientId, "grok-cli": P["grok-cli"]?.clientId },
}));

const passthroughKeys = new Set(REGISTRY.filter((r) => r.passthroughModels).map((r) => r.alias || r.id));
const MUSE_ALIASES = new Set(["oc", "opencode", "ocg", "opencode-go", "ocz", "opencode-zen"]);
const stripSuffix = (m) => m.replace(/\([^()]+\)\s*$/, "").trim();
function lookupRow(alias, model, edge) {
  return {
    alias, model, edge,
    isValidModel: PM.isValidModel(alias, model, passthroughKeys),
    upstreamId: PM.getModelUpstreamId(alias, model),
    type: PM.getModelType(alias, model),
    targetFormat: PM.getModelTargetFormat(alias, model),
    supportedFormats: PM.getModelSupportedFormats(alias, model),
    quotaFamily: PM.getModelQuotaFamily(alias, model),
    strip: PM.getModelStrip(alias, model),
    name: PM.findModelName(alias, model),
  };
}
const lookupRows = [];
for (const alias of Object.keys(PROVIDER_MODELS).sort()) {
  if (alias in ttsSynthetic) continue;
  const models = PROVIDER_MODELS[alias];
  const inputs = [];
  for (const m of models) {
    inputs.push([m.id, null]);
    if (/\([^()]+\)\s*$/.test(m.upstreamModelId ?? "")) inputs.push([`${m.id}(low)`, "preset-suffix"]);
    const dashed = m.id.replace(/(\d)\.(\d)/g, "$1-$2");
    if (dashed !== m.id) inputs.push([dashed, "dash-dot"], [`${dashed}(high)`, "dash-dot"]);
  }
  const first = models[0]?.id ?? "m";
  inputs.push(
    [`${first}(high)`, "thinking-suffix"],
    [`${first}(high)  `, "trailing-whitespace"],
    [`${first}(a(b))`, "nested-paren"],
    [`${first} (high)`, "space-before-suffix"],
    ["undeclared-model-xyz", "undeclared"],
    ["undeclared-model-xyz(medium)", "undeclared"],
  );
  const seen = new Set();
  for (const [model, edge] of inputs) {
    if (seen.has(model)) continue;
    seen.add(model);
    if (alias === "cx" && stripSuffix(model).endsWith(CODEX_REVIEW_SUFFIX)) continue;
    if (MUSE_ALIASES.has(alias) && isMuseSparkModel(model)) continue;
    lookupRows.push(lookupRow(alias, model, edge));
  }
}

const ttsFixture = Object.fromEntries(
  Object.entries(ttsSynthetic).sort().map(([k, t]) => [k, { provider: t.provider, kind: t.kind, models: PROVIDER_MODELS[k].map((m) => m.id) }]),
);

// ── Write ──────────────────────────────────────────────────────────────────

if (errors.length) {
  console.error(`generate.mjs: ${errors.length} error(s):`);
  for (const e of errors) console.error(`  ${e}`);
  process.exit(1);
}

const bundledDir = join(ROOT, "plugins", "bundled");
rmSync(bundledDir, { recursive: true, force: true });
mkdirSync(bundledDir, { recursive: true });
for (const p of plugins) {
  const toml = toToml(p);
  if (/client_secret|GOCSPX-/.test(toml) || credentials.some((c) => toml.includes(c.client_secret))) {
    console.error(`generate.mjs: secret leaked into ${p.id}.toml`);
    process.exit(1);
  }
  writeFileSync(join(bundledDir, `${p.id}.toml`), toml);
}

const rsList = (xs) => xs.map((x) => `    ${JSON.stringify(x)},`).join("\n");
const schemaDir = join(ROOT, "crates", "zerorouter-registry", "src", "schema");
writeFileSync(join(schemaDir, "oauth_params.rs"), `// ${HEADER}

/// \`oauth.params\` keys the core knows. Anything else is rejected (research R5).
pub const KNOWN_OAUTH_PARAMS: &[&str] = &[
${rsList([...oauthParamKeys].sort())}
];
`);
writeFileSync(join(schemaDir, "section_formats.rs"), `// ${HEADER}

/// Capability-section \`endpoint.format\` values that name a core media handler.
pub const KNOWN_SECTION_FORMATS: &[&str] = &[
${rsList([...sectionFormats].sort())}
];
`);

const credRs = credentials.map((c) => `    RawCredential {
        provider_id: ${JSON.stringify(c.provider_id)},
        client_secret: ${JSON.stringify(c.client_secret)},
        bound_hosts: &[${c.bound_hosts.map((h) => JSON.stringify(h)).join(", ")}],
    },`).join("\n");
writeFileSync(join(ROOT, "crates", "zerorouter-registry", "src", "credentials", "bundled.rs"), `// ${HEADER}
//
// Public "installed-app" OAuth client secrets that 9router ships in its source. They live
// here, in the core, so no plugin file or plugin-visible type ever carries them (FR-012).

static BUNDLED_CREDENTIALS: &[RawCredential] = &[
${credRs}
];
`);

const fixDir = join(ROOT, "tests", "fixtures", "9router");
mkdirSync(fixDir, { recursive: true });
for (const f of readdirSync(fixDir)) if (f.endsWith(".json")) rmSync(join(fixDir, f));
const writeFixture = (name, data) =>
  writeFileSync(join(fixDir, name), `${JSON.stringify({ source: HEADER, data }, null, 2)}\n`);
writeFixture("providers.json", JSON.parse(JSON.stringify(PROVIDERS)));
writeFixture("alias.json", aliasFixture);
writeFixture("oauth-urls.json", oauthFixture);
writeFixture("lookup.json", lookupRows);
writeFixture("tts-tables.json", ttsFixture);

console.log(`ref/9router@${SHA}`);
console.log(`  ${plugins.length} plugins, ${credentials.length} credentials (${credentials.map((c) => c.provider_id).join(", ")})`);
console.log(`  ${oauthParamKeys.size} oauth params, ${sectionFormats.size} section formats, ${lookupRows.length} lookup rows`);
for (const n of notes) console.log(`  note: ${n}`);
