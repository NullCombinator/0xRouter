//! Reading an adapter package into memory, from a directory or a `.tar.gz` archive.
//!
//! The same rules serve a local install and the catalogue: only regular files and directories,
//! no symlinks, hard links, devices, absolute or `..` paths, no duplicates, at most
//! [`MAX_ENTRIES`] entries and [`MAX_BYTES`] bytes of content. Nothing is written anywhere here;
//! the caller gets `(relative path, bytes)` pairs, which is also what the fingerprint takes.

use std::collections::BTreeSet;
use std::fs;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

use flate2::read::GzDecoder;

/// The most entries (files and directories) a package may hold.
pub const MAX_ENTRIES: usize = 64;
/// The most bytes of file content a package may hold.
pub const MAX_BYTES: u64 = 256 * 1024;
/// The most the decompressed archive stream may be: content, plus headers and padding.
const MAX_STREAM: u64 = MAX_BYTES + (MAX_ENTRIES as u64 + 8) * 1024;

#[derive(Debug, thiserror::Error)]
pub enum UnpackError {
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("{0}: not a regular file or directory (links and devices are not accepted)")]
    NotRegular(String),
    #[error("{0}: the path is absolute, climbs out with .., or is not UTF-8")]
    BadPath(String),
    #[error("{0}: the path appears twice")]
    Duplicate(String),
    #[error("more than {MAX_ENTRIES} entries")]
    TooManyEntries,
    #[error("more than {} KiB of content", MAX_BYTES / 1024)]
    TooLarge,
    #[error("the archive can't be read: {0}")]
    Archive(String),
    #[error("the input is neither a directory nor a file")]
    NotAPackage,
}

/// The files of a package: relative path (with `/`) and bytes.
pub type Files = Vec<(String, Vec<u8>)>;

/// Reads `input` (a directory, or a `.tar.gz` file). A symlink as `input` is refused.
pub fn read_package(input: &Path) -> Result<Files, UnpackError> {
    let io = |source| UnpackError::Io { path: input.to_owned(), source };
    let meta = fs::symlink_metadata(input).map_err(io)?;
    if meta.is_dir() {
        read_dir_tree(input)
    } else if meta.is_file() {
        let file = fs::File::open(input).map_err(io)?;
        read_archive(file)
    } else {
        Err(UnpackError::NotAPackage)
    }
}

/// Reads every regular file under `root`. Links and special files are refused, not followed.
pub fn read_dir_tree(root: &Path) -> Result<Files, UnpackError> {
    let mut files = Vec::new();
    let mut state = Budget::default();
    walk(root, "", &mut files, &mut state)?;
    Ok(files)
}

fn walk(dir: &Path, prefix: &str, out: &mut Files, budget: &mut Budget) -> Result<(), UnpackError> {
    let io = |path: &Path, source| UnpackError::Io { path: path.to_owned(), source };
    for entry in fs::read_dir(dir).map_err(|e| io(dir, e))? {
        let entry = entry.map_err(|e| io(dir, e))?;
        let path = entry.path();
        let name = entry.file_name().into_string().map_err(|_| UnpackError::BadPath(path.display().to_string()))?;
        let relative = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
        budget.entry()?;
        let meta = fs::symlink_metadata(&path).map_err(|e| io(&path, e))?;
        if meta.is_dir() {
            walk(&path, &relative, out, budget)?;
        } else if meta.is_file() {
            budget.bytes(meta.len())?;
            // `take` again: the file may have grown since `metadata`.
            let mut bytes = Vec::new();
            fs::File::open(&path)
                .and_then(|f| f.take(MAX_BYTES + 1).read_to_end(&mut bytes))
                .map_err(|e| io(&path, e))?;
            if bytes.len() as u64 != meta.len() {
                return Err(UnpackError::TooLarge);
            }
            out.push((relative, bytes));
        } else {
            return Err(UnpackError::NotRegular(relative));
        }
    }
    Ok(())
}

