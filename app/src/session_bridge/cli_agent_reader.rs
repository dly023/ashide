use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use walkdir::WalkDir;

use crate::cli_agent_jsonl::{
    nested_string as shared_nested_string, parse_jsonl_values, sha256_hex,
};
use crate::terminal::cli_agent_session_index::CurrentAppCliAgentSessionSourceTarget;
use crate::terminal::CLIAgent;

use super::adapter_registry::session_bridge_adapter_for_agent;
use super::ashide_store::SessionBridgeImportSource;
use super::ir::{SessionIr, SessionMessageIr, SessionTimestamp};
use super::SessionBridgeError;

#[derive(Debug, Clone)]
pub(crate) struct CliAgentSessionSourceBytes {
    pub(crate) reference: String,
    pub(crate) sha256: String,
    pub(crate) bytes: Vec<u8>,
}

#[derive(Debug, Clone)]
pub(crate) struct CliAgentSessionReadResult {
    pub(crate) session: SessionIr,
    pub(crate) source: SessionBridgeImportSource,
}

pub(crate) fn read_current_app_cli_agent_session(
    target: CurrentAppCliAgentSessionSourceTarget,
    title: Option<String>,
    cwd: Option<String>,
) -> Result<CliAgentSessionReadResult, SessionBridgeError> {
    let agent = target
        .agent
        .ok_or_else(|| SessionBridgeError::InvalidImport {
            message: "indexed CLI session is missing agent metadata".to_owned(),
        })?;
    let provider_session_id = target
        .provider_session_id
        .filter(|id| !id.trim().is_empty())
        .ok_or_else(|| SessionBridgeError::InvalidImport {
            message: "indexed CLI session is missing provider session id".to_owned(),
        })?;
    let source = read_current_app_cli_agent_session_source(&target.source)?;
    parse_cli_agent_session_source_bytes(agent, provider_session_id, source, title, cwd)
}

pub(crate) fn parse_cli_agent_session_source_bytes(
    agent: CLIAgent,
    provider_session_id: String,
    source: CliAgentSessionSourceBytes,
    title: Option<String>,
    cwd: Option<String>,
) -> Result<CliAgentSessionReadResult, SessionBridgeError> {
    let session = parse_cli_agent_session_ir(
        agent,
        &provider_session_id,
        &source.reference,
        &source.bytes,
        title,
        cwd,
    )?;
    let source = SessionBridgeImportSource {
        source_session_id: provider_session_id,
        reference: source.reference,
        sha256: source.sha256,
    };
    Ok(CliAgentSessionReadResult { session, source })
}

fn read_current_app_cli_agent_session_source(
    source: &str,
) -> Result<CliAgentSessionSourceBytes, SessionBridgeError> {
    let path = if let Some((index_path, session_id)) = split_codex_index_source(source) {
        validate_current_app_session_source_path(&index_path)?;
        find_codex_session_path_by_id(&session_id).ok_or_else(|| {
            SessionBridgeError::InvalidImport {
                message: format!("Codex transcript not found for indexed session {session_id}"),
            }
        })?
    } else {
        let path = PathBuf::from(source);
        validate_current_app_session_source_path(&path)?
    };
    let bytes = fs::read(&path)?;
    let reference = path.canonicalize().unwrap_or(path).display().to_string();
    Ok(CliAgentSessionSourceBytes {
        reference,
        sha256: sha256_hex(&bytes),
        bytes,
    })
}

fn validate_current_app_session_source_path(path: &Path) -> Result<PathBuf, SessionBridgeError> {
    let home_dir = dirs::home_dir().ok_or_else(|| SessionBridgeError::InvalidImport {
        message: "home directory is unavailable".to_owned(),
    })?;
    let canonical_path = path.canonicalize()?;
    let allowed_roots = [
        home_dir.join(".claude/projects"),
        home_dir.join(".codex/sessions"),
    ];
    let codex_index = home_dir.join(".codex/session_index.jsonl");
    if codex_index
        .canonicalize()
        .ok()
        .is_some_and(|index| canonical_path == index)
    {
        return Ok(canonical_path);
    }
    let is_allowed = allowed_roots.iter().any(|root| {
        root.canonicalize()
            .ok()
            .is_some_and(|root| canonical_path == root || canonical_path.starts_with(root))
    });
    if !is_allowed {
        return Err(SessionBridgeError::InvalidImport {
            message: format!(
                "refusing to read CLI session outside known history stores: {}",
                canonical_path.display()
            ),
        });
    }
    Ok(canonical_path)
}

