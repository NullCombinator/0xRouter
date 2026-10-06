//! The byte-for-byte gate for spec 008 (research R4): every listed CLI read, with and without
//! `--json`, on each fixture home with the server stopped and running, compared with the files
//! under `tests/golden/`. `NR_BLESS=1` writes them instead. The clock is pinned with
//! `NULLROUTER_TEST_NOW`; the home's path is printed as `<HOME>`. Nothing else is masked.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::Duration;

use nullrouter_engine::testkit::homes::{self, Fixture, NOW};

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

/// The reads, as argument lists after `--home`.
fn reads() -> Vec<Vec<String>> {
    let r = |n| homes::record_id(n);
    let mut v: Vec<Vec<String>> = [
        "accounts list",
        "accounts list --long",
        "quota",
        "quota xai",
        "quota xai work",
        "quota history xai work",
        "quota history grok-cli work --limit 1",
        "routing",
        "routing mixed",
        "records list",
        "records list --provider xai",
        "records list --account anthropic/main",
        "records list --agent ak_fixture1",
        "records list --model sonnet",
        "records list --reason warm",
        "records list --since 2026-10-03",
        "records list --since 2026-10-02T10:00:00Z",
        "records list --limit 2",
        "records list --agent ak_fixture1 --reason warm",
        "providers",
        "providers --capability tts",
        "model xai grok-4",
        "model nope grok-4",
        "plugins list",
        "plugins list --community",
        "keys list",
        "check",
        "resolve grok-cli/grok-build",
        "resolve mixed",
        "resolve lost",
        "resolve nope",
    ]
    .iter()
    .map(|c| c.split(' ').map(str::to_owned).collect())
    .collect();
    for n in [1, 2, 5, 6] {
        v.push(vec!["records".into(), "show".into(), r(n)]);
    }
    v.push(vec!["records".into(), "show".into(), "rq_missing".into()]);
    // Paging back (spec 008 US2).
    for extra in [["--limit", "2"].as_slice(), ["--agent", "ak_fixture1"].as_slice(), [].as_slice()] {
        let mut a = vec!["records".to_owned(), "list".into(), "--before".into(), r(5)];
        a.extend(extra.iter().map(|s| (*s).to_owned()));
        v.push(a);
    }
    v.push(vec!["records".into(), "list".into(), "--before".into(), "rq_missing".into()]);
    v
}

fn slug(args: &[String]) -> String {
    args.join("_").replace(['/', ':'], "-")
}

fn run(home: &Path, args: &[String], json: bool) -> Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_nullrouter"));
    c.arg("--home").arg(home);
    if json {
        c.arg("--json");
    }
    c.args(args).env("NULLROUTER_TEST_NOW", NOW).env_remove("NULLROUTER_HOME").stdin(Stdio::null()).output().unwrap()
}

fn render(o: &Output, home: &Path) -> String {
    let h = home.display().to_string();
    let clean = |b: &[u8]| String::from_utf8_lossy(b).replace(&h, "<HOME>");
    format!(
        "exit: {}\n--- stdout\n{}--- stderr\n{}",
        o.status.code().map_or("signal".to_owned(), |c| c.to_string()),
        clean(&o.stdout),
        clean(&o.stderr)
    )
}

struct Serving(Child);

impl Drop for Serving {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn serve(home: &Path) -> Serving {
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let child = Command::new(env!("CARGO_BIN_EXE_nullrouter"))
        .arg("--home")
        .arg(home)
        .args(["serve", "--listen", &format!("127.0.0.1:{port}")])
        .env("NULLROUTER_TEST_NOW", NOW)
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

fn check(fixture: &Fixture, state: &str, home: &Path) {
    let bless = std::env::var_os("NR_BLESS").is_some();
    let mut failures = Vec::new();
    for args in reads() {
        for json in [false, true] {
            let file = golden_dir().join(fixture.name).join(state).join(format!(
                "{}{}.txt",
                slug(&args),
                if json { ".json" } else { "" }
            ));
            let got = render(&run(home, &args, json), home);
            if bless {
                std::fs::create_dir_all(file.parent().unwrap()).unwrap();
                std::fs::write(&file, &got).unwrap();
            } else {
                match std::fs::read_to_string(&file) {
                    Ok(want) if want == got => {}
                    Ok(want) => failures.push(format!("{}\n--- want\n{want}\n--- got\n{got}", file.display())),
                    Err(_) => failures.push(format!("{}: no golden", file.display())),
                }
            }
        }
    }
    assert!(failures.is_empty(), "{} reads differ:\n{}", failures.len(), failures.join("\n=====\n"));
}

#[test]
fn every_read_matches_its_golden() {
    for fixture in homes::ALL {
        let dir = (fixture.build)();
        if fixture.name == "full" {
            homes::unfinished(dir.path());
        }
        check(fixture, "stopped", dir.path());
        if fixture.can_serve {
            let dir = (fixture.build)();
            let _serving = serve(dir.path());
            if fixture.name == "full" {
                homes::unfinished(dir.path());
            }
            check(fixture, "running", dir.path());
        }
    }
}
