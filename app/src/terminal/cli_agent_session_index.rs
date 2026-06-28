//! Best-effort current-app index for CLI-agent history files.
//!
//! This turns already persisted Claude/Codex session metadata into Ashide
//! workspace-session rows without executing any provider resume command.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Utc};

use serde_json::Value;
use walkdir::WalkDir;

use crate::app_state::{CliAgentSessionOrigin, WorkspaceSessionKind, WorkspaceSessionSnapshot};
use crate::session_bridge::adapter_registry::session_bridge_adapters;
use crate::terminal::CLIAgent;

#[derive(Clone, Debug, Default, serde::Deserialize, serde::Serialize)]
struct SessionUserState {
    #[serde(default)]
    aliases: HashMap<String, String>,
    #[serde(default)]
    pinned: HashSet<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct CurrentAppCliAgentSessionSourceTarget {
    pub(crate) source: String,
    pub(crate) agent: Option<CLIAgent>,
    pub(crate) provider_session_id: Option<String>,
}

#[derive(Debug)]
struct CandidateFile {
    path: PathBuf,
    modified: SystemTime,
}

#[derive(Debug)]
pub(crate) struct IndexedSession {
    pub(crate) agent: CLIAgent,
    pub(crate) id: String,
    pub(crate) path: PathBuf,
    pub(crate) snapshot_id: Option<String>,
    pub(crate) cwd: Option<String>,
    pub(crate) label: Option<String>,
    pub(crate) command: String,
    pub(crate) modified: SystemTime,
}

pub(crate) fn scan_current_app_cli_agent_sessions(
    limit_per_agent: usize,
) -> Vec<WorkspaceSessionSnapshot> {
    if limit_per_agent == 0 {
        return Vec::new();
    }

    let mut sessions = Vec::new();
    if let Some(home_dir) = dirs::home_dir() {
        let scanned = session_bridge_adapters()
            .iter()
            .filter(|adapter| adapter.capabilities.can_scan_current_app_history)
            .filter_map(|adapter| {
                adapter
                    .current_app_scanner
                    .map(|scanner| (adapter, scanner))
            })
            .map(|(adapter, scanner)| {
                let sessions = scanner(&home_dir, limit_per_agent);
                log::info!(
                    "Session Navigator current-app scan found {} {} sessions",
                    sessions.len(),
                    adapter.label
                );
                sessions
            })
            .collect::<Vec<_>>();
        for agent_sessions in scanned {
            sessions.extend(agent_sessions);
        }
        log::info!(
            "Session Navigator current-app scan found {} registered SessionBridge sessions",
            sessions.len()
        );
    } else {
        log::warn!("Session Navigator current-app scan skipped: home directory unavailable");
    }

    sessions.sort_by(|left, right| right.modified.cmp(&left.modified));
    let pinned_session_ids = pinned_session_ids();
    sessions
        .into_iter()
        .map(|session| {
            let id = session
                .snapshot_id
                .unwrap_or_else(|| external_session_snapshot_id(session.agent, &session.path));
            let mut snapshot = WorkspaceSessionSnapshot {
                id: id.clone(),
                kind: WorkspaceSessionKind::AgentTerminal,
                label: session.label,
                environment_authority_key: Some("local".to_owned()),
                cwd: session.cwd,
                startup_directory: None,
                cli_agent: Some(session.agent.to_serialized_name()),
                cli_command: Some(session.command),
                cli_agent_origin: Some(CliAgentSessionOrigin::PluginObserved),
                conversation_ids: Vec::new(),
                active_conversation_id: None,
                cli_agent_session_id: Some(session.id),
                is_active: false,
                is_pinned: false,
                updated_at_unix_ms: system_time_to_unix_ms(session.modified),
            };
            snapshot.is_pinned = snapshot.is_pinned_by(&pinned_session_ids);
            snapshot
        })
        .collect()
}

pub(crate) fn delete_current_app_cli_agent_session(snapshot_id: &str) -> Result<(), String> {
    if snapshot_id.starts_with("external-index:") {
        return delete_codex_session_index_entry(snapshot_id);
    }

    let path = path_from_external_session_snapshot_id(snapshot_id)
        .ok_or_else(|| format!("not an indexed CLI agent session id: {snapshot_id}"))?;
    let session_path = validate_mutable_session_path(&path)?;
    fs::remove_file(&session_path)
        .map_err(|error| format!("failed to delete {}: {error}", session_path.display()))?;
    set_session_pinned(snapshot_id, false)
}

pub(crate) fn current_app_cli_agent_session_source_target_from_id(
    snapshot_id: &str,
    cli_agent: Option<&str>,
    provider_session_id: Option<String>,
) -> Option<CurrentAppCliAgentSessionSourceTarget> {
    if let Some(path) = path_from_external_session_snapshot_id(snapshot_id) {
        let mut parts = snapshot_id.split(':');
        let _external = parts.next()?;
        let encoded_agent = parts.next()?;
        let agent = cli_agent
            .map(CLIAgent::from_serialized_name)
            .filter(|agent| !matches!(agent, CLIAgent::Unknown))
            .or_else(|| {
                let agent = CLIAgent::from_serialized_name(encoded_agent);
                (!matches!(agent, CLIAgent::Unknown)).then_some(agent)
            });
        return Some(CurrentAppCliAgentSessionSourceTarget {
            source: path.display().to_string(),
            agent,
            provider_session_id,
        });
    }

    let (agent, session_id) = session_id_from_external_index_session_snapshot_id(snapshot_id)?;
    let home_dir = dirs::home_dir()?;
    Some(CurrentAppCliAgentSessionSourceTarget {
        source: format!(
            "{}:{}",
            home_dir.join(".codex/session_index.jsonl").display(),
            session_id
        ),
        agent: Some(agent),
        provider_session_id: provider_session_id.or(Some(session_id)),
    })
}

pub(crate) fn session_aliases() -> HashMap<String, String> {
    read_session_user_state().aliases
}

pub(crate) fn set_session_alias(key: &str, alias: Option<&str>) -> Result<(), String> {
    let key = key.trim();
    if key.is_empty() {
        return Err("session alias key is empty".to_owned());
    }

    let mut state = read_session_user_state();
    match alias.map(str::trim).filter(|alias| !alias.is_empty()) {
        Some(alias) => {
            state.aliases.insert(key.to_owned(), alias.to_owned());
        }
        None => {
            state.aliases.remove(key);
        }
    }
    write_session_user_state(&state)
}

pub(crate) fn pinned_session_ids() -> HashSet<String> {
    read_session_user_state().pinned
}

pub(crate) fn set_session_pinned(session_id: &str, pinned: bool) -> Result<(), String> {
    let session_id = session_id.trim();
    if session_id.is_empty() {
        return Err("pinned session id is empty".to_owned());
    }

    let mut state = read_session_user_state();
    if pinned {
        state.pinned.insert(session_id.to_owned());
    } else {
        state.pinned.remove(session_id);
    }
    write_session_user_state(&state)
}

fn system_time_to_unix_ms(time: SystemTime) -> Option<i64> {
    let duration = time.duration_since(UNIX_EPOCH).ok()?;
    i64::try_from(duration.as_millis()).ok()
}

fn session_user_state_path() -> Option<PathBuf> {
    warp_core::paths::warp_home_config_dir().map(|config_dir| config_dir.join("session_state.json"))
}

fn read_session_user_state() -> SessionUserState {
    let Some(path) = session_user_state_path() else {
        return SessionUserState::default();
    };
    let Ok(contents) = fs::read_to_string(path) else {
        return SessionUserState::default();
    };
    serde_json::from_str::<SessionUserState>(&contents)
        .map(sanitize_session_user_state)
        .unwrap_or_default()
}

fn sanitize_session_user_state(mut state: SessionUserState) -> SessionUserState {
    state.aliases = state
        .aliases
        .into_iter()
        .filter_map(|(key, alias)| {
            let key = key.trim().to_owned();
            let alias = alias.trim().to_owned();
            (!key.is_empty() && !alias.is_empty()).then_some((key, alias))
        })
        .collect();
    state.pinned = state
        .pinned
        .into_iter()
        .map(|key| key.trim().to_owned())
        .filter(|key| !key.is_empty())
        .collect();
    state
}

fn write_session_user_state(state: &SessionUserState) -> Result<(), String> {
    let Some(path) = session_user_state_path() else {
        return Err("home directory is unavailable".to_owned());
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    }
    let contents = serde_json::to_string_pretty(&sanitize_session_user_state(state.clone()))
        .map_err(|error| format!("failed to encode session user state: {error}"))?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, contents)
        .map_err(|error| format!("failed to write {}: {error}", tmp.display()))?;
    fs::rename(&tmp, &path)
        .map_err(|error| format!("failed to replace {}: {error}", path.display()))
}

