//! Poll history: one JSON line per poll per account, and the tally checkpoint (research R15).
//!
//! `$NULLROUTER_HOME/quota/<provider>/<account>.jsonl` gets one [`Entry`] per completed poll,
//! good or failed, carrying the account's [tally](super::tally) since the previous entry. The
//! running tally is checkpointed to `<account>.tally.json` every [`CHECKPOINT_EVERY`] and at
//! shutdown, and reloaded at start, so a graceful restart loses nothing and a crash at most
//! one checkpoint interval. Files are 0600, directories 0700. Nothing is dropped
//! automatically (Clarifications Q4): [`prune`] and [`forget`] are the operator's.
//!
//! A checkpoint names the `at` of the newest entry it follows (`base`). Written right after
//! each entry, it is discarded at start when the history holds a newer entry: that entry
//! already carries the checkpoint's tokens (a crash between the two writes).
//!
//! Writers serialize on the history file's advisory lock, so a `prune` from the CLI and the
//! server's appends never interleave; an appender that waited re-opens the file a prune
//! replaced.
//!
//! ```text
//! {"v":1,"at":"2026-10-03T14:20:00.120Z","ok":true,"windows":[{"name":"5-hour","unit":"percent","used":38.0,"limit":100.0,"remaining":62.0,"resets_at":"2026-10-03T16:00:00.000Z"}],"tally":{"claude-sonnet-4-5":{"requests":12,"requests_usage_unreported":0,"input":5120,"output":2210,"cache_read":40960,"cache_write":1024}}}
//! ```

use std::collections::VecDeque;
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use super::QuotaWindow;
use super::poll::{PollHook, QuotaPoll};
use super::tally::{AccountTally, Tally};
use crate::clock;
use crate::files::{self, FileError};

/// The directory under the operator home.
pub const DIR: &str = "quota";
/// The entry format version.
pub const VERSION: u32 = 1;
/// How often the running tallies are checkpointed.
pub const CHECKPOINT_EVERY: Duration = Duration::from_secs(10);

/// A failed poll's class and reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntryError {
    pub class: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    pub reason: String,
}

/// One poll as kept (data-model § Quota poll).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub v: u32,
    /// RFC 3339 with milliseconds.
    pub at: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<EntryError>,
    #[serde(default)]
    pub windows: Vec<QuotaWindow>,
    /// The traffic since the previous entry, by upstream model.
    #[serde(default)]
    pub tally: AccountTally,
}

impl Entry {
    pub fn from_poll(poll: &QuotaPoll, tally: AccountTally) -> Self {
        let error = poll.error.as_ref().map(|e| EntryError {
            class: serde_json::to_value(e.class).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default(),
            status: e.status,
            reason: e.reason.clone(),
        });
        Self {
            v: VERSION,
            at: super::extract::rfc3339_millis(poll.at),
            ok: poll.ok(),
            error,
            windows: poll.windows.clone(),
            tally,
        }
    }

    pub fn time(&self) -> Option<SystemTime> {
        clock::parse_rfc3339(&self.at)
    }
}

/// The running tally as checkpointed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Checkpoint {
    v: u32,
    at: String,
    /// The `at` of the newest history entry when written.
    base: Option<String>,
    tally: AccountTally,
}

/// A name usable as one path component.
fn component<'a>(path: &Path, s: &'a str) -> Result<&'a str, FileError> {
    let ok = !s.is_empty() && !s.starts_with('.') && !s.contains(['/', '\\', '\0']);
    if ok { Ok(s) } else { Err(FileError::invalid(path, format!("{s:?} is not a provider or account name"))) }
}

/// `quota/<provider>/<account>.jsonl`.
pub fn history_file(home: &Path, provider: &str, account: &str) -> Result<PathBuf, FileError> {
    let dir = home.join(DIR);
    Ok(dir.join(component(&dir, provider)?).join(format!("{}.jsonl", component(&dir, account)?)))
}

/// `quota/<provider>/<account>.tally.json`.
pub fn tally_file(home: &Path, provider: &str, account: &str) -> Result<PathBuf, FileError> {
    let dir = home.join(DIR);
    Ok(dir.join(component(&dir, provider)?).join(format!("{}.tally.json", component(&dir, account)?)))
}

