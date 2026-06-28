//! Native CLI-agent (Claude / Codex) session history operations.
//!
//! The daemon runs natively on the remote host, so it can scan, read and
//! mutate the agent session stores under `~/.claude` and `~/.codex` directly
//! via `std::fs` — one round trip per operation, with no remote Python.
//!
//! Every function here is a faithful port of the Python heredocs that used to
//! live in `app/src/workspace/environment_runtime.rs`. Field-extraction
//! fallbacks (`first_string`/`nested_string`/`clean_cwd`, codex id/title
//! resolution, `ensure_allowed`, the atomic index rewrite) match the Python
//! behaviour exactly because they parse real user session files.

#![cfg(feature = "local_fs")]

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

use crate::cli_agent_jsonl::{nested_string, parse_jsonl_values, sha256_hex};

/// Default number of records the scan returns (mirrors the Python `LIMIT`).
const DEFAULT_SCAN_LIMIT: usize = 40;

/// A scanned session record, mirroring the Python JSON rows.
pub struct ScannedSession {
    pub agent: &'static str,
    pub id: String,
    pub source: String,
    pub label: Option<String>,
    pub cwd: Option<String>,
    pub modified_epoch_millis: Option<i64>,
}

/// Result of reading a resolved session source.
pub struct ReadSession {
    pub reference: String,
    pub sha256: String,
    pub content: Vec<u8>,
}

/// Archive vs delete, mirroring `CliAgentSessionMutation`.
#[derive(Clone, Copy)]
pub enum Mutation {
    Archive,
    Delete,
}

fn home_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))
}

fn mtime_ms(path: &Path) -> i64 {
    std::fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Walk `root` (skipping symlinked directories), collect `*.jsonl` files, sort
/// by mtime descending, and return up to `limit` newest paths.
///
/// Mirrors Python `recent_jsonl`.
fn recent_jsonl(root: &Path, limit: usize) -> Vec<PathBuf> {
    let mut out: Vec<(i64, PathBuf)> = Vec::new();
    if !root.is_dir() {
        return Vec::new();
    }
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(read_dir) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in read_dir.flatten() {
            let path = entry.path();
            let file_type = match entry.file_type() {
                Ok(ft) => ft,
                Err(_) => continue,
            };
            if file_type.is_dir() {
                // Skip symlinked directories (Python prunes islink dirs).
                if file_type.is_symlink() {
                    continue;
                }
                stack.push(path);
            } else {
                let is_jsonl = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.ends_with(".jsonl"));
                // Python additionally requires os.path.isfile (follows symlinks).
                if is_jsonl && path.is_file() {
                    out.push((mtime_ms(&path), path));
                }
            }
        }
    }
    // Python sorts (mtime, path) tuples in reverse — descending by mtime, then path.
    out.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
    out.into_iter().take(limit).map(|(_, path)| path).collect()
}

/// Read the first `limit` non-empty JSONL lines of `path`, yielding parsed
/// JSON values (skipping unparseable lines). Mirrors `read_jsonl_prefix`.
fn read_jsonl_prefix(path: &Path, limit: usize) -> Vec<Value> {
    let Ok(bytes) = std::fs::read(path) else {
        return Vec::new();
    };
    // Python opens with errors="replace"; from_utf8_lossy matches that.
    parse_jsonl_values(&String::from_utf8_lossy(&bytes), Some(limit))
}

/// First non-empty string among the candidates. Mirrors `first_string`.
fn first_string(values: &[Option<&str>]) -> Option<String> {
    for value in values {
        if let Some(value) = value {
            if !value.trim().is_empty() {
                return Some((*value).to_owned());
            }
        }
    }
    None
}

fn str_field<'a>(item: &'a Value, key: &str) -> Option<&'a str> {
    item.get(key).and_then(Value::as_str)
}

