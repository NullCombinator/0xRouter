//! Background tasks the dashboard starts and polls (spec 014, data-model § Task).
//!
//! In memory only. The table keeps the newest [`KEEP`] tasks and at most [`MAX_RUNNING`]
//! running at once; a worker owns a [`TaskHandle`] and reports through it.

use std::collections::VecDeque;
use std::fmt;
use std::str::FromStr;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard};

use jiff::Timestamp;
use nullrouter_registry::SecretString;

/// Tasks kept, newest first; only ended ones are evicted.
pub const KEEP: usize = 200;
/// Tasks that may run at once.
pub const MAX_RUNNING: usize = 4;

const ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
/// Tries at a unused id before `start` gives up.
const REROLLS: usize = 8;
const STOPPED: &str = "the task stopped unexpectedly";

/// `tk_` + 16 base32 characters.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TaskId(String);

impl TaskId {
    /// A fresh id, or `None` when the system random source fails: never a fixed id.
    fn random() -> Option<Self> {
        let mut b = [0u8; 16];
        getrandom::fill(&mut b).ok()?;
        let mut s = String::from("tk_");
        s.extend(b.iter().map(|x| ALPHABET[(x & 31) as usize] as char));
        Some(Self(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TaskId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for TaskId {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s.strip_prefix("tk_") {
            Some(rest) if rest.len() == 16 && rest.bytes().all(|c| ALPHABET.contains(&c)) => Ok(Self(s.to_string())),
            _ => Err(format!("{s}: not a task id")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskKind {
    Test,
    Signin,
    AdapterReview,
    AdapterBuild,
    AdapterRebuild,
    CatalogueInstall,
    ProxyFixed,
    QuotaPoll,
}

impl TaskKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Test => "test",
            Self::Signin => "signin",
            Self::AdapterReview => "adapter_review",
            Self::AdapterBuild => "adapter_build",
            Self::AdapterRebuild => "adapter_rebuild",
            Self::CatalogueInstall => "catalogue_install",
            Self::ProxyFixed => "proxy_fixed",
            Self::QuotaPoll => "quota_poll",
        }
    }
}

/// What a sign-in task waits for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskInput {
    Paste,
    Device { code: String, link: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskResult {
    Running,
    Done { text: String },
    Failed { text: String },
}

/// A snapshot of one task. Holds no secret.
#[derive(Debug, Clone)]
pub struct TaskView {
    pub id: TaskId,
    pub kind: TaskKind,
    pub subject: String,
    pub started: Timestamp,
    pub ended: Option<Timestamp>,
    pub progress: Vec<String>,
    pub result: TaskResult,
    pub input: Option<TaskInput>,
    /// The digest of the dashboard session that started the task, when one did (see
    /// [`Tasks::set_owner`]); `None` for a task no session owns.
    pub owner: Option<String>,
}

/// The table is full of running tasks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Busy;

impl fmt::Display for Busy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "busy: {MAX_RUNNING} tasks are running; try again when one ends")
    }
}

impl std::error::Error for Busy {}

struct Entry {
    view: TaskView,
    /// Where `Tasks::paste` delivers; dropped when the task ends.
    paste: Option<Sender<SecretString>>,
}

#[derive(Default)]
struct Table {
    /// Oldest first.
    entries: VecDeque<Entry>,
}

impl Table {
    fn find(&mut self, id: &TaskId) -> Option<&mut Entry> {
        self.entries.iter_mut().find(|e| &e.view.id == id)
    }

    fn running(&self) -> usize {
        self.entries.iter().filter(|e| e.view.result == TaskResult::Running).count()
    }
}

#[derive(Clone, Default)]
pub struct Tasks {
    table: Arc<Mutex<Table>>,
}

fn lock(m: &Mutex<Table>) -> MutexGuard<'_, Table> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Tasks {
    /// Registers a running task, or refuses when [`MAX_RUNNING`] already run.
    pub fn start(&self, kind: TaskKind, subject: impl Into<String>) -> Result<TaskHandle, Busy> {
        let mut t = lock(&self.table);
        if t.running() >= MAX_RUNNING {
            return Err(Busy);
        }
        // 80 random bits make a repeat all but impossible; a bounded re-roll still ends if the
        // source misbehaves, and a source that fails refuses the task rather than reusing an id.
        let mut fresh = None;
        for _ in 0..REROLLS {
            let Some(id) = TaskId::random() else { return Err(Busy) };
            if !t.entries.iter().any(|e| e.view.id == id) {
                fresh = Some(id);
                break;
            }
        }
        let Some(id) = fresh else { return Err(Busy) };
        t.entries.push_back(Entry {
            view: TaskView {
                id: id.clone(),
                kind,
                subject: subject.into(),
                started: Timestamp::now(),
                ended: None,
                progress: Vec::new(),
                result: TaskResult::Running,
                input: None,
                owner: None,
            },
            paste: None,
        });
        // Evict the oldest ended tasks beyond the bound; running ones stay.
        while t.entries.len() > KEEP {
            match t.entries.iter().position(|e| e.view.result != TaskResult::Running) {
                Some(i) => {
                    t.entries.remove(i);
                }
                None => break,
            }
        }
        Ok(TaskHandle { table: self.table.clone(), id, ended: false })
    }

    /// Records the dashboard session (its digest) that started the task, so its panel and paste
    /// belong to that session alone.
    pub fn set_owner(&self, id: &TaskId, owner: impl Into<String>) {
        if let Some(e) = lock(&self.table).find(id) {
            e.view.owner = Some(owner.into());
        }
    }

    pub fn get(&self, id: &TaskId) -> Option<TaskView> {
        lock(&self.table).find(id).map(|e| e.view.clone())
    }

    /// Newest first.
    pub fn list(&self) -> Vec<TaskView> {
        lock(&self.table).entries.iter().rev().map(|e| e.view.clone()).collect()
    }

    /// Hands a pasted code to a sign-in task that waits for one.
    pub fn paste(&self, id: &TaskId, code: SecretString) -> Result<(), String> {
        let mut t = lock(&self.table);
        let waiting =
            t.find(id).filter(|e| e.view.result == TaskResult::Running && e.view.input == Some(TaskInput::Paste));
        let Some(tx) = waiting.and_then(|e| e.paste.as_ref()) else {
            return Err(format!("{id}: not waiting for a code"));
        };
        tx.send(code).map_err(|_| format!("{id}: not waiting for a code"))
    }
}

/// The worker's side of a task. Dropping it unended marks the task failed.
pub struct TaskHandle {
    table: Arc<Mutex<Table>>,
    id: TaskId,
    ended: bool,
}

impl TaskHandle {
    pub fn id(&self) -> &TaskId {
        &self.id
    }

