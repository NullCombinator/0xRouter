//! `nullrouter dashboard` (spec 009, contracts/cli.md): issue the dashboard token, and report the
//! dashboard's state from the `dashboard` view.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Subcommand;
use nullrouter_engine::files::DashboardToken;
use nullrouter_registry::OperatorHome;
use nullrouter_server::views;
use serde_json::{Value, json};

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Issue a dashboard token, replacing any previous one. It is printed once; only its digest
    /// is stored.
    Token,
    /// Whether the dashboard is on, where it listens, and when its token was issued.
    Status,
}

fn fail(e: impl std::fmt::Display) -> ExitCode {
    eprintln!("{e}");
    ExitCode::from(1)
}

fn state(home: &OperatorHome) -> Result<Value, ExitCode> {
    Ok(super::read(home, views::dashboard::NEEDS, &json!({}), views::dashboard::build)?.json)
}

pub(crate) fn run(home: Option<PathBuf>, cmd: Command, as_json: bool) -> Result<ExitCode, ExitCode> {
    let home = home.map_or_else(OperatorHome::resolve, OperatorHome::new);
    match cmd {
        Command::Token => token(&home, as_json),
        Command::Status => status(&home, as_json),
    }
}

/// Writes the new digest, tells a running server, then prints the token: the only time it exists.
/// Stdout carries just the token, as `keys issue` does; the rest goes to stderr. The state is
/// read first, so a `config.toml` that doesn't load stops the command before anything is written.
///
/// The token is printed even when the running server refuses the reload: it is saved, and is the
/// one that works from the next good reload on. The output then says the previous token still
/// works on the running server, and the command fails (security-review.md L1).
fn token(home: &OperatorHome, as_json: bool) -> Result<ExitCode, ExitCode> {
    let before = state(home)?;
    let (token, record) = DashboardToken::issue();
    record.save(home.path()).map_err(fail)?;
    let applied = super::apply(home);
    let status = match &applied {
        Ok(s) => (*s).to_owned(),
        Err(e) => format!("{e}; the previous token still works there until a reload succeeds"),
    };
    let url = format!("http://{}", before["listen"].as_str().unwrap_or_default());
    // A running server whose dashboard didn't bind: the address may be someone else's.
    let not_listening = (before["server"] == "running" && before["enabled"] == true && before["serving"] != true)
        .then(|| before["error"].as_str().unwrap_or_default().to_owned());
    if as_json {
        println!("{}", json!({"token": token, "issued": record.issued, "url": url, "status": status}));
    } else {
        println!("{token}");
        match &not_listening {
            None => eprintln!("This is shown once. Open {url} and enter it."),
            Some(error) => eprintln!("This is shown once. The dashboard is not listening ({error}); don't open {url}."),
        }
        eprintln!("Browsers signed in with the previous token must enter this one.");
        if before["enabled"] == false {
            eprintln!("The dashboard is off (config.toml [dashboard] enabled = false).");
        }
        eprintln!("{status}");
    }
    Ok(if applied.is_ok() { ExitCode::SUCCESS } else { ExitCode::from(1) })
}

/// The first line of `status` (contracts/cli.md's table).
fn headline(v: &Value) -> String {
    let (listen, enabled) = (v["listen"].as_str().unwrap_or_default(), v["enabled"] == true);
    match (v["server"].as_str(), enabled) {
        (Some("running"), false) => "dashboard: off (config.toml [dashboard] enabled = false)".to_owned(),
        (Some("running"), true) if v["serving"] == true => format!("dashboard: on, listening on {listen}"),
        (Some("running"), true) => {
            format!("dashboard: on, not listening: {}", v["error"].as_str().unwrap_or(listen))
        }
        (_, true) => format!("dashboard: no server running; config.toml: on, {listen}"),
        (_, false) => "dashboard: no server running; config.toml: off".to_owned(),
    }
}

fn status(home: &OperatorHome, as_json: bool) -> Result<ExitCode, ExitCode> {
    let v = state(home)?;
    if as_json {
        println!("{v:#}");
    } else {
        println!("{}", headline(&v));
        match v["token_issued"].as_str() {
            Some(t) => println!("token: issued {t}"),
            None => println!("token: none; run nullrouter dashboard token"),
        }
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(server: &str, enabled: bool, serving: Value, error: Value) -> Value {
        json!({"enabled": enabled, "listen": "127.0.0.1:20130", "server": server, "serving": serving, "error": error})
    }

    #[test]
    fn each_state_has_its_line() {
        let cases = [
            (v("running", true, json!(true), Value::Null), "dashboard: on, listening on 127.0.0.1:20130"),
            (
                v("running", true, json!(false), json!("127.0.0.1:20130: Address already in use (os error 98)")),
                "dashboard: on, not listening: 127.0.0.1:20130: Address already in use (os error 98)",
            ),
            (
                v("running", false, json!(false), Value::Null),
                "dashboard: off (config.toml [dashboard] enabled = false)",
            ),
            (
                v("none", true, Value::Null, Value::Null),
                "dashboard: no server running; config.toml: on, 127.0.0.1:20130",
            ),
            (v("none", false, Value::Null, Value::Null), "dashboard: no server running; config.toml: off"),
        ];
        for (view, line) in cases {
            assert_eq!(headline(&view), line);
        }
    }
}