/// Validate and normalise a candidate cwd. Mirrors Python `clean_cwd`:
/// strips remote:/ssh: values, requires an absolute existing directory, and
/// rejects directories inside the session stores themselves.
fn clean_cwd(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    if value.is_empty() {
        return None;
    }
    if value.starts_with("remote:") || value.starts_with("ssh:") {
        return None;
    }
    let expanded = expand_user(value);
    if !expanded.is_absolute() {
        return None;
    }
    if !expanded.is_dir() {
        return None;
    }
    let home = home_dir();
    let session_roots = [
        home.join(".claude"),
        home.join(".codex").join("sessions"),
        home.join(".codex").join("session_index.jsonl"),
    ];
    let real_value = real(&expanded);
    for root in &session_roots {
        let real_root = real(root);
        if real_value == real_root || real_value.starts_with(&path_with_sep(&real_root)) {
            return None;
        }
    }
    // Python returns the expanded (not realpath'd) path.
    Some(expanded.to_string_lossy().into_owned())
}

/// Extract a cwd from a session item with the full Python fallback chain.
fn cwd_from_item(item: &Value) -> Option<String> {
    clean_cwd(
        first_string(&[
            str_field(item, "cwd"),
            str_field(item, "working_dir"),
            str_field(item, "workingDirectory"),
            nested_string(item, &["turn_context", "cwd"]),
            nested_string(item, &["payload", "cwd"]),
            nested_string(item, &["metadata", "cwd"]),
            nested_string(item, &["session", "cwd"]),
        ])
        .as_deref(),
    )
}

fn codex_title_from_item(item: &Value) -> Option<String> {
    first_string(&[
        str_field(item, "title"),
        nested_string(item, &["turn_context", "title"]),
        nested_string(item, &["payload", "title"]),
        nested_string(item, &["metadata", "title"]),
    ])
}

/// 把首条用户消息压成一行短标题:取第一行非空内容,裁到 ~80 字符(UIREQ-014
/// fallback —— 会话无标题时用首句而不是光秃秃的 agent 名)。
fn first_message_excerpt(text: &str) -> Option<String> {
    let line = text.lines().map(str::trim).find(|line| !line.is_empty())?;
    const MAX_CHARS: usize = 80;
    let chars: Vec<char> = line.chars().collect();
    if chars.len() <= MAX_CHARS {
        Some(line.to_owned())
    } else {
        let head: String = chars[..MAX_CHARS].iter().collect();
        Some(format!("{}…", head.trim_end()))
    }
}

/// Codex 首条**真实**用户消息:`event_msg` / `user_message` 的 payload.message
/// (response_item 里 role:user 的那些是注入的 AGENTS.md / 权限说明,跳过)。
fn codex_user_message_from_item(item: &Value) -> Option<String> {
    if str_field(item, "type") != Some("event_msg") {
        return None;
    }
    if nested_string(item, &["payload", "type"]) != Some("user_message") {
        return None;
    }
    nested_string(item, &["payload", "message"]).and_then(first_message_excerpt)
}

/// Claude 首条用户消息:`type:user` 项的 `message.content`(字符串形态)。
fn claude_user_message_from_item(item: &Value) -> Option<String> {
    if str_field(item, "type") != Some("user") {
        return None;
    }
    nested_string(item, &["message", "content"]).and_then(first_message_excerpt)
}

fn codex_session_id_from_item(item: &Value) -> Option<String> {
    if str_field(item, "type") == Some("session_meta") {
        return first_string(&[
            nested_string(item, &["payload", "id"]),
            str_field(item, "session_id"),
            str_field(item, "sessionId"),
        ]);
    }
    first_string(&[
        str_field(item, "session_id"),
        str_field(item, "sessionId"),
        nested_string(item, &["payload", "id"]),
    ])
}