    fn with(&self, f: impl FnOnce(&mut Entry)) {
        if let Some(e) = lock(&self.table).find(&self.id) {
            f(e);
        }
    }

    pub fn progress(&self, line: impl Into<String>) {
        let line = line.into();
        self.with(|e| e.view.progress.push(line));
    }

    pub fn set_input(&self, input: Option<TaskInput>) {
        self.with(|e| e.view.input = input);
    }

    /// The receiving end of the paste channel. Call it before `set_input(Some(Paste))`.
    /// Calling again replaces the channel; the earlier receiver then sees a disconnect.
    pub fn paste_receiver(&self) -> Receiver<SecretString> {
        let (tx, rx) = mpsc::channel();
        self.with(|e| e.paste = Some(tx));
        rx
    }

    pub fn done(mut self, text: impl Into<String>) {
        self.finish(TaskResult::Done { text: text.into() });
    }

    pub fn fail(mut self, text: impl Into<String>) {
        self.finish(TaskResult::Failed { text: text.into() });
    }

    fn finish(&mut self, result: TaskResult) {
        self.ended = true;
        self.with(|e| {
            if e.view.result == TaskResult::Running {
                e.view.result = result;
                e.view.ended = Some(Timestamp::now());
                e.view.input = None;
                e.paste = None;
            }
        });
    }
}

impl Drop for TaskHandle {
    fn drop(&mut self) {
        if !self.ended {
            self.finish(TaskResult::Failed { text: STOPPED.to_string() });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_shape_and_parse() {
        let t = Tasks::default();
        let h = t.start(TaskKind::Test, "combo:fast").unwrap();
        let s = h.id().to_string();
        assert_eq!(s.len(), 19);
        assert_eq!(s.parse::<TaskId>().unwrap(), *h.id());
        assert!("tk_ABCDEFGHIJKLMNOP".parse::<TaskId>().is_err());
        assert!("tk_abc".parse::<TaskId>().is_err());
        assert!("xx_abcdefghijklmnop".parse::<TaskId>().is_err());
        assert!("tk_abcdefghijklmn01".parse::<TaskId>().is_err());
        assert_eq!(TaskKind::AdapterRebuild.as_str(), "adapter_rebuild");
    }

    #[test]
    fn an_owner_is_recorded() {
        let t = Tasks::default();
        let h = t.start(TaskKind::Test, "x").unwrap();
        assert_eq!(t.get(h.id()).unwrap().owner, None);
        t.set_owner(h.id(), "abc");
        assert_eq!(t.get(h.id()).unwrap().owner.as_deref(), Some("abc"));
    }

    #[test]
    fn random_ids_differ() {
        assert_ne!(TaskId::random().unwrap(), TaskId::random().unwrap());
    }

    #[test]
    fn fifth_running_is_busy_until_one_ends() {
        let t = Tasks::default();
        let hs: Vec<_> = (0..4).map(|i| t.start(TaskKind::Test, format!("s{i}")).unwrap()).collect();
        let err = t.start(TaskKind::Test, "x").err().unwrap();
        assert_eq!(err.to_string(), "busy: 4 tasks are running; try again when one ends");
        let mut hs = hs.into_iter();
        hs.next().unwrap().done("ok");
        assert!(t.start(TaskKind::Test, "x").is_ok());
    }

    #[test]
    fn transitions_and_progress() {
        let t = Tasks::default();
        let h = t.start(TaskKind::Signin, "a/b").unwrap();
        let id = h.id().clone();
        h.progress("one");
        h.progress("two");
        h.set_input(Some(TaskInput::Device { code: "C".into(), link: "L".into() }));
        let v = t.get(&id).unwrap();
        assert_eq!(v.progress, ["one", "two"]);
        assert_eq!(v.result, TaskResult::Running);
        assert!(v.ended.is_none() && v.input.is_some());
        h.fail("nope");
        let v = t.get(&id).unwrap();
        assert_eq!(v.result, TaskResult::Failed { text: "nope".into() });
        assert!(v.ended.is_some() && v.input.is_none());
    }

    #[test]
    fn dropped_handle_fails_the_task() {
        let t = Tasks::default();
        let h = t.start(TaskKind::Test, "x").unwrap();
        let id = h.id().clone();
        drop(h);
        assert_eq!(t.get(&id).unwrap().result, TaskResult::Failed { text: "the task stopped unexpectedly".into() });
        assert_eq!(t.list().len(), 1);
    }

    #[test]
    fn keeps_newest_200_and_never_evicts_running() {
        let t = Tasks::default();
        let long = t.start(TaskKind::Test, "long").unwrap();
        let long_id = long.id().clone();
        let mut last = None;
        for i in 0..250 {
            let h = t.start(TaskKind::Test, format!("t{i}")).unwrap();
            last = Some(h.id().clone());
            h.done("ok");
        }
        let list = t.list();
        assert_eq!(list.len(), KEEP);
        assert_eq!(list[0].id, last.unwrap());
        assert!(t.get(&long_id).is_some());
        assert_eq!(list.last().unwrap().id, long_id);
        assert_eq!(list[1].subject, "t248");
    }

    #[test]
    fn paste_reaches_only_a_waiting_task() {
        let t = Tasks::default();
        let h = t.start(TaskKind::Signin, "a/b").unwrap();
        let id = h.id().clone();
        let msg = format!("{id}: not waiting for a code");
        assert_eq!(t.paste(&id, SecretString::new("c")), Err(msg.clone()));
        let rx = h.paste_receiver();
        h.set_input(Some(TaskInput::Paste));
        t.paste(&id, SecretString::new("code123")).unwrap();
        assert!(rx.recv().unwrap().matches("code123"));
        assert!(!format!("{:?}", t.get(&id).unwrap()).contains("code123"));
        h.done("signed in");
        assert_eq!(t.paste(&id, SecretString::new("c")), Err(msg));
    }
}