fn split_codex_index_source(source: &str) -> Option<(PathBuf, String)> {
    let (path, session_id) = source.rsplit_once(':')?;
    if session_id.trim().is_empty() {
        return None;
    }
    let home_dir = dirs::home_dir()?;
    let codex_index = home_dir.join(".codex/session_index.jsonl");
    let path = PathBuf::from(path);
    let path_matches_index = path
        .canonicalize()
        .ok()
        .zip(codex_index.canonicalize().ok())
        .is_some_and(|(path, index)| path == index);
    path_matches_index.then(|| (path, session_id.to_owned()))
}

fn find_codex_session_path_by_id(session_id: &str) -> Option<PathBuf> {
    let root = dirs::home_dir()?.join(".codex/sessions");
    if !root.is_dir() {
        return None;
    }
    let mut files = WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "jsonl"))
        .filter_map(|entry| {
            let modified = fs::metadata(entry.path()).ok()?.modified().ok()?;
            Some((modified, entry.path().to_path_buf()))
        })
        .collect::<Vec<_>>();
    files.sort_by(|left, right| right.0.cmp(&left.0));
    files.into_iter().find_map(|(_, path)| {
        codex_provider_session_id_for_file(&path)
            .filter(|id| id == session_id)
            .map(|_| path)
    })
}

fn codex_provider_session_id_for_file(path: &Path) -> Option<String> {
    let file_stem = path.file_stem()?.to_string_lossy().into_owned();
    let fallback = file_stem
        .strip_prefix("rollout-")
        .unwrap_or(&file_stem)
        .to_owned();
    let contents = fs::read_to_string(path).ok()?;
    for value in parse_jsonl_values(&contents, Some(80)) {
        if value.get("type").and_then(Value::as_str) != Some("session_meta") {
            continue;
        }
        if let Some(id) = nested_string(&value, &["payload", "id"]) {
            return Some(id);
        }
    }
    Some(fallback)
}

fn parse_cli_agent_session_ir(
    agent: CLIAgent,
    provider_session_id: &str,
    source_reference: &str,
    bytes: &[u8],
    title_override: Option<String>,
    cwd_override: Option<String>,
) -> Result<SessionIr, SessionBridgeError> {
    let adapter = session_bridge_adapter_for_agent(agent).ok_or_else(|| {
        SessionBridgeError::InvalidImport {
            message: format!(
                "{} has no registered SessionBridge adapter",
                agent.display_name()
            ),
        }
    })?;
    if !adapter.capabilities.can_read_cli_history {
        return Err(SessionBridgeError::InvalidImport {
            message: format!("{} session fork is not supported yet", adapter.label),
        });
    }
    let cli_reader = adapter
        .cli_reader
        .ok_or_else(|| SessionBridgeError::InvalidImport {
            message: format!("{} session fork is not supported yet", adapter.label),
        })?;
    cli_reader(
        provider_session_id,
        source_reference,
        bytes,
        title_override,
        cwd_override,
    )
}

pub(crate) fn parse_codex_session_ir(
    provider_session_id: &str,
    source_reference: &str,
    bytes: &[u8],
    title_override: Option<String>,
    cwd_override: Option<String>,
) -> Result<SessionIr, SessionBridgeError> {
    let mut session_id = provider_session_id.to_owned();
    let mut title = clean_optional(title_override);
    let mut fallback_title = None::<String>;
    let mut project_path = clean_optional(cwd_override);
    let mut messages = Vec::new();

    for value in parse_jsonl_values(&String::from_utf8_lossy(bytes), None) {
        if value.get("type").and_then(Value::as_str) == Some("session_meta") {
            if let Some(id) = nested_string(&value, &["payload", "id"]) {
                session_id = id;
            }
            if project_path.is_none() {
                project_path = nested_string(&value, &["payload", "cwd"]);
            }
            continue;
        }
        if value.get("type").and_then(Value::as_str) == Some("turn_context") {
            if project_path.is_none() {
                project_path = nested_string(&value, &["payload", "cwd"]);
            }
            if title.is_none() {
                title = nested_string(&value, &["payload", "summary"]);
            }
            continue;
        }
        if value.get("type").and_then(Value::as_str) != Some("response_item") {
            continue;
        }
        let Some(payload) = value.get("payload") else {
            continue;
        };
        if payload.get("type").and_then(Value::as_str) != Some("message") {
            continue;
        }
        let Some(role) = payload.get("role").and_then(Value::as_str) else {
            continue;
        };
        if role != "user" && role != "assistant" {
            continue;
        }
        let Some(text) = content_text(payload.get("content")) else {
            continue;
        };
        if role == "user" && fallback_title.is_none() {
            fallback_title = Some(title_from_text(&text));
        }
        messages.push(SessionMessageIr {
            role: role.to_owned(),
            text,
            timestamp: timestamp_from_value(value.get("timestamp")),
        });
    }

    finalize_cli_session_ir(
        "codex",
        session_id,
        source_reference,
        title.or(fallback_title),
        project_path,
        messages,
        "Codex",
    )
}

