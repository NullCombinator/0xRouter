//! `nullrouter dashboard token` and `dashboard status` through the binary (spec 009 US1,
//! contracts/cli.md): the token is shown once and only its digest is kept; `status` has a line for
//! each state, and `check` carries the bind-failure warning. A `serve` here is a real process.

use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use nullrouter_engine::files::DashboardToken;
use serde_json::Value;

struct Out {
    code: i32,
    stdout: String,
    stderr: String,
}

fn nr(home: &Path, args: &[&str]) -> Out {
    let o = Command::new(env!("CARGO_BIN_EXE_nullrouter"))
        .arg("--home")
        .arg(home)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    Out {
        code: o.status.code().unwrap_or(-1),
        stdout: String::from_utf8(o.stdout).unwrap(),
        stderr: String::from_utf8(o.stderr).unwrap(),
    }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn config(home: &Path, dashboard: &str) {
    std::fs::write(home.join("config.toml"), format!("schema = 1\n\n[dashboard]\n{dashboard}")).unwrap();
}

struct Serving(Child);

impl Drop for Serving {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A running `serve` on `home`; the operator socket exists only once the dashboard's state is set.
fn serve(home: &Path) -> Serving {
    let port = free_port();
    let child = Command::new(env!("CARGO_BIN_EXE_nullrouter"))
        .arg("--home")
        .arg(home)
        .args(["serve", "--listen", &format!("127.0.0.1:{port}")])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let serving = Serving(child);
    for _ in 0..500 {
        if home.join("run/operator.sock").exists() && std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return serving;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("the server did not start");
}

fn is_token(s: &str) -> bool {
    s.strip_prefix("nrd_").is_some_and(|rest| {
        rest.len() == 43 && rest.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    })
}

#[test]
fn token_is_shown_once_and_only_its_digest_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let out = nr(dir.path(), &["dashboard", "token"]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    let token = out.stdout.strip_suffix('\n').unwrap();
    assert!(is_token(token), "{token:?}");
    assert!(out.stderr.contains("This is shown once. Open http://127.0.0.1:20130 and enter it."), "{}", out.stderr);
    assert!(out.stderr.contains("Browsers signed in with the previous token must enter this one."), "{}", out.stderr);
    assert!(out.stderr.trim_end().ends_with("saved; applies at next start"), "{}", out.stderr);
    assert!(!out.stderr.contains(token), "stderr never repeats it");

    let file = dir.path().join("dashboard.toml");
    assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(!text.contains(token), "the token is never stored");
    let stored = DashboardToken::load(dir.path()).unwrap();
    assert_eq!(stored.digest, Some(DashboardToken::digest_of(token)));
    assert_eq!(text.lines().filter(|l| !l.is_empty()).count(), 3, "schema, digest and time only: {text}");

    // Issuing again replaces it.
    let again = nr(dir.path(), &["dashboard", "token"]);
    let second = again.stdout.trim_end();
    assert!(is_token(second) && second != token);
    assert_eq!(DashboardToken::load(dir.path()).unwrap().digest, Some(DashboardToken::digest_of(second)));

    // Nothing else prints it: not `status`, in text or JSON.
    for args in [&["dashboard", "status"][..], &["--json", "dashboard", "status"]] {
        let o = nr(dir.path(), args);
        let digest = DashboardToken::digest_of(second);
        assert!(!o.stdout.contains(second) && !o.stdout.contains(&digest), "{}", o.stdout);
    }
}

#[test]
fn token_json_shape() {
    let dir = tempfile::tempdir().unwrap();
    let out = nr(dir.path(), &["--json", "dashboard", "token"]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    let v: Value = serde_json::from_str(&out.stdout).unwrap();
    let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["issued", "status", "token", "url"]);
    assert!(is_token(v["token"].as_str().unwrap()));
    assert_eq!(v["url"], "http://127.0.0.1:20130");
    assert_eq!(v["status"], "saved; applies at next start");
    assert_eq!(DashboardToken::load(dir.path()).unwrap().issued.as_deref(), v["issued"].as_str());
}

#[test]
fn token_still_issues_when_the_dashboard_is_off_and_says_so() {
    let dir = tempfile::tempdir().unwrap();
    config(dir.path(), "enabled = false\n");
    let out = nr(dir.path(), &["dashboard", "token"]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(is_token(out.stdout.trim_end()));
    assert!(out.stderr.contains("The dashboard is off (config.toml [dashboard] enabled = false)."), "{}", out.stderr);
}

#[test]
fn a_config_that_does_not_load_stops_token_before_it_writes() {
    let dir = tempfile::tempdir().unwrap();
    config(dir.path(), "listen = \"0.0.0.0:20130\"\n");
    let out = nr(dir.path(), &["dashboard", "token"]);
    assert_ne!(out.code, 0);
    assert!(out.stderr.contains("must be a loopback address"), "{}", out.stderr);
    assert!(!dir.path().join("dashboard.toml").exists());
}

#[test]
fn status_with_no_server() {
    let dir = tempfile::tempdir().unwrap();
    let out = nr(dir.path(), &["dashboard", "status"]);
    assert_eq!(out.code, 0);
    assert_eq!(
        out.stdout,
        "dashboard: no server running; config.toml: on, 127.0.0.1:20130\ntoken: none; run nullrouter dashboard token\n"
    );
    let v: Value = serde_json::from_str(&nr(dir.path(), &["--json", "dashboard", "status"]).stdout).unwrap();
    assert_eq!((v["server"].as_str(), &v["serving"], &v["token_issued"]), (Some("none"), &Value::Null, &Value::Null));

    nr(dir.path(), &["dashboard", "token"]);
    let out = nr(dir.path(), &["dashboard", "status"]);
    assert_eq!(out.code, 0);
    let second = out.stdout.lines().nth(1).unwrap();
    assert!(second.starts_with("token: issued 20") && second.ends_with('Z'), "{second}");

    config(dir.path(), "enabled = false\n");
    let out = nr(dir.path(), &["dashboard", "status"]);
    assert_eq!(out.code, 0);
    assert!(out.stdout.starts_with("dashboard: no server running; config.toml: off\n"), "{}", out.stdout);
}

#[test]
fn status_while_serving_and_token_applies_to_the_running_server() {
    let dir = tempfile::tempdir().unwrap();
    let port = free_port();
    config(dir.path(), &format!("listen = \"127.0.0.1:{port}\"\n"));
    let _serving = serve(dir.path());

    let out = nr(dir.path(), &["dashboard", "status"]);
    assert_eq!(out.code, 0);
    assert_eq!(
        out.stdout,
        format!("dashboard: on, listening on 127.0.0.1:{port}\ntoken: none; run nullrouter dashboard token\n")
    );
    let v: Value = serde_json::from_str(&nr(dir.path(), &["--json", "dashboard", "status"]).stdout).unwrap();
    assert_eq!(v["server"], "running");
    assert_eq!((&v["serving"], &v["error"]), (&Value::Bool(true), &Value::Null));
    assert!(std::net::TcpStream::connect(("127.0.0.1", port)).is_ok(), "the dashboard port is open");

    let issued = nr(dir.path(), &["dashboard", "token"]);
    assert_eq!(issued.code, 0, "{}", issued.stderr);
    assert!(issued.stderr.contains(&format!("Open http://127.0.0.1:{port} and enter it.")), "{}", issued.stderr);
    assert!(issued.stderr.trim_end().ends_with("applied"), "{}", issued.stderr);
    let out = nr(dir.path(), &["dashboard", "status"]);
    assert!(out.stdout.lines().nth(1).unwrap().starts_with("token: issued 20"), "{}", out.stdout);
}

#[test]
fn status_and_check_when_the_port_is_taken() {
    let dir = tempfile::tempdir().unwrap();
    let taken = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = taken.local_addr().unwrap().port();
    config(dir.path(), &format!("listen = \"127.0.0.1:{port}\"\n"));
    let _serving = serve(dir.path());

    let out = nr(dir.path(), &["dashboard", "status"]);
    assert_eq!(out.code, 0, "a report, not a check");
    let first = out.stdout.lines().next().unwrap();
    assert!(first.starts_with(&format!("dashboard: on, not listening: 127.0.0.1:{port}: ")), "{first}");
    assert_eq!(out.stdout.lines().nth(1), Some("token: none; run nullrouter dashboard token"));
    let v: Value = serde_json::from_str(&nr(dir.path(), &["--json", "dashboard", "status"]).stdout).unwrap();
    assert_eq!(v["serving"], false);
    assert!(v["error"].as_str().unwrap().starts_with(&format!("127.0.0.1:{port}: ")), "{v}");

    let check = nr(dir.path(), &["check"]);
    let line = check.stdout.lines().find(|l| l.contains("dashboard not listening")).expect("a warning");
    assert!(line.starts_with(&format!("warning: dashboard not listening: 127.0.0.1:{port}: ")), "{line}");
    assert_eq!(check.code, 0, "a warning is not a check failure: {}", check.stdout);
    let v: Value = serde_json::from_str(&nr(dir.path(), &["--json", "check"]).stdout).unwrap();
    let notice = v["notices"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["text"].as_str().unwrap().contains("dashboard not listening"))
        .unwrap();
    assert_eq!((notice["level"].as_str(), notice["subject"].as_str()), (Some("warning"), Some("settings")));

    // Whoever holds the port isn't the dashboard: the token output doesn't send the operator there.
    let issued = nr(dir.path(), &["dashboard", "token"]);
    assert!(is_token(issued.stdout.trim_end()), "{}", issued.stderr);
    let warned = format!("The dashboard is not listening (127.0.0.1:{port}: ");
    assert!(issued.stderr.contains(&warned), "{}", issued.stderr);
    assert!(!issued.stderr.contains("Open http"), "{}", issued.stderr);
}

#[test]
fn off_means_the_port_is_closed() {
    let dir = tempfile::tempdir().unwrap();
    let port = free_port();
    config(dir.path(), &format!("enabled = false\nlisten = \"127.0.0.1:{port}\"\n"));
    let _serving = serve(dir.path());

    let out = nr(dir.path(), &["dashboard", "status"]);
    assert_eq!(out.code, 0);
    assert!(out.stdout.starts_with("dashboard: off (config.toml [dashboard] enabled = false)\n"), "{}", out.stdout);
    assert!(std::net::TcpStream::connect(("127.0.0.1", port)).is_err(), "nothing listens on the dashboard port");
    let check = nr(dir.path(), &["check"]);
    assert!(!check.stdout.contains("dashboard not listening"), "{}", check.stdout);
}
