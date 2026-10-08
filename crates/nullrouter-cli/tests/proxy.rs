//! `nullrouter proxy` (spec 013, US4) through the binary: the password comes from stdin or a
//! variable, never argv; a proxy in use can't be removed; assignments reach `config.toml` and
//! `accounts.toml`; `fixed` probes.

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use nullrouter_engine::accounts::{self, Accounts};

const PASSWORD: &str = "pw-CLI-PROXY-SENTINEL-0009";

fn nr(home: &Path, args: &[&str], stdin: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_nullrouter"))
        .arg("--home")
        .arg(home)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let _ = child.stdin.take().unwrap().write_all(stdin.as_bytes());
    child.wait_with_output().unwrap()
}

fn text(o: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))
}

fn ok(home: &Path, args: &[&str], stdin: &str) -> String {
    let o = nr(home, args, stdin);
    assert!(o.status.success(), "{args:?}: {}", text(&o));
    text(&o)
}

fn refused(home: &Path, args: &[&str]) -> String {
    let o = nr(home, args, "");
    assert!(!o.status.success(), "{args:?} should have been refused: {}", text(&o));
    text(&o)
}

#[test]
fn a_proxy_is_defined_assigned_listed_and_removed_without_a_password_on_screen() {
    let dir = tempfile::tempdir().unwrap();
    let h = dir.path();
    ok(h, &["accounts", "add", "anthropic", "main"], "sk-cli-SENTINEL-0010\n");

    let said = ok(h, &["proxy", "add", "eu", "http://127.0.0.1:3128", "--username", "u"], &format!("{PASSWORD}\n"));
    assert!(!said.contains(PASSWORD));
    let file = h.join("proxies.toml");
    assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
    assert!(std::fs::read_to_string(&file).unwrap().contains(PASSWORD), "the file holds it, as accounts.toml does");

    ok(h, &["proxy", "add", "us", "socks5://127.0.0.1:1080", "--username", "v", "--password-env", "NR_TEST_UNSET"], "");
    assert!(std::fs::read_to_string(&file).unwrap().contains("NR_TEST_UNSET"));

    let again = refused(h, &["proxy", "add", "eu", "http://127.0.0.1:1"]);
    assert!(again.contains("eu"), "{again}");
    let creds = refused(h, &["proxy", "add", "bad", "http://u:p@127.0.0.1:1"]);
    assert!(!creds.contains("p@"), "a refused url is not echoed with its credentials: {creds}");

    ok(h, &["proxy", "use", "eu", "--all"], "");
    ok(h, &["proxy", "use", "us", "--provider", "anthropic"], "");
    ok(h, &["proxy", "use", "none", "--account", "anthropic/main"], "");
    assert_eq!(Accounts::load(&h.join(accounts::FILE)).unwrap().get("anthropic", "main").unwrap().proxy.as_deref(), Some("none"));
    let config = std::fs::read_to_string(h.join("config.toml")).unwrap();
    assert!(config.contains("[connection]") && config.contains("proxy = \"eu\""), "{config}");
    assert!(config.contains("[provider.anthropic.connection]") && config.contains("proxy = \"us\""), "{config}");

    let rows: serde_json::Value = serde_json::from_str(&ok(h, &["--json", "proxy", "list"], "")).unwrap();
    assert_eq!(rows[0]["name"], "eu");
    assert_eq!(rows[0]["user"], true);
    assert_eq!(rows[0]["used_by"][0], "all providers");
    assert_eq!(rows[1]["used_by"][0], "provider anthropic");
    let table = ok(h, &["proxy", "list"], "");
    assert!(table.contains("user ✓") && !table.contains(PASSWORD) && !table.contains("127.0.0.1:3128@"), "{table}");

    let in_use = refused(h, &["proxy", "remove", "us"]);
    assert!(in_use.contains("provider anthropic"), "names the assignment: {in_use}");

    ok(h, &["proxy", "clear", "--provider", "anthropic"], "");
    ok(h, &["proxy", "remove", "us"], "");
    assert!(!std::fs::read_to_string(&file).unwrap().contains("\"us\""));
}

#[test]
fn unknown_names_are_refused_with_what_is_known() {
    let dir = tempfile::tempdir().unwrap();
    let h = dir.path();
    ok(h, &["accounts", "add", "anthropic", "main"], "sk-cli-SENTINEL-0011\n");
    ok(h, &["proxy", "add", "eu", "http://127.0.0.1:3128"], "");

    assert!(refused(h, &["proxy", "use", "gone", "--all"]).contains("known proxies: eu"));
    assert!(refused(h, &["proxy", "use", "eu", "--provider", "nope"]).contains("known providers:"));
    assert!(refused(h, &["proxy", "use", "eu", "--account", "anthropic/other"]).contains("anthropic/main"));
    assert!(refused(h, &["proxy", "remove", "gone"]).contains("known proxies: eu"));
    assert!(!h.join("config.toml").exists() || !std::fs::read_to_string(h.join("config.toml")).unwrap().contains("gone"));
}

