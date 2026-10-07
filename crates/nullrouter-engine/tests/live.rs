//! Live checks against the real providers (T060), run by the operator with their own
//! accounts: `NR_LIVE=1 cargo test -p nullrouter-engine --test live -- text`. Skipped
//! unless `NR_LIVE=1`. The accounts come from `$NULLROUTER_HOME` (else `~/.0router`); a
//! provider without an account is skipped with a message.
//!
//! `NR_LIVE_MODELS` (comma-separated `provider/model`) replaces the default text targets.
//!
//! `-- continuation` (T096) sends a prefilled assistant turn to each model `anthropic`
//! declares `[continuation]` for, which must continue it, and to the probes in
//! `PREFILL_PROBES` (or `NR_LIVE_PREFILL`), whose answers are printed for the operator.
//!
//! `-- types` (T087) sends one non-text request per type and provider. Video is submitted
//! and polled once only with `NR_LIVE_VIDEO=1`, since it is billed per clip.
//!
//! Slice 005 (research R17): `-- signin_anthropic signin_grok_cli` (L1, L3),
//! `-- token_lifetimes` (L4) and `-- quota` (L2, L5) use the sign-in accounts the operator
//! signed in with `nullrouter accounts signin` into the same home; they print findings
//! and run one at a time. See `docs/operator-config.md` § Live checks.

use std::sync::Arc;
use std::time::{Duration, Instant};

use nullrouter_engine::attempt::{self, Answer, Media, MediaAnswer, TextRequest};
use nullrouter_engine::keys::AgentId;
use nullrouter_engine::records::{Outcome, RequestRecord};
use nullrouter_engine::state::Engine;
use nullrouter_registry::OperatorHome;
use nullrouter_registry::schema::ModelType;
use nullrouter_wire::codec::response::ForClient;
use nullrouter_wire::codec::types::TypeValue;
use nullrouter_wire::codec::{request, response};
use reqwest::header::HeaderMap;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

/// One cheap target per text provider and wire. opencode's Messages wire takes `x-api-key`
/// and neither opencode wire gets fingerprint tools (R4): these requests carry no tools.
const TARGETS: &[&str] = &[
    "anthropic/claude-sonnet-4-20250514",
    "openrouter/openai/gpt-4o-mini",
    "opencode-zen/gpt-5-nano",
    "opencode-zen/claude-haiku-4-5",
    "opencode-go/deepseek-flash",
];

fn live() -> bool {
    std::env::var("NR_LIVE").is_ok_and(|v| v == "1")
}

fn targets() -> Vec<String> {
    match std::env::var("NR_LIVE_MODELS") {
        Ok(v) if !v.trim().is_empty() => v.split(',').map(|s| s.trim().to_owned()).collect(),
        _ => TARGETS.iter().map(|s| (*s).to_owned()).collect(),
    }
}

