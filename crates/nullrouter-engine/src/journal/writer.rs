//! The journal writer thread: ordered appends, acked open and close, periodic `fdatasync`
//! (research R11, R12; contracts/record-journal.md).
//!
//! One dedicated `std::thread` owns every file under `records/` and `routing/`. Request tasks
//! only send lines over a channel (and await an ack for `open` and `close`), so file I/O never
//! blocks the async executor. Lines are written in the order they were sent, across all files.
//!
//! - **Ack**: sent after `write(2)` returns, not after `fsync`. A 0router crash can't lose an
//!   acked line; a power loss can lose up to about one second.
//! - **Sync**: `fdatasync` on dirty files at most every [`Options::sync_every`], and once at
//!   shutdown. A new file is followed by an `fsync` of its directory.
//! - **Lock**: `records.lock` is held per write batch, so a `prune` or `forget` rewrite from
//!   the CLI never interleaves with an append.
//! - **Disk full**: a failing write puts the line, and every later one, in a buffer of at most
//!   [`HOLD_LINES`] lines. Records beyond that are counted and dropped. Every
//!   [`Options::retry_every`] the buffer is written again, in order. Acks resolve with an
//!   error at once, and the request continues.

use std::collections::{BTreeSet, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use serde_json::Value;
use tokio::sync::oneshot;

/// Most lines held in memory while the disk refuses writes (Clarifications Q2).
pub const HOLD_LINES: usize = 10_000;
/// How many queued lines one write batch takes.
const BATCH: usize = 256;

/// Which file a line goes to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// `records/<day>.jsonl`, by UTC day of the request's arrival (`YYYY-MM-DD`).
    Records { day: String },
    /// `routing/warm.jsonl`.
    Warm,
    /// `routing/ledger.jsonl`.
    Ledger,
    /// `routing/verdicts.jsonl` (spec 011).
    Verdicts,
}

impl Target {
    /// The segment for a request that arrived at `t`.
    pub fn records_at(t: SystemTime) -> Self {
        Self::Records { day: crate::clock::rfc3339(t)[..10].to_owned() }
    }

    fn path(&self, home: &Path) -> PathBuf {
        match self {
            Self::Records { day } => home.join("records").join(format!("{day}.jsonl")),
            Self::Warm => home.join("routing").join("warm.jsonl"),
            Self::Ledger => home.join("routing").join("ledger.jsonl"),
            Self::Verdicts => home.join("routing").join(crate::verdict::store::FILE),
        }
    }

    fn is_record(&self) -> bool {
        matches!(self, Self::Records { .. })
    }
}

/// Why an ack didn't come back clean.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum JournalError {
    /// The disk refused the write: the line is held in memory, or dropped when the buffer is full.
    #[error("record not kept: the journal can't write")]
    NotKept,
    /// The writer has shut down.
    #[error("the journal is closed")]
    Closed,
}

/// The ack for a line: resolves when the line was written (or couldn't be).
pub struct Ack(oneshot::Receiver<Result<(), JournalError>>);

impl Ack {
    pub async fn wait(self) -> Result<(), JournalError> {
        self.0.await.unwrap_or(Err(JournalError::Closed))
    }

    /// Blocks the calling thread. For tests and the CLI; never call it from an async task.
    pub fn wait_blocking(self) -> Result<(), JournalError> {
        self.0.blocking_recv().unwrap_or(Err(JournalError::Closed))
    }
}

/// What the routing view, `check` and the log show about writing (data-model § Journal health).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Health {
    /// Every line written so far has reached the file.
    pub kept: bool,
    /// When writing first failed, while it still fails.
    pub since: Option<SystemTime>,
    /// Requests with at least one line dropped since then.
    pub unkept_requests: u64,
    /// Lines held in memory, waiting for the disk.
    pub held_lines: usize,
    /// The last successful `fdatasync`.
    pub last_sync: Option<SystemTime>,
}

/// Timing knobs. The defaults are the contract's; tests shorten them.
#[derive(Debug, Clone, Copy)]
pub struct Options {
    pub sync_every: Duration,
    pub retry_every: Duration,
}

impl Default for Options {
    fn default() -> Self {
        Self { sync_every: Duration::from_secs(1), retry_every: Duration::from_secs(5) }
    }
}

