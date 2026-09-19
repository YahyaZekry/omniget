//! Incremental Server-Sent Events parser (bytes in, events out), plus the
//! newline-delimited JSON splitter Ollama's native `/api/chat` needs.
//! Owned by f2-llm-providers.
//!
//! Design constraints (prompt §7): the body is never materialised as one
//! `String`. Callers push the byte chunks `reqwest` hands them; the parser
//! keeps only the tail of the current incomplete line plus the data buffer of
//! the event being assembled. Line terminators follow the WHATWG SSE spec:
//! LF, CR or CRLF, with CRLF allowed to straddle two chunks.

use serde_json::Value;

use super::error::{LlmError, ERR_LLM_PARSE};

/// Hard ceiling for one line and for one event's `data` buffer. A hostile or
/// broken server must not be able to grow our memory without bound: past this
/// the line/event is dropped and the parser reports `ERR_LLM_PARSE`.
pub const MAX_LINE_BYTES: usize = 1024 * 1024;
pub const MAX_EVENT_BYTES: usize = 1024 * 1024;

/// Splits a byte stream into lines across chunk boundaries.
#[derive(Debug, Default)]
pub struct LineSplitter {
    /// Tail of the line that the last chunk left unfinished.
    buf: Vec<u8>,
    /// Last chunk ended on a bare CR: a leading LF in the next chunk is part
    /// of the same CRLF terminator and must not open an empty line.
    pending_lf: bool,
    /// A line blew the ceiling: swallow its bytes until the next terminator.
    dropping: bool,
    /// Sticky until read by `take_overflow`.
    overflowed: bool,
}

impl LineSplitter {
    pub fn new() -> Self {
        Self::default()
    }

    /// `true` once (and then cleared) if a line was dropped for being longer
    /// than `MAX_LINE_BYTES`.
    pub fn take_overflow(&mut self) -> bool {
        std::mem::take(&mut self.overflowed)
    }

    fn emit_line(&mut self, tail: &[u8], on_line: &mut impl FnMut(&[u8])) {
        if self.dropping {
            // The line that just ended was already over the ceiling.
            self.dropping = false;
            self.buf.clear();
            return;
        }
        if self.buf.len() + tail.len() > MAX_LINE_BYTES {
            self.overflowed = true;
            self.buf.clear();
            return;
        }
        if self.buf.is_empty() {
            on_line(tail);
        } else {
            self.buf.extend_from_slice(tail);
            on_line(&self.buf);
            self.buf.clear();
        }
    }

    pub fn push(&mut self, bytes: &[u8], mut on_line: impl FnMut(&[u8])) {
        let mut i = 0usize;
        if self.pending_lf {
            self.pending_lf = false;
            if bytes.first() == Some(&b'\n') {
                i = 1;
            }
        }
        let mut start = i;
        while i < bytes.len() {
            match bytes[i] {
                b'\n' => {
                    self.emit_line(&bytes[start..i], &mut on_line);
                    i += 1;
                    start = i;
                }
                b'\r' => {
                    self.emit_line(&bytes[start..i], &mut on_line);
                    i += 1;
                    if i < bytes.len() {
                        if bytes[i] == b'\n' {
                            i += 1;
                        }
                    } else {
                        self.pending_lf = true;
                    }
                    start = i;
                }
                _ => i += 1,
            }
        }
        if start < bytes.len() {
            let tail = &bytes[start..];
            if self.dropping {
                return;
            }
            if self.buf.len() + tail.len() > MAX_LINE_BYTES {
                // Stop buffering now instead of at the terminator, so a line
                // with no terminator at all cannot grow forever either.
                self.overflowed = true;
                self.dropping = true;
                self.buf.clear();
            } else {
                self.buf.extend_from_slice(tail);
            }
        }
    }

    /// Flushes a trailing line that the stream ended without terminating.
    pub fn finish(&mut self, mut on_line: impl FnMut(&[u8])) {
        if self.dropping {
            self.dropping = false;
            self.buf.clear();
        } else if !self.buf.is_empty() {
            on_line(&self.buf);
            self.buf.clear();
        }
        self.pending_lf = false;
    }
}

