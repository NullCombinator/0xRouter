//! Incremental framers for provider streams (research R5). Push-based: bytes go in as
//! they arrive, complete frames come out, and a frame split across chunks at any byte is
//! reassembled. Nothing is buffered beyond the current partial frame.

use memchr::memchr;
use zerorouter_registry::schema::Framing;

/// One provider frame. `data` is the frame's text: SSE `data:` lines joined with `\n`, an
/// NDJSON line, or one JSON array element.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub event: Option<String>,
    pub data: String,
}

impl Frame {
    /// The `data: [DONE]` sentinel of `sse_data_done` streams.
    pub fn is_done(&self) -> bool {
        self.data == "[DONE]"
    }
}

#[derive(Debug)]
pub struct Framer {
    framing: Framing,
    buf: Vec<u8>,
    /// Bytes of `buf` already scanned (SSE/NDJSON: up to the last newline).
    scanned: usize,
    sse: SseState,
    json: JsonState,
}

#[derive(Debug, Default)]
struct SseState {
    event: Option<String>,
    data: Option<String>,
}

#[derive(Debug, Default)]
struct JsonState {
    depth: u32,
    in_str: bool,
    escaped: bool,
    start: Option<usize>,
}

impl Framer {
    pub fn new(framing: Framing) -> Self {
        Self { framing, buf: Vec::new(), scanned: 0, sse: SseState::default(), json: JsonState::default() }
    }

    /// Feeds one chunk; returns the frames it completed.
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<Frame> {
        self.buf.extend_from_slice(chunk);
        let mut out = Vec::new();
        match self.framing {
            Framing::JsonArray => self.scan_json(&mut out),
            _ => self.scan_lines(&mut out),
        }
        out
    }

    /// End of stream: flushes a final frame that had no terminator.
    pub fn finish(&mut self) -> Vec<Frame> {
        let mut out = Vec::new();
        match self.framing {
            Framing::JsonArray => {}
            _ => {
                // After a scan, `buf` holds only the unterminated last line.
                if !self.buf.is_empty() {
                    let rest = std::mem::take(&mut self.buf);
                    self.line(trim_cr(&rest), &mut out);
                    self.scanned = 0;
                }
                if matches!(self.framing, Framing::SseNamed | Framing::SseData | Framing::SseDataDone) {
                    self.dispatch(&mut out);
                }
            }
        }
        out
    }

    fn scan_lines(&mut self, out: &mut Vec<Frame>) {
        let mut start = 0;
        while let Some(i) = memchr(b'\n', &self.buf[self.scanned..]) {
            let end = self.scanned + i;
            let line = trim_cr(&self.buf[start..end]).to_vec();
            self.line(&line, out);
            start = end + 1;
            self.scanned = start;
        }
        self.buf.drain(..start);
        self.scanned -= start;
        // The unscanned tail has no newline; the next feed resumes at its end.
        self.scanned = self.buf.len();
    }

    fn line(&mut self, line: &[u8], out: &mut Vec<Frame>) {
        let line = String::from_utf8_lossy(line);
        if self.framing == Framing::Ndjson {
            let t = line.trim();
            if !t.is_empty() {
                out.push(Frame { event: None, data: t.to_owned() });
            }
            return;
        }
        if line.is_empty() {
            return self.dispatch(out);
        }
        if line.starts_with(':') {
            return;
        }
        let (field, value) = match line.split_once(':') {
            Some((f, v)) => (f, v.strip_prefix(' ').unwrap_or(v)),
            None => (&*line, ""),
        };
        match field {
            "event" => self.sse.event = Some(value.to_owned()),
            "data" => match &mut self.sse.data {
                Some(d) => {
                    d.push('\n');
                    d.push_str(value);
                }
                None => self.sse.data = Some(value.to_owned()),
            },
            _ => {}
        }
    }

    fn dispatch(&mut self, out: &mut Vec<Frame>) {
        let event = self.sse.event.take();
        if let Some(data) = self.sse.data.take() {
            out.push(Frame { event, data });
        }
    }

