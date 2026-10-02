//! The community set (research R19): the providers outside the chosen five, embedded in the
//! binary (no network) and installed on request. Install runs the gate and the fit check,
//! then copies the file to `$NULLROUTER_HOME/plugins/`. Fit is re-checked on every load.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::{fmt, fs, io};

use crate::fit::FitVerdict;
use crate::load::{self, OperatorHome};
use crate::validate::ValidationError;

include!(concat!(env!("OUT_DIR"), "/community_plugins.rs"));

/// One embedded community plugin and its verdict against this core.
#[derive(Debug, Clone)]
pub struct CommunityPlugin {
    /// The file stem: the provider id the generator wrote.
    pub id: &'static str,
    pub src: &'static str,
    /// The gate's errors, or the fit verdict, with private endpoints allowed: whether the
    /// core can run it. `install` checks again under the operator's setting.
    pub verdict: Result<FitVerdict, Vec<ValidationError>>,
}

impl CommunityPlugin {
    pub fn file(&self) -> String {
        format!("plugins/community/{}.toml", self.id)
    }
}

/// Every community plugin, by id. Verdicts are computed once.
pub fn community() -> &'static [CommunityPlugin] {
    static SET: OnceLock<Vec<CommunityPlugin>> = OnceLock::new();
    SET.get_or_init(|| {
        COMMUNITY
            .iter()
            .map(|(name, src)| {
                let id = name.strip_suffix(".toml").unwrap_or(name);
                let file = format!("plugins/community/{name}");
                let verdict = load::check_user_plugin(src, Path::new(&file), true);
                CommunityPlugin { id, src, verdict }
            })
            .collect()
    })
}

/// Why an install or uninstall didn't happen.
#[derive(Debug)]
pub enum InstallError {
    UnknownId(String),
    Invalid(Vec<ValidationError>),
    /// The refusal message, listing every unsupported part.
    Unsupported(String),
    AlreadyInstalled(PathBuf),
    NotInstalled(PathBuf),
    Io(PathBuf, io::Error),
}

impl fmt::Display for InstallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownId(id) => {
                write!(f, "no community plugin {id:?}; `nullrouter plugins list --community` lists them")
            }
            Self::Invalid(errors) => {
                let lines: Vec<String> = errors.iter().map(ToString::to_string).collect();
                write!(f, "{}", lines.join("\n"))
            }
            Self::Unsupported(message) => f.write_str(message),
            Self::AlreadyInstalled(p) => write!(f, "{} already exists", p.display()),
            Self::NotInstalled(p) => write!(f, "{} is not installed", p.display()),
            Self::Io(p, e) => write!(f, "{}: {e}", p.display()),
        }
    }
}

impl std::error::Error for InstallError {}

fn installed_path(id: &str, home: &OperatorHome) -> PathBuf {
    home.plugins_dir().join(format!("{id}.toml"))
}

/// Gate + fit under the operator's `allow_private_endpoints`, then copy to `plugins/`.
pub fn install(id: &str, home: &OperatorHome) -> Result<PathBuf, InstallError> {
    let plugin = community().iter().find(|p| p.id == id).ok_or_else(|| InstallError::UnknownId(id.to_owned()))?;
    let dest = installed_path(id, home);
    if dest.exists() {
        return Err(InstallError::AlreadyInstalled(dest));
    }
    let allow_private = load::allow_private(home).map_err(InstallError::Invalid)?;
    let file = plugin.file();
    let verdict =
        load::check_user_plugin(plugin.src, Path::new(&file), allow_private).map_err(InstallError::Invalid)?;
    if let Some(message) = verdict.message(&file) {
        return Err(InstallError::Unsupported(message));
    }
    let dir = home.plugins_dir();
    fs::create_dir_all(&dir).map_err(|e| InstallError::Io(dir.clone(), e))?;
    // Atomic: a load never sees half a file. The temporary name isn't `*.toml`, so it's
    // never scanned.
    let tmp = dir.join(format!(".{id}.toml.tmp"));
    fs::write(&tmp, plugin.src).map_err(|e| InstallError::Io(tmp.clone(), e))?;
    fs::rename(&tmp, &dest).map_err(|e| InstallError::Io(dest.clone(), e))?;
    Ok(dest)
}

/// Removes an installed community plugin's file.
pub fn uninstall(id: &str, home: &OperatorHome) -> Result<PathBuf, InstallError> {
    if !community().iter().any(|p| p.id == id) {
        return Err(InstallError::UnknownId(id.to_owned()));
    }
    let path = installed_path(id, home);
    match fs::remove_file(&path) {
        Ok(()) => Ok(path),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Err(InstallError::NotInstalled(path)),
        Err(e) => Err(InstallError::Io(path, e)),
    }
}

/// Whether `id` is installed in `home`.
pub fn is_installed(id: &str, home: &OperatorHome) -> bool {
    installed_path(id, home).is_file()
}