/// Failures a test can switch on.
#[cfg(feature = "testkit")]
#[derive(Debug, Default)]
pub struct Faults {
    /// Every write fails with `ENOSPC`.
    pub fail_writes: std::sync::atomic::AtomicBool,
    /// Acks are held back until this is cleared. The lines are still written.
    pub pause_acks: std::sync::atomic::AtomicBool,
    /// How many `written()` marks resolve even while `pause_acks` is set. A test that wants the
    /// `open` ack to pass and the `close` ack to wait sets it to 1.
    pub free_marks: std::sync::atomic::AtomicUsize,
    /// Each file's length at its last `fdatasync`: what a power loss leaves.
    pub synced: Mutex<std::collections::BTreeMap<PathBuf, u64>>,
}

type AckTx = oneshot::Sender<Result<(), JournalError>>;

enum Msg {
    Line(Item),
    /// Everything before this has been handled; then the thread syncs and stops.
    Shutdown(mpsc::SyncSender<()>),
    /// Everything before this has been written and synced.
    Barrier(mpsc::SyncSender<()>),
    /// Resolves once everything before it has been written (not synced).
    Mark(AckTx),
    /// Replaces a routing file with `body`, in order with the appends (a compaction or a forget).
    Replace(Target, String),
}

struct Item {
    target: Target,
    /// The line without its newline.
    line: String,
    /// The request the line belongs to, for counting unkept requests.
    id: Option<String>,
    ack: Option<AckTx>,
}

/// The handle request tasks hold.
pub struct Writer {
    tx: Mutex<Option<mpsc::Sender<Msg>>>,
    health: Arc<Mutex<Health>>,
    syncs: Arc<AtomicU64>,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
    #[cfg(feature = "testkit")]
    faults: Arc<Faults>,
}

impl Writer {
    /// Creates `records/` and `routing/` under `home` (0700) and starts the thread.
    pub fn start(home: &Path, options: Options) -> io::Result<Self> {
        for dir in ["records", "routing"] {
            make_private_dir(&home.join(dir))?;
        }
        let (tx, rx) = mpsc::channel();
        let health = Arc::new(Mutex::new(Health { kept: true, ..Health::default() }));
        let syncs = Arc::new(AtomicU64::new(0));
        #[cfg(feature = "testkit")]
        let faults = Arc::new(Faults::default());
        let thread = {
            let state = Thread {
                home: home.to_owned(),
                options,
                health: health.clone(),
                syncs: syncs.clone(),
                #[cfg(feature = "testkit")]
                faults: faults.clone(),
                pending: VecDeque::new(),
                dropped: BTreeSet::new(),
                dirty: BTreeSet::new(),
                held_acks: Vec::new(),
                last_sync: Instant::now(),
                next_retry: None,
            };
            std::thread::Builder::new().name("journal-writer".into()).spawn(move || state.run(rx))?
        };
        Ok(Self {
            tx: Mutex::new(Some(tx)),
            health,
            syncs,
            thread: Mutex::new(Some(thread)),
            #[cfg(feature = "testkit")]
            faults,
        })
    }

    /// Queues a line; the writer wraps `fields` in `{"v":1,"t":t,…}`. Doesn't wait.
    pub fn append(&self, target: Target, t: &str, fields: Value) {
        self.send(target, t, fields, None);
    }

    /// Replaces a routing file's whole content: written to a temporary file, `fdatasync`ed,
    /// renamed into place and the directory `fsync`ed. Runs in order with the appends, so
    /// callers queue it under the same lock as the lines it summarises. Skipped (and retried at
    /// the next compaction) while the disk is refusing writes.
    pub fn replace(&self, target: Target, body: String) {
        let tx = self.tx.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(tx) = tx.as_ref() {
            let _ = tx.send(Msg::Replace(target, body));
        }
    }