fn file_stem(path: &Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn claude_session(path: &Path) -> ScannedSession {
    let mut sid = file_stem(path);
    let mut cwd = None;
    let mut label: Option<String> = None;
    let mut first_user_message: Option<String> = None;
    for item in read_jsonl_prefix(path, 200) {
        if let Some(session_id) = str_field(&item, "sessionId").filter(|s| !s.is_empty()) {
            sid = session_id.to_owned();
        }
        if cwd.is_none() {
            cwd = cwd_from_item(&item);
        }
        if label.is_none() {
            label = str_field(&item, "aiTitle").map(str::to_owned);
        }
        if first_user_message.is_none() {
            first_user_message = claude_user_message_from_item(&item);
        }
        if cwd.is_some() && label.is_some() {
            break;
        }
    }
    // UIREQ-014:真标题优先;无标题用首条用户消息兜底;再没有就返回 None,
    // 交给展示层(restored_session_label)显示 agent 名。
    let label = label.or(first_user_message);
    ScannedSession {
        agent: "claude",
        id: sid,
        source: path.to_string_lossy().into_owned(),
        label,
        cwd,
        modified_epoch_millis: Some(mtime_ms(path)),
    }
}

fn codex_session(path: &Path) -> ScannedSession {
    let stem = file_stem(path);
    let mut sid = stem
        .strip_prefix("rollout-")
        .map(str::to_owned)
        .unwrap_or(stem);
    let mut cwd = None;
    let mut label: Option<String> = None;
    let mut first_user_message: Option<String> = None;
    for item in read_jsonl_prefix(path, 200) {
        if cwd.is_none() {
            cwd = cwd_from_item(&item);
        }
        if label.is_none() {
            label = codex_title_from_item(&item);
        }
        if first_user_message.is_none() {
            first_user_message = codex_user_message_from_item(&item);
        }
        if let Some(found) = codex_session_id_from_item(&item) {
            sid = found;
        }
        if cwd.is_some() && label.is_some() {
            break;
        }
    }
    // UIREQ-014:真标题优先;无标题用首条用户消息兜底;再没有交给展示层显示 agent 名。
    let label = label.or(first_user_message);
    ScannedSession {
        agent: "codex",
        id: sid,
        source: path.to_string_lossy().into_owned(),
        label,
        cwd,
        modified_epoch_millis: Some(mtime_ms(path)),
    }
}

/// Scan recent Claude/Codex sessions and return them oldest-first, trimmed to
/// the newest `limit`. Mirrors the Python scan heredoc end-to-end.
pub fn scan_sessions(limit: usize) -> Vec<ScannedSession> {
    let limit = if limit == 0 { DEFAULT_SCAN_LIMIT } else { limit };
    let home = home_dir();
    let mut sessions: Vec<ScannedSession> = Vec::new();

    for path in recent_jsonl(&home.join(".claude").join("projects"), limit) {
        sessions.push(claude_session(&path));
    }
    for path in recent_jsonl(&home.join(".codex").join("sessions"), limit) {
        sessions.push(codex_session(&path));
    }

    let index_path = home.join(".codex").join("session_index.jsonl");
    if index_path.is_file() {
        let index_path_str = index_path.to_string_lossy().into_owned();
        for item in read_jsonl_prefix(&index_path, limit * 2) {
            let sid = first_string(&[
                str_field(&item, "id"),
                str_field(&item, "session_id"),
            ]);
            if let Some(sid) = sid {
                let modified = item
                    .get("updated_at_unix_ms")
                    .and_then(Value::as_i64)
                    .unwrap_or_else(|| mtime_ms(&index_path));
                sessions.push(ScannedSession {
                    agent: "codex",
                    id: sid.clone(),
                    source: format!("{index_path_str}:{sid}"),
                    // session_index.jsonl 只有摘要、无完整消息,拿不到首句;
                    // 无标题时返回 None,展示层显示 agent 名(UIREQ-014)。
                    label: codex_title_from_item(&item),
                    cwd: cwd_from_item(&item),
                    modified_epoch_millis: Some(modified),
                });
            }
        }
    }

    // Sort ascending by modified_ms (Python `sorted(..., key=modified_ms or 0)`),
    // dedup by (agent, source|id), then keep the newest `limit`.
    sessions.sort_by_key(|s| s.modified_epoch_millis.unwrap_or(0));
    let mut seen = std::collections::HashSet::new();
    let mut out: Vec<ScannedSession> = Vec::new();
    for session in sessions {
        let dedup_source = if session.source.is_empty() {
            session.id.clone()
        } else {
            session.source.clone()
        };
        if !seen.insert((session.agent, dedup_source)) {
            continue;
        }
        out.push(session);
    }
    let skip = out.len().saturating_sub(limit);
    out.split_off(skip)
}

// ── Path allow-list + resolution ──────────────────────────────────

fn expand_user(path: &str) -> PathBuf {
    if path == "~" {
        return home_dir();
    }
    if let Some(stripped) = path.strip_prefix("~/") {
        return home_dir().join(stripped);
    }
    PathBuf::from(path)
}

/// `os.path.realpath(os.path.expanduser(path))`. Falls back to the expanded
/// (lexical) path when canonicalize fails (e.g. the path does not exist),
/// matching realpath's best-effort behaviour for missing leaves.
fn real(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn real_str(path: &str) -> PathBuf {
    real(&expand_user(path))
}

/// Append the platform path separator so prefix checks only match component
/// boundaries (mirrors `path.startswith(root + os.sep)`).
fn path_with_sep(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(std::path::MAIN_SEPARATOR_STR);
    PathBuf::from(s)
}

/// Validate that `path` is under a known agent session store. Mirrors the
/// Python `ensure_allowed`. Returns the realpath'd allowed path.
fn ensure_allowed(path: &str) -> Result<PathBuf, String> {
    let home = home_dir();
    let resolved = real_str(path);
    let allowed = [
        real(&home.join(".claude").join("projects")),
        real(&home.join(".codex").join("sessions")),
    ];
    let allowed_index = real(&home.join(".codex").join("session_index.jsonl"));
    if resolved == allowed_index {
        return Ok(resolved);
    }
    for root in &allowed {
        if resolved == *root || resolved.starts_with(&path_with_sep(root)) {
            return Ok(resolved);
        }
    }
    Err(format!(
        "refusing to mutate path outside known agent session stores: {}",
        resolved.display()
    ))
}

/// Split a `<codex_index_path>:<sid>` source into (index_path, sid) when the
/// path resolves to `~/.codex/session_index.jsonl`. Mirrors `split_index_source`.
fn split_index_source(source: &str) -> Option<(String, String)> {
    let (path, sid) = source.rsplit_once(':')?;
    if sid.trim().is_empty() {
        return None;
    }
    let home = home_dir();
    let codex_index = home.join(".codex").join("session_index.jsonl");
    if real_str(path) == real(&codex_index) {
        Some((path.to_owned(), sid.to_owned()))
    } else {
        None
    }
}

// ── Read path ─────────────────────────────────────────────────────

fn codex_session_id_for_file(path: &Path) -> String {
    let stem = file_stem(path);
    let fallback = stem
        .strip_prefix("rollout-")
        .map(str::to_owned)
        .unwrap_or(stem);
    for item in read_jsonl_prefix(path, 80) {
        if str_field(&item, "type") == Some("session_meta") {
            let sid = first_string(&[
                nested_string(&item, &["payload", "id"]),
                str_field(&item, "session_id"),
                str_field(&item, "sessionId"),
            ]);
            if let Some(sid) = sid {
                return sid;
            }
        }
    }
    fallback
}

/// Find the codex transcript whose session id matches `session_id` by scanning
/// `~/.codex/sessions` newest-first. Mirrors `find_codex_transcript`.
fn find_codex_transcript(session_id: &str) -> Option<PathBuf> {
    let root = home_dir().join(".codex").join("sessions");
    if !root.is_dir() {
        return None;
    }
    let mut candidates: Vec<(i64, PathBuf)> = Vec::new();
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        let Ok(read_dir) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in read_dir.flatten() {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                if file_type.is_symlink() {
                    continue;
                }
                stack.push(path);
            } else if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".jsonl"))
            {
                candidates.push((mtime_ms(&path), path));
            }
        }
    }
    candidates.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
    for (_, path) in candidates {
        if codex_session_id_for_file(&path) == session_id {
            return Some(path);
        }
    }
    None
}

