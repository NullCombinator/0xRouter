//! Edits to `config.toml` as text: one key in one table, kept in place, and the checked write
//! that refuses a file the server would not load. The CLI and the dashboard share these.

use std::fmt::Write as _;

use nullrouter_registry::OperatorHome;
use nullrouter_registry::schema::OperatorConfig;

/// `text` with `key` set to `value` (or removed) in the table `[header]`; every other line is
/// kept as written. The flag is whether anything changed.
pub fn edit(text: &str, header: &str, key: &str, value: Option<&str>) -> (String, bool) {
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    let is_table = |l: &str| l.trim_start().starts_with('[');
    let is_key = |l: &str| l.split_once('=').is_some_and(|(k, _)| k.trim() == key);
    let start = lines.iter().position(|l| l.trim() == format!("[{header}]"));
    let join = |lines: Vec<String>| {
        let mut out = lines.join("\n");
        out.push('\n');
        out
    };
    let Some(start) = start else {
        let Some(v) = value else { return (text.to_owned(), false) };
        let mut out = text.to_owned();
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        if !out.is_empty() {
            out.push('\n');
        }
        let _ = write!(out, "[{header}]\n{key} = {v}\n");
        return (out, true);
    };
    let end = lines[start + 1..].iter().position(|l| is_table(l)).map_or(lines.len(), |i| start + 1 + i);
    let at = (start + 1..end).find(|&i| is_key(&lines[i]));
    match (at, value) {
        (Some(i), Some(v)) => lines[i] = format!("{key} = {v}"),
        (None, Some(v)) => {
            // After the table's last line that isn't blank.
            let after = (start + 1..end).rev().find(|&i| !lines[i].trim().is_empty()).map_or(start + 1, |i| i + 1);
            lines.insert(after, format!("{key} = {v}"));
        }
        (None, None) => return (text.to_owned(), false),
        (Some(i), None) => {
            lines.remove(i);
            let end = end - 1;
            // A table with nothing left goes too, with the blank line that followed it.
            if (start + 1..end).all(|j| lines[j].trim().is_empty()) {
                let upto = if lines.get(end).is_some_and(|l| l.trim().is_empty()) { end + 1 } else { end };
                lines.drain(start..upto.min(lines.len()));
            }
        }
    }
    (join(lines), true)
}

/// `text` with the top-level `key` set to `value` (or removed); every other line is kept as
/// written. A new key goes ahead of the first table. The flag is whether anything changed.
pub fn edit_top(text: &str, key: &str, value: Option<&str>) -> (String, bool) {
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    let first_table = lines.iter().position(|l| l.trim_start().starts_with('[')).unwrap_or(lines.len());
    let at = (0..first_table).find(|&i| lines[i].split_once('=').is_some_and(|(k, _)| k.trim() == key));
    match (at, value) {
        (Some(i), Some(v)) => {
            let line = format!("{key} = {v}");
            if lines[i] == line {
                return (text.to_owned(), false);
            }
            lines[i] = line;
        }
        (Some(i), None) => {
            lines.remove(i);
        }
        (None, Some(v)) => {
            // After the last non-blank line before the first table.
            let after = (0..first_table).rev().find(|&i| !lines[i].trim().is_empty()).map_or(0, |i| i + 1);
            lines.insert(after, format!("{key} = {v}"));
            if after + 1 == first_table + 1 && first_table < lines.len() - 1 {
                lines.insert(after + 1, String::new());
            }
        }
        (None, None) => return (text.to_owned(), false),
    }
    let mut out = lines.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    (out, true)
}