    /// An ack that resolves once every line queued before this call has been written (not
    /// synced). `Err(NotKept)` while the disk refuses writes. A request waits on it before its
    /// last byte, so its `close` line is in the file first (spec 006, FR-037).
    pub fn written(&self) -> Ack {
        let (tx, rx) = oneshot::channel();
        let tx_guard = self.tx.lock().unwrap_or_else(|e| e.into_inner());
        let refused = match tx_guard.as_ref() {
            Some(t) => match t.send(Msg::Mark(tx)) {
                Ok(()) => return Ack(rx),
                Err(mpsc::SendError(Msg::Mark(tx))) => Some(tx),
                Err(_) => None,
            },
            None => Some(tx),
        };
        if let Some(tx) = refused {
            let _ = tx.send(Err(JournalError::Closed));
        }
        Ack(rx)
    }

    /// Queues a line and returns its ack.
    pub fn append_acked(&self, target: Target, t: &str, fields: Value) -> Ack {
        let (tx, rx) = oneshot::channel();
        self.send(target, t, fields, Some(tx));
        Ack(rx)
    }

    fn send(&self, target: Target, t: &str, fields: Value, ack: Option<AckTx>) {
        let id = fields.get("id").and_then(Value::as_str).map(str::to_owned);
        let item = Item { target, line: render(t, fields), id, ack };
        let tx = self.tx.lock().unwrap_or_else(|e| e.into_inner());
        let refused = match tx.as_ref() {
            Some(tx) => match tx.send(Msg::Line(item)) {
                Ok(()) => return,
                Err(mpsc::SendError(Msg::Line(item))) => item,
                Err(_) => return,
            },
            None => item,
        };
        if let Some(ack) = refused.ack {
            let _ = ack.send(Err(JournalError::Closed));
        }
    }

    pub fn health(&self) -> Health {
        self.health.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// How many `fdatasync` rounds have run.
    pub fn sync_rounds(&self) -> u64 {
        self.syncs.load(Ordering::Relaxed)
    }

    /// Blocks until every line sent so far is written and synced.
    pub fn flush_blocking(&self) {
        let (tx, rx) = mpsc::sync_channel(1);
        let sent = self.tx.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|t| t.send(Msg::Barrier(tx)));
        if matches!(sent, Some(Ok(()))) {
            let _ = rx.recv();
        }
    }