pub(crate) fn io_err(path: &Path) -> impl FnOnce(io::Error) -> FileError + '_ {
    move |source| FileError::Io { path: path.to_owned(), source }
}

/// Creates the file's directories, 0700.
pub(crate) fn private_dirs(path: &Path) -> Result<(), FileError> {
    match path.parent() {
        Some(dir) => DirBuilder::new().recursive(true).mode(0o700).create(dir).map_err(io_err(path)),
        None => Ok(()),
    }
}

/// The history file opened for appending and locked; re-opened when a prune replaced it
/// while this one waited for the lock.
pub(crate) fn open_append(path: &Path) -> Result<File, FileError> {
    private_dirs(path)?;
    loop {
        let f = OpenOptions::new().append(true).create(true).mode(0o600).open(path).map_err(io_err(path))?;
        f.lock().map_err(io_err(path))?;
        let same = fs::metadata(path).ok().map(|m| m.ino()) == Some(f.metadata().map_err(io_err(path))?.ino());
        if same {
            return Ok(f);
        }
    }
}

/// Appends `entry` to its account's history.
pub fn append(home: &Path, provider: &str, account: &str, entry: &Entry) -> Result<(), FileError> {
    let path = history_file(home, provider, account)?;
    let mut line = serde_json::to_string(entry).map_err(|e| FileError::invalid(&path, e.to_string()))?;
    line.push('\n');
    let mut f = open_append(&path)?;
    f.write_all(line.as_bytes()).and_then(|()| f.sync_data()).map_err(io_err(&path))
}

/// Calls `each` with the file's lines, newest first, until it returns `false`. Reads only
/// as much of the file's tail as needed.
pub(crate) fn lines_backwards(f: &mut File, mut each: impl FnMut(&[u8]) -> bool) -> io::Result<()> {
    const CHUNK: u64 = 8 * 1024;
    let mut pos = f.metadata()?.len();
    let mut carry: Vec<u8> = Vec::new();
    while pos > 0 {
        let n = CHUNK.min(pos);
        pos -= n;
        let mut buf = vec![0u8; n as usize];
        f.seek(SeekFrom::Start(pos))?;
        f.read_exact(&mut buf)?;
        buf.extend_from_slice(&carry);
        let mut end = buf.len();
        while let Some(i) = buf[..end].iter().rposition(|&b| b == b'\n') {
            let line = &buf[i + 1..end];
            if !line.is_empty() && !each(line) {
                return Ok(());
            }
            end = i;
        }
        buf.truncate(end);
        carry = buf;
    }
    if !carry.is_empty() {
        each(&carry);
    }
    Ok(())
}

/// The account's newest entries, oldest first: at most `limit`, none before `since`. An
/// unreadable line is skipped. No file: none.
pub fn read(
    home: &Path,
    provider: &str,
    account: &str,
    since: Option<SystemTime>,
    limit: Option<usize>,
) -> Result<Vec<Entry>, FileError> {
    let path = history_file(home, provider, account)?;
    let mut f = match File::open(&path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(io_err(&path)(e)),
    };
    let mut out = Vec::new();
    if limit == Some(0) {
        return Ok(out);
    }
    lines_backwards(&mut f, |line| {
        let Ok(e) = serde_json::from_slice::<Entry>(line) else { return true };
        if let (Some(s), Some(t)) = (since, e.time())
            && t < s
        {
            return false;
        }
        out.push(e);
        limit.is_none_or(|l| out.len() < l)
    })
    .map_err(io_err(&path))?;
    out.reverse();
    Ok(out)
}

/// The `at` of the account's newest entry.
fn newest_at(home: &Path, provider: &str, account: &str) -> Result<Option<String>, FileError> {
    Ok(read(home, provider, account, None, Some(1))?.pop().map(|e| e.at))
}

