use std::path::PathBuf;
use std::process::ExitCode;

use serde_json::{Value, json};
use zerorouter_registry::{Resolution, UnifiedMember};

fn member(m: &UnifiedMember) -> Value {
    json!({ "provider": m.provider, "requested": m.requested, "upstream_id": m.upstream_id, "catalogued": m.catalogued })
}

/// `--json` shape (stable; the quickstart parses it):
/// `{"kind":"direct","provider","requested","upstream_id","catalogued"}`,
/// `{"kind":"unified","name","model_kind","members":[{"provider","requested","upstream_id","catalogued"}]}`,
/// or `{"kind":"not_found","error"}`.
pub(crate) fn run(home: Option<PathBuf>, target: &str, as_json: bool) -> Result<ExitCode, ExitCode> {
    let reg = crate::open(home)?.snapshot();
    let (out, code) = match reg.resolve(target) {
        Ok(Resolution::Direct { provider, requested, upstream_id, catalogued }) => (
            json!({ "kind": "direct", "provider": provider.id, "requested": requested,
                    "upstream_id": upstream_id, "catalogued": catalogued }),
            0,
        ),
        Ok(Resolution::Unified(u)) => (
            json!({ "kind": "unified", "name": u.name, "model_kind": u.kind.map(|k| k.as_str()),
                    "members": u.members.iter().map(member).collect::<Vec<_>>() }),
            0,
        ),
        Err(e) => (json!({ "kind": "not_found", "error": e.to_string() }), 2),
    };
    if as_json {
        println!("{out:#}");
    } else {
        match out["kind"].as_str() {
            Some("direct") => println!(
                "direct {} → {} (upstream {}{})",
                target,
                out["provider"].as_str().unwrap_or_default(),
                out["upstream_id"].as_str().unwrap_or_default(),
                if out["catalogued"] == true { "" } else { ", uncatalogued" }
            ),
            Some("unified") => {
                println!("unified {target}:");
                for (i, m) in out["members"].as_array().into_iter().flatten().enumerate() {
                    println!(
                        "  {i}. {} {} → upstream {}",
                        m["provider"].as_str().unwrap_or_default(),
                        m["requested"].as_str().unwrap_or_default(),
                        m["upstream_id"].as_str().unwrap_or_default()
                    );
                }
            }
            _ => eprintln!("not found: {}", out["error"].as_str().unwrap_or_default()),
        }
    }
    Ok(ExitCode::from(code))
}
