//! `unified [NAME]` through the binary (spec 008 US3, contracts/cli.md). The byte-for-byte output
//! of each form is also pinned by the golden suite.

use std::path::Path;
use std::process::{Command, Output};

use nullrouter_engine::testkit::homes;

fn nr(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_nullrouter"))
        .env("NULLROUTER_TEST_NOW", homes::NOW)
        .arg("--home")
        .arg(home)
        .args(args)
        .output()
        .unwrap()
}

fn out(o: &Output) -> String {
    String::from_utf8(o.stdout.clone()).unwrap()
}

fn err(o: &Output) -> String {
    String::from_utf8(o.stderr.clone()).unwrap()
}

#[test]
fn the_list_shows_each_model_its_members_and_what_was_dropped() {
    let home = homes::full();
    let o = nr(home.path(), &["unified"]);
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    let text = out(&o);
    assert!(text.contains("mixed"), "{text}");
    assert!(text.contains("  0. "), "{text}");
    assert!(text.contains("→ upstream "), "{text}");
    assert!(text.contains("dropped unified model lost: member provider "), "{text}");

    let j: serde_json::Value = serde_json::from_str(&out(&nr(home.path(), &["--json", "unified"]))).unwrap();
    assert!(j["unified"].as_array().unwrap().iter().any(|m| m["name"] == "mixed"));
    assert_eq!(j["dropped"][0]["name"], "lost");
    assert!(!j["unified"].as_array().unwrap().iter().any(|m| m["name"] == "lost"));
}

#[test]
fn one_model_is_the_object_resolve_prints_byte_for_byte() {
    let home = homes::full();
    let unified = nr(home.path(), &["--json", "unified", "mixed"]);
    let resolve = nr(home.path(), &["--json", "resolve", "mixed"]);
    assert_eq!(unified.status.code(), Some(0));
    assert_eq!(unified.stdout, resolve.stdout);
    // Each list entry is that same object.
    let j: serde_json::Value = serde_json::from_str(&out(&nr(home.path(), &["--json", "unified"]))).unwrap();
    let one: serde_json::Value = serde_json::from_str(&out(&unified)).unwrap();
    assert!(j["unified"].as_array().unwrap().contains(&one));
    // With NAME there are no dropped lines.
    assert!(!out(&nr(home.path(), &["unified", "mixed"])).contains("dropped"));
}

#[test]
fn a_name_that_is_not_loaded_exits_2_like_resolve() {
    let home = homes::full();
    for name in ["nope", "lost"] {
        let o = nr(home.path(), &["unified", name]);
        assert_eq!(o.status.code(), Some(2), "{name}");
        assert!(err(&o).starts_with("not found: "), "{}", err(&o));
        let (u, r) = (nr(home.path(), &["--json", "unified", name]), nr(home.path(), &["--json", "resolve", name]));
        assert_eq!(u.status.code(), Some(2));
        assert_eq!(u.stdout, r.stdout);
    }
}

#[test]
fn an_empty_home_says_how_to_declare_one() {
    let home = homes::empty();
    let o = nr(home.path(), &["unified"]);
    assert_eq!(o.status.code(), Some(0));
    assert_eq!(out(&o), "no unified models; declare one with [[unified_model]] in config.toml\n");
    let j = nr(home.path(), &["--json", "unified"]);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&out(&j)).unwrap(),
        serde_json::json!({"unified": [], "dropped": []})
    );
}

#[test]
fn a_home_that_does_not_load_prints_the_startup_errors() {
    let home = homes::broken();
    let o = nr(home.path(), &["unified"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(err(&o).starts_with("startup failed:\n"), "{}", err(&o));
    assert!(out(&o).is_empty());
}

#[test]
fn add_set_and_remove_edit_the_config_and_say_when_it_applies() {
    let home = homes::empty();
    let o = nr(home.path(), &["unified", "add", "u", "openrouter/x/a", "openrouter/x/b"]);
    // One provider cannot be listed twice, so the second member is refused and nothing is written.
    assert_eq!(o.status.code(), Some(1), "{}", err(&o));
    assert!(err(&o).contains("already a member"), "{}", err(&o));
    let o = nr(home.path(), &["unified", "add", "u", "openrouter/x/a"]);
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    assert_eq!(out(&o), "u: declared (1 member), saved; applies at next start\n");
    assert!(out(&nr(home.path(), &["unified", "u"])).contains("openrouter x/a"));
    let o = nr(home.path(), &["unified", "add", "u", "openrouter/x/a"]);
    assert_eq!((o.status.code(), err(&o).as_str()), (Some(1), "u: already declared; use set\n"));
    let o = nr(home.path(), &["unified", "set", "u", "openrouter/x/c"]);
    assert_eq!(out(&o), "u: updated (1 member), saved; applies at next start\n");
    assert!(out(&nr(home.path(), &["unified", "u"])).contains("openrouter x/c"));
    let o = nr(home.path(), &["unified", "set", "zz", "openrouter/x/c"]);
    assert_eq!(err(&o), "zz: not declared; use add\n");
    let o = nr(home.path(), &["unified", "remove", "u", "--yes"]);
    assert_eq!(out(&o), "u: removed, saved; applies at next start\n");
    assert_eq!(
        out(&nr(home.path(), &["unified"])),
        "no unified models; declare one with [[unified_model]] in config.toml\n"
    );
}

#[test]
fn a_model_a_combo_lists_cannot_be_removed() {
    let home = homes::empty();
    nr(home.path(), &["unified", "add", "u", "openrouter/x/a"]);
    nr(home.path(), &["combos", "add", "fast", "u"]);
    let o = nr(home.path(), &["unified", "remove", "u", "--yes"]);
    assert_eq!(
        (o.status.code(), err(&o).as_str()),
        (Some(1), "u: combo fast uses it; remove it from the combo first\n")
    );
    assert!(out(&nr(home.path(), &["unified", "u"])).starts_with("u"));
}

#[test]
fn remove_asks_first() {
    use std::io::Write;
    use std::process::Stdio;
    let home = homes::empty();
    nr(home.path(), &["unified", "add", "u", "openrouter/x/a"]);
    let mut child = Command::new(env!("CARGO_BIN_EXE_nullrouter"))
        .arg("--home")
        .arg(home.path())
        .args(["unified", "remove", "u"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"n\n").unwrap();
    let o = child.wait_with_output().unwrap();
    assert_eq!(o.status.code(), Some(1));
    assert!(err(&o).contains("unified model u is removed"), "{}", err(&o));
    assert!(out(&nr(home.path(), &["unified", "u"])).starts_with("u"));
}
