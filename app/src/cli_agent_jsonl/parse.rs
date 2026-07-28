//! JSONL parsing and session metadata extraction for CLI agents.

#[cfg(feature = "local_fs")]
use std::fs;
use std::io::{BufRead, BufReader, Read};
#[cfg(feature = "local_fs")]
use std::path::Path;

use chrono::DateTime;
use serde_json::Value;
use sha2::{Digest, Sha256};

#[cfg(feature = "local_fs")]
use super::error::CliAgentSessionScanError;

/// Returns the canonical Codex provider session UUID carried by a transcript,
/// index record, rollout filename, or explicit resume command.
///
/// Codex JSONL objects also carry message/tool-call ids such as `msg_*` and
/// `fc_*`. Those ids are runtime object identities, never resumable session
/// identities, so every discovery and command boundary must pass through this
/// function before persisting or using a Codex provider session id.
pub(crate) fn canonical_codex_session_id(candidate: &str) -> Option<String> {
    fn is_uuid_like(value: &str) -> bool {
        value.len() == 36
            && value.bytes().enumerate().all(|(index, byte)| match index {
                8 | 13 | 18 | 23 => byte == b'-',
                _ => byte.is_ascii_hexdigit(),
            })
    }

    let candidate = candidate.trim();
    if is_uuid_like(candidate) {
        return Some(candidate.to_owned());
    }
    let suffix_start = candidate.len().checked_sub(36)?;
    let suffix = candidate.get(suffix_start..)?;
    is_uuid_like(suffix).then(|| suffix.to_owned())
}