/// Resolve a source to an allowed transcript path. Mirrors `source_path`.
fn resolve_source_path(source: &str) -> Result<PathBuf, String> {
    if let Some((path, sid)) = split_index_source(source) {
        ensure_allowed(&path)?;
        let transcript = find_codex_transcript(&sid)
            .ok_or_else(|| format!("Codex transcript not found for indexed session {sid}"))?;
        return ensure_allowed(&transcript.to_string_lossy());
    }
    ensure_allowed(source)
}

/// Resolve `source` and read the resulting transcript bytes. Mirrors the Python
/// read heredoc; `content` is returned raw (the proto carries bytes directly,
/// so no base64 wrapping is needed on the wire).
pub fn read_session(source: &str) -> Result<ReadSession, String> {
    let path = resolve_source_path(source)?;
    let content = std::fs::read(&path)
        .map_err(|err| format!("failed to read session file {}: {err}", path.display()))?;
    let sha256 = sha256_hex(&content);
    Ok(ReadSession {
        reference: path.to_string_lossy().into_owned(),
        sha256,
        content,
    })
}


// ── Mutate path ───────────────────────────────────────────────────

/// Allocate a non-colliding path next to `path`. Mirrors `unique_path`.
fn unique_path(path: &Path) -> Result<PathBuf, String> {
    if !path.exists() {
        return Ok(path.to_path_buf());
    }
    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (stem, ext) = match file_name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem.to_owned(), format!(".{ext}")),
        _ => (file_name.clone(), String::new()),
    };
    for index in 1..1000 {
        let candidate = parent.join(format!("{stem}-{index}{ext}"));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(format!(
        "could not allocate archive path for {}",
        path.display()
    ))
}