    fn scan_json(&mut self, out: &mut Vec<Frame>) {
        let s = &mut self.json;
        let mut i = self.scanned;
        while i < self.buf.len() {
            let b = self.buf[i];
            if s.in_str {
                if s.escaped {
                    s.escaped = false;
                } else if b == b'\\' {
                    s.escaped = true;
                } else if b == b'"' {
                    s.in_str = false;
                }
            } else {
                match b {
                    b'"' => {
                        s.in_str = true;
                        if s.depth == 1 {
                            s.start = Some(i);
                        }
                    }
                    b'{' | b'[' => {
                        if s.depth == 1 {
                            s.start = Some(i);
                        }
                        s.depth += 1;
                    }
                    b'}' | b']' => {
                        s.depth = s.depth.saturating_sub(1);
                        if s.depth == 1
                            && let Some(st) = s.start.take()
                        {
                            let data = String::from_utf8_lossy(&self.buf[st..=i]).into_owned();
                            out.push(Frame { event: None, data });
                        }
                    }
                    _ => {}
                }
            }
            i += 1;
        }
        // Keep only the element in progress.
        let keep = s.start.unwrap_or(self.buf.len());
        self.buf.drain(..keep);
        if let Some(st) = &mut s.start {
            *st -= keep;
        }
        self.scanned = self.buf.len();
    }
}

fn trim_cr(line: &[u8]) -> &[u8] {
    line.strip_suffix(b"\r").unwrap_or(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all(framing: Framing, chunks: &[&[u8]]) -> Vec<Frame> {
        let mut f = Framer::new(framing);
        let mut out: Vec<Frame> = chunks.iter().flat_map(|c| f.feed(c)).collect();
        out.extend(f.finish());
        out
    }

    /// Every split point gives the same frames as one chunk.
    fn split_invariant(framing: Framing, src: &[u8]) -> Vec<Frame> {
        let whole = all(framing, &[src]);
        for i in 0..=src.len() {
            assert_eq!(all(framing, &[&src[..i], &src[i..]]), whole, "split at {i}");
        }
        let bytes: Vec<&[u8]> = src.chunks(1).collect();
        assert_eq!(all(framing, &bytes), whole, "byte by byte");
        whole
    }

    fn frame(event: Option<&str>, data: &str) -> Frame {
        Frame { event: event.map(str::to_owned), data: data.to_owned() }
    }

    #[test]
    fn named_sse_with_crlf_comments_and_multiline_data() {
        let src = b"event: message_start\r\ndata: {\"a\":1}\r\n\r\n: keepalive\n\nevent: ping\ndata: x\ndata: y\n\ndata:z\n\n";
        let got = split_invariant(Framing::SseNamed, src);
        assert_eq!(got, [frame(Some("message_start"), "{\"a\":1}"), frame(Some("ping"), "x\ny"), frame(None, "z")]);
    }

    #[test]
    fn data_sse_with_done_and_unterminated_tail() {
        let got = split_invariant(Framing::SseDataDone, b"data: {\"x\":\"\xc3\xa9\"}\n\ndata: [DONE]");
        assert_eq!(got, [frame(None, "{\"x\":\"é\"}"), frame(None, "[DONE]")]);
        assert!(got[1].is_done());
    }

    #[test]
    fn ndjson() {
        let got = split_invariant(Framing::Ndjson, b"{\"a\":1}\r\n\n{\"b\":2}");
        assert_eq!(got, [frame(None, "{\"a\":1}"), frame(None, "{\"b\":2}")]);
    }

    #[test]
    fn json_array_with_braces_in_strings() {
        let src = b"[{\"t\":\"a}\\\"[\"},\r\n{\"n\":[1,{\"x\":2}]}\n]";
        let got = split_invariant(Framing::JsonArray, src);
        assert_eq!(got, [frame(None, "{\"t\":\"a}\\\"[\"}"), frame(None, "{\"n\":[1,{\"x\":2}]}")]);
    }
}
