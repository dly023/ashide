//! Shared JSONL primitives for reading Claude / Codex CLI-agent session files.
//!
//! Both consumers parse the same on-disk JSONL transcript/index formats:
//!
//! - [`crate::session_bridge::cli_agent_reader`] runs locally and parses a
//!   transcript into a full `SessionIr` (every user/assistant message).
//! - [`crate::environment_runtime_transport::cli_agent_sessions`] runs natively
//!   inside the daemon on the remote host and scans / reads / mutates the stores.
//!
//! These small, side-effect-free primitives are the genuinely shared surface,
//! so they live here once instead of being duplicated (and silently diverging)
//! on both sides. The higher-level extraction (full-content IR vs lightweight
//! scan metadata) and the path allow-listing stay per-context on purpose:
//! they encode different responsibilities and different security postures.

use serde_json::Value;
use sha2::{Digest, Sha256};

/// Parse JSONL `text` into values, skipping blank and unparseable lines.
///
/// `limit` bounds the number of *physical* lines consumed before filtering
/// (the daemon scan only needs a prefix); `None` consumes the whole text (the
/// reader needs every message). Blank/unparseable lines inside the consumed
/// window are dropped, so the result may be shorter than `limit`.
pub fn parse_jsonl_values(text: &str, limit: Option<usize>) -> Vec<Value> {
    let mut out = Vec::new();
    let mut consumed = 0usize;
    for line in text.lines() {
        if limit.is_some_and(|limit| consumed >= limit) {
            break;
        }
        consumed += 1;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<Value>(line) {
            out.push(value);
        }
    }
    out
}

/// Follow a key path through nested JSON objects to a non-empty string leaf.
pub fn nested_string<'a>(value: &'a Value, path: &[&str]) -> Option<&'a str> {
    let mut current = value;
    for key in path {
        current = current.get(*key)?;
    }
    current.as_str().filter(|text| !text.trim().is_empty())
}

/// Lowercase hex SHA-256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_jsonl_skips_blank_and_unparseable_lines() {
        let text = "\n{\"a\":1}\nnot json\n  {\"b\":2}  \n";
        let values = parse_jsonl_values(text, None);
        assert_eq!(values.len(), 2);
        assert_eq!(values[0]["a"], 1);
        assert_eq!(values[1]["b"], 2);
    }

    #[test]
    fn parse_jsonl_limit_counts_physical_lines() {
        // Blank lines count toward the physical-line limit (matches `.lines().take`).
        let text = "\n{\"a\":1}\n{\"b\":2}\n{\"c\":3}\n";
        let values = parse_jsonl_values(text, Some(2));
        // Lines 1 (blank) and 2 ({"a":1}) are consumed → only one value.
        assert_eq!(values.len(), 1);
        assert_eq!(values[0]["a"], 1);
    }

    #[test]
    fn nested_string_walks_objects_and_rejects_blank() {
        let value: Value = serde_json::json!({"payload": {"id": "abc", "blank": "  "}});
        assert_eq!(nested_string(&value, &["payload", "id"]), Some("abc"));
        assert_eq!(nested_string(&value, &["payload", "blank"]), None);
        assert_eq!(nested_string(&value, &["payload", "missing"]), None);
    }
}
