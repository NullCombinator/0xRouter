//! `source_fp`: the same canonical form as `nullrouter-adapters::fingerprint` (every regular
//! file, sorted by path, as `path\0len\0bytes`). The builder keeps its own copy so it does not
//! depend on the core crates; `tests/build.rs` should check the two agree.

use std::fs;
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::BuildError;

/// `sha256:` and the hex digest of `files` (relative `/` paths, any order).
#[must_use]
pub fn source_fp_of_files(files: &[(String, Vec<u8>)]) -> String {
    let mut order: Vec<&(String, Vec<u8>)> = files.iter().collect();
    order.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    let mut hash = Sha256::new();
    for (path, bytes) in order {
        hash.update(path.as_bytes());
        hash.update([0]);
        hash.update(bytes.len().to_string().as_bytes());
        hash.update([0]);
        hash.update(bytes);
    }
    format!("sha256:{:x}", hash.finalize())
}

/// Reads every file under `root`, refusing anything that is not a regular file or directory.
pub(crate) fn read_tree(root: &Path) -> Result<Vec<(String, Vec<u8>)>, BuildError> {
    let mut out = Vec::new();
    walk(root, "", &mut out)?;
    Ok(out)
}

fn walk(dir: &Path, prefix: &str, out: &mut Vec<(String, Vec<u8>)>) -> Result<(), BuildError> {
    let entries = fs::read_dir(dir).map_err(|e| BuildError::io(dir.display().to_string(), e))?;
    for entry in entries {
        let entry = entry.map_err(|e| BuildError::io(dir.display().to_string(), e))?;
        let path = entry.path();
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| BuildError::Source(format!("{}: the name is not UTF-8", path.display())))?;
        let relative = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
        let kind = fs::symlink_metadata(&path).map_err(|e| BuildError::io(path.display().to_string(), e))?.file_type();
        if kind.is_dir() {
            walk(&path, &relative, out)?;
        } else if kind.is_file() {
            out.push((relative, fs::read(&path).map_err(|e| BuildError::io(path.display().to_string(), e))?));
        } else {
            return Err(BuildError::Source(format!("{}: not a regular file or directory", path.display())));
        }
    }
    Ok(())
}

/// The fingerprint of the tree under `root`.
pub fn source_fp(root: &Path) -> Result<String, BuildError> {
    Ok(source_fp_of_files(&read_tree(root)?))
}