/// One dispatched SSE event.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SseEvent {
    /// `event:` field; `None` when the stream only sends `data:`
    /// (OpenAI-compatible providers) — the spec default is `message`.
    pub event: Option<String>,
    /// Concatenated `data:` lines, newline-joined, without the trailing LF.
    pub data: String,
    /// Last `id:` seen, which the spec keeps sticky across events.
    pub id: Option<String>,
    pub retry: Option<u64>,
}

impl SseEvent {
    /// OpenAI-compatible terminator.
    pub fn is_done(&self) -> bool {
        self.data.trim() == "[DONE]"
    }

    pub fn json(&self) -> Option<Value> {
        serde_json::from_str(&self.data).ok()
    }
}

/// Incremental SSE parser. Feed it chunks; it appends dispatched events to the
/// caller's buffer so a hot stream allocates no `Vec` per chunk.
#[derive(Debug, Default)]
pub struct SseParser {
    lines: LineSplitter,
    data: String,
    event: Option<String>,
    last_id: Option<String>,
    retry: Option<u64>,
    saw_data: bool,
    /// The event being assembled blew `MAX_EVENT_BYTES`: drop it whole.
    dropping: bool,
    /// Sticky until read by `take_overflow`.
    overflowed: bool,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, bytes: &[u8], out: &mut Vec<SseEvent>) {
        // Fields are moved out of `self` so the closure can borrow them while
        // `self.lines` is mutably borrowed by `push`.
        let data = &mut self.data;
        let event = &mut self.event;
        let last_id = &mut self.last_id;
        let retry = &mut self.retry;
        let saw_data = &mut self.saw_data;
        let dropping = &mut self.dropping;
        let overflowed = &mut self.overflowed;
        self.lines.push(bytes, |line| {
            if line.is_empty() {
                if *dropping {
                    // The event was over the ceiling: swallow it entirely.
                    *dropping = false;
                    *saw_data = false;
                    let _ = event.take();
                    data.clear();
                    return;
                }
                if *saw_data || event.is_some() {
                    if data.ends_with('\n') {
                        data.pop();
                    }
                    out.push(SseEvent {
                        event: event.take(),
                        data: std::mem::take(data),
                        id: last_id.clone(),
                        retry: *retry,
                    });
                }
                data.clear();
                *saw_data = false;
                return;
            }
            if line[0] == b':' {
                return; // comment / keep-alive
            }
            let (field, value) = match line.iter().position(|b| *b == b':') {
                Some(pos) => {
                    let mut v = &line[pos + 1..];
                    if v.first() == Some(&b' ') {
                        v = &v[1..];
                    }
                    (&line[..pos], v)
                }
                None => (line, &line[line.len()..]),
            };
            if *dropping {
                return;
            }
            match field {
                b"data" => {
                    if data.len() + value.len() + 1 > MAX_EVENT_BYTES {
                        *overflowed = true;
                        *dropping = true;
                        data.clear();
                        return;
                    }
                    data.push_str(&String::from_utf8_lossy(value));
                    data.push('\n');
                    *saw_data = true;
                }
                b"event" => *event = Some(String::from_utf8_lossy(value).into_owned()),
                b"id" => {
                    if !value.contains(&0) {
                        *last_id = Some(String::from_utf8_lossy(value).into_owned());
                    }
                }
                b"retry" => {
                    if let Ok(ms) = String::from_utf8_lossy(value).parse::<u64>() {
                        *retry = Some(ms);
                    }
                }
                _ => {}
            }
        });
    }

    /// Dispatches whatever a truncated stream left behind. Some servers close
    /// right after the last `data:` line without the blank line.
    pub fn finish(&mut self, out: &mut Vec<SseEvent>) {
        let mut tail: Vec<u8> = Vec::new();
        self.lines.finish(|line| tail.extend_from_slice(line));
        if !tail.is_empty() {
            tail.push(b'\n');
            tail.push(b'\n');
            self.push(&tail, out);
        } else if self.saw_data {
            self.push(b"\n", out);
        }
    }

    /// The parse error owed to the caller when a line or an event was dropped
    /// for blowing its ceiling. Reading it clears the flag, so a provider that
    /// checks after every `push` sees each overflow exactly once.
    pub fn take_overflow(&mut self) -> Option<LlmError> {
        let line = self.lines.take_overflow();
        let event = std::mem::take(&mut self.overflowed);
        if line || event {
            return Some(LlmError::new(
                ERR_LLM_PARSE,
                format!(
                    "SSE {} over the {} byte ceiling: dropped",
                    if line { "line" } else { "event" },
                    if line {
                        MAX_LINE_BYTES
                    } else {
                        MAX_EVENT_BYTES
                    }
                ),
            ));
        }
        None
    }

    /// Convenience for tests and one-shot bodies.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<SseEvent> {
        let mut out = Vec::new();
        self.push(bytes, &mut out);
        out
    }
}