    /// Writes what is queued, syncs, and stops the thread. Later lines get [`JournalError::Closed`].
    pub fn shutdown(&self) {
        let (tx, rx) = mpsc::sync_channel(1);
        let sent = self.tx.lock().unwrap_or_else(|e| e.into_inner()).take().map(|t| t.send(Msg::Shutdown(tx)));
        if matches!(sent, Some(Ok(()))) {
            let _ = rx.recv();
        }
        if let Some(h) = self.thread.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = h.join();
        }
    }

    #[cfg(feature = "testkit")]
    pub fn faults(&self) -> &Faults {
        &self.faults
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// `{"v":1,"t":t,…fields}`. A non-object body is wrapped under `"data"`.
fn render(t: &str, fields: Value) -> String {
    let mut line = serde_json::Map::new();
    line.insert("v".into(), 1.into());
    line.insert("t".into(), t.into());
    match fields {
        Value::Object(m) => line.extend(m),
        Value::Null => {}
        other => {
            line.insert("data".into(), other);
        }
    }
    Value::Object(line).to_string()
}

fn make_private_dir(dir: &Path) -> io::Result<()> {
    if dir.is_dir() {
        return Ok(());
    }
    fs::DirBuilder::new().mode(0o700).recursive(true).create(dir)?;
    if let Some(parent) = dir.parent() {
        sync_dir(parent)?;
    }
    Ok(())
}

/// `fsync` of a directory, so a file created or renamed in it survives a power loss.
pub fn sync_dir(dir: &Path) -> io::Result<()> {
    File::open(dir)?.sync_all()
}

/// Replaces `path` with `body`: a temporary file next to it, `fdatasync`ed, renamed into place,
/// then the directory `fsync`ed. A crash leaves the old file or the new one, never a mix.
pub fn replace_file(path: &Path, body: &str) -> io::Result<()> {
    let tmp = path.with_extension("jsonl.tmp");
    let mut f = OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
    f.write_all(body.as_bytes())?;
    f.sync_data()?;
    fs::rename(&tmp, path)?;
    match path.parent() {
        Some(dir) => sync_dir(dir),
        None => Ok(()),
    }
}

fn is_disk_full(e: &io::Error) -> bool {
    // ENOSPC and EDQUOT.
    matches!(e.raw_os_error(), Some(28 | 122)) || e.kind() == io::ErrorKind::StorageFull
}

struct Thread {
    home: PathBuf,
    options: Options,
    health: Arc<Mutex<Health>>,
    syncs: Arc<AtomicU64>,
    #[cfg(feature = "testkit")]
    faults: Arc<Faults>,
    /// Lines the disk refused, oldest first.
    pending: VecDeque<Item>,
    /// Requests with a dropped line.
    dropped: BTreeSet<String>,
    dirty: BTreeSet<PathBuf>,
    /// Acks written but held back by a test's `pause_acks`.
    held_acks: Vec<(AckTx, Result<(), JournalError>)>,
    last_sync: Instant,
    next_retry: Option<Instant>,
}

impl Thread {
    fn run(mut self, rx: Receiver<Msg>) {
        let mut stop: Option<mpsc::SyncSender<()>> = None;
        loop {
            let wait = self.wait();
            let first = match rx.recv_timeout(wait) {
                Ok(m) => Some(m),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break,
            };
            let mut batch: Vec<Msg> = first.into_iter().collect();
            while batch.len() < BATCH
                && let Ok(m) = rx.try_recv()
            {
                batch.push(m);
            }
            let mut barriers = Vec::new();
            let mut marks = Vec::new();
            let mut lines = Vec::new();
            for m in batch {
                match m {
                    Msg::Line(i) => lines.push(i),
                    Msg::Barrier(b) => barriers.push(b),
                    Msg::Mark(a) => marks.push(a),
                    Msg::Replace(t, b) => {
                        // Lines sent before it are written first; lines after it follow.
                        self.write_batch(std::mem::take(&mut lines));
                        self.replace_file((t, b));
                    }
                    Msg::Shutdown(s) => stop = Some(s),
                }
            }
            self.write_batch(lines);
            for a in marks {
                let result = if self.pending.is_empty() { Ok(()) } else { Err(JournalError::NotKept) };
                if self.mark_is_free() {
                    let _ = a.send(result);
                } else {
                    self.settle(Some(a), result);
                }
            }
            self.maintain(!barriers.is_empty() || stop.is_some());
            for b in barriers {
                let _ = b.send(());
            }
            if stop.is_some() {
                break;
            }
        }
        self.maintain(true);
        self.release_acks(true);
        if let Some(s) = stop {
            let _ = s.send(());
        }
    }

    /// How long to sleep before something is due.
    fn wait(&self) -> Duration {
        let now = Instant::now();
        let mut wait = Duration::from_secs(3600);
        if !self.dirty.is_empty() {
            wait = wait.min((self.last_sync + self.options.sync_every).saturating_duration_since(now));
        }
        if let Some(at) = self.next_retry {
            wait = wait.min(at.saturating_duration_since(now));
        }
        if !self.held_acks.is_empty() {
            wait = wait.min(Duration::from_millis(5));
        }
        wait.max(Duration::from_millis(1))
    }

    fn write_batch(&mut self, lines: Vec<Item>) {
        if lines.is_empty() {
            return;
        }
        let _lock = if lines.iter().any(|i| i.target.is_record()) { self.lock() } else { None };
        for item in lines {
            if !self.pending.is_empty() {
                // Order matters: nothing jumps ahead of lines the disk refused.
                self.hold(item);
                continue;
            }
            match self.write_one(&item) {
                Ok(()) => self.settle(item.ack, Ok(())),
                Err(e) => {
                    self.fail(&e);
                    self.hold(item);
                }
            }
        }
    }

    fn replace_file(&mut self, (target, body): (Target, String)) {
        if !self.pending.is_empty() {
            return;
        }
        let path = target.path(&self.home);
        let _lock = self.lock();
        match replace_file(&path, &body) {
            Ok(()) => {
                self.dirty.remove(&path);
            }
            Err(e) => tracing::warn!("journal: could not compact {}: {e}", path.display()),
        }
    }

    /// The advisory lock the CLI's rewrites also take. Without it (it can't be created), writing
    /// goes on: the lock only orders appends against a rewrite.
    fn lock(&self) -> Option<File> {
        let f = OpenOptions::new().append(true).create(true).mode(0o600).open(self.home.join("records.lock")).ok()?;
        f.lock().ok()?;
        Some(f)
    }

    fn write_one(&mut self, item: &Item) -> io::Result<()> {
        #[cfg(feature = "testkit")]
        if self.faults.fail_writes.load(Ordering::Relaxed) {
            return Err(io::Error::from_raw_os_error(28));
        }
        let path = item.target.path(&self.home);
        let (mut f, created) = match OpenOptions::new().append(true).create_new(true).mode(0o600).open(&path) {
            Ok(f) => (f, true),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => (OpenOptions::new().append(true).open(&path)?, false),
            Err(e) => return Err(e),
        };
        if created && let Some(dir) = path.parent() {
            sync_dir(dir)?;
        }
        let before = f.metadata()?.len();
        let mut bytes = Vec::with_capacity(item.line.len() + 1);
        bytes.extend_from_slice(item.line.as_bytes());
        bytes.push(b'\n');
        if let Err(e) = f.write_all(&bytes) {
            // A partial line would glue itself to the next one; cut it off.
            let _ = f.set_len(before);
            return Err(e);
        }
        self.dirty.insert(path);
        Ok(())
    }

    /// Takes a refused line into the buffer, or counts it dropped when the buffer is full.
    fn hold(&mut self, mut item: Item) {
        let ack = item.ack.take();
        if self.pending.len() >= HOLD_LINES {
            if let Some(id) = &item.id {
                self.dropped.insert(id.clone());
            }
        } else {
            self.pending.push_back(item);
        }
        self.settle(ack, Err(JournalError::NotKept));
        let mut h = self.health.lock().unwrap_or_else(|e| e.into_inner());
        h.unkept_requests = self.dropped.len() as u64;
        h.held_lines = self.pending.len();
    }

    fn fail(&mut self, e: &io::Error) {
        let first = {
            let mut h = self.health.lock().unwrap_or_else(|e| e.into_inner());
            let first = h.kept;
            if first {
                h.kept = false;
                h.since = Some(SystemTime::now());
            }
            first
        };
        if first {
            let what = if is_disk_full(e) { "the disk is full" } else { "a write failed" };
            tracing::warn!("journal: {what} ({e}); holding up to {HOLD_LINES} lines and retrying");
        }
        self.next_retry.get_or_insert_with(|| Instant::now() + self.options.retry_every);
    }

    /// Whether a test let this mark through while acks are paused.
    fn mark_is_free(&self) -> bool {
        #[cfg(feature = "testkit")]
        let free =
            self.faults.free_marks.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1)).is_ok();
        #[cfg(not(feature = "testkit"))]
        let free = false;
        free
    }

    fn settle(&mut self, ack: Option<AckTx>, result: Result<(), JournalError>) {
        let Some(ack) = ack else { return };
        #[cfg(feature = "testkit")]
        if self.faults.pause_acks.load(Ordering::Relaxed) {
            self.held_acks.push((ack, result));
            return;
        }
        let _ = ack.send(result);
    }

    fn release_acks(&mut self, all: bool) {
        #[cfg(feature = "testkit")]
        let paused = !all && self.faults.pause_acks.load(Ordering::Relaxed);
        #[cfg(not(feature = "testkit"))]
        let paused = false;
        let _ = all;
        if !paused {
            for (ack, result) in self.held_acks.drain(..) {
                let _ = ack.send(result);
            }
        }
    }

    /// Retries the held lines when due, syncs when due (or when `force`d), and releases acks.
    fn maintain(&mut self, force: bool) {
        let now = Instant::now();
        if !self.pending.is_empty() && (force || self.next_retry.is_some_and(|at| now >= at)) {
            self.retry();
        }
        if !self.dirty.is_empty() && (force || now >= self.last_sync + self.options.sync_every) {
            self.sync();
        }
        self.release_acks(false);
    }

    fn retry(&mut self) {
        let _lock = self.lock();
        while let Some(item) = self.pending.pop_front() {
            if let Err(e) = self.write_one(&item) {
                self.pending.push_front(item);
                self.next_retry = Some(Instant::now() + self.options.retry_every);
                tracing::debug!("journal: still can't write: {e}");
                break;
            }
        }
        let mut h = self.health.lock().unwrap_or_else(|e| e.into_inner());
        h.held_lines = self.pending.len();
        if self.pending.is_empty() {
            let lost = self.dropped.len();
            let since = h.since.take();
            h.kept = true;
            h.unkept_requests = 0;
            self.next_retry = None;
            self.dropped.clear();
            if since.is_some() {
                tracing::warn!("journal: writing again; {lost} requests were not kept");
            }
        }
    }

    fn sync(&mut self) {
        let mut failed = BTreeSet::new();
        for path in std::mem::take(&mut self.dirty) {
            // `fdatasync` works on any descriptor of the inode, so a file that a rewrite has
            // since replaced is synced through its new name.
            let synced = File::open(&path).and_then(|f| f.sync_data().and_then(|()| f.metadata()));
            match synced {
                Ok(_meta) => {
                    #[cfg(feature = "testkit")]
                    self.faults.synced.lock().unwrap_or_else(|e| e.into_inner()).insert(path, _meta.len());
                }
                Err(_) if path.exists() => {
                    failed.insert(path);
                }
                Err(_) => {}
            }
        }
        self.dirty = failed;
        self.last_sync = Instant::now();
        self.syncs.fetch_add(1, Ordering::Relaxed);
        if self.dirty.is_empty() {
            self.health.lock().unwrap_or_else(|e| e.into_inner()).last_sync = Some(SystemTime::now());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use serde_json::json;

    use super::*;

    fn fast() -> Options {
        Options { sync_every: Duration::from_millis(20), retry_every: Duration::from_millis(30) }
    }

    fn lines(path: &Path) -> Vec<Value> {
        fs::read_to_string(path).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect()
    }

    fn day() -> Target {
        Target::Records { day: "2026-10-04".into() }
    }

    #[test]
    fn lines_land_in_order_across_files_with_the_envelope() {
        let home = tempfile::tempdir().unwrap();
        let w = Writer::start(home.path(), fast()).unwrap();
        for n in 0..50 {
            w.append(day(), "attempt", json!({"id": "rq_1", "n": n}));
            w.append(Target::Warm, "warm", json!({"n": n}));
        }
        w.append(Target::Ledger, "ledger", json!({"target": "sonnet"}));
        w.flush_blocking();
        let rec = lines(&home.path().join("records/2026-10-04.jsonl"));
        assert_eq!(rec.len(), 50);
        assert_eq!(rec[0], json!({"v": 1, "t": "attempt", "id": "rq_1", "n": 0}));
        assert!(rec.iter().enumerate().all(|(i, l)| l["n"] == i));
        assert_eq!(lines(&home.path().join("routing/warm.jsonl")).len(), 50);
        assert_eq!(lines(&home.path().join("routing/ledger.jsonl"))[0]["t"], "ledger");
    }

    #[test]
    fn files_are_private_and_new_ones_have_their_directory_synced() {
        let home = tempfile::tempdir().unwrap();
        let w = Writer::start(home.path(), fast()).unwrap();
        w.append(day(), "open", json!({"id": "rq_1"}));
        w.append(Target::Warm, "warm", json!({}));
        w.flush_blocking();
        let mode = |p: &str| fs::metadata(home.path().join(p)).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode("records"), 0o700);
        assert_eq!(mode("routing"), 0o700);
        assert_eq!(mode("records/2026-10-04.jsonl"), 0o600);
        assert_eq!(mode("routing/warm.jsonl"), 0o600);
        assert_eq!(mode("records.lock"), 0o600);
    }

    #[tokio::test]
    async fn an_ack_resolves_after_the_line_is_in_the_file() {
        let home = tempfile::tempdir().unwrap();
        let w = Writer::start(home.path(), fast()).unwrap();
        let ack = w.append_acked(day(), "open", json!({"id": "rq_1"}));
        ack.wait().await.unwrap();
        // No flush and no sync: the line is already readable.
        assert_eq!(lines(&home.path().join("records/2026-10-04.jsonl")).len(), 1);
    }

    #[test]
    fn dirty_files_are_synced_about_every_interval_and_not_when_idle() {
        let home = tempfile::tempdir().unwrap();
        let w = Writer::start(home.path(), fast()).unwrap();
        w.append(day(), "open", json!({"id": "rq_1"}));
        std::thread::sleep(Duration::from_millis(150));
        let rounds = w.sync_rounds();
        assert!(rounds >= 1, "the dirty file was synced");
        assert!(w.health().last_sync.is_some());
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(w.sync_rounds(), rounds, "nothing dirty, nothing to sync");
    }

    #[test]
    fn shutdown_writes_the_queue_and_syncs_once() {
        let home = tempfile::tempdir().unwrap();
        let w = Writer::start(
            home.path(),
            Options { sync_every: Duration::from_secs(3600), retry_every: Duration::from_secs(3600) },
        )
        .unwrap();
        w.append(day(), "open", json!({"id": "rq_1"}));
        assert_eq!(w.sync_rounds(), 0);
        w.shutdown();
        assert_eq!(w.sync_rounds(), 1);
        assert_eq!(lines(&home.path().join("records/2026-10-04.jsonl")).len(), 1);
        // A line after shutdown is refused, not lost silently.
        let ack = w.append_acked(day(), "open", json!({"id": "rq_2"}));
        assert_eq!(ack.wait_blocking(), Err(JournalError::Closed));
    }

    #[cfg(feature = "testkit")]
    #[test]
    fn a_full_disk_holds_lines_in_order_and_writes_them_when_space_returns() {
        let home = tempfile::tempdir().unwrap();
        let w = Writer::start(home.path(), fast()).unwrap();
        w.append(day(), "open", json!({"id": "rq_0"}));
        w.flush_blocking();
        w.faults().fail_writes.store(true, Ordering::Relaxed);
        let ack = w.append_acked(day(), "open", json!({"id": "rq_1"}));
        assert_eq!(ack.wait_blocking(), Err(JournalError::NotKept), "the request is told, and goes on");
        for n in 2..5 {
            w.append(day(), "open", json!({"id": format!("rq_{n}")}));
        }
        w.append(Target::Warm, "warm", json!({"n": 1}));
        w.flush_blocking();
        let h = w.health();
        assert!(!h.kept && h.since.is_some());
        assert_eq!(h.held_lines, 5);
        assert_eq!(lines(&home.path().join("records/2026-10-04.jsonl")).len(), 1, "nothing written meanwhile");

        w.faults().fail_writes.store(false, Ordering::Relaxed);
        std::thread::sleep(Duration::from_millis(200));
        let h = w.health();
        assert!(h.kept && h.since.is_none() && h.held_lines == 0, "{h:?}");
        let ids: Vec<String> = lines(&home.path().join("records/2026-10-04.jsonl"))
            .iter()
            .map(|l| l["id"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(ids, ["rq_0", "rq_1", "rq_2", "rq_3", "rq_4"], "held lines are written in the order sent");
        assert_eq!(lines(&home.path().join("routing/warm.jsonl")).len(), 1);
    }

    #[cfg(feature = "testkit")]
    #[test]
    fn beyond_the_buffer_requests_are_counted_not_kept() {
        let home = tempfile::tempdir().unwrap();
        let w = Writer::start(home.path(), Options { retry_every: Duration::from_secs(3600), ..fast() }).unwrap();
        w.faults().fail_writes.store(true, Ordering::Relaxed);
        for n in 0..HOLD_LINES + 7 {
            w.append(day(), "open", json!({"id": format!("rq_{n}")}));
        }
        w.flush_blocking();
        let h = w.health();
        assert_eq!((h.held_lines, h.unkept_requests), (HOLD_LINES, 7), "{h:?}");
    }

    #[cfg(feature = "testkit")]
    #[test]
    fn paused_acks_wait_while_the_line_is_written() {
        let home = tempfile::tempdir().unwrap();
        let w = Writer::start(home.path(), fast()).unwrap();
        w.faults().pause_acks.store(true, Ordering::Relaxed);
        let ack = w.append_acked(day(), "close", json!({"id": "rq_1"}));
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(lines(&home.path().join("records/2026-10-04.jsonl")).len(), 1, "written");
        let waiter = std::thread::spawn(move || ack.wait_blocking());
        std::thread::sleep(Duration::from_millis(100));
        assert!(!waiter.is_finished(), "but not acked");
        w.faults().pause_acks.store(false, Ordering::Relaxed);
        assert_eq!(waiter.join().unwrap(), Ok(()));
    }
}
