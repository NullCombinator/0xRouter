use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use nullrouter_registry::validate_user_plugin;

pub(crate) fn run(files: &[PathBuf]) -> Result<ExitCode, ExitCode> {
    let mut ok = true;
    for path in files {
        let result = fs::read_to_string(path).map_err(|e| vec![format!("{}: {e}", path.display())]).and_then(|src| {
            validate_user_plugin(&src, path).map_err(|errs| errs.iter().map(ToString::to_string).collect())
        });
        match result {
            Ok(_) => println!("OK {}", path.display()),
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
