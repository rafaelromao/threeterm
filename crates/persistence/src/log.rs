//! Append-only NDJSON `transactions.log` with a SHA-256 digest chain.
//!
//! Each [`LogEntry`] carries a `previous_digest_hex` link to the prior
//! entry's `terminal_digest_hex` (or to [`EMPTY_LOG_DIGEST_HEX`] for the
//! first entry). The reader NEVER trusts an in-file `terminal_digest_hex`
//! value: it recomputes the digest from the canonical JSON encoding of
//! the entry (with `terminal_digest_hex` set to the empty string) and
//! compares against the stored value.

use serde::{Deserialize, Serialize};

use crate::manifest::sha256_hex;

/// Length of a hex-encoded digest (32 bytes = 64 hex chars).
pub const DIGEST_HEX_LEN: usize = 64;

/// The all-zero digest anchored as the chain's `previous_digest_hex`
/// before any entry has been appended.
pub const EMPTY_LOG_DIGEST_HEX: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";

/// One record in the append-only `transactions.log`. Carries the
/// `previous_digest_hex` link and a self-computed `terminal_digest_hex`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogEntry {
    pub log_index: u32,
    pub previous_digest_hex: String,
    pub terminal_digest_hex: String,
    pub feature_id: String,
    pub kind: String,
}

impl LogEntry {
    /// Construct a new entry. `terminal_digest_hex` is computed from
    /// the canonical encoding of the entry-with-blank-terminal so the
    /// stored value is exactly what a reader would recompute.
    pub fn new(log_index: u32, previous_digest_hex: &str, feature_id: &str, kind: &str) -> Self {
        let mut entry = LogEntry {
            log_index,
            previous_digest_hex: previous_digest_hex.to_string(),
            terminal_digest_hex: String::new(),
            feature_id: feature_id.to_string(),
            kind: kind.to_string(),
        };
        let terminal = compute_terminal(&entry);
        entry.terminal_digest_hex = terminal;
        entry
    }
}

/// Compute the terminal digest for `entry` from its canonical encoding
/// with `terminal_digest_hex` left blank.
pub fn compute_terminal(entry: &LogEntry) -> String {
    let mut cloned = entry.clone();
    cloned.terminal_digest_hex = String::new();
    let bytes = serde_json::to_vec(&cloned).expect("canonical entry serializes");
    sha256_hex(&bytes)
}

/// Verify every entry's `terminal_digest_hex` matches its self-recompute
/// AND that each entry's `previous_digest_hex` matches the prior entry's
/// `terminal_digest_hex` (or equals [`EMPTY_LOG_DIGEST_HEX`] for the first
/// entry). Returns the recomputed chain-terminal digest on success.
pub fn verify_chain(entries: &[LogEntry]) -> Result<String, LogError> {
    let mut prior_terminal = EMPTY_LOG_DIGEST_HEX.to_string();
    for (idx, entry) in entries.iter().enumerate() {
        if entry.log_index as usize != idx {
            return Err(LogError::LogBrokenLink {
                log_index: entry.log_index,
                detail: format!(
                    "log_index out of sequence: expected {idx}, found {}",
                    entry.log_index
                ),
            });
        }
        if entry.previous_digest_hex != prior_terminal {
            return Err(LogError::LogBrokenLink {
                log_index: entry.log_index,
                detail: format!("previous_digest_hex mismatch at entry {}", entry.log_index),
            });
        }
        let recomputed = compute_terminal(entry);
        if recomputed != entry.terminal_digest_hex {
            return Err(LogError::LogBrokenLink {
                log_index: entry.log_index,
                detail: "terminal_digest_hex failed self-recompute".to_string(),
            });
        }
        prior_terminal = entry.terminal_digest_hex.clone();
    }
    Ok(prior_terminal)
}

/// Failure modes reported by [`verify_chain`] and the bundle reader.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogError {
    /// The chain's `previous_digest_hex` link or self-recomputed
    /// `terminal_digest_hex` does not match. The bundle reader
    /// distinguishes this from a final-digest mismatch.
    LogBrokenLink { log_index: u32, detail: String },
    /// `transactions.log` is missing from the bundle root.
    LogMissing,
    /// A line in `transactions.log` was not parseable JSON.
    Malformed { line: usize, detail: String },
    /// The bundle reader could not access the file system.
    Io { detail: String },
}

impl std::fmt::Display for LogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LogError::LogBrokenLink { log_index, detail } => {
                write!(f, "log_broken_link at entry {log_index}: {detail}")
            }
            LogError::LogMissing => write!(f, "log_missing"),
            LogError::Malformed { line, detail } => {
                write!(f, "malformed log line {line}: {detail}")
            }
            LogError::Io { detail } => write!(f, "io error: {detail}"),
        }
    }
}