/// Newline-delimited JSON, which Ollama's native `/api/chat` speaks instead of
/// SSE (`Content-Type: application/x-ndjson`, one object per line, no `data:`).
#[derive(Debug, Default)]
pub struct NdjsonParser {
    lines: LineSplitter,
}

impl NdjsonParser {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, bytes: &[u8], out: &mut Vec<Value>) {
        self.lines.push(bytes, |line| {
            if line.iter().all(|b| b.is_ascii_whitespace()) {
                return;
            }
            if let Ok(v) = serde_json::from_slice::<Value>(line) {
                out.push(v);
            }
        });
    }

    pub fn finish(&mut self, out: &mut Vec<Value>) {
        let mut tail: Vec<u8> = Vec::new();
        self.lines.finish(|line| tail.extend_from_slice(line));
        if !tail.is_empty() {
            if let Ok(v) = serde_json::from_slice::<Value>(&tail) {
                out.push(v);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines_of(chunks: &[&[u8]]) -> Vec<String> {
        let mut s = LineSplitter::new();
        let mut out: Vec<String> = Vec::new();
        for c in chunks {
            s.push(c, |l| out.push(String::from_utf8_lossy(l).into_owned()));
        }
        s.finish(|l| out.push(String::from_utf8_lossy(l).into_owned()));
        out
    }

    #[test]
    fn splits_lf_cr_and_crlf() {
        assert_eq!(lines_of(&[b"a\nb\r\nc\rd"]), ["a", "b", "c", "d"]);
    }

    #[test]
    fn crlf_split_across_chunks_is_one_terminator() {
        assert_eq!(lines_of(&[b"a\r", b"\nb\n"]), ["a", "b"]);
    }

    #[test]
    fn line_split_across_three_chunks() {
        assert_eq!(lines_of(&[b"he", b"ll", b"o\n"]), ["hello"]);
    }

    #[test]
    fn empty_lines_are_kept() {
        assert_eq!(lines_of(&[b"a\n\nb\n"]), ["a", "", "b"]);
    }

    #[test]
    fn parses_a_plain_data_event() {
        let ev = SseParser::new().feed(b"data: {\"a\":1}\n\n");
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].data, "{\"a\":1}");
        assert_eq!(ev[0].event, None);
        assert_eq!(ev[0].json().unwrap()["a"], 1);
    }

    #[test]
    fn named_events_carry_the_event_field() {
        let ev = SseParser::new().feed(
            b"event: content_block_delta\ndata: {\"i\":0}\n\nevent: message_stop\ndata: {}\n\n",
        );
        assert_eq!(ev.len(), 2);
        assert_eq!(ev[0].event.as_deref(), Some("content_block_delta"));
        assert_eq!(ev[1].event.as_deref(), Some("message_stop"));
    }

    #[test]
    fn multi_line_data_is_joined_with_newlines() {
        let ev = SseParser::new().feed(b"data: one\ndata: two\ndata:three\n\n");
        assert_eq!(ev[0].data, "one\ntwo\nthree");
    }

    #[test]
    fn comments_and_keepalives_are_ignored() {
        let ev = SseParser::new().feed(b": ping\n\n: keep-alive\ndata: x\n\n");
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].data, "x");
    }

    #[test]
    fn done_sentinel_is_detected() {
        let ev = SseParser::new().feed(b"data: [DONE]\n\n");
        assert!(ev[0].is_done());
        assert!(ev[0].json().is_none());
    }

    #[test]
    fn id_is_sticky_and_retry_parsed() {
        let ev = SseParser::new().feed(b"id: 7\nretry: 2500\ndata: a\n\ndata: b\n\n");
        assert_eq!(ev[0].id.as_deref(), Some("7"));
        assert_eq!(ev[0].retry, Some(2500));
        assert_eq!(ev[1].id.as_deref(), Some("7"));
    }

    #[test]
    fn field_without_colon_is_a_field_with_empty_value() {
        // A bare "data" line appends an empty string, so the event still fires.
        let ev = SseParser::new().feed(b"data\n\n");
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].data, "");
    }

    #[test]
    fn only_one_leading_space_is_stripped() {
        let ev = SseParser::new().feed(b"data:  x\n\n");
        assert_eq!(ev[0].data, " x");
    }

    #[test]
    fn byte_by_byte_feeding_matches_one_shot() {
        let body = b"event: a\ndata: {\"n\":1}\n\ndata: [DONE]\n\n";
        let one = SseParser::new().feed(body);
        let mut p = SseParser::new();
        let mut many = Vec::new();
        for b in body.iter() {
            p.push(&[*b], &mut many);
        }
        assert_eq!(one, many);
    }

    #[test]
    fn finish_dispatches_an_unterminated_event() {
        let mut p = SseParser::new();
        let mut out = p.feed(b"data: tail");
        assert!(out.is_empty());
        p.finish(&mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].data, "tail");
    }

    #[test]
    fn an_endless_line_is_dropped_and_reported_once() {
        let mut p = SseParser::new();
        let mut out = Vec::new();
        // 1.5 MiB of a single unterminated "data:" line, in 64 KiB chunks.
        let chunk = vec![b'x'; 64 * 1024];
        p.push(b"data: ", &mut out);
        for _ in 0..24 {
            p.push(&chunk, &mut out);
        }
        p.push(b"\n\n", &mut out);
        assert!(out.is_empty(), "the oversized event must not be dispatched");
        let err = p.take_overflow().expect("overflow must be reported");
        assert_eq!(err.code, ERR_LLM_PARSE);
        // Reading clears it; a second read is clean.
        assert!(p.take_overflow().is_none());
    }

    #[test]
    fn the_stream_keeps_working_after_a_dropped_line() {
        let mut p = SseParser::new();
        let mut out = Vec::new();
        p.push(b"data: ", &mut out);
        p.push(&vec![b'x'; MAX_LINE_BYTES + 1], &mut out);
        p.push(b"\n\ndata: {\"a\":1}\n\n", &mut out);
        assert!(p.take_overflow().is_some());
        assert_eq!(out.len(), 1, "the good event after it still arrives");
        assert_eq!(out[0].json().unwrap()["a"], 1);
    }

    /// Many short `data:` lines can also add up past the event ceiling.
    #[test]
    fn an_event_built_from_many_lines_is_capped_too() {
        let mut p = SseParser::new();
        let mut out = Vec::new();
        let line = format!("data: {}\n", "y".repeat(8 * 1024));
        for _ in 0..200 {
            p.push(line.as_bytes(), &mut out);
        }
        p.push(b"\n", &mut out);
        assert!(out.is_empty());
        assert!(p.take_overflow().is_some());
    }

    #[test]
    fn nothing_overflows_on_a_normal_stream() {
        let mut p = SseParser::new();
        let mut out = Vec::new();
        p.push(b"data: {\"a\":1}\n\ndata: [DONE]\n\n", &mut out);
        assert_eq!(out.len(), 2);
        assert!(p.take_overflow().is_none());
    }

    #[test]
    fn ndjson_splits_objects_per_line() {
        let mut p = NdjsonParser::new();
        let mut out = Vec::new();
        p.push(b"{\"a\":1}\n{\"a\":2}\n", &mut out);
        assert_eq!(out.len(), 2);
        assert_eq!(out[1]["a"], 2);
    }

    #[test]
    fn ndjson_tolerates_split_objects_and_missing_final_newline() {
        let mut p = NdjsonParser::new();
        let mut out = Vec::new();
        p.push(b"{\"a\":", &mut out);
        p.push(b"1}\n{\"a\":2}", &mut out);
        assert_eq!(out.len(), 1);
        p.finish(&mut out);
        assert_eq!(out.len(), 2);
    }
}
