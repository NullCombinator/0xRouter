//! The operator socket (contracts/operator-cli.md § Operator socket protocol): NDJSON over
//! a Unix socket at `$NULLROUTER_HOME/run/operator.sock`, mode 0600 inside a 0700
//! directory. One request per line, one response per line.
//!
//! | Request | Response |
//! |---|---|
//! | `{"op":"reload"}` | `{"ok":true,"generation":N}` (plus `"notes":[…]` when unified models' limits differ), or the error with the old snapshot kept |
//! | `{"op":"records.list","provider"?,"unified_model"?,"account"?,"agent"?,"model"?,"reason"?,"since"?,"limit"?,"before"?}` | `{"ok":true,"records":[…]}`: the journal's records plus those still in flight, newest first, those with an id below `before` (which must name a record, else `{"ok":false,"error":"no record rq_…"}`) |
//! | `{"op":"records.get","id":"rq_…"}` | `{"ok":true,"record":{…}}` |
//! | `{"op":"keys.last_used"}` | `{"ok":true,"last_used":{"<key id>":"<RFC 3339>"\|null}}`: for every key in `keys.toml`, the arrival of its newest record, from the cached segment index and the requests still in flight; `null` for a key no record names |
//! | `{"op":"usage.totals","from":"<RFC 3339>"\|null,"to":"<RFC 3339>"}` | `{"ok":true,"totals":{…}}`: requests, tokens, Est. Cost and per-agent and per-provider counts for arrivals in `[from, to)`, from the journal (finished days cached) and the requests still in memory |
//! | `{"op":"latency.summary","from":"<RFC 3339>","to":"<RFC 3339>"}` | `{"ok":true,"latency":{agents,providers}}`: per agent key and per provider, router overhead and time to first token as p50/p95 (nearest rank), the provider's own wait, requests and the last response, for arrivals in `[from, to)`, from the journal and the requests still in memory; never cached |
//! | `{"op":"records.forget","account"?:"P/N","agent"?:KEY}` | `{"ok":true,"fingerprints":N}`: the agent's fingerprints (or the account's fingerprints and ledger entries) leave memory and `routing/warm.jsonl`, and the live ring; the CLI then rewrites the record segments |
//! | `{"op":"accounts.state"}` | `{"ok":true,"accounts":[…]}`: per account `kind`, `state`, `state_since`, `state_reason`, `expires_at`, cooldowns |
//! | `{"op":"routing.view","target"?}` | `{"ok":true,"amortization":{start,length},"journal":{…},"targets":[…],"warnings":[…]}`: per target and account the pace, share, deficit, priority, cache lifetime, quota source, each window's remaining amount, unit, reset and reserve, and the fit's `meter`, `outside_use` and `fit_note`; warnings add the fit's break, failed save and unacknowledged usage alerts |
//! | `{"op":"routing.health"}` | `{"ok":true,"journal":{kept,since,unkept_requests,held_lines,last_sync,last_sync_age_s}}` |
//! | `{"op":"server.status"}` | `{"ok":true,"client_listen":"…"\|null,"dashboard":{enabled,listen,serving,error}}`: the address `serve` bound for clients, and the dashboard listener's state |
//! | `{"op":"live.snapshot"}` | `{"ok":true,"as_of":…,"paused_proxies":[…],"in_flight":[…]}`: what is in flight now, each request in its current phase |
//! | `{"op":"proxy.fixed","name"}` | `{"ok":true,"reachable":true}` and the pause cleared, or `{"ok":true,"reachable":false,"reason"}`; an unknown name is `{"ok":false,"error"}` listing the known ones |
//! | `{"op":"connection.view","provider"?}` | `{"ok":true,"providers":[{id,timeouts:{connect,headers,first_token,stall}:{ms\|null,source},models:[{id,timeouts}]}]}`: the effective timeouts and where each came from, and the models whose timeouts differ; an unknown provider is `{"ok":false,"error"}` listing the known ones |
//! | `{"op":"quota.list"}`, `{"op":"quota.poll"}`, `{"op":"quota.checkpoint"}` | see [`crate::quota`] |
//! | `{"op":"quota.outside","provider"?,"account"?,"since"?,"limit"?}`, `{"op":"quota.alerts"}`, `{"op":"quota.ack","id"?,"provider"?,"account"?}`, `{"op":"quota.prune","before","provider"?,"account"?}` | see [`crate::quota`] |
//! | `{"op":"test.plan","target"?,"account"?,"all"?}` | `{"ok":true,"pairs":[{provider,account,model,type,skip?}],"calls":{"<type>":N}}` (spec 011); a combo target adds `"combo":NAME` and counts as 1 call of its kind |
//! | `{"op":"test.run","target"?,"account"?,"all"?}` | streamed: one `{"event":"result","result":TestResult}` line per pair (a combo: one `{"event":"combo","result":ComboResult}`), then `{"ok":true,"done":{pass,broken,unknown,skipped}}`. Closing the connection cancels calls not yet sent |
//! | `{"op":"verdicts.list","provider"?,"account"?,"model"?,"state"?}` | `{"ok":true,"verdicts":[{provider,account,model,…Verdict,"waiting"?}],"combos":[{combo,…,"waiting"?}]}`: `waiting` says why a due retest can't run yet |
//! | `{"op":"verdicts.set","provider","account","model","state":"broken"\|"clear","note"?}` | `{"ok":true}`, or `{"ok":false,"error":"no verdict for …"}` on clearing an untested pair |

