//! Reads a JSONL file that another process appends to, from a saved byte
//! offset (A2). A last line without its newline yet is kept for the next
//! read; a file that got shorter (truncated or replaced) is read again from
//! the start, and the caller dedupes what it already counted.

use serde_json::Value;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// At most this much is read per call, so a huge backlog is taken in steps.
pub const MAX_READ: u64 = 8 * 1024 * 1024;

#[derive(Debug)]
pub struct JsonlTail {
    pub path: PathBuf,
    /// Bytes consumed (complete lines only).
    pub offset: u64,
    partial: Vec<u8>,
    /// The file was replaced or truncated since the last read.
    pub restarted: bool,
}

impl JsonlTail {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            offset: 0,
            partial: Vec::new(),
            restarted: false,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The complete new lines since the last call, parsed. Lines that are
    /// not JSON are skipped.
    pub fn read_new(&mut self) -> Vec<Value> {
        self.restarted = false;
        let Ok(mut file) = std::fs::File::open(&self.path) else {
            return Vec::new();
        };
        let len = file.metadata().map(|m| m.len()).unwrap_or(0);
        let consumed = self.offset + self.partial.len() as u64;
        if len < consumed {
            self.offset = 0;
            self.partial.clear();
            self.restarted = true;
        }
        let start = self.offset + self.partial.len() as u64;
        if len <= start || file.seek(SeekFrom::Start(start)).is_err() {
            return Vec::new();
        }
        let mut chunk = Vec::new();
        if file.take(MAX_READ).read_to_end(&mut chunk).is_err() {
            return Vec::new();
        }
        let mut buffer = std::mem::take(&mut self.partial);
        buffer.extend_from_slice(&chunk);
        let complete = match buffer.iter().rposition(|b| *b == b'\n') {
            Some(at) => at + 1,
            None => 0,
        };
        self.partial = buffer.split_off(complete);
        self.offset += complete as u64;
        buffer
            .split(|b| *b == b'\n')
            .filter(|line| !line.iter().all(u8::is_ascii_whitespace))
            .filter_map(|line| serde_json::from_slice(line).ok())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
    use std::io::Write;

    fn append(path: &Path, text: &str) {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        file.write_all(text.as_bytes()).unwrap();
    }

    #[test]
    fn lines_are_read_once_and_a_partial_last_line_waits() {
        let dir = TempDir::new("tail");
        let path = dir.join("s.jsonl");
        let mut tail = JsonlTail::new(&path);
        assert!(tail.read_new().is_empty(), "no file yet");
        append(&path, "{\"n\":1}\n{\"n\":2}\n{\"n\":");
        let got = tail.read_new();
        assert_eq!(got.len(), 2);
        assert_eq!(tail.offset, 16);
        assert!(
            tail.read_new().is_empty(),
            "nothing new, the partial line waits"
        );
        append(&path, "3}\nnot json\n\n{\"n\":4}\n");
        let got: Vec<i64> = tail
            .read_new()
            .iter()
            .map(|v| v["n"].as_i64().unwrap())
            .collect();
        assert_eq!(
            got,
            [3, 4],
            "the partial line is completed; junk is skipped"
        );
        assert!(!tail.restarted);
    }

    #[test]
    fn a_truncated_or_replaced_file_is_read_again_from_the_start() {
        let dir = TempDir::new("tail_trunc");
        let path = dir.join("s.jsonl");
        append(&path, "{\"n\":1}\n{\"n\":2}\n");
        let mut tail = JsonlTail::new(&path);
        assert_eq!(tail.read_new().len(), 2);
        std::fs::write(&path, "{\"n\":9}\n").unwrap();
        let got = tail.read_new();
        assert!(tail.restarted);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0]["n"], 9);
        assert_eq!(tail.offset, 8);
    }
}