pub(crate) fn scan_claude_sessions(home_dir: &Path, limit: usize) -> Vec<IndexedSession> {
    let root = home_dir.join(".claude/projects");
    recent_jsonl_files(&root, limit)
        .into_iter()
        .filter_map(|file| parse_claude_session(&file.path, file.modified))
        .collect()
}

pub(crate) fn scan_codex_sessions(home_dir: &Path, limit: usize) -> Vec<IndexedSession> {
    let root = home_dir.join(".codex/sessions");
    let mut sessions = recent_jsonl_files(&root, limit)
        .into_iter()
        .filter_map(|file| parse_codex_session(&file.path, file.modified))
        .collect::<Vec<_>>();
    // Keep both physical rollout JSONL files and session_index rows. The UI
    // deduplicates them into one logical session, while archive/delete needs
    // every backing source so the same session does not reappear after refresh.
    sessions.extend(parse_codex_session_index(
        &home_dir.join(".codex/session_index.jsonl"),
        limit,
    ));
    sessions.sort_by(|left, right| right.modified.cmp(&left.modified));
    sessions.truncate(limit);
    sessions
}

fn recent_jsonl_files(root: &Path, limit: usize) -> Vec<CandidateFile> {
    if !root.is_dir() {
        return Vec::new();
    }

    let mut files = WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "jsonl"))
        .filter_map(|entry| {
            let metadata = fs::metadata(entry.path()).ok()?;
            Some(CandidateFile {
                path: entry.path().to_path_buf(),
                modified: metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            })
        })
        .collect::<Vec<_>>();

    files.sort_by(|left, right| right.modified.cmp(&left.modified));
    files.truncate(limit);
    files
}