/// Every `(provider, account)` with a history or a checkpoint under `home`, filtered.
fn accounts_on_disk(
    home: &Path,
    provider: Option<&str>,
    account: Option<&str>,
) -> Result<Vec<(String, String)>, FileError> {
    let root = home.join(DIR);
    let mut out = Vec::new();
    let dirs = match fs::read_dir(&root) {
        Ok(d) => d,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(io_err(&root)(e)),
    };
    for d in dirs.flatten() {
        let p = d.file_name().to_string_lossy().into_owned();
        if provider.is_some_and(|want| want != p) || !d.path().is_dir() {
            continue;
        }
        for f in fs::read_dir(d.path()).map_err(io_err(&d.path()))?.flatten() {
            let name = f.file_name().to_string_lossy().into_owned();
            // The outside-use file sits beside the history but is not one.
            if name.ends_with(".outside.jsonl") {
                continue;
            }
            let Some(a) = name.strip_suffix(".jsonl").or_else(|| name.strip_suffix(".tally.json")) else {
                continue;
            };
            if a.starts_with('.') || account.is_some_and(|want| want != a) {
                continue;
            }
            let key = (p.clone(), a.to_owned());
            if !out.contains(&key) {
                out.push(key);
            }
        }
    }
    out.sort();
    Ok(out)
}

/// Deletes entries before `before` from every history (of `provider`, and `account`, when
/// given). Returns how many were deleted. Unreadable lines are kept. The account's outside-use
/// file is pruned too, under the same lock: see [`prune_outside`]. The count is history entries only.
pub fn prune(
    home: &Path,
    before: SystemTime,
    provider: Option<&str>,
    account: Option<&str>,
) -> Result<usize, FileError> {
    let mut removed = 0;
    for (p, a) in accounts_on_disk(home, provider, account)? {
        let path = history_file(home, &p, &a)?;
        let mut f = match OpenOptions::new().read(true).open(&path) {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(io_err(&path)(e)),
        };
        f.lock().map_err(io_err(&path))?;
        let mut text = String::new();
        f.read_to_string(&mut text).map_err(io_err(&path))?;
        let mut kept = String::with_capacity(text.len());
        let mut dropped = 0;
        for line in text.lines().filter(|l| !l.is_empty()) {
            let old = serde_json::from_str::<Entry>(line).ok().and_then(|e| e.time()).is_some_and(|t| t < before);
            if old {
                dropped += 1;
            } else {
                kept.push_str(line);
                kept.push('\n');
            }
        }
        if dropped > 0 {
            files::write_private(&path, &kept)?;
            removed += dropped;
        }
        // `f` still holds the history lock, which the outside-use writer takes too.
        prune_outside(home, &p, &a, before)?;
    }
    Ok(removed)
}

/// Drops the outside-use lines whose `start` is before `before`, except unacknowledged alerts
/// and their entries. An acknowledged alert, an `ack` and a `reclassified` line go with the entry
/// or alert they name; unreadable lines are kept. The caller holds the history lock.
fn prune_outside(home: &Path, provider: &str, account: &str, before: SystemTime) -> Result<(), FileError> {
    use super::fit::outside::{Line, outside_file};
    use std::collections::HashSet;

    let path = outside_file(home, provider, account)?;
    let Some(text) = files::read_private(&path)? else { return Ok(()) };
    let lines: Vec<(&str, Option<Line>)> =
        text.lines().filter(|l| !l.is_empty()).map(|l| (l, serde_json::from_str::<Line>(l).ok())).collect();
    let acked: HashSet<&str> = lines
        .iter()
        .filter_map(|(_, l)| match l {
            Some(Line::Ack { alert, .. }) => Some(alert.as_str()),
            _ => None,
        })
        .collect();
    // Entries an unacknowledged alert names stay.
    let pinned: HashSet<&str> = lines
        .iter()
        .filter_map(|(_, l)| match l {
            Some(Line::Alert { id, entry, .. }) if !acked.contains(id.as_str()) => Some(entry.as_str()),
            _ => None,
        })
        .collect();
    let old: HashSet<&str> = lines
        .iter()
        .filter_map(|(_, l)| match l {
            Some(Line::Entry(e)) if !pinned.contains(e.id.as_str()) && e.start_time().is_some_and(|t| t < before) => {
                Some(e.id.as_str())
            }
            _ => None,
        })
        .collect();
    // Alerts that go: acknowledged ones whose entry goes.
    let gone_alerts: HashSet<&str> = lines
        .iter()
        .filter_map(|(_, l)| match l {
            Some(Line::Alert { id, entry, .. }) if acked.contains(id.as_str()) && old.contains(entry.as_str()) => {
                Some(id.as_str())
            }
            _ => None,
        })
        .collect();
    let mut kept = String::with_capacity(text.len());
    let mut dropped = false;
    for (raw, line) in &lines {
        let drop = match line {
            Some(Line::Entry(e)) => old.contains(e.id.as_str()),
            Some(Line::Alert { id, .. }) => gone_alerts.contains(id.as_str()),
            Some(Line::Ack { alert, .. }) => gone_alerts.contains(alert.as_str()),
            Some(Line::Reclassified { entry, .. }) => old.contains(entry.as_str()),
            None => false,
        };
        if drop {
            dropped = true;
        } else {
            kept.push_str(raw);
            kept.push('\n');
        }
    }
    if dropped {
        files::write_private(&path, &kept)?;
    }
    Ok(())
}