/// Reads a `.tar.gz` stream. The layout may be at the archive's root or under one top-level
/// directory, which is stripped.
pub fn read_archive(source: impl Read) -> Result<Files, UnpackError> {
    let stream = GzDecoder::new(source).take(MAX_STREAM);
    let mut archive = tar::Archive::new(stream);
    let bad = |e: io::Error| UnpackError::Archive(e.to_string());
    let mut files: Files = Vec::new();
    let mut seen = BTreeSet::new();
    let mut budget = Budget::default();
    for entry in archive.entries().map_err(bad)? {
        let entry = entry.map_err(bad)?;
        let kind = entry.header().entry_type();
        // A pax global header (from `git archive`) carries no file.
        if kind.is_pax_global_extensions() {
            continue;
        }
        let shown = String::from_utf8_lossy(&entry.path_bytes()).into_owned();
        let Some(relative) = clean(&entry, &shown)? else { continue };
        budget.entry()?;
        if kind.is_dir() {
            continue;
        }
        if !kind.is_file() {
            return Err(UnpackError::NotRegular(shown));
        }
        if !seen.insert(relative.clone()) {
            return Err(UnpackError::Duplicate(relative));
        }
        let size = entry.header().size().map_err(bad)?;
        budget.bytes(size)?;
        let mut bytes = Vec::new();
        entry.take(size).read_to_end(&mut bytes).map_err(bad)?;
        files.push((relative, bytes));
    }
    Ok(strip_single_top(files))
}

/// The entry's path with `.` parts dropped, or `None` for the archive root itself.
fn clean<R: Read>(entry: &tar::Entry<'_, R>, shown: &str) -> Result<Option<String>, UnpackError> {
    let path = entry.path().map_err(|_| UnpackError::BadPath(shown.to_owned()))?;
    let mut parts = Vec::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::Normal(p) => parts.push(p.to_str().ok_or_else(|| UnpackError::BadPath(shown.to_owned()))?.to_owned()),
            _ => return Err(UnpackError::BadPath(shown.to_owned())),
        }
    }
    Ok((!parts.is_empty()).then(|| parts.join("/")))
}

/// Drops the one directory every path starts with, when no file sits at the root.
fn strip_single_top(files: Files) -> Files {
    let first = |p: &str| p.split('/').next().unwrap_or("").to_owned();
    let top = files.first().map(|(p, _)| first(p));
    let nested = files.iter().all(|(p, _)| p.contains('/'));
    match top {
        Some(top) if nested && files.iter().all(|(p, _)| first(p) == top) => {
            files.into_iter().map(|(p, b)| (p[top.len() + 1..].to_owned(), b)).collect()
        }
        _ => files,
    }
}

#[derive(Default)]
struct Budget {
    entries: usize,
    bytes: u64,
}

impl Budget {
    fn entry(&mut self) -> Result<(), UnpackError> {
        self.entries += 1;
        if self.entries > MAX_ENTRIES { Err(UnpackError::TooManyEntries) } else { Ok(()) }
    }

    fn bytes(&mut self, n: u64) -> Result<(), UnpackError> {
        self.bytes = self.bytes.saturating_add(n);
        if self.bytes > MAX_BYTES { Err(UnpackError::TooLarge) } else { Ok(()) }
    }
}

/// Writes `files` under `dest`, creating directories. The paths come from this module, so they
/// hold only normal components; one that doesn't is refused anyway.
pub fn write_tree(files: &Files, dest: &Path) -> Result<(), UnpackError> {
    for (relative, bytes) in files {
        let path = Path::new(relative);
        if relative.is_empty() || !path.components().all(|c| matches!(c, Component::Normal(_))) {
            return Err(UnpackError::BadPath(relative.clone()));
        }
        let target = dest.join(path);
        let io = |source| UnpackError::Io { path: target.clone(), source };
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(io)?;
        }
        fs::write(&target, bytes).map_err(io)?;
    }
    Ok(())
}
