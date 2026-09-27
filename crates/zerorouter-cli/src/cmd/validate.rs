use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use zerorouter_registry::PluginSource;
use zerorouter_registry::validate::validate;

pub(crate) fn run(files: &[PathBuf]) -> Result<ExitCode, ExitCode> {
    let mut ok = true;
    for path in files {
        let file = path.display().to_string();
        let result = fs::read_to_string(path)
            .map_err(|e| vec![format!("{file}: {e}")])
            .and_then(|src| {
                validate(&src, PluginSource::User(path.clone()), &file)
                    .map_err(|errs| errs.iter().map(ToString::to_string).collect())
            });
        match result {
            Ok(_) => println!("OK {file}"),
            Err(errors) => {
                ok = false;
                for e in errors {
                    println!("{e}");
                }
            }
        }
    }
    Ok(ExitCode::from(u8::from(!ok)))
}