fn parse_claude_session(path: &Path, modified: SystemTime) -> Option<IndexedSession> {
    let file = File::open(path).ok()?;
    let mut id = path.file_stem()?.to_string_lossy().into_owned();
    let mut cwd = None;
    let mut label = None;

    for line in BufReader::new(file).lines().map_while(Result::ok).take(200) {
        let value: Value = serde_json::from_str(&line).ok()?;
        if let Some(session_id) = value.get("sessionId").and_then(Value::as_str) {
            id = session_id.to_owned();
        }
        if cwd.is_none() {
            cwd = value
                .get("cwd")
                .and_then(Value::as_str)
                .filter(|cwd| !cwd.trim().is_empty())
                .map(str::to_owned);
        }
        if label.is_none() {
            label = value
                .get("aiTitle")
                .and_then(Value::as_str)
                .filter(|title| !title.trim().is_empty())
                .map(str::to_owned);
        }
        if cwd.is_some() && label.is_some() {
            break;
        }
    }

    Some(IndexedSession {
        agent: CLIAgent::Claude,
        id,
        path: path.to_path_buf(),
        snapshot_id: None,
        cwd,
        label,
        command: CLIAgent::Claude.command_prefix().to_owned(),
        modified,
    })
}

fn parse_codex_session(path: &Path, modified: SystemTime) -> Option<IndexedSession> {
    let file = File::open(path).ok()?;
    let file_stem = path.file_stem()?.to_string_lossy().into_owned();
    let mut id = file_stem
        .strip_prefix("rollout-")
        .unwrap_or(&file_stem)
        .to_owned();
    let mut cwd = None;

    for line in BufReader::new(file).lines().map_while(Result::ok).take(40) {
        let value: Value = serde_json::from_str(&line).ok()?;
        if value.get("type").and_then(Value::as_str) != Some("session_meta") {
            continue;
        }
        let Some(payload) = value.get("payload") else {
            continue;
        };
        if let Some(session_id) = payload.get("id").and_then(Value::as_str) {
            id = session_id.to_owned();
        }
        cwd = payload
            .get("cwd")
            .and_then(Value::as_str)
            .filter(|cwd| !cwd.trim().is_empty())
            .map(str::to_owned);
        break;
    }

    Some(IndexedSession {
        agent: CLIAgent::Codex,
        id,
        path: path.to_path_buf(),
        snapshot_id: None,
        cwd,
        label: Some("Codex".to_owned()),
        command: CLIAgent::Codex.command_prefix().to_owned(),
        modified,
    })
}

