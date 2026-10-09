//! `nullrouter live` (spec 013, contracts/cli.md): the requests in flight, from the running
//! server's `live.snapshot`. It redraws once a second on a terminal and prints one snapshot per
//! second otherwise; `--json` prints one snapshot and stops. Ctrl-C ends it.

use std::fmt::Write as _;
use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use nullrouter_registry::OperatorHome;
use nullrouter_server::operator::{self, CallError};
use serde_json::{Value, json};

fn fail(msg: impl std::fmt::Display) -> ExitCode {
    eprintln!("{msg}");
    ExitCode::from(1)
}

pub(crate) fn run(home: Option<PathBuf>, as_json: bool) -> Result<ExitCode, ExitCode> {
    let home = home.map_or_else(OperatorHome::resolve, OperatorHome::new);
    let terminal = std::io::stdout().is_terminal();
    loop {
        let snap = snapshot(&home)?;
        if as_json {
            println!("{}", json!({"as_of": snap["as_of"], "paused_proxies": snap["paused_proxies"], "in_flight": snap["in_flight"]}));
            return Ok(ExitCode::SUCCESS);
        }
        if terminal {
            print!("\x1b[2J\x1b[H");
        }
        println!("{}", render(&snap));
        std::thread::sleep(Duration::from_secs(1));
    }
}

fn snapshot(home: &OperatorHome) -> Result<Value, ExitCode> {
    match operator::call(home, &json!({"op": "live.snapshot"})) {
        Ok(a) if a["ok"] == true => Ok(a),
        Ok(a) => Err(fail(a["error"].as_str().unwrap_or("the server refused the request"))),
        Err(CallError::NoServer(_)) => Err(fail("no server is running")),
        Err(e) => Err(fail(e)),
    }
}

fn text(v: &Value) -> String {
    v.as_str().unwrap_or("-").chars().filter(|c| !c.is_control()).collect()
}

fn span(ms: f64) -> String {
    if ms < 1000.0 { format!("{ms:.0} ms") } else { format!("{:.1} s", ms / 1000.0) }
}

fn name(phase: &str) -> String {
    match phase {
        "router_overhead" => "overhead".into(),
        p => p.replace('_', " "),
    }
}

/// `#1 failed in headers · overhead 4 ms · headers 6.0 s`, one entry per ended attempt.
fn finished(list: &Value) -> String {
    let mut parts = Vec::new();
    for a in list.as_array().into_iter().flatten() {
        let mut one = format!("#{}", a["n"]);
        if let Some(p) = a["ended_in"].as_str() {
            let _ = write!(one, " failed in {}", name(p));
        }
        for (phase, ms) in a["phases"].as_object().into_iter().flatten() {
            let _ = write!(one, " · {} {}", name(phase), span(ms.as_f64().unwrap_or(0.0)));
        }
        parts.push(one);
    }
    parts.join("  ")
}

/// The time of day in an RFC 3339 stamp.
fn clock(v: &Value) -> String {
    v.as_str().and_then(|t| t.get(11..19)).unwrap_or("-").to_owned()
}