use std::collections::BTreeSet;
use std::future::Future;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use nullrouter_engine::journal::{records, summary};
use nullrouter_engine::records::Query;
use nullrouter_engine::routing::view::TargetView;
use nullrouter_engine::state::{Engine, EngineState};
use nullrouter_engine::tests::{self as model_tests, Planned};
use nullrouter_engine::verdict::{Source, State};
use nullrouter_registry::OperatorHome;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader as AsyncBufReader, Lines};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// The socket's path under the operator home.
pub fn socket_path(home: &OperatorHome) -> PathBuf {
    home.path().join("run").join("operator.sock")
}

/// Binds the socket, replacing a stale one. Refuses when another server answers on it.
pub fn bind(home: &OperatorHome) -> std::io::Result<UnixListener> {
    let path = socket_path(home);
    let dir = path.parent().unwrap_or(home.path());
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    if path.exists() {
        if std::os::unix::net::UnixStream::connect(&path).is_ok() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AddrInUse,
                format!("{}: another server is running on this home", path.display()),
            ));
        }
        std::fs::remove_file(&path)?;
    }
    let listener = UnixListener::bind(&path)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

/// Answers on `listener` until `shutdown` resolves, then removes the socket file.
pub async fn serve(engine: Arc<Engine>, listener: UnixListener, shutdown: impl Future<Output = ()>) {
    let path = listener.local_addr().ok().and_then(|a| a.as_pathname().map(Path::to_owned));
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            () = &mut shutdown => break,
            conn = listener.accept() => match conn {
                Ok((stream, _)) => { tokio::spawn(connection(engine.clone(), stream)); }
                Err(e) => tracing::warn!("operator socket: {e}"),
            },
        }
    }
    if let Some(p) = path {
        let _ = std::fs::remove_file(p);
    }
}

async fn connection(engine: Arc<Engine>, stream: UnixStream) {
    let (read, mut write) = stream.into_split();
    let mut lines = AsyncBufReader::new(read).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let answer = match serde_json::from_str::<Value>(&line) {
            Ok(req) if req.get("op").and_then(Value::as_str) == Some("test.run") => {
                match test_run(&engine, &req, &mut lines, &mut write).await {
                    Some(done) => done,
                    None => return,
                }
            }
            Ok(req) => handle(&engine, &req).await,
            Err(e) => json!({"ok": false, "error": format!("not JSON: {e}")}),
        };
        if send(&mut write, &answer).await.is_err() {
            return;
        }
    }
}

async fn send(write: &mut OwnedWriteHalf, v: &Value) -> std::io::Result<()> {
    let mut out = v.to_string();
    out.push('\n');
    write.write_all(out.as_bytes()).await
}

/// The pairs a `test.plan` or `test.run` request names.
fn planned(engine: &Engine, req: &Value) -> Result<Vec<Planned>, String> {
    let target = req.get("target").and_then(Value::as_str);
    let account = req.get("account").and_then(Value::as_str);
    let all = req.get("all").and_then(Value::as_bool).unwrap_or(false);
    if target.is_some() == all {
        return Err("name a target, or all".into());
    }
    model_tests::expand(engine, &engine.snapshot(), target, account)
}

/// The combo `req` targets, if it names one, with the type of its call: its test is one call
/// through it (research R14).
fn combo_target(engine: &Engine, req: &Value) -> Option<Result<(String, String), String>> {
    let target = req.get("target").and_then(Value::as_str)?;
    let st = engine.snapshot();
    let combo = st.registry.combo(target)?;
    if req.get("account").is_some_and(|a| !a.is_null()) {
        return Some(Err(format!("{target} is a combo: its test isn't per account")));
    }
    Some(Ok((combo.name.clone(), model_tests::combo::kind(combo).to_string())))
}

fn test_plan(engine: &Engine, req: &Value) -> Value {
    match combo_target(engine, req) {
        Some(Ok((name, ty))) => return json!({"ok": true, "pairs": [], "combo": name, "calls": {ty: 1}}),
        Some(Err(e)) => return json!({"ok": false, "error": e}),
        None => {}
    }
    match planned(engine, req) {
        Ok(p) => json!({"ok": true, "pairs": p, "calls": model_tests::calls(&p)}),
        Err(e) => json!({"ok": false, "error": e}),
    }
}