/// Deletes the account's history, checkpoint and outside-use file. `false`: there was none.
pub fn forget(home: &Path, provider: &str, account: &str) -> Result<bool, FileError> {
    let mut found = false;
    let history = history_file(home, provider, account)?;
    if let Ok(f) = File::open(&history) {
        f.lock().map_err(io_err(&history))?;
    }
    let outside = super::fit::outside::outside_file(home, provider, account)?;
    for path in [history, tally_file(home, provider, account)?, outside] {
        match fs::remove_file(&path) {
            Ok(()) => found = true,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(io_err(&path)(e)),
        }
    }
    Ok(found)
}

/// The checkpointed tally to reload: none when the history holds an entry newer than the
/// checkpoint's base (that entry already carries it).
fn load_checkpoint(home: &Path, provider: &str, account: &str) -> Result<Option<AccountTally>, FileError> {
    let path = tally_file(home, provider, account)?;
    let Some(text) = files::read_private(&path)? else { return Ok(None) };
    let cp: Checkpoint = serde_json::from_str(&text).map_err(|e| FileError::invalid(&path, e.to_string()))?;
    let newest = newest_at(home, provider, account)?.and_then(|s| clock::parse_rfc3339(&s));
    let base = cp.base.as_deref().and_then(clock::parse_rfc3339);
    let stale = match (newest, base) {
        (None, _) => false,
        (Some(_), None) => true,
        (Some(n), Some(b)) => n > b,
    };
    Ok((!stale).then_some(cp.tally))
}

fn write_checkpoint(home: &Path, provider: &str, account: &str, tally: &AccountTally) -> Result<(), FileError> {
    let path = tally_file(home, provider, account)?;
    let cp = Checkpoint {
        v: VERSION,
        at: super::extract::rfc3339_millis(SystemTime::now()),
        base: newest_at(home, provider, account)?,
        tally: tally.clone(),
    };
    let text = serde_json::to_string(&cp).map_err(|e| FileError::invalid(&path, e.to_string()))?;
    private_dirs(&path)?;
    files::write_private(&path, &text)
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Called with `(provider, account, entry)` after an entry is durably appended.
pub type EntryObserver = Arc<dyn Fn(&str, &str, &Entry) + Send + Sync>;

#[derive(Default)]
struct Observers(Mutex<Vec<EntryObserver>>);

impl std::fmt::Debug for Observers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Observers").field("len", &lock(&self.0).len()).finish()
    }
}

/// The engine's poll history writer and running tallies.
#[derive(Debug)]
pub struct History {
    home: PathBuf,
    /// Every account's running tally.
    pub tally: Tally,
    /// Polls waiting to be written, in order, each with the tally taken when it landed.
    queue: Mutex<VecDeque<(QuotaPoll, AccountTally)>>,
    /// Held by whoever writes files: one writer at a time, in queue order.
    writer: Mutex<()>,
    checkpoint_ms: AtomicU64,
    /// Told of each entry once it is on disk (the quota fit learns from them, spec 012).
    observers: Observers,
}