fn first_canonical_codex_session_id(candidates: &[Option<&str>]) -> Option<String> {
    candidates
        .iter()
        .flatten()
        .find_map(|candidate| canonical_codex_session_id(candidate))
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CliAgentSessionMetadata {
    pub session_id: Option<String>,
    pub cwd: Option<String>,
    pub title: Option<String>,
    pub first_user_message: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CodexSessionIndexRecord {
    pub(crate) session_id: String,
    pub(crate) cwd: Option<String>,
    pub(crate) title: Option<String>,
    pub(crate) updated_at_epoch_millis: Option<i64>,
}

impl CliAgentSessionMetadata {
    pub fn display_title(&self) -> Option<String> {
        self.title
            .clone()
            .or_else(|| self.first_user_message.clone())
    }
}

/// Parse JSONL `text` into values, skipping blank and unparseable lines.
///
/// `limit` bounds the number of *physical* lines consumed before filtering
/// (the daemon scan only needs a prefix); `None` consumes the whole text (the
/// reader needs every message). Blank/unparseable lines inside the consumed
/// window are dropped, so the result may be shorter than `limit`.
pub fn parse_jsonl_values(text: &str, limit: Option<usize>) -> Vec<Value> {
    let mut out = Vec::new();
    for (consumed, line) in text.lines().enumerate() {
        if limit.is_some_and(|limit| consumed >= limit) {
            break;
        }
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

pub(crate) fn first_string(values: &[Option<&str>]) -> Option<String> {
    values
        .iter()
        .flatten()
        .map(|value| value.trim())
        .find(|value| !value.is_empty())
        .map(str::to_owned)
}

pub(crate) fn string_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|text| !text.trim().is_empty())
}

fn cwd_candidate(value: &Value) -> Option<String> {
    first_string(&[
        string_field(value, "cwd"),
        string_field(value, "working_dir"),
        string_field(value, "workingDirectory"),
        nested_string(value, &["turn_context", "cwd"]),
        nested_string(value, &["payload", "cwd"]),
        nested_string(value, &["metadata", "cwd"]),
        nested_string(value, &["session", "cwd"]),
    ])
}

pub fn codex_title_from_item(value: &Value) -> Option<String> {
    first_string(&[
        string_field(value, "thread_name"),
        string_field(value, "title"),
        nested_string(value, &["turn_context", "title"]),
        nested_string(value, &["payload", "title"]),
        nested_string(value, &["metadata", "title"]),
    ])
}

/// 把首条真实用户消息压成一行短标题。
pub fn first_message_excerpt(text: &str) -> Option<String> {
    let line = text.lines().map(str::trim).find(|line| !line.is_empty())?;
    const MAX_CHARS: usize = 80;
    let mut chars = line.chars();
    let head = chars.by_ref().take(MAX_CHARS).collect::<String>();
    if chars.next().is_none() {
        Some(head)
    } else {
        Some(format!("{}…", head.trim_end()))
    }
}

pub fn codex_user_message_from_item(value: &Value) -> Option<String> {
    if string_field(value, "type") != Some("event_msg")
        || nested_string(value, &["payload", "type"]) != Some("user_message")
    {
        return None;
    }
    nested_string(value, &["payload", "message"]).and_then(first_message_excerpt)
}

pub fn claude_user_message_from_item(value: &Value) -> Option<String> {
    if string_field(value, "type") != Some("user") {
        return None;
    }
    nested_string(value, &["message", "content"]).and_then(first_message_excerpt)
}

fn codex_session_id(values: &[Value]) -> Option<String> {
    values
        .iter()
        .filter(|value| string_field(value, "type") == Some("session_meta"))
        .find_map(|value| {
            first_canonical_codex_session_id(&[
                nested_string(value, &["payload", "id"]),
                nested_string(value, &["payload", "session_id"]),
                nested_string(value, &["payload", "sessionId"]),
                string_field(value, "session_id"),
                string_field(value, "sessionId"),
            ])
        })
        .or_else(|| {
            values.iter().find_map(|value| {
                first_canonical_codex_session_id(&[
                    string_field(value, "session_id"),
                    string_field(value, "sessionId"),
                ])
            })
        })
}

pub fn codex_session_metadata(values: &[Value]) -> CliAgentSessionMetadata {
    let mut metadata = CliAgentSessionMetadata {
        session_id: codex_session_id(values),
        ..Default::default()
    };
    for value in values {
        if metadata.cwd.is_none() {
            metadata.cwd = cwd_candidate(value);
        }
        if metadata.title.is_none() {
            metadata.title = codex_title_from_item(value);
        }
        if metadata.first_user_message.is_none() {
            metadata.first_user_message = codex_user_message_from_item(value);
        }
    }
    metadata
}

pub(crate) fn codex_session_index_record(value: &Value) -> Option<CodexSessionIndexRecord> {
    let session_id = first_canonical_codex_session_id(&[
        string_field(value, "id"),
        string_field(value, "session_id"),
        string_field(value, "sessionId"),
    ])?;
    let updated_at_epoch_millis = value
        .get("updated_at_unix_ms")
        .and_then(Value::as_i64)
        .or_else(|| {
            string_field(value, "updated_at")
                .and_then(|updated_at| DateTime::parse_from_rfc3339(updated_at).ok())
                .map(|updated_at| updated_at.timestamp_millis())
        });
    Some(CodexSessionIndexRecord {
        session_id,
        cwd: cwd_candidate(value),
        title: codex_title_from_item(value),
        updated_at_epoch_millis,
    })
}

pub fn claude_session_metadata(values: &[Value]) -> CliAgentSessionMetadata {
    let mut metadata = CliAgentSessionMetadata::default();
    for value in values {
        if let Some(session_id) = string_field(value, "sessionId") {
            metadata.session_id = Some(session_id.to_owned());
        }
        if metadata.cwd.is_none() {
            metadata.cwd = cwd_candidate(value);
        }
        if metadata.title.is_none() {
            metadata.title = string_field(value, "aiTitle").map(str::to_owned);
        }
        if metadata.first_user_message.is_none() {
            metadata.first_user_message = claude_user_message_from_item(value);
        }
    }
    metadata
}

/// Lowercase hex SHA-256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

pub(crate) fn read_jsonl_prefix_values(
    path: &Path,
    byte_limit: usize,
) -> Result<Vec<Value>, CliAgentSessionScanError> {
    let mut file = fs::File::open(path)
        .map_err(|error| CliAgentSessionScanError::io(path, "读取 Omp session", error))?;
    let mut bytes = vec![0; byte_limit];
    let read = file
        .read(&mut bytes)
        .map_err(|error| CliAgentSessionScanError::io(path, "读取 Omp session prefix", error))?;
    bytes.truncate(read);
    Ok(parse_jsonl_values(&String::from_utf8_lossy(&bytes), None))
}

#[cfg(feature = "local_fs")]
pub(crate) fn read_jsonl_values_from_path(
    path: &Path,
    limit: Option<usize>,
) -> Result<Vec<Value>, CliAgentSessionScanError> {
    let file = fs::File::open(path)
        .map_err(|error| CliAgentSessionScanError::io(path, "读取 CLI-agent 会话文件", error))?;
    match limit {
        Some(limit) => read_jsonl_values_from_reader(BufReader::new(file), Some(limit), path),
        None => {
            let mut reader = BufReader::new(file);
            let mut text = String::new();
            reader.read_to_string(&mut text).map_err(|error| {
                CliAgentSessionScanError::io(path, "读取 CLI-agent 会话文件", error)
            })?;
            Ok(parse_jsonl_values(&text, None))
        }
    }
}

#[cfg(feature = "local_fs")]
pub(crate) fn read_jsonl_values_from_path_with_physical_line_limit(
    path: &Path,
    physical_limit: usize,
) -> Result<Vec<Value>, CliAgentSessionScanError> {
    let file = fs::File::open(path)
        .map_err(|error| CliAgentSessionScanError::io(path, "读取 Codex session index", error))?;
    let mut values = Vec::new();
    for (line_number, line) in BufReader::new(file).lines().enumerate() {
        if line_number >= physical_limit {
            return Err(CliAgentSessionScanError::discovery_candidate_limit(
                path,
                physical_limit,
            ));
        }
        let line = line.map_err(|error| {
            CliAgentSessionScanError::io(path, "读取 Codex session index", error)
        })?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<Value>(line) {
            values.push(value);
        }
    }
    Ok(values)
}

#[cfg(feature = "local_fs")]
fn read_jsonl_values_from_reader(
    reader: impl BufRead,
    limit: Option<usize>,
    path: &Path,
) -> Result<Vec<Value>, CliAgentSessionScanError> {
    let mut values = Vec::new();
    let mut consumed = 0;
    let mut lines = reader.lines();
    while limit.is_none_or(|limit| consumed < limit) {
        let Some(line) = lines.next() else {
            break;
        };
        consumed += 1;
        let line = line.map_err(|error| {
            CliAgentSessionScanError::io(path, "读取 CLI-agent 会话文件", error)
        })?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<Value>(line) {
            values.push(value);
        }
    }
    Ok(values)
}