/// Streams `test.run`'s results; returns the closing line, or `None` once the client has gone
/// (the calls not yet sent are cancelled; those in flight finish and keep their verdicts).
async fn test_run(
    engine: &Arc<Engine>,
    req: &Value,
    lines: &mut Lines<AsyncBufReader<OwnedReadHalf>>,
    write: &mut OwnedWriteHalf,
) -> Option<Value> {
    match combo_target(engine, req) {
        Some(Ok((name, _))) => return combo_run(engine, name, lines, write).await,
        Some(Err(e)) => return Some(json!({"ok": false, "error": e})),
        None => {}
    }
    let planned = match planned(engine, req) {
        Ok(p) => p,
        Err(e) => return Some(json!({"ok": false, "error": e})),
    };
    let stop = CancellationToken::new();
    let (tx, mut rx) = mpsc::channel(16);
    tokio::spawn({
        let (engine, stop, run) = (engine.clone(), stop.clone(), model_tests::run_id());
        async move { model_tests::run(&engine, planned, Source::Test, &run, stop, tx).await }
    });
    let (mut pass, mut broken, mut unknown, mut skipped) = (0, 0, 0, 0);
    loop {
        tokio::select! {
            r = rx.recv() => {
                let Some(r) = r else { break };
                match r.state {
                    Some(State::Pass) => pass += 1,
                    Some(State::Broken) => broken += 1,
                    Some(State::Unknown) => unknown += 1,
                    None => skipped += 1,
                }
                if send(write, &json!({"event": "result", "result": r})).await.is_err() {
                    stop.cancel();
                    return None;
                }
            }
            l = lines.next_line() => if !matches!(l, Ok(Some(_))) {
                stop.cancel();
                return None;
            },
        }
    }
    Some(json!({"ok": true, "done": {"pass": pass, "broken": broken, "unknown": unknown, "skipped": skipped}}))
}

/// `test.run` for a combo: one call through it, then its nested result as one line.
async fn combo_run(
    engine: &Arc<Engine>,
    name: String,
    lines: &mut Lines<AsyncBufReader<OwnedReadHalf>>,
    write: &mut OwnedWriteHalf,
) -> Option<Value> {
    let stop = CancellationToken::new();
    let mut task = tokio::spawn({
        let (engine, stop, run) = (engine.clone(), stop.clone(), model_tests::run_id());
        async move { model_tests::combo::run_combo(&engine, &name, Source::Test, &run, &stop).await }
    });
    let ended = loop {
        tokio::select! {
            r = &mut task => break r,
            l = lines.next_line() => if !matches!(l, Ok(Some(_))) {
                stop.cancel();
                return None;
            },
        }
    };
    let r = match ended {
        Ok(Ok(Some(r))) => r,
        Ok(Ok(None)) => return Some(json!({"ok": false, "error": "the combo test was cancelled"})),
        Ok(Err(e)) => return Some(json!({"ok": false, "error": e})),
        Err(e) => return Some(json!({"ok": false, "error": format!("the combo test failed: {e}")})),
    };
    let mut done = json!({"pass": 0, "broken": 0, "unknown": 0, "skipped": 0});
    done[r.state.as_str()] = json!(1);
    if send(write, &json!({"event": "combo", "result": r})).await.is_err() {
        return None;
    }
    Some(json!({"ok": true, "done": done}))
}

/// The listeners `serve` bound (spec 009, R8).
fn server_status(engine: &Engine) -> Value {
    let d = engine.status.dashboard();
    json!({
        "ok": true,
        "client_listen": engine.status.client_listen(),
        "dashboard": { "enabled": d.enabled, "listen": d.listen, "serving": d.serving, "error": d.error },
    })
}