impl std::error::Error for LogError {}

/// Encode `entries` as the NDJSON contents of `transactions.log`. Each
/// entry is followed by `\n`; the final byte is `\n`. The bytes are
/// produced by `serde_json::to_vec` so the encoding uses BTreeMap-stable
/// keys.
pub fn encode_log(entries: &[LogEntry]) -> Vec<u8> {
    let mut out = Vec::new();
    for entry in entries {
        let line = serde_json::to_vec(entry).expect("entry serializes");
        out.extend_from_slice(&line);
        out.push(b'\n');
    }
    out
}

/// Decode the NDJSON contents of `transactions.log`. Empty content
/// (zero lines) is treated as an empty (but valid) log.
pub fn decode_log(bytes: &[u8]) -> Result<Vec<LogEntry>, LogError> {
    let mut entries = Vec::new();
    if bytes.is_empty() {
        return Ok(entries);
    }
    for (idx, line) in bytes.split(|b| *b == b'\n').enumerate() {
        if line.is_empty() {
            continue;
        }
        match serde_json::from_slice::<LogEntry>(line) {
            Ok(entry) => entries.push(entry),
            Err(err) => {
                return Err(LogError::Malformed {
                    line: idx,
                    detail: err.to_string(),
                });
            }
        }
    }
    Ok(entries)
}

/// In-memory equivalent of `transactions.log` — a vector of entries
/// plus the recomputed terminal digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransactionLog {
    pub entries: Vec<LogEntry>,
}

impl TransactionLog {
    /// An empty chain whose terminal digest is [`EMPTY_LOG_DIGEST_HEX`].
    pub fn empty() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Append a feature to the chain. Returns the chain's terminal digest
    /// after the append.
    pub fn append_feature(&mut self, feature_id: &str, kind: &str) -> String {
        let log_index = self.entries.len() as u32;
        let previous = match self.entries.last() {
            Some(last) => last.terminal_digest_hex.clone(),
            None => EMPTY_LOG_DIGEST_HEX.to_string(),
        };
        let entry = LogEntry::new(log_index, &previous, feature_id, kind);
        self.entries.push(entry);
        self.entries
            .last()
            .expect("entry present after push")
            .terminal_digest_hex
            .clone()
    }

    pub fn entries(&self) -> &[LogEntry] {
        &self.entries
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Re-verify the chain and return the terminal digest.
    pub fn terminal_digest_hex(&self) -> Result<String, LogError> {
        verify_chain(&self.entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_one_entry_links_to_empty_digest() {
        let mut log = TransactionLog::empty();
        let terminal = log.append_feature("box-1", "box");
        assert_eq!(log.entries().len(), 1);
        assert_eq!(log.entries()[0].log_index, 0);
        assert_eq!(log.entries()[0].previous_digest_hex, EMPTY_LOG_DIGEST_HEX);
        assert_eq!(log.entries()[0].terminal_digest_hex, terminal);
        assert_eq!(terminal.len(), DIGEST_HEX_LEN);
    }

    #[test]
    fn append_two_entries_links_chain() {
        let mut log = TransactionLog::empty();
        let first = log.append_feature("box-1", "box");
        let second = log.append_feature("box-2", "box");

        assert_eq!(log.entries().len(), 2);
        assert_eq!(log.entries()[1].log_index, 1);
        assert_eq!(log.entries()[1].previous_digest_hex, first);
        assert_eq!(log.entries()[1].terminal_digest_hex, second);
        assert_ne!(first, second);
    }

    #[test]
    fn encode_then_decode_round_trips() {
        let mut log = TransactionLog::empty();
        log.append_feature("box-1", "box");
        log.append_feature("box-2", "box");

        let bytes = encode_log(log.entries());
        let decoded = decode_log(&bytes).expect("decode succeeds");
        assert_eq!(decoded, log.entries().to_vec());
    }

    #[test]
    fn tampering_with_terminal_digest_is_detected() {
        let mut log = TransactionLog::empty();
        log.append_feature("box-1", "box");

        let mut cloned = log.entries()[0].clone();
        cloned.terminal_digest_hex = "ff".repeat(32);
        let entries = vec![cloned];

        let err = verify_chain(&entries).expect_err("tamper is detected");
        match err {
            LogError::LogBrokenLink { log_index, .. } => assert_eq!(log_index, 0),
            other => panic!("expected LogBrokenLink, got {other:?}"),
        }
    }

    #[test]
    fn empty_log_terminal_is_zero_digest() {
        let log = TransactionLog::empty();
        let terminal = log.terminal_digest_hex().expect("empty log verifies");
        assert_eq!(terminal, EMPTY_LOG_DIGEST_HEX);
    }
}