impl History {
    /// The history under `home`, with every valid checkpoint reloaded. A checkpoint that
    /// can't be read is reported and left in place.
    pub fn open(home: &Path) -> Self {
        let h = Self {
            home: home.to_owned(),
            tally: Tally::default(),
            queue: Mutex::default(),
            writer: Mutex::default(),
            checkpoint_ms: AtomicU64::new(CHECKPOINT_EVERY.as_millis() as u64),
            observers: Observers::default(),
        };
        match accounts_on_disk(home, None, None) {
            Ok(list) => {
                for (p, a) in list {
                    match load_checkpoint(home, &p, &a) {
                        Ok(Some(t)) => h.tally.restore(&p, &a, &t),
                        Ok(None) => {}
                        Err(e) => tracing::warn!("tally checkpoint not reloaded: {e}"),
                    }
                }
            }
            Err(e) => tracing::warn!("tally checkpoints not reloaded: {e}"),
        }
        h
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    /// Registers `f` to run after each entry is durably appended, on the writing thread. It
    /// must not call back into this history's writers.
    pub fn on_entry(&self, f: EntryObserver) {
        lock(&self.observers.0).push(f);
    }

    /// How often the server checkpoints (tests shorten it).
    pub fn checkpoint_every(&self) -> Duration {
        Duration::from_millis(self.checkpoint_ms.load(Ordering::Relaxed))
    }

    pub fn set_checkpoint_every(&self, d: Duration) {
        self.checkpoint_ms.store(d.as_millis().max(1) as u64, Ordering::Relaxed);
    }

    /// The hook for [`QuotaBoard::on_poll`](super::poll::QuotaBoard::on_poll): queues the
    /// poll and writes it on the blocking pool (inline when there's no runtime).
    pub fn hook(self: &Arc<Self>) -> PollHook {
        let this = Arc::downgrade(self);
        Arc::new(move |poll: &QuotaPoll| {
            let Some(h) = this.upgrade() else { return };
            // The tally restarts the moment the poll lands, not when its entry is written: the
            // estimate between polls must not count traffic the poll already includes.
            let taken = h.tally.take_poll(&poll.provider, &poll.account, poll.ok());
            lock(&h.queue).push_back((poll.clone(), taken));
            match tokio::runtime::Handle::try_current() {
                Ok(rt) => {
                    rt.spawn_blocking(move || h.drain());
                }
                Err(_) => h.drain(),
            }
        })
    }

    /// Writes every queued poll. Blocking.
    pub fn drain(&self) {
        let _w = lock(&self.writer);
        loop {
            let Some((poll, taken)) = lock(&self.queue).pop_front() else { break };
            if let Err(e) = self.record_locked(&poll, taken) {
                tracing::warn!(provider = poll.provider, account = poll.account, "quota poll not kept: {e}");
            }
        }
    }

    /// Writes one poll's entry now (the queue is bypassed). Blocking.
    pub fn record(&self, poll: &QuotaPoll) -> Result<Entry, FileError> {
        let _w = lock(&self.writer);
        let taken = self.tally.take_poll(&poll.provider, &poll.account, poll.ok());
        self.record_locked(poll, taken)
    }

    /// Appends the entry with the tally taken for it, then rewrites the checkpoint past it. A
    /// failed append puts the tally back for the next entry.
    fn record_locked(&self, poll: &QuotaPoll, taken: AccountTally) -> Result<Entry, FileError> {
        let (p, a) = (poll.provider.as_str(), poll.account.as_str());
        let entry = Entry::from_poll(poll, taken);
        if let Err(e) = append(&self.home, p, a, &entry) {
            self.tally.restore(p, a, &entry.tally);
            return Err(e);
        }
        if let Err(e) = write_checkpoint(&self.home, p, a, &self.tally.get(p, a)) {
            // The entry holds the taken tally; an old checkpoint is stale against it.
            tracing::warn!(provider = p, account = a, "tally checkpoint not written: {e}");
            self.tally.mark_dirty(p, a);
        }
        let observers: Vec<EntryObserver> = lock(&self.observers.0).clone();
        for observe in observers {
            observe(p, a, &entry);
        }
        Ok(entry)
    }

    /// Writes queued polls, then checkpoints every tally changed since its last checkpoint.
    /// Returns how many checkpoints were written. Blocking.
    pub fn checkpoint(&self) -> usize {
        self.drain();
        let _w = lock(&self.writer);
        let mut written = 0;
        for (p, a) in self.tally.take_dirty() {
            match write_checkpoint(&self.home, &p, &a, &self.tally.get(&p, &a)) {
                Ok(()) => written += 1,
                Err(e) => {
                    tracing::warn!(provider = p, account = a, "tally checkpoint not written: {e}");
                    self.tally.mark_dirty(&p, &a);
                }
            }
        }
        written
    }
}

impl History {
    /// Rebuilds what the journal knows and the checkpoints don't (research R6): the hourly
    /// counters of the last [`HORIZON`](super::tally::HORIZON) and the traffic after each account's last
    /// checkpoint or poll entry, so a crash loses no counted traffic. Reads the record journal;
    /// call it once, before serving. Blocking. Returns how many attempts were counted.
    pub fn recover_counters(&self, now: SystemTime) -> usize {
        let since = now.checked_sub(super::tally::HORIZON);
        let filter = crate::journal::records::Filter { since, ..Default::default() };
        let mut resume: std::collections::HashMap<(String, String), Option<SystemTime>> =
            std::collections::HashMap::new();
        let mut counted = 0;
        // Oldest first, so each hour's counters and the sliding list fill in order.
        for r in crate::journal::records::read(&self.home, &filter).iter().rev() {
            let Some(arrived) = r["arrived"].as_str().and_then(clock::parse_rfc3339) else { continue };
            for a in r["attempts"].as_array().into_iter().flatten() {
                let (Some(p), Some(acct), Some(model)) =
                    (a["provider"].as_str(), a["account"].as_str(), a["model"].as_str())
                else {
                    continue;
                };
                // A skipped attempt sent nothing; one still running when the process died has no usage.
                let sent = matches!(a["outcome"]["state"].as_str(), Some("ok" | "failed" | "cancelled"));
                let Some(ended) = a["ended"].as_f64().filter(|_| sent) else { continue };
                let at = arrived + Duration::from_secs_f64((ended / 1000.0).max(0.0));
                let usage = usage_of(&a["usage"]);
                if usage.is_some_and(|u| u.estimated) {
                    continue;
                }
                let from =
                    *resume.entry((p.to_owned(), acct.to_owned())).or_insert_with(|| resume_point(&self.home, p, acct));
                self.tally.recover_attempt(p, acct, model, usage.as_ref(), at, from.is_none_or(|f| at > f));
                counted += 1;
            }
        }
        counted
    }
}

/// An attempt's usage as the journal wrote it. `None`: not reported.
fn usage_of(v: &serde_json::Value) -> Option<crate::records::Usage> {
    if !v.is_object() {
        return None;
    }
    let n = |k: &str| v[k].as_u64();
    Some(crate::records::Usage {
        input: n("input"),
        output: n("output"),
        cache_read: n("cache_read"),
        cache_write: n("cache_write"),
        reasoning: n("reasoning"),
        input_semantics: v["input_semantics"]
            .as_str()
            .and_then(nullrouter_registry::schema::InputSemantics::parse)
            .unwrap_or(nullrouter_registry::schema::InputSemantics::ExcludesCache),
        estimated: v["estimated"].as_bool().unwrap_or(false),
    })
}

/// The time the account's reloaded tally is current to: its checkpoint, or its newest poll
/// entry when that is later. `None`: it has neither, so every journal attempt counts.
fn resume_point(home: &Path, provider: &str, account: &str) -> Option<SystemTime> {
    let checkpoint = tally_file(home, provider, account)
        .ok()
        .and_then(|p| files::read_private(&p).ok().flatten())
        .and_then(|t| serde_json::from_str::<Checkpoint>(&t).ok())
        .and_then(|c| clock::parse_rfc3339(&c.at));
    let entry = newest_at(home, provider, account).ok().flatten().and_then(|s| clock::parse_rfc3339(&s));
    checkpoint.max(entry)
}

/// Lists the accounts with a history under `home`: `(provider, account)` sorted.
pub fn list(home: &Path) -> Result<Vec<(String, String)>, FileError> {
    accounts_on_disk(home, None, None)
}

impl crate::state::Engine {
    /// [`History::checkpoint`] on the blocking pool: writes queued polls and checkpoints
    /// changed tallies (`quota.checkpoint`, the 10 s timer, shutdown).
    pub async fn checkpoint_tallies(&self) -> usize {
        let h = self.history.clone();
        tokio::task::spawn_blocking(move || h.checkpoint()).await.unwrap_or_else(|e| {
            tracing::error!("tally checkpoint failed: {e}");
            0
        })
    }
}