/// One request's answer.
pub async fn handle(engine: &Arc<Engine>, req: &Value) -> Value {
    let str_of = |k: &str| req.get(k).and_then(Value::as_str).map(str::to_owned);
    match req.get("op").and_then(Value::as_str) {
        Some("reload") => match engine.reload().await {
            Ok(r) => {
                for a in &r.unused_accounts {
                    tracing::warn!("account {a} names a provider that isn't loaded");
                }
                let notes: Vec<String> =
                    engine.snapshot().registry.report().notes.iter().map(ToString::to_string).collect();
                for n in &notes {
                    tracing::info!("{n}");
                }
                let mut answer = json!({"ok": true, "generation": r.generation});
                if !notes.is_empty() {
                    answer["notes"] = json!(notes);
                }
                answer
            }
            Err(e) => {
                let error = engine.snapshot().redactor.redact(&e.to_string()).into_owned();
                tracing::error!("reload failed, the previous state stays: {error}");
                json!({"ok": false, "error": error})
            }
        },
        Some("records.list") => records_list(engine, req).await,
        Some("live.snapshot") => live_snapshot(engine),
        Some("connection.view") => {
            let st = engine.snapshot();
            match nullrouter_engine::connection::view(&st.registry, str_of("provider").as_deref()) {
                Ok(mut v) => {
                    nullrouter_engine::connection::add_proxies(&mut v, &st, &engine.proxy_board);
                    v
                }
                Err(error) => json!({"ok": false, "error": error}),
            }
        }
        Some("proxy.fixed") => proxy_fixed(engine, str_of("name")).await,
        Some("records.get") => {
            let Some(id) = str_of("id") else { return json!({"ok": false, "error": "the request names no id"}) };
            if let Some(r) = engine.records.get(&id) {
                let mut record = json!(r);
                nullrouter_engine::phases::decorate(&mut record, None);
                return json!({"ok": true, "record": record});
            }
            let home = engine.home().path().to_owned();
            let found = tokio::task::spawn_blocking(move || nullrouter_engine::journal::records::get(&home, &id)).await;
            match found {
                Ok(Some(mut r)) => {
                    nullrouter_engine::phases::decorate(&mut r, None);
                    json!({"ok": true, "record": r})
                }
                _ => json!({"ok": false, "error": "no such record"}),
            }
        }
        Some("keys.last_used") => keys_last_used(engine).await,
        Some("usage.totals") => usage_totals(engine, req).await,
        Some("latency.summary") => latency_summary(engine, req).await,
        Some("records.forget") => {
            let st = engine.snapshot();
            let now = nullrouter_engine::clock::now();
            let (account, agent) = (str_of("account"), str_of("agent"));
            let fingerprints = match (&account, &agent) {
                (Some(a), None) => match a.split_once('/') {
                    Some((p, n)) => {
                        engine
                            .records
                            .forget(|r| records::Filter { account: Some(a.clone()), ..Default::default() }.matches(r));
                        nullrouter_engine::route::drop_account(engine, &st, p, n, now)
                    }
                    None => return json!({"ok": false, "error": "--account is provider/name"}),
                },
                (None, Some(k)) => {
                    engine
                        .records
                        .forget(|r| records::Filter { agent: Some(k.clone()), ..Default::default() }.matches(r));
                    nullrouter_engine::route::drop_agent(engine, &st, k, now)
                }
                _ => return json!({"ok": false, "error": "name an account or an agent, not both"}),
            };
            json!({"ok": true, "fingerprints": fingerprints})
        }
        Some("accounts.state") => {
            let st = engine.snapshot();
            let active = engine.cooldowns.active();
            let accounts: Vec<Value> = st
                .accounts
                .iter()
                .map(|a| {
                    let cooling: Vec<Value> = active
                        .iter()
                        .filter(|(p, n, _, _)| *p == a.provider && *n == a.name)
                        .map(|(_, _, m, left)| json!({"model": m, "remaining_ms": left.as_millis() as u64}))
                        .collect();
                    // Sign-in states (research R10); tokens never cross the socket.
                    let state = engine.tokens.state(a);
                    let time = nullrouter_engine::clock::rfc3339;
                    let expires_at = a
                        .is_signin()
                        .then(|| engine.tokens.get(&a.provider, &a.name))
                        .flatten()
                        .map(|v| time(v.entry.expires_at));
                    // `needs_sign_in` without tokens has no time.
                    let since = state.since().filter(|t| *t > std::time::UNIX_EPOCH).map(time);
                    json!({
                        "provider": a.provider,
                        "name": a.name,
                        "kind": if a.is_signin() { "signin" } else { "key" },
                        "disabled": a.disabled,
                        "state": state.name(),
                        "state_since": since,
                        "state_reason": state.reason(),
                        "expires_at": expires_at,
                        "level": engine.cooldowns.level(&a.provider, &a.name),
                        "cooling": cooling,
                    })
                })
                .collect();
            json!({"ok": true, "accounts": accounts})
        }
        Some("routing.view") => routing_view(engine, str_of("target").as_deref()),
        Some("routing.health") => json!({"ok": true, "journal": journal_health(engine)}),
        Some("server.status") => server_status(engine),
        Some("quota.list") => crate::quota::list(engine, req),
        Some("quota.poll") => crate::quota::poll_now(engine, req).await,
        Some("quota.checkpoint") => crate::quota::checkpoint(engine).await,
        Some("quota.outside") => crate::quota::outside(engine, req).await,
        Some("quota.alerts") => crate::quota::alerts(engine).await,
        Some("quota.ack") => crate::quota::ack(engine, req).await,
        Some("quota.prune") => crate::quota::prune(engine, req).await,
        Some("test.plan") => test_plan(engine, req),
        Some("test.run") => json!({"ok": false, "error": "test.run streams: send it on its own connection"}),
        Some("verdicts.list") => verdicts_list(engine, req),
        Some("verdicts.set") => verdicts_set(engine, req),
        Some(op) => json!({"ok": false, "error": format!("unknown op {op:?}")}),
        None => json!({"ok": false, "error": "the request names no op"}),
    }
}