pub(crate) fn parse_claude_session_ir(
    provider_session_id: &str,
    source_reference: &str,
    bytes: &[u8],
    title_override: Option<String>,
    cwd_override: Option<String>,
) -> Result<SessionIr, SessionBridgeError> {
    let mut session_id = provider_session_id.to_owned();
    let mut title = clean_optional(title_override);
    let mut fallback_title = None::<String>;
    let mut project_path = clean_optional(cwd_override);
    let mut messages = Vec::new();

    for value in parse_jsonl_values(&String::from_utf8_lossy(bytes), None) {
        if let Some(id) = value.get("sessionId").and_then(Value::as_str) {
            session_id = id.to_owned();
        }
        if project_path.is_none() {
            project_path = value
                .get("cwd")
                .and_then(Value::as_str)
                .filter(|cwd| !cwd.trim().is_empty())
                .map(str::to_owned);
        }
        if value.get("type").and_then(Value::as_str) == Some("ai-title") && title.is_none() {
            title = value
                .get("aiTitle")
                .and_then(Value::as_str)
                .filter(|title| !title.trim().is_empty())
                .map(str::to_owned);
            continue;
        }
        if value.get("type").and_then(Value::as_str) == Some("last-prompt") && title.is_none() {
            title = value
                .get("lastPrompt")
                .and_then(Value::as_str)
                .filter(|title| !title.trim().is_empty())
                .map(str::to_owned);
            continue;
        }
        let Some(top_level_type) = value.get("type").and_then(Value::as_str) else {
            continue;
        };
        if top_level_type != "user" && top_level_type != "assistant" {
            continue;
        }
        let Some(message) = value.get("message") else {
            continue;
        };
        let Some(role) = message.get("role").and_then(Value::as_str) else {
            continue;
        };
        if role != "user" && role != "assistant" {
            continue;
        }
        let Some(text) = content_text(message.get("content")) else {
            continue;
        };
        if role == "user" && fallback_title.is_none() {
            fallback_title = Some(title_from_text(&text));
        }
        messages.push(SessionMessageIr {
            role: role.to_owned(),
            text,
            timestamp: timestamp_from_value(value.get("timestamp")),
        });
    }

    finalize_cli_session_ir(
        "claude",
        session_id,
        source_reference,
        title.or(fallback_title),
        project_path,
        messages,
        "Claude Code",
    )
}

fn finalize_cli_session_ir(
    source: &str,
    provider_session_id: String,
    source_reference: &str,
    title: Option<String>,
    project_path: Option<String>,
    messages: Vec<SessionMessageIr>,
    fallback_title: &str,
) -> Result<SessionIr, SessionBridgeError> {
    if messages.is_empty() {
        return Err(SessionBridgeError::InvalidImport {
            message: format!("no user/assistant messages found in {source} transcript"),
        });
    }
    let created_at = messages
        .first()
        .and_then(|message| message.timestamp.clone());
    let updated_at = messages
        .last()
        .and_then(|message| message.timestamp.clone());
    Ok(SessionIr {
        source: source.to_owned(),
        session_id: provider_session_id.clone(),
        title: title
            .and_then(|title| clean_optional(Some(title)))
            .unwrap_or_else(|| fallback_title.to_owned()),
        project_path,
        created_at,
        updated_at,
        messages,
        artifacts: Vec::new(),
        metadata: json!({
            "sessionBridge": {
                "operation": "read_cli_agent_history",
                "providerSessionId": provider_session_id,
                "sourceReference": source_reference,
                "source": source,
            }
        }),
    })
}