/// A TOML basic-string body: backslashes and quotes escaped.
pub fn quoted(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// The loader's own checks on `edited` as the home's `config.toml`, with nothing written: a file
/// that fails them would not load, and the next start or reload would refuse it. The message is the
/// loader's, one line per failed rule, after the file and `the edit would not load`. A failure the home already has without the edit (a broken
/// plugin, say) is not the edit's: only rules the edit newly breaks refuse it, so a change still
/// saves, and the reload says why it was not applied.
pub fn check(home: &OperatorHome, edited: &str) -> Result<(), String> {
    let Err(errors) = nullrouter_registry::check_config_text(home, edited) else { return Ok(()) };
    let line = |e: &nullrouter_registry::ValidationError| format!("{}: {}", e.path, e.rule);
    let before: Vec<String> = match std::fs::read_to_string(home.config_file()) {
        Ok(current) => nullrouter_registry::check_config_text(home, &current).err().unwrap_or_default(),
        Err(_) => nullrouter_registry::check_config_text(home, "").err().unwrap_or_default(),
    }
    .iter()
    .map(line)
    .collect();
    let new: Vec<String> = errors.iter().map(line).filter(|l| !before.contains(l)).collect();
    if new.is_empty() {
        Ok(())
    } else {
        Err(format!("{}: the edit would not load: {}", home.config_file().display(), new.join("\n")))
    }
}

/// Writes `edited` as `config.toml` after the loader accepts it ([`check`]). A file the server
/// would refuse is never written; the write goes through a temp file and a rename.
pub fn write_checked(home: &OperatorHome, edited: &str) -> Result<(), String> {
    check(home, edited)?;
    write_file(home, edited)
}

/// [`write_checked`] for a caller that already ran [`check`]: only the parse is repeated.
pub fn write_file(home: &OperatorHome, edited: &str) -> Result<(), String> {
    let path = home.config_file();
    toml::from_str::<OperatorConfig>(edited)
        .map_err(|e| format!("{}: the edit would not load: {e}", path.display()))?;
    std::fs::create_dir_all(home.path()).map_err(|e| e.to_string())?;
    write_atomic(&path, edited.as_bytes()).map_err(|e| format!("{}: {e}", path.display()))
}

static WRITERS: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Holds the `config.toml` writers' lock: a process-wide mutex plus an advisory exclusive lock on
/// the `config.toml.lock` sibling (std `File::lock`; no locking crate), so the dashboard's ops and
/// a CLI process take turns. Hold it across read, edit and write. Not reentrant: do not call
/// [`write_file`] and take it again in the same call chain from inside the held section.
pub struct ConfigLock {
    _file: Option<std::fs::File>,
    _guard: std::sync::MutexGuard<'static, ()>,
}

/// Takes the writers' lock for the home's `config.toml`.
pub fn lock(home: &OperatorHome) -> ConfigLock {
    let guard = WRITERS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let _ = std::fs::create_dir_all(home.path());
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(home.config_file().with_extension("toml.lock"))
        .ok()
        .filter(|f| f.lock().is_ok());
    ConfigLock { _file: file, _guard: guard }
}

/// Writes `bytes` to `path` through a unique temp file in the same directory (`create_new`, 0600,
/// `sync_all`), then renames it over `path`. A mode the file already had is kept.
pub fn write_atomic(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let name = path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let tmp = path.with_file_name(format!(
        ".{name}.{}.{}.tmp",
        std::process::id(),
        N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let mode = std::fs::metadata(path).map_or(0o600, |m| m.permissions().mode() & 0o777);
    let result = (|| {
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp)?;
        f.write_all(bytes)?;
        f.set_permissions(std::fs::Permissions::from_mode(mode))?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_go_through_unique_temp_files_keep_the_mode_and_leave_nothing_behind() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let home = OperatorHome::new(dir.path());
        std::fs::write(home.config_file(), "schema = 1\n").unwrap();
        std::fs::set_permissions(home.config_file(), std::fs::Permissions::from_mode(0o600)).unwrap();
        std::thread::scope(|s| {
            for _ in 0..8 {
                s.spawn(|| {
                    let _g = lock(&home);
                    write_file(&home, "schema = 1\n").unwrap();
                });
            }
        });
        let mode = std::fs::metadata(home.config_file()).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let stray: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".tmp"))
            .collect();
        assert!(stray.is_empty(), "{stray:?}");
    }

    #[test]
    fn the_loader_refuses_a_target_that_names_nothing_and_nothing_is_written() {
        let dir = tempfile::tempdir().unwrap();
        let home = OperatorHome::new(dir.path());
        let bad = "schema = 1\n[routing.amortization_for]\n\"sonnet\" = \"2h\"\n";
        let why = write_checked(&home, bad).unwrap_err();
        assert!(why.contains("sonnet"), "{why}");
        assert!(!home.config_file().exists(), "nothing written");
        write_checked(&home, "schema = 1\n").unwrap();
        assert!(home.config_file().exists());
    }

    const H: &str = "provider.acme.connection";

    #[test]
    fn a_setting_is_added_replaced_and_given_a_table() {
        assert_eq!(
            edit("", H, "header_timeout_ms", Some("10000")),
            ("[provider.acme.connection]\nheader_timeout_ms = 10000\n".into(), true)
        );
        let with =
            "schema = 1\n[provider.acme.connection]\n# note\nheader_timeout_ms = 5\n\n[server]\nlisten = \"x\"\n";
        assert_eq!(
            edit(with, H, "header_timeout_ms", Some("9")).0,
            "schema = 1\n[provider.acme.connection]\n# note\nheader_timeout_ms = 9\n\n[server]\nlisten = \"x\"\n"
        );
        // A new key goes after the table's last line, ahead of the blank line before the next.
        assert_eq!(
            edit(with, H, "stall_timeout_ms", Some("7")).0,
            "schema = 1\n[provider.acme.connection]\n# note\nheader_timeout_ms = 5\nstall_timeout_ms = 7\n\n[server]\nlisten = \"x\"\n"
        );
        assert_eq!(
            edit("schema = 1", H, "stall_timeout_ms", Some("7")).0,
            "schema = 1\n\n[provider.acme.connection]\nstall_timeout_ms = 7\n"
        );
    }

    #[test]
    fn a_top_level_key_is_set_in_place_or_ahead_of_the_first_table() {
        assert_eq!(edit_top("", "a", Some("true")), ("a = true\n".into(), true));
        assert_eq!(edit_top("schema = 1\n", "a", Some("true")).0, "schema = 1\na = true\n");
        assert_eq!(edit_top("a = false\n[t]\na = 1\n", "a", Some("true")).0, "a = true\n[t]\na = 1\n");
        assert_eq!(edit_top("[t]\na = 1\n", "a", Some("true")).0, "a = true\n\n[t]\na = 1\n");
        assert_eq!(edit_top("a = true\n[t]\na = 1\n", "a", None).0, "[t]\na = 1\n");
        assert_eq!(edit_top("a = true\n", "a", Some("true")), ("a = true\n".into(), false));
        assert_eq!(edit_top("[t]\n", "a", None), ("[t]\n".into(), false));
    }

    #[test]
    fn unsetting_the_last_key_removes_the_table_and_other_tables_stay() {
        let text = "schema = 1\n\n[provider.acme.connection]\nheader_timeout_ms = 5\n\n[server]\nlisten = \"x\"\n";
        assert_eq!(edit(text, H, "header_timeout_ms", None), ("schema = 1\n\n[server]\nlisten = \"x\"\n".into(), true));
        let two = "[provider.acme.connection]\nheader_timeout_ms = 5\nstall_timeout_ms = 7\n";
        assert_eq!(edit(two, H, "header_timeout_ms", None).0, "[provider.acme.connection]\nstall_timeout_ms = 7\n");
        assert_eq!(edit(two, H, "connect_timeout_ms", None), (two.into(), false));
        assert_eq!(edit("", H, "connect_timeout_ms", None), (String::new(), false));
    }
}