/// `verdicts.list`: every verdict the filter keeps, with why its retest waits, and the combo
/// results (those only with no provider, account or model filter). Reasons pass the redactor.
fn verdicts_list(engine: &Engine, req: &Value) -> Value {
    use nullrouter_engine::tests::retest;
    use nullrouter_engine::verdict::{Filter, State, store};

    let str_of = |k: &str| req.get(k).and_then(Value::as_str).map(str::to_owned);
    let state = match str_of("state") {
        Some(s) => match State::parse(&s) {
            Some(s) => Some(s),
            None => return json!({"ok": false, "error": "--state is pass, broken or unknown"}),
        },
        None => None,
    };
    let filter = Filter { provider: str_of("provider"), account: str_of("account"), model: str_of("model"), state };
    let st = engine.snapshot();
    let now = nullrouter_engine::clock::now();
    let redact = |line: &mut Value| {
        if let Some(r) = line["reason"].as_str() {
            let shown = st.redactor.redact(r).into_owned();
            line["reason"] = json!(shown);
        }
    };
    let verdicts: Vec<Value> = engine
        .verdicts
        .list(&filter)
        .into_iter()
        .map(|(pair, v)| {
            let mut line = store::set_line(&pair, &v);
            if let Some(map) = line.as_object_mut() {
                map.remove("basis");
            }
            redact(&mut line);
            if let Some(why) = v.next.and_then(|_| retest::waiting(engine, &st, &pair, now)) {
                line["waiting"] = json!(why);
            }
            line
        })
        .collect();
    let by_pair = filter.provider.is_some() || filter.account.is_some() || filter.model.is_some();
    let combos: Vec<Value> = if by_pair {
        Vec::new()
    } else {
        let all = engine.verdicts.snapshot();
        all.combos
            .iter()
            .filter(|(_, c)| state.is_none_or(|s| s == c.state))
            .map(|(name, c)| {
                let mut line = store::combo_line(name, c);
                if let Some(map) = line.as_object_mut() {
                    map.remove("definition");
                }
                redact(&mut line);
                let combo = c.next.and_then(|_| st.registry.combo(name));
                if let Some(why) = combo.and_then(|k| retest::combo_waiting(engine, &st, k)) {
                    line["waiting"] = json!(why);
                }
                line
            })
            .collect()
    };
    json!({"ok": true, "verdicts": verdicts, "combos": combos})
}

/// `verdicts.set`: the operator marks a pair BROKEN (with an optional note) or clears it.
fn verdicts_set(engine: &Engine, req: &Value) -> Value {
    use nullrouter_engine::verdict::{self, Mark};

    let str_of = |k: &str| req.get(k).and_then(Value::as_str).map(str::to_owned);
    let (Some(provider), Some(account), Some(model)) = (str_of("provider"), str_of("account"), str_of("model")) else {
        return json!({"ok": false, "error": "verdicts.set names a provider, an account and a model"});
    };
    let mark = match str_of("state").as_deref() {
        Some("broken") => Mark::Broken { note: str_of("note").filter(|n| !n.trim().is_empty()) },
        Some("clear") => Mark::Clear,
        _ => return json!({"ok": false, "error": "state is broken or clear"}),
    };
    match verdict::mark(engine, &provider, &account, &model, mark) {
        Ok(()) => json!({"ok": true}),
        Err(e) => json!({"ok": false, "error": e}),
    }
}

/// `keys.last_used`: the journal read runs on the blocking pool, as `records.get`'s does.
/// The requests in flight (spec 013, contracts/operator-socket.md). Paused proxies come with
/// the proxy slice; until then the list is empty. The agent shows as the key's name.
fn live_snapshot(engine: &Arc<Engine>) -> Value {
    let st = engine.snapshot();
    let names: std::collections::HashMap<&str, &str> = st.keys.iter().map(|k| (k.id.as_str(), k.name.as_str())).collect();
    let in_flight: Vec<Value> = engine
        .live
        .snapshot()
        .into_iter()
        .filter_map(|s| serde_json::to_value(&s).ok().map(|v| (s.agent, v)))
        .map(|(agent, mut v)| {
            v["agent"] = json!(names.get(agent.as_str()).copied().unwrap_or(&agent));
            v
        })
        .collect();
    json!({
        "ok": true,
        "as_of": nullrouter_engine::clock::now_rfc3339(),
        "paused_proxies": engine
            .proxy_board
            .list()
            .into_iter()
            .map(|(name, p)| json!({"name": name, "since": p.since, "reason": p.reason}))
            .collect::<Vec<_>>(),
        "in_flight": in_flight,
    })
}

/// `proxy.fixed`: probes the proxy, and resumes it if it answers (clarify Q1).
async fn proxy_fixed(engine: &Arc<Engine>, name: Option<String>) -> Value {
    let Some(name) = name else { return json!({"ok": false, "error": "the request names no proxy"}) };
    let st = engine.snapshot();
    let Some(proxy) = st.clients.proxies().get(&name) else {
        let known: Vec<&str> = st.clients.proxies().iter().map(|p| p.name.as_str()).collect();
        return json!({"ok": false, "error": format!("no proxy {name:?}; known proxies: {}", known.join(", "))});
    };
    match engine.proxy_board.fixed(proxy, std::time::Duration::from_secs(10)).await {
        Ok(()) => json!({"ok": true, "reachable": true}),
        Err(reason) => json!({"ok": true, "reachable": false, "reason": reason}),
    }
}