fn archive_dir_for(path: &Path) -> PathBuf {
    path.parent()
        .unwrap_or_else(|| Path::new(""))
        .join(".ashide-archive")
}

fn archive_path_for(path: &Path) -> Result<PathBuf, String> {
    let archive_dir = archive_dir_for(path);
    std::fs::create_dir_all(&archive_dir)
        .map_err(|err| format!("failed to create archive dir {}: {err}", archive_dir.display()))?;
    let file_name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    unique_path(&archive_dir.join(file_name))
}

/// Rewrite the codex index jsonl removing the entry with `sid`, archiving the
/// removed lines when `mutation == Archive`. Atomic via temp+rename. Mirrors
/// `mutate_index_entry`.
fn mutate_index_entry(path: &str, sid: &str, mutation: Mutation) -> Result<(), String> {
    let path = ensure_allowed(path)?;
    if !path.is_file() {
        return Err(format!("index file does not exist: {}", path.display()));
    }
    let bytes = std::fs::read(&path)
        .map_err(|err| format!("failed to read index {}: {err}", path.display()))?;
    let text = String::from_utf8_lossy(&bytes);

    let mut kept: Vec<String> = Vec::new();
    let mut removed: Vec<String> = Vec::new();
    // Preserve original line terminators by splitting inclusively.
    for line in split_keep_newlines(&text) {
        let trimmed = line.trim_end_matches(['\n', '\r']);
        match serde_json::from_str::<Value>(trimmed) {
            Ok(item) => {
                if str_field(&item, "id") == Some(sid) {
                    removed.push(line.to_owned());
                } else {
                    kept.push(line.to_owned());
                }
            }
            Err(_) => kept.push(line.to_owned()),
        }
    }

    if removed.is_empty() {
        return Ok(());
    }

    if matches!(mutation, Mutation::Archive) {
        let archive_dir = archive_dir_for(&path);
        std::fs::create_dir_all(&archive_dir).map_err(|err| {
            format!(
                "failed to create archive dir {}: {err}",
                archive_dir.display()
            )
        })?;
        let archive_path = unique_path(&archive_dir.join(format!("session_index-{sid}.jsonl")))?;
        std::fs::write(&archive_path, removed.concat()).map_err(|err| {
            format!(
                "failed to write archive {}: {err}",
                archive_path.display()
            )
        })?;
    }

    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let tmp = PathBuf::from(format!("{}.ashide.{now_ms}.tmp", path.display()));
    std::fs::write(&tmp, kept.concat())
        .map_err(|err| format!("failed to write temp index {}: {err}", tmp.display()))?;
    std::fs::rename(&tmp, &path)
        .map_err(|err| format!("failed to replace index {}: {err}", path.display()))?;
    Ok(())
}

