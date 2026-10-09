//! `nullrouter model <provider> [<model>]`: exit 2 when the provider is unknown. Without a model,
//! every model the provider declares, each as `model <provider> <model>` shows it.

use std::path::PathBuf;
use std::process::ExitCode;

use nullrouter_registry::OperatorHome;
use nullrouter_server::views;
use serde_json::json;

pub(crate) fn run(
    home: Option<PathBuf>,
    provider: &str,
    model: Option<&str>,
    as_json: bool,
) -> Result<ExitCode, ExitCode> {
    let home = home.map_or_else(OperatorHome::resolve, OperatorHome::new);
    let args = json!({"provider": provider, "model": model});
    let out = super::read(&home, views::model::NEEDS, &args, views::model::build)?.json;
    if out["kind"] == "not_found" {
        if as_json {
            println!("{out:#}");
        } else {
            eprintln!("not found: {}", out["error"].as_str().unwrap_or_default());
        }
        return Ok(ExitCode::from(2));
    }
    if as_json {
        println!("{out:#}");
    } else if let Some(list) = out.as_array() {
        // One block per model, a blank line between.
        for (i, m) in list.iter().enumerate() {
            if i > 0 {
                println!();
            }
            print_fields(m);
        }
    } else {
        print_fields(&out);
    }
    Ok(ExitCode::SUCCESS)
}

fn print_fields(model: &serde_json::Value) {
    for (k, v) in model.as_object().into_iter().flatten() {
        println!("{k}: {v}");
    }
}
