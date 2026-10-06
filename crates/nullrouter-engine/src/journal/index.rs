//! A read-time index of one record segment: where each request's lines are, so a page far back in
//! a long segment reads only the lines it shows (spec 008, research R6/R7).
//!
//! The index lives in memory only and writes nothing. A segment is append-only, so a cached index
//! is brought up to date by reading just the new lines; a segment that was replaced (a prune or
//! forget renames a new file into place) or shrank is indexed again from its start.

use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::os::unix::fs::{FileExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use serde::Deserialize;

use crate::clock;

/// Segments kept indexed; past this the cache starts over.
const KEPT: usize = 8;

static CACHE: Mutex<Option<HashMap<PathBuf, Arc<Mutex<Segment>>>>> = Mutex::new(None);

/// What the index needs of a line.
#[derive(Deserialize)]
pub(super) struct Head<'a> {
    pub t: &'a str,
    pub id: &'a str,
    /// The agent key's id; only an `open` line carries it.
    #[serde(borrow)]
    pub agent: Option<&'a str>,
    /// When the request arrived (RFC 3339); only an `open` line carries it.
    #[serde(borrow)]
    pub arrived: Option<&'a str>,
}

/// One request's lines in a segment.
#[derive(Default)]
pub struct Entry {
    /// Whether its `open` line was seen; without one the lines are strays and are skipped.
    pub opened: bool,
    /// `(offset, length)` of each line, with its newline, in write order.
    lines: Vec<(u64, u32)>,
}

/// A segment's index: every request by id, and how far the segment has been read.
#[derive(Default)]
pub struct Segment {
    ino: u64,
    /// Bytes indexed: the end of the last whole line read.
    len: u64,
    pub entries: BTreeMap<String, Entry>,
    /// The newest arrival of each agent that has a request in the segment (spec 009, research R9),
    /// as the time and as the `arrived` text the record carries, so a caller can show it as written.
    pub newest: BTreeMap<String, (SystemTime, String)>,
}

impl Segment {
    /// Reads the lines written since the last refresh.
    fn refresh(&mut self, file: &File) -> std::io::Result<()> {
        let meta = file.metadata()?;
        if meta.ino() != self.ino || meta.len() < self.len {
            *self = Segment { ino: meta.ino(), ..Segment::default() };
        }
        let mut reader = BufReader::new(file);
        reader.seek(SeekFrom::Start(self.len))?;
        let mut line = Vec::new();
        loop {
            line.clear();
            let n = reader.read_until(b'\n', &mut line)?;
            // A line without its newline is still being written; the next refresh takes it.
            if n == 0 || line.last() != Some(&b'\n') {
                return Ok(());
            }
            if let Ok(head) = serde_json::from_slice::<Head>(&line) {
                let entry = self.entries.entry(head.id.to_owned()).or_default();
                entry.opened |= head.t == "open";
                entry.lines.push((self.len, n as u32));
                if let (true, Some(agent), Some(text)) = (head.t == "open", head.agent, head.arrived)
                    && let Some(at) = clock::parse_rfc3339(text)
                {
                    let slot = self.newest.entry(agent.to_owned()).or_insert_with(|| (at, text.to_owned()));
                    if at > slot.0 {
                        *slot = (at, text.to_owned());
                    }
                }
            }
            self.len += n as u64;
        }
    }

    /// The text of a request's lines, as `fold` reads them.
    pub fn text(&self, file: &File, id: &str) -> String {
        let mut out = Vec::new();
        for &(at, len) in self.entries.get(id).map_or(&[][..], |e| &e.lines[..]) {
            let mut buf = vec![0u8; len as usize];
            if file.read_exact_at(&mut buf, at).is_ok() {
                out.extend_from_slice(&buf);
            }
        }
        String::from_utf8_lossy(&out).into_owned()
    }
}