async fn keys_last_used(engine: &Arc<Engine>) -> Value {
    use nullrouter_engine::keys::{self, Keys};

    let home = engine.home().path().to_owned();
    let read = tokio::task::spawn_blocking(move || {
        let list = Keys::load(&home.join(keys::FILE))?;
        let ids: Vec<String> = list.iter().map(|k| k.id.clone()).collect();
        Ok::<_, nullrouter_engine::files::FileError>((ids, records::last_arrived(&home)))
    })
    .await;
    let (ids, mut last) = match read {
        Ok(Ok(read)) => read,
        Ok(Err(e)) => return json!({"ok": false, "error": e.to_string()}),
        Err(e) => return json!({"ok": false, "error": format!("the read failed: {e}")}),
    };
    // A request in flight is a record too, and may not be in the journal yet.
    let parse = nullrouter_engine::clock::parse_rfc3339;
    for r in engine.records.query(&Query::default()) {
        let Some(agent) = &r.agent else { continue };
        let newer = match last.get(&agent.key) {
            Some(known) => parse(&r.arrived).zip(parse(known)).is_some_and(|(new, old)| new > old),
            None => parse(&r.arrived).is_some(),
        };
        if newer {
            last.insert(agent.key.clone(), r.arrived.clone());
        }
    }
    let shown: serde_json::Map<String, Value> =
        ids.into_iter().map(|id| (id.clone(), last.get(&id).map_or(Value::Null, |t| json!(t)))).collect();
    json!({"ok": true, "last_used": shown})
}

/// The window an op names: `from` is null for all time, `to` is required.
fn window_of(req: &Value) -> Result<summary::Window, Value> {
    let parse = nullrouter_engine::clock::parse_rfc3339;
    let to = req.get("to").and_then(Value::as_str).and_then(parse);
    let from = match req.get("from") {
        None | Some(Value::Null) => None,
        Some(f) => match f.as_str().and_then(parse) {
            Some(t) => Some(t),
            None => return Err(json!({"ok": false, "error": "from is not an RFC 3339 time"})),
        },
    };
    match to {
        Some(to) => Ok(summary::Window { from, to }),
        None => Err(json!({"ok": false, "error": "to is not an RFC 3339 time"})),
    }
}

/// `usage.totals`: the journal read runs on the blocking pool, with the requests still in the
/// live ring merged over it.
async fn usage_totals(engine: &Arc<Engine>, req: &Value) -> Value {
    let w = match window_of(req) {
        Ok(w) => w,
        Err(e) => return e,
    };
    let home = engine.home().path().to_owned();
    let st = engine.snapshot();
    let live: Vec<Value> =
        engine.records.query(&Query::default()).iter().filter_map(|r| serde_json::to_value(r).ok()).collect();
    let read = tokio::task::spawn_blocking(move || {
        let prices = summary::prices_of(&st.registry, &st.accounts);
        summary::totals_with(&home, &w, &prices, Some(st.generation), &live, true)
    })
    .await;
    match read {
        Ok(totals) => {
            json!({"ok": true, "totals": totals, "window": {"from": w.from.map(nullrouter_engine::clock::rfc3339), "to": nullrouter_engine::clock::rfc3339(w.to)}})
        }
        Err(e) => json!({"ok": false, "error": format!("the read failed: {e}")}),
    }
}

/// `latency.summary`: as `usage.totals`, on the blocking pool with the live ring merged over disk.
async fn latency_summary(engine: &Arc<Engine>, req: &Value) -> Value {
    let w = match window_of(req) {
        Ok(w) => w,
        Err(e) => return e,
    };
    let home = engine.home().path().to_owned();
    let live: Vec<Value> =
        engine.records.query(&Query::default()).iter().filter_map(|r| serde_json::to_value(r).ok()).collect();
    let read = tokio::task::spawn_blocking(move || summary::latency(&home, &w, &live, true)).await;
    match read {
        Ok(latency) => json!({"ok": true, "latency": latency}),
        Err(e) => json!({"ok": false, "error": format!("the read failed: {e}")}),
    }
}