#[test]
fn fixed_probes_the_proxy_and_resumes_only_when_it_answers() {
    let dir = tempfile::tempdir().unwrap();
    let h = dir.path();
    let up = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let down = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap()
    };
    ok(h, &["proxy", "add", "up", &format!("http://{}", up.local_addr().unwrap())], "");
    ok(h, &["proxy", "add", "down", &format!("http://{down}")], "");

    let o = nr(h, &["proxy", "fixed", "down"], "");
    assert!(!o.status.success());
    assert!(text(&o).contains("down still unreachable"), "{}", text(&o));
    let o = nr(h, &["proxy", "fixed", "up"], "");
    assert!(o.status.success(), "{}", text(&o));
    assert!(text(&o).contains("up reachable; traffic resumed"));
    assert!(refused(h, &["proxy", "fixed", "nope"]).contains("known proxies"));
}

#[test]
fn accounts_list_and_check_show_the_proxy_a_pause_and_a_name_nobody_defined() {
    let dir = tempfile::tempdir().unwrap();
    let h = dir.path();
    ok(h, &["accounts", "add", "anthropic", "main"], "sk-cli-SENTINEL-0011\n");
    ok(h, &["proxy", "add", "eu", "http://127.0.0.1:3128"], "");
    ok(h, &["proxy", "use", "eu", "--account", "anthropic/main"], "");

    let list = ok(h, &["accounts", "list"], "");
    assert!(list.contains("proxy") && list.contains("eu (account)"), "{list}");

    // A pause (written as the engine writes it) is an error with the fix named.
    let routing = h.join("routing");
    std::fs::create_dir_all(&routing).unwrap();
    std::fs::set_permissions(&routing, std::fs::Permissions::from_mode(0o700)).unwrap();
    nullrouter_engine::files::write_private(
        &routing.join("proxies.json"),
        r#"{"paused":{"eu":{"since":"2026-10-08T10:00:00Z","reason":"connect to proxy failed"}}}"#,
    )
    .unwrap();
    let o = nr(h, &["check"], "");
    let said = text(&o);
    assert!(!o.status.success(), "{said}");
    assert!(said.contains("error: proxy eu is paused (unreachable since 2026-10-08T10:00:00Z)"), "{said}");
    assert!(said.contains("nullrouter proxy fixed eu"), "{said}");

    // An assignment naming a proxy nobody defined is a note, not an error.
    let mut list = Accounts::load(&h.join(accounts::FILE)).unwrap();
    list.set_proxy("anthropic", "main", Some("gone".into())).unwrap();
    list.save().unwrap();
    let said = text(&nr(h, &["check"], ""));
    assert!(said.contains("note: account anthropic/main is assigned proxy \"gone\", which isn't defined"), "{said}");
}

#[test]
fn a_password_from_the_environment_is_not_printed_by_any_command() {
    const ENV_PASSWORD: &str = "env-pw-SENTINEL-0023";
    const USER: &str = "env-user-SENTINEL-0024";
    let dir = tempfile::tempdir().unwrap();
    let h = dir.path();
    ok(h, &["accounts", "add", "anthropic", "main"], "sk-cli-SENTINEL-0012\n");
    let run = |args: &[&str]| {
        let o = Command::new(env!("CARGO_BIN_EXE_nullrouter"))
            .arg("--home")
            .arg(h)
            .args(args)
            .env("NR_PROXY_PW", ENV_PASSWORD)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        text(&o)
    };
    let mut said = vec![run(&["proxy", "add", "eu", "http://127.0.0.1:3128", "--username", USER, "--password-env", "NR_PROXY_PW"])];
    assert!(std::fs::read_to_string(h.join("proxies.toml")).unwrap().contains("NR_PROXY_PW"));
    assert!(!std::fs::read_to_string(h.join("proxies.toml")).unwrap().contains(ENV_PASSWORD), "only the variable's name is kept");
    said.push(run(&["proxy", "use", "eu", "--all"]));
    for args in [
        &["proxy", "list"][..],
        &["--json", "proxy", "list"],
        &["accounts", "list"],
        &["--json", "accounts", "list"],
        &["check"],
        &["--json", "check"],
        &["connection", "show"],
        &["proxy", "fixed", "eu"],
    ] {
        said.push(run(args));
    }
    for (i, out) in said.iter().enumerate() {
        assert!(!out.contains(ENV_PASSWORD), "command {i} printed the environment password: {out}");
        assert!(!out.contains(USER), "command {i} printed the username: {out}");
    }
}

#[test]
fn connection_set_takes_reuse_and_http2_as_switches_and_show_names_them() {
    let dir = tempfile::tempdir().unwrap();
    let h = dir.path();
    ok(h, &["accounts", "add", "anthropic", "main"], "sk-cli-SENTINEL-0013\n");

    ok(h, &["connection", "set", "anthropic", "http2", "off"], "");
    ok(h, &["connection", "set", "anthropic", "reuse", "off"], "");
    let config = std::fs::read_to_string(h.join("config.toml")).unwrap();
    assert!(config.contains("http2 = false") && config.contains("reuse = false"), "{config}");
    let shown = ok(h, &["connection", "show", "anthropic"], "");
    assert!(shown.contains("http2") && shown.contains("reuse"), "{shown}");

    let bad = refused(h, &["connection", "set", "anthropic", "http2", "maybe"]);
    assert!(bad.contains("on") && bad.contains("off"), "{bad}");
    ok(h, &["connection", "unset", "anthropic", "http2"], "");
    assert!(!std::fs::read_to_string(h.join("config.toml")).unwrap().contains("http2"));
}