/// Runs `f` on the segment's index, brought up to date, and on the segment opened for reading.
pub fn with<T>(path: &Path, f: impl FnOnce(&Segment, &File) -> T) -> Option<T> {
    let slot = {
        let mut cache = CACHE.lock().ok()?;
        let map = cache.get_or_insert_with(HashMap::new);
        if map.len() >= KEPT && !map.contains_key(path) {
            map.clear();
        }
        map.entry(path.to_owned()).or_default().clone()
    };
    let mut seg = slot.lock().ok()?;
    // One handle for both, so the offsets and the reads see the same file even if it is replaced.
    let file = File::open(path).ok()?;
    seg.refresh(&file).ok()?;
    Some(f(&seg, &file))
}

/// The newest arrival of each agent in the segment at `path`, from its index brought up to date.
/// `None` when the segment can't be read.
pub fn newest(path: &Path) -> Option<BTreeMap<String, (SystemTime, String)>> {
    with(path, |seg, _| seg.newest.clone())
}

#[cfg(test)]
mod tests {
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::time::Duration;

    use super::*;

    fn open(id: &str, agent: Option<&str>, arrived: &str) -> String {
        let agent = agent.map_or(String::new(), |a| format!(r#","agent":"{a}""#));
        format!("{{\"t\":\"open\",\"id\":\"{id}\",\"arrived\":\"{arrived}\"{agent}}}\n")
    }

    fn at(s: &str) -> SystemTime {
        clock::parse_rfc3339(s).unwrap()
    }

    fn append(path: &Path, text: &str) {
        OpenOptions::new().create(true).append(true).open(path).unwrap().write_all(text.as_bytes()).unwrap();
    }

    #[test]
    fn a_refresh_after_appends_updates_the_newest_arrival() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("2026-10-01.jsonl");
        append(&path, &open("rq_1", Some("ak_a"), "2026-10-01T08:00:00.000Z"));
        append(&path, &open("rq_2", Some("ak_b"), "2026-10-01T09:00:00.000Z"));
        let first = newest(&path).unwrap();
        assert_eq!(first["ak_a"].0, at("2026-10-01T08:00:00.000Z"));
        assert_eq!(first["ak_b"].0, at("2026-10-01T09:00:00.000Z"));
        append(&path, &open("rq_3", Some("ak_a"), "2026-10-01T10:00:30.250Z"));
        // A close line carries no agent and changes nothing.
        append(&path, "{\"t\":\"close\",\"id\":\"rq_1\"}\n");
        let second = newest(&path).unwrap();
        assert_eq!(second["ak_a"].0, at("2026-10-01T10:00:30Z") + Duration::from_millis(250));
        assert_eq!(second["ak_b"].0, at("2026-10-01T09:00:00Z"));
        assert_eq!(second["ak_a"].1, "2026-10-01T10:00:30.250Z", "the text is kept as written");
    }

    #[test]
    fn a_replaced_segment_recomputes_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("2026-10-02.jsonl");
        append(&path, &open("rq_1", Some("ak_a"), "2026-10-02T08:00:00.000Z"));
        append(&path, &open("rq_2", Some("ak_a"), "2026-10-02T09:00:00.000Z"));
        assert_eq!(newest(&path).unwrap()["ak_a"].0, at("2026-10-02T09:00:00Z"));
        // A forget or prune renames a new file into place: a new inode, shorter content.
        let next = dir.path().join("next.tmp");
        append(&next, &open("rq_1", Some("ak_a"), "2026-10-02T08:00:00.000Z"));
        std::fs::rename(&next, &path).unwrap();
        assert_eq!(newest(&path).unwrap()["ak_a"].0, at("2026-10-02T08:00:00Z"));
        let gone = dir.path().join("next.tmp");
        append(&gone, &open("rq_9", None, "2026-10-02T11:00:00.000Z"));
        std::fs::rename(&gone, &path).unwrap();
        assert!(newest(&path).unwrap().is_empty(), "the agent's records are gone");
    }

    #[test]
    fn an_agent_in_no_segment_has_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("2026-10-03.jsonl");
        append(&path, &open("rq_1", Some("ak_a"), "2026-10-03T08:00:00.000Z"));
        append(&path, &open("rq_2", None, "2026-10-03T09:00:00.000Z"));
        let found = newest(&path).unwrap();
        assert!(!found.contains_key("ak_other"));
        assert_eq!(found.len(), 1, "a request with no agent counts for nobody");
    }
}