fn render(snap: &Value) -> String {
    let flight = snap["in_flight"].as_array().map_or(&[][..], Vec::as_slice);
    let mut out = String::new();
    if flight.is_empty() {
        out.push_str("nothing in flight");
    } else {
        let _ = write!(out, "in flight {} · {}  (Ctrl-C to quit)", flight.len(), clock(&snap["as_of"]));
    }
    for p in snap["paused_proxies"].as_array().into_iter().flatten() {
        let _ = write!(out, "\npaused proxies: {} (unreachable since {})", text(&p["name"]), clock(&p["since"]));
    }
    if flight.is_empty() {
        return out;
    }
    let mut rows = vec![["AGENT", "TARGET", "ATTEMPT", "PHASE", "SINCE ARRIVAL", "FINISHED"].map(String::from)];
    for r in flight {
        let a = &r["attempt"];
        let (attempt, phase) = if a.is_null() {
            ("—".to_owned(), format!("router overhead {}", span(r["since_arrival_ms"].as_f64().unwrap_or(0.0))))
        } else {
            let who = match a["account"].is_string() {
                true => format!("{}/{}", text(&a["provider"]), text(&a["account"])),
                false => text(&a["provider"]),
            };
            (
                format!("{} {who}", a["n"]),
                format!("{} {}", name(a["phase"].as_str().unwrap_or("-")), span(a["in_phase_ms"].as_f64().unwrap_or(0.0))),
            )
        };
        rows.push([
            text(&r["agent"]),
            text(&r["target"]),
            attempt,
            phase,
            span(r["since_arrival_ms"].as_f64().unwrap_or(0.0)),
            finished(&r["finished"]),
        ]);
    }
    let widths: Vec<usize> = (0..5).map(|i| rows.iter().map(|r| r[i].chars().count()).max().unwrap_or(0)).collect();
    out.push('\n');
    for r in &rows {
        let mut line = String::new();
        for (i, &w) in widths.iter().enumerate() {
            let _ = write!(line, "{:<w$}  ", r[i]);
        }
        line.push_str(&r[5]);
        let _ = write!(out, "\n{}", line.trim_end());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_in_flight_says_so_and_still_shows_a_paused_proxy() {
        let snap = json!({"as_of": "2026-10-07T14:02:07Z", "paused_proxies": [], "in_flight": []});
        assert_eq!(render(&snap), "nothing in flight");
        let snap = json!({"as_of": "2026-10-07T14:02:07Z", "in_flight": [],
            "paused_proxies": [{"name": "eu-exit", "since": "2026-10-07T13:58:12Z", "reason": "x"}]});
        assert_eq!(render(&snap), "nothing in flight\npaused proxies: eu-exit (unreachable since 13:58:12)");
    }

    #[test]
    fn a_request_shows_its_phase_and_what_its_earlier_attempts_took() {
        let snap = json!({"as_of": "2026-10-07T14:02:07Z", "paused_proxies": [], "in_flight": [
            {"id": "rq_1", "agent": "ci", "target": "gpt-5", "since_arrival_ms": 19000.0,
             "attempt": {"n": 2, "provider": "openrouter", "account": "main", "model": "m",
                         "phase": "generation", "in_phase_ms": 11400.0, "proxy": null},
             "finished": [{"n": 1, "ended_in": "headers", "phases": {"router_overhead": 4.0, "headers": 6000.0}}]},
            {"id": "rq_2", "agent": "bob", "target": "opus", "since_arrival_ms": 1.0, "attempt": null, "finished": []}]});
        let out = render(&snap);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "in flight 2 · 14:02:07  (Ctrl-C to quit)");
        assert!(lines[2].starts_with("AGENT") && lines[2].contains("SINCE ARRIVAL"), "{out}");
        assert!(lines[3].contains("2 openrouter/main") && lines[3].contains("generation 11.4 s"), "{out}");
        assert!(lines[3].ends_with("#1 failed in headers · overhead 4 ms · headers 6.0 s"), "{out}");
        assert!(lines[4].contains('—') && lines[4].contains("router overhead 1 ms"), "{out}");
    }

    #[test]
    fn control_characters_in_a_target_never_reach_the_terminal() {
        let snap = json!({"as_of": "2026-10-07T14:02:07Z", "paused_proxies": [], "in_flight": [
            {"id": "rq_1", "agent": "a\u{1b}[31m", "target": "t", "since_arrival_ms": 1.0, "attempt": null, "finished": []}]});
        assert!(!render(&snap).contains('\u{1b}'));
    }

    #[test]
    fn without_a_server_it_says_so_and_exits_1() {
        let dir = tempfile::tempdir().unwrap();
        let got = run(Some(dir.path().to_owned()), true);
        assert_eq!(format!("{:?}", got.err()), format!("{:?}", Some(ExitCode::from(1))));
    }
}