async fn settled(engine: &Engine, id: &str) -> RequestRecord {
    for _ in 0..600 {
        let r = engine.records.get(id).unwrap();
        if r.outcome != Outcome::InProgress {
            return r;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("record {id} never finished");
}

/// Sends one request in `client`'s style and returns the answer's text and the record.
async fn send(engine: &Arc<Engine>, client: &str, target: &str, body: Value) -> (String, RequestRecord) {
    let st = engine.snapshot();
    let style = st.style(client).unwrap().clone();
    let ir = request::decode(&style, &body).unwrap();
    let id = nullrouter_engine::records::new_id();
    engine.records.insert(RequestRecord::new(id.clone(), "live".into(), style.id.clone()));
    let stream = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
    let req = TextRequest {
        id: id.clone(),
        arrived: Instant::now(),
        client: style.clone(),
        body: body.clone(),
        ir,
        headers: HeaderMap::new(),
        agent: AgentId::new("ak_live", Some("live-1")),
        target: target.into(),
        stream,
        cancel: CancellationToken::new(),
        media: None,
        count: false,
        pin: None,
        test: None,
    };
    let text = match engine.text(st.clone(), req).await {
        Ok(Answer::Whole { status, raw, answer, .. }) => {
            assert_eq!(status, 200, "{target}: {}", String::from_utf8_lossy(&raw));
            match *answer {
                ForClient::AsReceived { read } => read
                    .or_else(|| response::decode(&style, &serde_json::from_slice(&raw).ok()?).ok())
                    .map(|r| r.text())
                    .unwrap_or_default(),
                ForClient::Rebuilt { read, .. } => read.text(),
            }
        }
        Ok(Answer::Events { rx, .. }) => match attempt::collect(&style, &body, rx).await {
            Ok(r) => r.text(),
            Err(e) => panic!("{target}: the stream ended in an error: {e:?}"),
        },
        Ok(Answer::Media(_) | Answer::Count { .. }) => panic!("{target}: a non-text answer to a text request"),
        Err(f) => panic!("{target}: {} {}", f.status, f.message),
    };
    (text, settled(engine, &id).await)
}

#[tokio::test]
async fn text() {
    if !live() {
        eprintln!("skipped: set NR_LIVE=1 to run the live checks");
        return;
    }
    let (engine, report) = Engine::open(OperatorHome::resolve()).unwrap();
    assert!(report.registry.diagnostics.is_empty(), "{:#?}", report.registry.diagnostics);
    let engine = Arc::new(engine);
    let st = engine.snapshot();
    let mut ran = 0;
    for target in targets() {
        let provider = target.split('/').next().unwrap();
        if st.accounts.for_provider(provider).all(|a| a.disabled) {
            eprintln!("{target}: skipped, no enabled {provider} account");
            continue;
        }
        for stream in [false, true] {
            let body = json!({
                "model": target,
                "max_tokens": 64,
                "stream": stream,
                "messages": [{ "role": "user", "content": "Reply with the single word: pong" }],
            });
            let started = Instant::now();
            let (text, rec) = send(&engine, "openai-chat", &target, body).await;
            let (input, output) = rec.usage.map_or((None, None), |u| (u.input, u.output));
            eprintln!(
                "{target} stream={stream}: {:?} in {} ms, input {input:?} output {output:?}, {} attempt(s)",
                text.trim(),
                started.elapsed().as_millis(),
                rec.attempts.len()
            );
            assert_eq!(rec.outcome, Outcome::Succeeded, "{target} stream={stream}: {rec:#?}");
            assert!(!text.trim().is_empty(), "{target} stream={stream}: an empty answer");
            assert!(input.is_some() && output.is_some(), "{target} stream={stream}: usage not recorded: {rec:#?}");
        }
        ran += 1;
    }
    assert!(ran > 0, "no provider had an account under {}", engine.home().path().display());
}

/// Models whose prefill support is unknown: Claude 4.6+ should refuse it (400); the
/// openrouter and opencode families decide whether they get a `[continuation]`.
const PREFILL_PROBES: &[&str] = &[
    "anthropic/claude-sonnet-4-6",
    "opencode-zen/claude-sonnet-4",
    "opencode-zen/claude-sonnet-4-6",
    "opencode-zen/claude-haiku-4-5",
    "openrouter/anthropic/claude-sonnet-4",
    "openrouter/openai/gpt-4o-mini",
];

/// A Messages request ending in the prefill "1 2 3 4": a model that continues it goes on
/// from 5 instead of starting over.
fn prefilled(target: &str) -> Value {
    json!({
        "model": target,
        "max_tokens": 64,
        "stream": true,
        "messages": [
            { "role": "user", "content": "Count from 1 to 10, separated by spaces. Nothing else." },
            { "role": "assistant", "content": "1 2 3 4" },
        ],
    })
}

/// Sends a prefilled request; the answer's text, or the failure.
async fn try_prefill(engine: &Arc<Engine>, target: &str) -> Result<String, String> {
    let st = engine.snapshot();
    let style = st.style("anthropic-messages").unwrap().clone();
    let body = prefilled(target);
    let ir = request::decode(&style, &body).unwrap();
    let id = nullrouter_engine::records::new_id();
    engine.records.insert(RequestRecord::new(id.clone(), "live".into(), style.id.clone()));
    let req = TextRequest {
        id,
        arrived: Instant::now(),
        client: style.clone(),
        body: body.clone(),
        ir,
        headers: HeaderMap::new(),
        agent: AgentId::new("ak_live", Some("live-1")),
        target: target.into(),
        stream: true,
        cancel: CancellationToken::new(),
        media: None,
        count: false,
        pin: None,
        test: None,
    };
    match engine.text(st, req).await {
        Ok(Answer::Events { rx, .. }) => {
            attempt::collect(&style, &body, rx).await.map(|r| r.text()).map_err(|e| format!("{e:?}"))
        }
        Ok(_) => Err("not a stream".into()),
        Err(f) => Err(format!("{} {}", f.status, f.message)),
    }
}

/// Whether `text` continues "1 2 3 4" rather than starting over.
fn continues(text: &str) -> bool {
    let t = text.trim_start();
    t.starts_with('5') && !t.contains("1 2 3")
}

#[tokio::test]
async fn continuation() {
    if !live() {
        eprintln!("skipped: set NR_LIVE=1 to run the live checks");
        return;
    }
    let (engine, report) = Engine::open(OperatorHome::resolve()).unwrap();
    assert!(report.registry.diagnostics.is_empty(), "{:#?}", report.registry.diagnostics);
    let engine = Arc::new(engine);
    let st = engine.snapshot();
    let has_account = |target: &str| st.accounts.for_provider(target.split('/').next().unwrap()).any(|a| !a.disabled);
    let declared: Vec<String> = st
        .registry
        .provider("anthropic")
        .ok()
        .and_then(|p| p.endpoints.get(&ModelType::Text)?.0.first())
        .and_then(|e| e.continuation.as_ref())
        .map(|c| c.models.iter().map(|m| format!("anthropic/{m}")).collect())
        .unwrap_or_default();
    let mut ran = 0;
    for target in &declared {
        if !has_account(target) {
            eprintln!("{target}: skipped, no enabled account");
            continue;
        }
        let got = try_prefill(&engine, target).await;
        eprintln!("declared {target}: {got:?}");
        let text = got.unwrap_or_else(|e| panic!("{target} is declared to continue a prefill but failed: {e}"));
        assert!(continues(&text), "{target} is declared to continue a prefill but started over: {text:?}");
        ran += 1;
    }
    let probes: Vec<String> = match std::env::var("NR_LIVE_PREFILL") {
        Ok(v) if !v.trim().is_empty() => v.split(',').map(|s| s.trim().to_owned()).collect(),
        _ => PREFILL_PROBES.iter().map(|s| (*s).to_owned()).collect(),
    };
    for target in probes.iter().filter(|t| has_account(t)) {
        let verdict = match try_prefill(&engine, target).await {
            Ok(text) if continues(&text) => format!("continues: {:?}", text.trim()),
            Ok(text) => format!("starts over: {:?}", text.trim()),
            Err(e) => format!("refused: {e}"),
        };
        eprintln!("probe {target}: {verdict}");
        ran += 1;
    }
    assert!(ran > 0, "no provider had an account under {}", engine.home().path().display());
}

/// What a non-text request came back with.
enum Got {
    Value(TypeValue),
    Bytes(String, Vec<u8>),
    Job(String),
}

/// Sends one non-text request in the openai-chat style.
async fn send_media(engine: &Arc<Engine>, ty: ModelType, target: &str, body: Value) -> (Got, RequestRecord) {
    let st = engine.snapshot();
    let style = st.style("openai-chat").unwrap().clone();
    let codec = style.type_codec(ty).unwrap().clone();
    let input = codec.decode_request(&body).unwrap();
    let id = nullrouter_engine::records::new_id();
    engine.records.insert(RequestRecord::new(id.clone(), "live".into(), style.id.clone()));
    let job = ty == ModelType::Video;
    let req = TextRequest {
        id: id.clone(),
        arrived: Instant::now(),
        client: style,
        body,
        ir: Default::default(),
        headers: HeaderMap::new(),
        agent: AgentId::new("ak_live", Some("live-1")),
        target: target.into(),
        stream: false,
        cancel: CancellationToken::new(),
        media: Some(Media { ty, codec, variant: None, input, voice: None, job }),
        count: false,
        pin: None,
        test: None,
    };
    let got = match engine.text(st, req).await {
        Ok(Answer::Media(MediaAnswer::Value(v))) => Got::Value(v),
        Ok(Answer::Media(MediaAnswer::Bytes { content_type, mut rx })) => {
            let mut bytes = Vec::new();
            while let Some(chunk) = rx.recv().await {
                bytes.extend_from_slice(&chunk.unwrap_or_else(|e| panic!("{target}: the audio broke off: {e}")));
            }
            Got::Bytes(content_type, bytes)
        }
        Ok(Answer::Media(MediaAnswer::Job { id, .. })) => Got::Job(id),
        Ok(_) => panic!("{target}: a text answer to a non-text request"),
        Err(f) => panic!("{target}: {} {}", f.status, f.message),
    };
    let rec = if job { engine.records.get(&id).unwrap() } else { settled(engine, &id).await };
    (got, rec)
}

#[tokio::test]
async fn types() {
    if !live() {
        eprintln!("skipped: set NR_LIVE=1 to run the live checks");
        return;
    }
    let (engine, report) = Engine::open(OperatorHome::resolve()).unwrap();
    assert!(report.registry.diagnostics.is_empty(), "{:#?}", report.registry.diagnostics);
    let engine = Arc::new(engine);
    let st = engine.snapshot();
    let has = |p: &str| {
        let ok = st.accounts.for_provider(p).any(|a| !a.disabled);
        if !ok {
            eprintln!("{p}: skipped, no enabled account");
        }
        ok
    };
    let mut ran = 0;
    if has("openrouter") {
        let (got, rec) = send_media(
            &engine,
            ModelType::Embeddings,
            "openrouter/openai/text-embedding-3-small",
            json!({"model": "x", "input": "pong"}),
        )
        .await;
        let Got::Value(v) = got else { panic!("embeddings: not a value") };
        let dims =
            v.items.first().and_then(|i| i.get("output.embedding")).and_then(Value::as_array).map_or(0, Vec::len);
        eprintln!("openrouter embeddings: {dims} dimensions, usage {:?}", rec.usage);
        assert!(dims > 0 && rec.outcome == Outcome::Succeeded, "{rec:#?}");

        let (got, rec) = send_media(
            &engine,
            ModelType::Image,
            "openrouter/openai/gpt-image-1",
            json!({"model": "x", "prompt": "a small red square", "size": "1024x1024"}),
        )
        .await;
        let Got::Value(v) = got else { panic!("image: not a value") };
        let item = v.items.first().expect("image: no data");
        eprintln!(
            "openrouter image: b64 {} chars, url {:?}",
            item.str("output.b64_json").map_or(0, |s| s.len()),
            item.str("output.url")
        );
        assert_eq!(rec.outcome, Outcome::Succeeded, "{rec:#?}");

        let (got, rec) = send_media(
            &engine,
            ModelType::Tts,
            "openrouter/openai/gpt-4o-mini-tts",
            json!({"model": "x", "input": "pong"}),
        )
        .await;
        let Got::Bytes(ctype, bytes) = got else { panic!("openrouter speech: not audio bytes") };
        eprintln!("openrouter speech: {} bytes of {ctype}", bytes.len());
        assert!(!bytes.is_empty() && rec.outcome == Outcome::Succeeded, "{rec:#?}");

        if std::env::var("NR_LIVE_VIDEO").is_ok_and(|v| v == "1") {
            let (got, rec) = send_media(
                &engine,
                ModelType::Video,
                "openrouter/google/veo-3.1",
                json!({"model": "x", "prompt": "a red ball rolling", "duration": 4}),
            )
            .await;
            let Got::Job(vj) = got else { panic!("video: not a job") };
            let (_, bindings, status) = engine
                .job_get(&st, &vj, "ak_live", "openai-chat")
                .await
                .unwrap_or_else(|f| panic!("video poll: {} {}", f.status, f.message));
            eprintln!("openrouter video: {vj} is {status:?} ({bindings:?}), record {}", rec.id);
        }
        ran += 1;
    }
    if has("elevenlabs") {
        let (got, rec) = send_media(
            &engine,
            ModelType::Tts,
            "elevenlabs/eleven_flash_v2_5",
            json!({"model": "x", "input": "Hello from null router."}),
        )
        .await;
        let Got::Bytes(ctype, audio) = got else { panic!("elevenlabs speech: not audio bytes") };
        eprintln!("elevenlabs speech: {} bytes of {ctype}", audio.len());
        assert!(!audio.is_empty() && rec.outcome == Outcome::Succeeded, "{rec:#?}");

        let file = nullrouter_wire::primitives::body::file(&audio, Some("speech.mp3"), Some(&ctype));
        let (got, rec) =
            send_media(&engine, ModelType::Stt, "elevenlabs/scribe_v2", json!({"model": "x", "file": file})).await;
        let Got::Value(v) = got else { panic!("elevenlabs transcription: not a value") };
        eprintln!("elevenlabs transcription: {:?}", v.str("output.text"));
        assert!(
            v.str("output.text").is_some_and(|t| !t.trim().is_empty()) && rec.outcome == Outcome::Succeeded,
            "{rec:#?}"
        );
        ran += 1;
    }
    assert!(ran > 0, "no non-text provider had an account under {}", engine.home().path().display());
}

// ---------------------------------------------------------------------------------------
// Slice 005 live checks (research R17, L1–L5; tasks T045, T056, T078). The operator first
// signs accounts in with the real CLI into the same `$NULLROUTER_HOME`, e.g.
// `NULLROUTER_HOME=.nr-live nullrouter accounts signin anthropic max`. These checks print
// findings for the operator to write into the bundled plugins; they fail only when the
// setup is broken, never on a provider's answer. Requests L1 and L3 send are built here
// and sent directly (not through the attempt loop), so a refusal is printed instead of
// taking the account out of service, and the plugins stay as they are. Nothing printed
// carries a token: every provider text goes through the engine's redactor first.
// ---------------------------------------------------------------------------------------

use nullrouter_engine::accounts::{Account, Released, release};
use nullrouter_engine::identity::{self, FillContext};
use nullrouter_engine::signin::refresh::{Refreshed, lead, lifetime};
use nullrouter_engine::state::EngineState;
use nullrouter_engine::tokens::AccountState;
use nullrouter_registry::schema::{AuthScheme, HeaderValue as IdentityValue, ProviderEntity, QuotaBody};
use reqwest::header::{AUTHORIZATION, HeaderName, HeaderValue};

/// The slice 005 checks share the operator's tokens: one at a time, so two engines never
/// refresh the same rotating refresh token at once (the loser would read `invalid_grant`).
static SIGNIN_LIVE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// How long one live call may take (grok-cli streams its whole answer).
const CALL_TIMEOUT: Duration = Duration::from_secs(120);

/// Opens the operator home for a slice 005 check; `None` (with a message) without `NR_LIVE=1`.
fn open_live() -> Option<Arc<Engine>> {
    if !live() {
        eprintln!("skipped: set NR_LIVE=1 to run the live checks");
        return None;
    }
    let (engine, report) = Engine::open(OperatorHome::resolve()).unwrap();
    assert!(report.registry.diagnostics.is_empty(), "{:#?}", report.registry.diagnostics);
    Some(Arc::new(engine))
}

/// The enabled sign-in accounts of `provider`, with a skip message when there is none.
fn signin_accounts<'s>(engine: &Engine, st: &'s EngineState, provider: &'s str) -> Vec<&'s Account> {
    let found: Vec<&Account> = st.accounts.for_provider(provider).filter(|a| a.is_signin() && !a.disabled).collect();
    if found.is_empty() {
        eprintln!(
            "{provider}: skipped, no enabled sign-in account under {}; first run \
             `NULLROUTER_HOME={} nullrouter accounts signin {provider} <name>`",
            engine.home().path().display(),
            engine.home().path().display()
        );
    }
    found
}

/// `text` redacted, on one line, at most `max` characters.
fn shown(st: &EngineState, text: &str, max: usize) -> String {
    let red = st.redactor.redact(text);
    let line = red.split_whitespace().collect::<Vec<_>>().join(" ");
    match line.char_indices().nth(max) {
        Some((i, _)) => format!("{}… ({} chars in all)", &line[..i], line.chars().count()),
        None => line,
    }
}

/// The account's state as the accounts list shows it.
fn state_line(state: &AccountState) -> String {
    match state.reason() {
        Some(r) => format!("{} ({r})", state.name()),
        None => state.name().to_owned(),
    }
}

/// Which `[identity]` headers a check sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Identity {
    /// Every declared header, placeholders filled (what the engine sends).
    Full,
    /// Only the headers with a fixed value.
    FixedOnly,
}

/// Static headers, then `[identity]` (sign-in accounts), then the credential, as the engine's
/// background calls build them; the credential goes only to a host the account is bound to.
/// Returns the headers and the names of the identity headers sent.
#[allow(clippy::too_many_arguments)]
fn call_headers(
    engine: &Engine,
    st: &EngineState,
    entity: &ProviderEntity,
    account: &Account,
    released: &Released<'_>,
    url: &url::Url,
    extra: &indexmap::IndexMap<String, String>,
    which: Identity,
    upstream_model: &str,
) -> Result<(HeaderMap, Vec<String>), String> {
    let host = url.host_str().unwrap_or_default();
    let bound = match released {
        Released::Key(_) => account.hosts.contains(host),
        Released::Token(v) => v.entry.hosts.contains(host),
    };
    if !bound {
        return Err(format!("host {host} is not one {}/{}'s credential is bound to", account.provider, account.name));
    }
    let floor = st.registry.floor();
    let mut headers = HeaderMap::new();
    let mut put = |k: &str, v: &str| -> Result<bool, String> {
        let name = HeaderName::from_bytes(k.as_bytes()).map_err(|_| format!("header {k}: invalid name"))?;
        if floor.lists(name.as_str()) {
            return Ok(false);
        }
        headers.insert(name, HeaderValue::from_str(v).map_err(|_| format!("header {k}: invalid value"))?);
        Ok(true)
    };
    for (k, v) in extra {
        put(k, v)?;
    }
    let mut sent = Vec::new();
    if let (Released::Token(view), Some(decl)) = (released, &entity.identity) {
        let session_id = engine.sessions.id_for(&AgentId::new("ak_live", Some("live-1")));
        let install_id = engine.install_id().map_err(|e| e.to_string())?.to_owned();
        let ctx = FillContext {
            session_id: &session_id,
            request_id: &identity::uuid_v4(),
            turns: 1,
            upstream_model,
            claims: Some(&view.entry.claims),
            install_id: &install_id,
        };
        let fixed: Vec<&str> = decl
            .headers
            .iter()
            .filter(|(_, v)| matches!(v, IdentityValue::Fixed(_)))
            .map(|(k, _)| k.as_str())
            .collect();
        for (k, v) in identity::headers(decl, &ctx) {
            if v.is_empty() || (which == Identity::FixedOnly && !fixed.contains(&k)) {
                continue;
            }
            if put(k, &v)? {
                sent.push(k.to_owned());
            }
        }
    }
    let (header, scheme) = match (released, &entity.signin) {
        (Released::Token(_), Some(s)) => (Some(s.auth.header.as_str()), Some(s.auth.scheme)),
        _ => {
            let a = entity.auth.as_ref();
            (a.and_then(|a| a.header.as_deref()), a.and_then(|a| a.scheme))
        }
    };
    let name = match header {
        Some(h) => HeaderName::from_bytes(h.as_bytes()).map_err(|_| format!("auth header {h}: invalid"))?,
        None => AUTHORIZATION,
    };
    let scheme = scheme.unwrap_or(if name == AUTHORIZATION { AuthScheme::Bearer } else { AuthScheme::Raw });
    let value = released.secret().with_exposed(|s| match scheme {
        AuthScheme::Bearer => HeaderValue::from_str(&format!("Bearer {s}")).ok(),
        AuthScheme::Raw => HeaderValue::from_str(s).ok(),
        _ => None,
    });
    let mut value = value.ok_or_else(|| format!("auth scheme {scheme} can't carry this credential"))?;
    value.set_sensitive(true);
    headers.insert(name, value);
    Ok((headers, sent))
}

/// What one direct call came back with.
struct Reply {
    status: u16,
    headers: HeaderMap,
    body: bytes::Bytes,
}

/// Sends one call with `account`'s credential (refreshed first when near expiry, as a
/// request would). `Err` is a setup problem or a network failure, already redacted.
#[allow(clippy::too_many_arguments)]
async fn direct(
    engine: &Arc<Engine>,
    account: &Account,
    method: &str,
    url: &str,
    extra: &indexmap::IndexMap<String, String>,
    which: Identity,
    upstream_model: &str,
    body: bytes::Bytes,
) -> Result<(Reply, Vec<String>), String> {
    if account.is_signin() {
        match engine.fresh_for_use(&account.provider, &account.name).await {
            Refreshed::Fresh => {}
            Refreshed::Transient(r) | Refreshed::Permanent(r) => return Err(format!("token refresh failed: {r}")),
        }
    }
    let st = engine.snapshot();
    let entity = st.registry.provider(&account.provider).map_err(|e| e.to_string())?;
    let url = url::Url::parse(url).map_err(|e| format!("{url}: {e}"))?;
    let released = release(account, entity, &st.tokens).map_err(|w| shown(&st, &w.to_string(), 300))?;
    let (headers, sent) = call_headers(engine, &st, entity, account, &released, &url, extra, which, upstream_model)?;
    let method = reqwest::Method::from_bytes(method.to_ascii_uppercase().as_bytes()).map_err(|e| e.to_string())?;
    let call = async {
        let resp = st.http.request(method, url).headers(headers).body(body).send().await?;
        let status = resp.status().as_u16();
        let headers = resp.headers().clone();
        let body = resp.bytes().await?;
        Ok::<_, reqwest::Error>(Reply { status, headers, body })
    };
    match tokio::time::timeout(CALL_TIMEOUT, call).await {
        Err(_) => Err(format!("no answer within {} s", CALL_TIMEOUT.as_secs())),
        Ok(Err(e)) => Err(shown(&st, &e.to_string(), 300)),
        Ok(Ok(reply)) => Ok((reply, sent)),
    }
}

/// The text endpoint's URL and static headers.
fn text_endpoint(entity: &ProviderEntity) -> (String, indexmap::IndexMap<String, String>) {
    let e = entity.endpoints.get(&ModelType::Text).and_then(|s| s.0.first()).expect("a text endpoint");
    (e.url.clone(), e.headers.clone())
}

/// The output text of a Responses SSE stream (`response.output_text.delta` events), and the
/// first `error` / `response.failed` event's text, if any.
fn sse_text(body: &[u8]) -> (String, Option<String>) {
    let mut text = String::new();
    let mut error = None;
    for line in String::from_utf8_lossy(body).lines() {
        let Some(data) = line.strip_prefix("data:") else { continue };
        let Ok(v) = serde_json::from_str::<Value>(data.trim()) else { continue };
        match v.get("type").and_then(Value::as_str) {
            Some("response.output_text.delta") => text.push_str(v.get("delta").and_then(Value::as_str).unwrap_or("")),
            Some("error" | "response.failed") if error.is_none() => error = Some(v.to_string()),
            _ => {}
        }
    }
    (text, error)
}

/// L1 (T045): does a signed-in anthropic account serve a tiny Messages request with the
/// body as a client sends it? A refusal prints the status and the provider's text, which
/// becomes `[[signin.refused]]`.
#[tokio::test]
async fn signin_anthropic() {
    let _one = SIGNIN_LIVE.lock().await;
    let Some(engine) = open_live() else { return };
    let st = engine.snapshot();
    eprintln!(
        "L1 redirect: the token store doesn't record which redirect a sign-in used. Note what \
         `accounts signin anthropic` showed: \"the page shows a code; paste it here\" means \
         the hosted code page worked for this client id; a loopback/paste-the-address prompt \
         means it fell back."
    );
    let entity = st.registry.provider("anthropic").unwrap();
    let (url, headers) = text_endpoint(entity);
    let model = "claude-sonnet-4-20250514";
    for account in signin_accounts(&engine, &st, "anthropic") {
        let state = st.tokens.state(account);
        let tier = st.tokens.get(&account.provider, &account.name).and_then(|v| v.entry.claims.tier.clone());
        eprintln!("anthropic/{}: signed in, {}, tier {tier:?}", account.name, state_line(&state));
        if !state.serves() {
            eprintln!("anthropic/{}: skipped, out of service", account.name);
            continue;
        }
        let body = json!({
            "model": model,
            "max_tokens": 5,
            "messages": [{ "role": "user", "content": "Reply with the single word: pong" }],
        });
        let bytes = bytes::Bytes::from(serde_json::to_vec(&body).unwrap());
        let mut headers = headers.clone();
        headers.insert("content-type".into(), "application/json".into());
        match direct(&engine, account, "POST", &url, &headers, Identity::Full, model, bytes).await {
            Err(e) => eprintln!("anthropic/{}: not sent: {e}", account.name),
            Ok((r, sent)) if (200..300).contains(&r.status) => {
                let v: Value = serde_json::from_slice(&r.body).unwrap_or_default();
                let text = v["content"][0]["text"].as_str().unwrap_or_default();
                eprintln!(
                    "anthropic/{}: SERVED {} with identity headers {sent:?}, body untouched: {:?}, usage {}",
                    account.name,
                    r.status,
                    text.trim(),
                    shown(&st, &v["usage"].to_string(), 300)
                );
            }
            Ok((r, sent)) => eprintln!(
                "anthropic/{}: REFUSED {} with identity headers {sent:?}; provider text for \
                 [[signin.refused]]: {}",
                account.name,
                r.status,
                shown(&st, &String::from_utf8_lossy(&r.body), 1000)
            ),
        }
    }
}

/// L3 (T045): which grok-cli `[identity]` headers does cli-chat-proxy need, does it serve
/// the client's Responses body untouched (no `store`, `reasoning.summary`, `include`), and
/// does it refuse a body with an `item_reference` and foreign item ids? One tiny request
/// per variant, through the first serving account.
#[tokio::test]
async fn signin_grok_cli() {
    let _one = SIGNIN_LIVE.lock().await;
    let Some(engine) = open_live() else { return };
    let st = engine.snapshot();
    let entity = st.registry.provider("grok-cli").unwrap();
    let (url, headers) = text_endpoint(entity);
    let mut headers = headers.clone();
    headers.insert("content-type".into(), "application/json".into());
    headers.insert("accept".into(), "text/event-stream".into());
    let model = "grok-build";
    let plain = json!({
        "model": model,
        "stream": true,
        "input": [{ "role": "user", "content": "Reply with the single word: pong" }],
    });
    let foreign = json!({
        "model": model,
        "stream": true,
        "input": [
            { "role": "user", "content": "Say hi." },
            { "type": "item_reference", "id": "rs_68f0c0ffee0123456789abcdef012345" },
            {
                "type": "message",
                "id": "msg_68f0c0ffee0123456789abcdef012345",
                "role": "assistant",
                "content": [{ "type": "output_text", "text": "Hi." }],
            },
            { "role": "user", "content": "Reply with the single word: pong" },
        ],
    });
    let variants = [
        ("a) full [identity] headers, plain body", Identity::Full, &plain),
        ("b) fixed headers only, plain body", Identity::FixedOnly, &plain),
        ("c) full headers, item_reference + foreign ids", Identity::Full, &foreign),
    ];
    let accounts = signin_accounts(&engine, &st, "grok-cli");
    for account in &accounts {
        eprintln!("grok-cli/{}: signed in, {}", account.name, state_line(&st.tokens.state(account)));
    }
    let Some(account) = accounts.iter().find(|a| st.tokens.state(a).serves()) else {
        if !accounts.is_empty() {
            eprintln!("grok-cli: skipped, no account in service");
        }
        return;
    };
    for (label, which, body) in variants {
        let bytes = bytes::Bytes::from(serde_json::to_vec(body).unwrap());
        match direct(&engine, account, "POST", &url, &headers, which, model, bytes).await {
            Err(e) => eprintln!("grok-cli {label}: not sent: {e}"),
            Ok((r, sent)) => {
                let (text, error) = sse_text(&r.body);
                let verdict = if (200..300).contains(&r.status) && error.is_none() && !text.trim().is_empty() {
                    format!("PASSED {}: {:?}", r.status, text.trim())
                } else if let Some(e) = error {
                    format!("FAILED {} with a stream error: {}", r.status, shown(&st, &e, 800))
                } else {
                    format!("FAILED {}: {}", r.status, shown(&st, &String::from_utf8_lossy(&r.body), 800))
                };
                eprintln!("grok-cli {label} (identity headers {sent:?}): {verdict}");
            }
        }
    }
}

/// L4 (T056): each sign-in account's real token lifetime, and whether one refresh rotates
/// the refresh token. The refresh goes through the engine (written to `tokens.toml` before
/// the swap), so the account stays usable.
#[tokio::test]
async fn token_lifetimes() {
    let _one = SIGNIN_LIVE.lock().await;
    let Some(engine) = open_live() else { return };
    let st = engine.snapshot();
    let min_lead = engine.refresher.timing().min_lead;
    let accounts: Vec<&Account> = st.accounts.iter().filter(|a| a.is_signin() && !a.disabled).collect();
    if accounts.is_empty() {
        eprintln!(
            "skipped: no enabled sign-in account under {}; sign in first with \
             `NULLROUTER_HOME=… nullrouter accounts signin <provider> <name>`",
            engine.home().path().display()
        );
        return;
    }
    let mins = |d: Duration| format!("{:.1} min", d.as_secs_f64() / 60.0);
    for account in accounts {
        let who = format!("{}/{}", account.provider, account.name);
        let Some(before) = st.tokens.get(&account.provider, &account.name) else {
            eprintln!("{who}: no tokens stored; sign in again");
            continue;
        };
        let decl_lead =
            st.registry.provider(&account.provider).ok().and_then(|p| p.signin.as_ref()).map(|s| s.refresh_lead);
        let stored = lifetime(&before.entry);
        eprintln!(
            "{who}: {}, stored token lifetime (expires_in at receipt) {}, refresh_lead {:?}",
            state_line(&before.state),
            mins(stored),
            decl_lead.map(mins)
        );
        if before.entry.refresh_token.is_none() {
            eprintln!("{who}: no refresh token issued; nothing to rotate");
            continue;
        }
        if matches!(before.state, AccountState::NeedsSignIn { .. }) {
            eprintln!("{who}: skipped the refresh, the account needs signing in again");
            continue;
        }
        let old = before.entry.pair();
        match engine.refresh_account(&account.provider, &account.name).await {
            Refreshed::Fresh => {}
            Refreshed::Transient(r) | Refreshed::Permanent(r) => {
                eprintln!("{who}: refresh FAILED: {}", shown(&st, &r, 300));
                continue;
            }
        }
        let after = engine.tokens.get(&account.provider, &account.name).expect("refreshed tokens");
        let fresh = lifetime(&after.entry);
        let rotated = match (&old.refresh, &after.entry.refresh_token) {
            (Some(o), Some(n)) => !o.with_exposed(|o| n.matches(o)),
            _ => false,
        };
        let access_changed = !old.access.with_exposed(|o| after.entry.access_token.matches(o));
        eprintln!(
            "{who}: refreshed; expires_in {} ({} s), refresh token {}, access token {}",
            mins(fresh),
            fresh.as_secs(),
            if rotated { "ROTATED" } else { "kept (not rotated)" },
            if access_changed { "replaced" } else { "unchanged" }
        );
        if let Some(l) = decl_lead {
            let effective = lead(l, min_lead, fresh);
            let short = fresh < 2 * l;
            eprintln!(
                "{who}: effective refresh lead {}{}",
                mins(effective),
                if short { "; lifetime < 2 x refresh_lead: adjust refresh_lead (T056)" } else { "" }
            );
        }
    }
}

/// L2 + L5 (T078): each `[quota]` source's raw answer next to the windows the declared
/// extractor reads from it (the fallback only when the primary yields none, as the poller
/// does), then the `x-ratelimit-*` headers `api.x.ai/v1/models` returns to xai accounts.
#[tokio::test]
async fn quota() {
    let _one = SIGNIN_LIVE.lock().await;
    let Some(engine) = open_live() else { return };
    let st = engine.snapshot();
    let mut ran = 0;
    let accounts: Vec<&Account> = st.accounts.iter().filter(|a| !a.disabled).collect();
    for &account in &accounts {
        let who = format!("{}/{}", account.provider, account.name);
        let Ok(entity) = st.registry.provider(&account.provider) else { continue };
        let Some(decl) = nullrouter_engine::quota::poll::reported(entity, account) else { continue };
        let state = st.tokens.state(account);
        if !state.serves() {
            eprintln!("{who}: skipped, {}", state_line(&state));
            continue;
        }
        ran += 1;
        for (i, source) in decl.sources().enumerate() {
            let label = if i == 0 { "primary" } else { "fallback" };
            let req = &source.request;
            let body = match req.body {
                QuotaBody::None => bytes::Bytes::new(),
                QuotaBody::GrpcWebEmpty => bytes::Bytes::from_static(&[0u8; 5]),
            };
            let got = direct(&engine, account, &req.method, &req.url, &req.headers, Identity::Full, "", body).await;
            let r = match got {
                Ok((r, _)) => r,
                Err(e) => {
                    eprintln!("{who} {label} {}: not sent: {e}", req.url);
                    break;
                }
            };
            let raw = if req.body == QuotaBody::GrpcWebEmpty || std::str::from_utf8(&r.body).is_err() {
                let hex: String = r.body.iter().take(48).map(|b| format!("{b:02x}")).collect();
                format!("{} bytes, starting {hex}", r.body.len())
            } else {
                shown(&st, &String::from_utf8_lossy(&r.body), 1500)
            };
            eprintln!("{who} {label} {} -> {}: {raw}", req.url, r.status);
            if !(200..300).contains(&r.status) {
                break;
            }
            let windows = nullrouter_engine::quota::read(source, &r.body);
            if windows.is_empty() {
                eprintln!("{who} {label}: NO WINDOW extracted");
                continue;
            }
            for w in &windows {
                eprintln!(
                    "{who} {label}: window {:?} ({:?}) used {:?} limit {:?} remaining {:?} resets {:?}",
                    w.name,
                    w.unit,
                    w.used,
                    w.limit,
                    w.remaining,
                    w.resets_at.map(nullrouter_engine::quota::extract::rfc3339_millis)
                );
            }
            break;
        }
    }
    if ran == 0 {
        eprintln!("quota: no account with a [quota] section is in service");
    }

    // L2: one GET per kind of xai account (sign-in first, then key).
    let xai: Vec<&Account> = accounts.iter().copied().filter(|a| a.provider == "xai").collect();
    if xai.is_empty() {
        eprintln!("xai: skipped, no enabled account");
    }
    for signin in [true, false] {
        let Some(&account) = xai.iter().find(|a| a.is_signin() == signin && st.tokens.state(a).serves()) else {
            continue;
        };
        let who = format!("xai/{} ({})", account.name, if signin { "sign-in" } else { "key" });
        let got = direct(
            &engine,
            account,
            "GET",
            "https://api.x.ai/v1/models",
            &indexmap::IndexMap::new(),
            Identity::Full,
            "",
            bytes::Bytes::new(),
        )
        .await;
        match got {
            Err(e) => eprintln!("{who}: not sent: {e}"),
            Ok((r, _)) => {
                let limits: Vec<String> = r
                    .headers
                    .iter()
                    .filter(|(k, _)| k.as_str().starts_with("x-ratelimit-"))
                    .map(|(k, v)| format!("{k}: {}", v.to_str().unwrap_or("?")))
                    .collect();
                eprintln!("{who}: GET /v1/models -> {}", r.status);
                if limits.is_empty() {
                    eprintln!("{who}: NO x-ratelimit-* headers (xai keeps \"quota not reported\")");
                } else {
                    eprintln!("{who}: x-ratelimit-* headers: {limits:#?}");
                }
                if !(200..300).contains(&r.status) {
                    eprintln!("{who}: {}", shown(&st, &String::from_utf8_lossy(&r.body), 500));
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Slice 006, L7 (T085, SC-007): the routing view matches the provider's own polls.

/// Every window of the view's account `provider/account` for `target`.
fn view_windows(
    engine: &Engine,
    target: &str,
    provider: &str,
    account: &str,
) -> Vec<nullrouter_engine::routing::view::WindowView> {
    let st = engine.snapshot();
    nullrouter_engine::route::view_all(engine, &st, Some(target), std::time::SystemTime::now())
        .into_iter()
        .flat_map(|t| t.accounts)
        .find(|a| a.provider == provider && a.account == account)
        .map(|a| a.windows)
        .unwrap_or_default()
}

/// For each polled account: poll it and compare the view's `remaining_now` with the poll's
/// `remaining` (they must agree: nothing was sent in between), send one tiny request through the
/// provider, and check that the view's drop equals its own metered `cost_since_poll`. A second
/// poll then shows what the provider charged; the difference is printed as a meter correction
/// for `plugins/bundled/*.toml`, never asserted, because a provider rounds its percentages.
///
/// `-- live_routing_matches_polls --nocapture`. Skipped unless `NR_LIVE=1`; with it, a run that
/// checks no account fails, naming the home it read and the account kinds it needs.
#[tokio::test]
async fn live_routing_matches_polls() {
    let _one = SIGNIN_LIVE.lock().await;
    let Some(engine) = open_live() else { return };
    let st = engine.snapshot();
    let mut checked = 0;
    let mut corrections = Vec::new();
    for account in st.accounts.iter().filter(|a| !a.disabled) {
        let who = format!("{}/{}", account.provider, account.name);
        let Ok(entity) = st.registry.provider(&account.provider) else { continue };
        if nullrouter_engine::quota::poll::reported(entity, account).is_none() {
            continue;
        }
        let Some(target) = targets().into_iter().find(|t| t.split('/').next() == Some(account.provider.as_str()))
        else {
            eprintln!("{who}: skipped, no live target for {}", account.provider);
            continue;
        };
        let Some(first) = engine.poll_quota(&account.provider, &account.name).await else {
            eprintln!("{who}: skipped, the poll did not run");
            continue;
        };
        if !first.ok() {
            eprintln!("{who}: skipped, the poll failed: {:?}", first.error);
            continue;
        }
        // 1. The view agrees with the poll, window by window.
        let before = view_windows(&engine, &target, &account.provider, &account.name);
        assert!(!before.is_empty(), "{who}: the routing view has no windows for {target}");
        for w in &first.windows {
            let Some(v) = before.iter().find(|v| v.name == w.name) else {
                eprintln!("{who}: window {:?} is reported but the view does not show it (no meter names it)", w.name);
                continue;
            };
            let reported = v.remaining_at_poll.expect("a polled window has a remaining");
            assert!(
                v.cost_since_poll.abs() < 1e-6,
                "{who} {}: cost since the poll is {} right after it",
                w.name,
                v.cost_since_poll
            );
            assert!(
                (v.remaining_now - reported).abs() <= reported.abs() * 1e-6 + 1e-6,
                "{who} {}: the view says {} left, the poll says {reported}",
                w.name,
                v.remaining_now
            );
            eprintln!("{who} {}: view {:.0} = poll {reported:.0} {}", w.name, v.remaining_now, v.unit);
        }
        checked += 1;

        // 2. One tiny request: the view's drop is the cost it metered.
        let body = json!({"model": target, "max_tokens": 16, "messages": [{"role": "user", "content": "Reply with the single word: pong"}]});
        let (_, rec) = send(&engine, "openai-chat", &target, body).await;
        assert_eq!(rec.outcome, Outcome::Succeeded, "{who}: {rec:#?}");
        let served = rec.served_by.as_ref().map(|s| (s.provider.clone(), s.account.clone().unwrap_or_default()));
        if served.as_ref().map(|(p, a)| (p.as_str(), a.as_str()))
            != Some((account.provider.as_str(), account.name.as_str()))
        {
            eprintln!("{who}: the request went to {served:?}; the drop is checked for that account on its own turn");
            continue;
        }
        let after = view_windows(&engine, &target, &account.provider, &account.name);
        for (b, a) in before.iter().zip(&after) {
            let drop = b.remaining_now - a.remaining_now;
            assert!(
                (drop - a.cost_since_poll).abs() <= a.cost_since_poll.abs() * 1e-6 + 1e-6,
                "{who} {}: remaining fell by {drop} but the metered cost is {}",
                a.name,
                a.cost_since_poll
            );
            eprintln!("{who} {}: fell by {drop:.0} {}, the meter charged {:.0}", a.name, a.unit, a.cost_since_poll);
        }

        // 3. The provider's own account of it.
        tokio::time::sleep(Duration::from_secs(3)).await;
        if let Some(second) = engine.poll_quota(&account.provider, &account.name).await.filter(|p| p.ok()) {
            for (w0, w1) in first.windows.iter().zip(&second.windows) {
                let (Some(r0), Some(r1), Some(a)) = (
                    w0.remaining.or(w0.used.map(|u| 100.0 - u)),
                    w1.remaining.or(w1.used.map(|u| 100.0 - u)),
                    after.iter().find(|a| a.name == w1.name),
                ) else {
                    continue;
                };
                // A percent window is converted through the meter's capacity.
                let provider_drop = if w1.unit == nullrouter_registry::schema::QuotaUnit::Percent {
                    (r0 - r1) / 100.0 * a.capacity
                } else {
                    r0 - r1
                };
                let line = format!(
                    "{who} {}: provider charged {provider_drop:.0}, the meter charged {:.0} ({} capacity {:.0})",
                    w1.name, a.cost_since_poll, a.unit, a.capacity
                );
                eprintln!("{line}");
                if (provider_drop - a.cost_since_poll).abs() > a.capacity * 0.01 {
                    corrections.push(line);
                }
            }
        }
    }
    // A run that compared nothing proves nothing: say where it looked and what it needs.
    assert!(
        checked > 0,
        "live_routing_matches_polls checked no account. It read the home {} and found no enabled \
         account with a quota poll that has a live target and answered the poll. Add one of an \
         anthropic, grok-cli, opencode-go or opencode-zen account to that home (set NULLROUTER_HOME \
         to point at the right one), then rerun.",
        engine.home().path().display()
    );
    if !corrections.is_empty() {
        eprintln!(
            "\nMETER CORRECTIONS NEEDED (over 1% of capacity; record each in plugins/bundled/*.toml with a dated source comment):"
        );
        for c in &corrections {
            eprintln!("  {c}");
        }
    }
}