/// `records.list`: what the journal holds plus what is still in flight (the live ring is the
/// fresher copy of a record it has). Reading the segments happens off the executor.
async fn records_list(engine: &Arc<Engine>, req: &Value) -> Value {
    let str_of = |k: &str| req.get(k).and_then(Value::as_str).map(str::to_owned);
    let limit = req.get("limit").and_then(Value::as_u64).map(|n| n as usize);
    let filter = records::Filter {
        provider: str_of("provider"),
        unified_model: str_of("unified_model"),
        account: str_of("account"),
        agent: str_of("agent"),
        model: str_of("model"),
        reason: str_of("reason"),
        since: str_of("since").and_then(|s| nullrouter_engine::clock::parse_rfc3339(&s)),
        limit,
        before: str_of("before"),
        test: req.get("test").and_then(Value::as_bool),
    };
    let home = engine.home().path().to_owned();
    if let Some(id) = &filter.before {
        let (h, wanted) = (home.clone(), id.clone());
        let named = tokio::task::spawn_blocking(move || records::cursor_exists(&h, &wanted)).await.unwrap_or(false);
        // A request still in flight is a record too, and only the server knows it.
        let live = engine.records.query(&Query::default()).iter().any(|r| &r.id == id);
        if !named && !live {
            return json!({"ok": false, "error": format!("no record {id}")});
        }
    }
    let read = filter.clone();
    let mut disk = tokio::task::spawn_blocking(move || records::read(&home, &read)).await.unwrap_or_default();
    let live: Vec<Value> = engine
        .records
        .query(&Query::default())
        .iter()
        .filter_map(|r| serde_json::to_value(r).ok())
        .filter(|r| filter.matches(r))
        .collect();
    let ids: std::collections::BTreeSet<String> =
        live.iter().filter_map(|r| r["id"].as_str().map(str::to_owned)).collect();
    disk.retain(|r| r["id"].as_str().is_none_or(|id| !ids.contains(id)));
    disk.extend(live);
    disk.iter_mut().for_each(|r| nullrouter_engine::phases::decorate(r, None));
    // A request in flight shows the phase it is in now (FR-013).
    for r in disk.iter_mut().filter(|r| r["outcome"] == "in_progress") {
        if let Some((phase, ms)) = r["id"].as_str().and_then(|id| engine.live.current(id)) {
            r["slowest"] = json!({
                "phase": phase.name(),
                "ms": ms,
                "side": nullrouter_engine::phases::side(phase),
                "in_progress": true,
            });
        }
    }
    disk.sort_by(|a, b| b["id"].as_str().cmp(&a["id"].as_str()));
    disk.truncate(limit.unwrap_or(usize::MAX));
    json!({"ok": true, "records": disk})
}

/// The journal's health, as the routing view and `check` show it.
fn journal_health(engine: &Engine) -> Value {
    let h = engine.journal.health();
    let time = |t: std::time::SystemTime| nullrouter_engine::clock::rfc3339(t);
    json!({
        "kept": h.kept,
        "since": h.since.map(time),
        "unkept_requests": h.unkept_requests,
        "held_lines": h.held_lines,
        "last_sync": h.last_sync.map(time),
        "last_sync_age_s": h.last_sync.and_then(|t| t.elapsed().ok()).map(|d| d.as_secs_f64()),
    })
}

/// The fit's warnings for the routing view (spec 012, FR-016, FR-027, FR-028): a save that
/// failed per provider, a break per relearning number, and each account's unacknowledged usage
/// alerts. An account serving several targets is named once.
fn fit_warnings(engine: &Engine, st: &EngineState, targets: &[TargetView]) -> Vec<String> {
    let mut out: Vec<String> =
        st.registry.providers().filter_map(|p| engine.fit_learner.save_warning(&p.id)).collect();
    let mut seen: BTreeSet<(&str, &str)> = BTreeSet::new();
    for a in targets.iter().flat_map(|t| &t.accounts) {
        if !seen.insert((a.provider.as_str(), a.account.as_str())) {
            continue;
        }
        let who = format!("{}/{}", a.provider, a.account);
        for w in &a.meter {
            for n in w.numbers.iter().filter(|n| n.state == "relearning") {
                if let Some(since) = n.since {
                    let when = weekday_time(since);
                    let text = format!("{who}: provider rules changed around {when} on {} {}, relearning", w.window, n.number);
                    out.push(text);
                }
            }
        }
        let alerts = a.outside_use.alerts.len();
        if alerts > 0 {
            out.push(format!("{who}: {alerts} unacknowledged usage alerts (nullrouter quota alerts)"));
        }
    }
    out
}

/// `Tue 14:00`, in the system's time zone; the RFC 3339 text when it does not parse.
fn weekday_time(t: std::time::SystemTime) -> String {
    let rfc = nullrouter_engine::clock::rfc3339(t);
    match rfc.parse::<jiff::Timestamp>() {
        Ok(ts) => ts.to_zoned(jiff::tz::TimeZone::system()).strftime("%a %H:%M").to_string(),
        Err(_) => rfc,
    }
}