fn content_text(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(text) => clean_optional(Some(text.clone())),
        Value::Array(items) => {
            let parts = items
                .iter()
                .filter_map(content_part_text)
                .collect::<Vec<_>>();
            clean_optional(Some(parts.join("\n\n")))
        }
        Value::Object(_) => content_part_text(value?),
        Value::Null | Value::Bool(_) | Value::Number(_) => None,
    }
}

fn content_part_text(value: &Value) -> Option<String> {
    let object = value.as_object()?;
    let part_type = object.get("type").and_then(Value::as_str);
    match part_type {
        Some("text" | "input_text" | "output_text") | None => object
            .get("text")
            .and_then(Value::as_str)
            .filter(|text| !text.trim().is_empty())
            .map(str::to_owned),
        Some(_) => None,
    }
}

fn nested_string(value: &Value, path: &[&str]) -> Option<String> {
    shared_nested_string(value, path).map(str::to_owned)
}

fn timestamp_from_value(value: Option<&Value>) -> Option<SessionTimestamp> {
    match value? {
        Value::String(text) if !text.trim().is_empty() => {
            Some(SessionTimestamp::String(text.clone()))
        }
        Value::Number(number) => number
            .as_i64()
            .map(SessionTimestamp::Integer)
            .or_else(|| number.as_f64().map(SessionTimestamp::Float)),
        Value::Null | Value::Bool(_) | Value::Array(_) | Value::Object(_) | Value::String(_) => {
            None
        }
    }
}

fn title_from_text(text: &str) -> String {
    let mut title = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(80)
        .collect::<String>();
    if title.is_empty() {
        title = "Converted CLI session".to_owned();
    }
    title
}

fn clean_optional(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_codex_response_items_into_session_ir() {
        let bytes = br#"
{"timestamp":"2026-06-20T01:00:00Z","type":"session_meta","payload":{"id":"codex-1","cwd":"/repo"}}
{"timestamp":"2026-06-20T01:00:01Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"hello"}]}}
{"timestamp":"2026-06-20T01:00:02Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"world"}]}}
"#;
        let result = parse_cli_agent_session_source_bytes(
            CLIAgent::Codex,
            "codex-1".to_owned(),
            CliAgentSessionSourceBytes {
                reference: "/tmp/rollout.jsonl".to_owned(),
                sha256: "hash".to_owned(),
                bytes: bytes.to_vec(),
            },
            None,
            None,
        )
        .unwrap();

        assert_eq!(result.session.source, "codex");
        assert_eq!(result.session.project_path.as_deref(), Some("/repo"));
        assert_eq!(result.session.messages.len(), 2);
        assert_eq!(result.session.messages[0].role, "user");
        assert_eq!(result.session.messages[0].text, "hello");
        assert_eq!(result.session.messages[1].role, "assistant");
        assert_eq!(result.session.messages[1].text, "world");
        assert_eq!(result.source.source_session_id, "codex-1");
    }

    #[test]
    fn parses_claude_text_messages_and_skips_tool_results() {
        let bytes = br#"
{"type":"ai-title","aiTitle":"Claude title","sessionId":"claude-1"}
{"timestamp":"2026-06-20T01:00:01Z","type":"user","sessionId":"claude-1","cwd":"/repo","message":{"role":"user","content":"hello"}}
{"timestamp":"2026-06-20T01:00:02Z","type":"assistant","sessionId":"claude-1","message":{"role":"assistant","content":[{"type":"thinking","thinking":"x"},{"type":"text","text":"world"}]}}
{"timestamp":"2026-06-20T01:00:03Z","type":"user","sessionId":"claude-1","message":{"role":"user","content":[{"type":"tool_result","content":"skip"}]}}
"#;
        let result = parse_cli_agent_session_source_bytes(
            CLIAgent::Claude,
            "claude-1".to_owned(),
            CliAgentSessionSourceBytes {
                reference: "/tmp/claude.jsonl".to_owned(),
                sha256: "hash".to_owned(),
                bytes: bytes.to_vec(),
            },
            None,
            None,
        )
        .unwrap();

        assert_eq!(result.session.title, "Claude title");
        assert_eq!(result.session.messages.len(), 2);
        assert_eq!(result.session.messages[0].text, "hello");
        assert_eq!(result.session.messages[1].text, "world");
    }
}
