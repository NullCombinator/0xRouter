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

use serde::Deserialize;

/// Segments kept indexed; past this the cache starts over.
const KEPT: usize = 8;

static CACHE: Mutex<Option<HashMap<PathBuf, Arc<Mutex<Segment>>>>> = Mutex::new(None);

/// What the index needs of a line.
#[derive(Deserialize)]
struct Head<'a> {
    t: &'a str,
    id: &'a str,
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