/// `routing.view`: every target's accounts as the next cold decision sees them.
fn routing_view(engine: &Engine, target: Option<&str>) -> Value {
    let st = engine.snapshot();
    let now = nullrouter_engine::clock::now();
    let targets = nullrouter_engine::route::view_all(engine, &st, target, now);
    let mut warnings: Vec<String> = targets.iter().flat_map(nullrouter_engine::routing::view::warnings).collect();
    warnings.extend(fit_warnings(engine, &st, &targets));
    warnings.dedup();
    let health = journal_health(engine);
    if health["kept"] == false {
        warnings.push(format!(
            "records not kept since {}: {} requests",
            health["since"].as_str().unwrap_or("?"),
            health["unkept_requests"]
        ));
    }
    let length = st.settings().routing.amortization;
    let default = nullrouter_engine::routing::AmortizationWindow {
        start: nullrouter_engine::routing::ledger::window_start(now, length),
        length,
    };
    json!({
        "ok": true,
        "now": nullrouter_engine::clock::rfc3339(now),
        "amortization": default,
        "journal": health,
        "targets": targets,
        "warnings": warnings,
    })
}

/// Why [`call`] got no answer.
#[derive(Debug, thiserror::Error)]
pub enum CallError {
    #[error("no server is running on {0}")]
    NoServer(String),
    #[error("operator socket: {0}")]
    Io(#[from] std::io::Error),
    #[error("operator socket: bad answer: {0}")]
    BadAnswer(String),
}

/// Sends one request to the running server and waits for its answer (blocking; for the
/// CLI).
pub fn call(home: &OperatorHome, req: &Value) -> Result<Value, CallError> {
    let path = socket_path(home);
    let mut stream =
        std::os::unix::net::UnixStream::connect(&path).map_err(|_| CallError::NoServer(path.display().to_string()))?;
    stream.set_read_timeout(Some(Duration::from_secs(60)))?;
    let mut line = req.to_string();
    line.push('\n');
    stream.write_all(line.as_bytes())?;
    let mut answer = String::new();
    BufReader::new(stream).read_line(&mut answer)?;
    serde_json::from_str(&answer).map_err(|e| CallError::BadAnswer(e.to_string()))
}

/// [`call`] for a streamed answer: `event` is called with each line that carries an `event`,
/// and the closing line is returned. No read timeout: a test may run for many minutes, and
/// dropping the connection (the CLI exiting) cancels what is not yet sent.
pub fn call_stream(home: &OperatorHome, req: &Value, mut event: impl FnMut(&Value)) -> Result<Value, CallError> {
    let path = socket_path(home);
    let mut stream =
        std::os::unix::net::UnixStream::connect(&path).map_err(|_| CallError::NoServer(path.display().to_string()))?;
    let mut line = req.to_string();
    line.push('\n');
    stream.write_all(line.as_bytes())?;
    for line in BufReader::new(stream).lines() {
        let v: Value = serde_json::from_str(&line?).map_err(|e| CallError::BadAnswer(e.to_string()))?;
        if v.get("event").is_some() {
            event(&v);
        } else {
            return Ok(v);
        }
    }
    Err(CallError::BadAnswer("the server closed the connection".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn answers_over_the_socket_and_replaces_a_stale_one() {
        let dir = tempfile::tempdir().unwrap();
        let home = OperatorHome::new(dir.path());
        let (engine, _) = Engine::open(home.clone()).unwrap();
        let engine = Arc::new(engine);
        engine.history.tally.attempt("p", "a", "m", None);
        // A stale file from a server that died.
        std::fs::create_dir_all(dir.path().join("run")).unwrap();
        std::fs::write(socket_path(&home), "").unwrap();
        let listener = bind(&home).unwrap();
        let mode = std::fs::metadata(socket_path(&home)).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert!(bind(&home).is_err(), "a live socket is not taken over");
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let task = tokio::spawn(serve(engine.clone(), listener, async move {
            let _ = stopped.await;
        }));
        let h = home.clone();
        let answers = tokio::task::spawn_blocking(move || {
            [
                json!({"op": "reload"}),
                json!({"op": "records.get", "id": "rq_x"}),
                json!({"op": "nope"}),
                json!({"op": "quota.checkpoint"}),
                json!({"op": "records.list", "before": "rq_nobody"}),
            ]
            .iter()
            .map(|r| call(&h, r).unwrap())
            .collect::<Vec<_>>()
        })
        .await
        .unwrap();
        assert_eq!(answers[0], json!({"ok": true, "generation": 2}));
        assert_eq!(answers[1]["ok"], false);
        assert_eq!(answers[2]["ok"], false);
        // The running tally reaches its checkpoint file on request.
        assert_eq!(answers[3], json!({"ok": true, "written": 1}));
        assert_eq!(answers[4], json!({"ok": false, "error": "no record rq_nobody"}));
        let cp = nullrouter_engine::quota::history::tally_file(dir.path(), "p", "a").unwrap();
        assert!(std::fs::read_to_string(cp).unwrap().contains("\"requests\":1"));
        stop.send(()).unwrap();
        task.await.unwrap();
        assert!(!socket_path(&home).exists());
        assert!(matches!(call(&home, &json!({"op": "reload"})), Err(CallError::NoServer(_))));
    }
}