/// Split text into lines while keeping the trailing newline on each line, so a
/// rewrite round-trips the original bytes (minus removed lines).
fn split_keep_newlines(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut start = 0;
    for (idx, &b) in bytes.iter().enumerate() {
        if b == b'\n' {
            out.push(&text[start..=idx]);
            start = idx + 1;
        }
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

/// Delete or archive a session file. Mirrors `mutate_file`.
fn mutate_file(source: &str, mutation: Mutation) -> Result<(), String> {
    let path = ensure_allowed(source)?;
    if !path.is_file() {
        return Ok(());
    }
    match mutation {
        Mutation::Delete => std::fs::remove_file(&path)
            .map_err(|err| format!("failed to delete {}: {err}", path.display())),
        Mutation::Archive => {
            let dest = archive_path_for(&path)?;
            std::fs::rename(&path, &dest).map_err(|err| {
                format!(
                    "failed to archive {} -> {}: {err}",
                    path.display(),
                    dest.display()
                )
            })
        }
    }
}

/// Archive or delete a session source (file or codex index entry). Mirrors the
/// top-level dispatch of the Python mutate heredoc.
pub fn mutate_session(source: &str, mutation: Mutation) -> Result<(), String> {
    if let Some((path, sid)) = split_index_source(source) {
        mutate_index_entry(&path, &sid, mutation)
    } else {
        mutate_file(source, mutation)
    }
}

#[cfg(test)]
mod uireq014_first_message_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn first_message_excerpt_takes_first_nonblank_line_and_truncates() {
        assert_eq!(
            first_message_excerpt("  \n\n  帮我看看这个 bug  \n more"),
            Some("帮我看看这个 bug".to_owned())
        );
        assert_eq!(first_message_excerpt("   \n  "), None);
        let excerpt = first_message_excerpt(&"x".repeat(200)).unwrap();
        assert!(excerpt.chars().count() <= 81, "{excerpt}");
        assert!(excerpt.ends_with('…'));
    }

    #[test]
    fn codex_user_message_only_from_event_msg_user_message() {
        // 真实用户 prompt(event_msg / user_message)。
        let prompt = json!({
            "type": "event_msg",
            "payload": { "type": "user_message", "message": "帮我 caffeinate 一下" }
        });
        assert_eq!(
            codex_user_message_from_item(&prompt),
            Some("帮我 caffeinate 一下".to_owned())
        );
        // response_item 里 role:user 的是注入的 AGENTS.md / 权限说明 —— 必须跳过。
        let injected = json!({
            "type": "response_item",
            "payload": { "type": "message", "role": "user",
                "content": [{ "type": "input_text", "text": "# AGENTS.md instructions" }] }
        });
        assert_eq!(codex_user_message_from_item(&injected), None);
        let non_user = json!({"type": "event_msg", "payload": {"type": "task_started"}});
        assert_eq!(codex_user_message_from_item(&non_user), None);
    }

    #[test]
    fn claude_user_message_from_type_user_content() {
        let item = json!({
            "type": "user",
            "message": { "role": "user", "content": "继续这个会话" }
        });
        assert_eq!(
            claude_user_message_from_item(&item),
            Some("继续这个会话".to_owned())
        );
        let assistant = json!({
            "type": "assistant",
            "message": { "role": "assistant", "content": "hi" }
        });
        assert_eq!(claude_user_message_from_item(&assistant), None);
    }
}