fn parse_codex_session_index(path: &Path, limit: usize) -> Vec<IndexedSession> {
    let Ok(file) = File::open(path) else {
        return Vec::new();
    };
    let fallback_modified = fs::metadata(path)
        .ok()
        .and_then(|metadata| metadata.modified().ok())
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let mut sessions = BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter_map(|line| parse_codex_session_index_line(path, &line, fallback_modified))
        .collect::<Vec<_>>();
    sessions.sort_by(|left, right| right.modified.cmp(&left.modified));
    sessions.truncate(limit);
    sessions
}

fn parse_codex_session_index_line(
    path: &Path,
    line: &str,
    fallback_modified: SystemTime,
) -> Option<IndexedSession> {
    let value: Value = serde_json::from_str(line).ok()?;
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.trim().is_empty())?
        .to_owned();
    let label = value
        .get("thread_name")
        .and_then(Value::as_str)
        .filter(|title| !title.trim().is_empty())
        .map(str::to_owned)
        .or_else(|| Some("Codex".to_owned()));
    let modified = value
        .get("updated_at")
        .and_then(Value::as_str)
        .and_then(|updated_at| DateTime::parse_from_rfc3339(updated_at).ok())
        .map(|updated_at| updated_at.with_timezone(&Utc).into())
        .unwrap_or(fallback_modified);

    Some(IndexedSession {
        agent: CLIAgent::Codex,
        id: id.clone(),
        path: path.to_path_buf(),
        snapshot_id: Some(external_index_session_snapshot_id(CLIAgent::Codex, &id)),
        cwd: None,
        label,
        command: CLIAgent::Codex.command_prefix().to_owned(),
        modified,
    })
}

pub(crate) fn external_session_snapshot_id_for_path(agent: CLIAgent, path: &Path) -> String {
    external_session_snapshot_id(agent, path)
}

fn external_session_snapshot_id(agent: CLIAgent, path: &Path) -> String {
    format!(
        "external:{}:{}",
        agent.to_serialized_name(),
        hex_encode(path.to_string_lossy().as_bytes())
    )
}

fn external_index_session_snapshot_id(agent: CLIAgent, session_id: &str) -> String {
    format!(
        "external-index:{}:{}",
        agent.to_serialized_name(),
        hex_encode(session_id.as_bytes())
    )
}

fn session_id_from_external_index_session_snapshot_id(
    snapshot_id: &str,
) -> Option<(CLIAgent, String)> {
    let mut parts = snapshot_id.split(':');
    if parts.next()? != "external-index" {
        return None;
    }
    let agent = CLIAgent::from_serialized_name(parts.next()?);
    if matches!(agent, CLIAgent::Unknown) {
        return None;
    }
    let encoded_id = parts.next()?;
    let bytes = hex_decode(encoded_id)?;
    Some((agent, String::from_utf8(bytes).ok()?))
}

