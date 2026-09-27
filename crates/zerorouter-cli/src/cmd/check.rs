use std::path::PathBuf;
use std::process::ExitCode;

use serde_json::json;

pub(crate) fn run(home: Option<PathBuf>, as_json: bool) -> Result<ExitCode, ExitCode> {
    let handle = crate::open(home)?;
    let reg = handle.snapshot();
    let r = reg.report();

    if as_json {
        let out = json!({
            "home": handle.home().path(),
            "providers": { "bundled": r.bundled, "user": r.user },
            "unified_models": r.unified_models,
            "pending_conflicts": r.pending_conflicts.iter().map(|c| json!({ "id": c.id, "path": c.path })).collect::<Vec<_>>(),
            "declined": r.declined.iter().map(|c| json!({ "id": c.id, "path": c.path })).collect::<Vec<_>>(),
            "withheld_credentials": r.withheld_credentials.iter()
                .map(|w| json!({ "provider": w.provider, "offending_url": w.offending_url.as_str() })).collect::<Vec<_>>(),
            "skipped": r.skipped.iter().map(|s| json!({
                "path": s.path, "id": s.id, "errors": s.errors.iter().map(ToString::to_string).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
            "dropped_unified_models": r.dropped_unified_models.iter()
                .map(|d| json!({ "name": d.name, "provider": d.provider })).collect::<Vec<_>>(),
        });
        println!("{out:#}");
    } else {
        println!("home: {}", handle.home().path().display());
        println!("providers: {} ({} bundled, {} user)", r.bundled + r.user, r.bundled, r.user);
        println!("unified models: {}", r.unified_models);
        for c in &r.pending_conflicts {
            println!(
                "conflict pending: {} shadows bundled {}; bundled is active until plugin_decisions.{} is set",
                c.path.display(),
                c.id,
                c.id
            );
        }
        for c in &r.declined {
            println!("declined: {} (bundled {} stays active)", c.path.display(), c.id);
        }
        for w in &r.withheld_credentials {
            println!("credential WITHHELD: {w}");
        }
        for s in &r.skipped {
            println!("skipped: {}", s.path.display());
            for e in &s.errors {
                println!("  {e}");
            }
        }
        for d in &r.dropped_unified_models {
            println!("dropped unified model {}: member provider {} was skipped", d.name, d.provider);
        }
    }
    let errors = !r.skipped.is_empty() || !r.dropped_unified_models.is_empty();
    Ok(ExitCode::from(u8::from(errors)))
}