fn path_from_external_session_snapshot_id(snapshot_id: &str) -> Option<PathBuf> {
    let mut parts = snapshot_id.split(':');
    if parts.next()? != "external" {
        return None;
    }
    let agent = parts.next()?;
    if matches!(CLIAgent::from_serialized_name(agent), CLIAgent::Unknown) {
        return None;
    }
    let encoded_path = parts.next()?;
    let bytes = hex_decode(encoded_path)?;
    Some(PathBuf::from(String::from_utf8(bytes).ok()?))
}

fn delete_codex_session_index_entry(snapshot_id: &str) -> Result<(), String> {
    remove_codex_session_index_entry(snapshot_id)?;
    set_session_pinned(snapshot_id, false)
}

fn remove_codex_session_index_entry(snapshot_id: &str) -> Result<String, String> {
    let (agent, session_id) = session_id_from_external_index_session_snapshot_id(snapshot_id)
        .ok_or_else(|| format!("not an indexed CLI agent session id: {snapshot_id}"))?;
    if !matches!(agent, CLIAgent::Codex) {
        return Err(format!(
            "unsupported indexed CLI agent: {}",
            agent.display_name()
        ));
    }
    let home_dir = dirs::home_dir().ok_or_else(|| "home directory is unavailable".to_owned())?;
    let index_path = home_dir.join(".codex/session_index.jsonl");
    let contents = fs::read_to_string(&index_path)
        .map_err(|error| format!("failed to read {}: {error}", index_path.display()))?;
    let mut removed_line = None;
    let mut kept_lines = Vec::new();
    for line in contents.lines() {
        let line_id = serde_json::from_str::<Value>(line)
            .ok()
            .and_then(|value| value.get("id").and_then(Value::as_str).map(str::to_owned));
        if line_id.as_deref() == Some(session_id.as_str()) {
            removed_line = Some(line.to_owned());
        } else {
            kept_lines.push(line);
        }
    }
    let Some(removed_line) = removed_line else {
        return Err(format!(
            "session {session_id} not found in {}",
            index_path.display()
        ));
    };
    let mut rewritten = kept_lines.join("\n");
    if !rewritten.is_empty() {
        rewritten.push('\n');
    }
    fs::write(&index_path, rewritten)
        .map_err(|error| format!("failed to write {}: {error}", index_path.display()))?;
    Ok(removed_line)
}

fn validate_mutable_session_path(path: &Path) -> Result<PathBuf, String> {
    let home_dir = dirs::home_dir().ok_or_else(|| "home directory is unavailable".to_owned())?;
    let canonical_path = path
        .canonicalize()
        .map_err(|error| format!("failed to resolve {}: {error}", path.display()))?;
    let allowed_roots = [
        home_dir.join(".claude/projects"),
        home_dir.join(".codex/sessions"),
    ];
    let is_under_allowed_root = allowed_roots.iter().any(|root| {
        root.canonicalize()
            .ok()
            .is_some_and(|root| canonical_path.starts_with(root))
    });
    if !is_under_allowed_root {
        return Err(format!(
            "refusing to mutate session outside CLI-agent history roots: {}",
            canonical_path.display()
        ));
    }
    if canonical_path
        .extension()
        .is_none_or(|extension| extension != "jsonl")
    {
        return Err(format!(
            "refusing to mutate non-jsonl session file: {}",
            canonical_path.display()
        ));
    }

    Ok(canonical_path)
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

fn hex_decode(encoded: &str) -> Option<Vec<u8>> {
    if !encoded.len().is_multiple_of(2) {
        return None;
    }

    let mut bytes = Vec::with_capacity(encoded.len() / 2);
    let mut chars = encoded.as_bytes().iter().copied();
    while let Some(high) = chars.next() {
        let low = chars.next()?;
        bytes.push((hex_value(high)? << 4) | hex_value(low)?);
    }
    Some(bytes)
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}
