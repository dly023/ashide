//! Best-effort current-app index for CLI-agent history files.
//!
//! This turns already persisted Claude/Codex session metadata into Ashide
//! workspace-session rows without executing any provider resume command.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Utc};

use serde_json::Value;
use walkdir::WalkDir;

use crate::app_state::{CliAgentSessionOrigin, WorkspaceSessionKind, WorkspaceSessionSnapshot};
use crate::session_bridge::adapter_registry::session_bridge_adapters;
use crate::terminal::CLIAgent;

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
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
    let Some(home_dir) = dirs::home_dir() else {
        log::warn!("Session Navigator current-app scan skipped: home directory unavailable");
        return Vec::new();
    };
    let config_dir = warp_core::paths::warp_home_config_dir();
    scan_current_app_cli_agent_sessions_with_dirs(limit_per_agent, &home_dir, config_dir.as_deref())
}

fn scan_current_app_cli_agent_sessions_with_dirs(
    limit_per_agent: usize,
    home_dir: &Path,
    config_dir: Option<&Path>,
) -> Vec<WorkspaceSessionSnapshot> {
    if limit_per_agent == 0 {
        return Vec::new();
    }

    let mut sessions = Vec::new();
    let scanned = session_bridge_adapters()
        .iter()
        .filter(|adapter| adapter.capabilities.can_scan_current_app_history)
        .filter_map(|adapter| {
            adapter
                .current_app_scanner
                .map(|scanner| (adapter, scanner))
        })
        .map(|(adapter, scanner)| {
            let sessions = scanner(home_dir, limit_per_agent);
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

    sessions.sort_by(|left, right| right.modified.cmp(&left.modified));
    let pinned_session_ids = pinned_session_ids_with_config(config_dir);
    sessions
        .into_iter()
        .map(|session| indexed_session_to_snapshot(session, &pinned_session_ids))
        .collect()
}

fn indexed_session_to_snapshot(
    session: IndexedSession,
    pinned_session_ids: &HashSet<String>,
) -> WorkspaceSessionSnapshot {
    let id = session
        .snapshot_id
        .unwrap_or_else(|| external_session_snapshot_id(session.agent, &session.path));
    let mut snapshot = WorkspaceSessionSnapshot {
        id,
        container_uuid: None,
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
        is_live_container: false,
    };
    snapshot.is_pinned = snapshot.is_pinned_by(pinned_session_ids);
    snapshot
}

pub(crate) fn delete_current_app_cli_agent_session(snapshot_id: &str) -> Result<(), String> {
    let home_dir = dirs::home_dir().ok_or_else(|| "home directory is unavailable".to_owned())?;
    let config_dir = warp_core::paths::warp_home_config_dir();
    delete_current_app_cli_agent_session_with_dirs(snapshot_id, &home_dir, config_dir.as_deref())
}

fn delete_current_app_cli_agent_session_with_dirs(
    snapshot_id: &str,
    home_dir: &Path,
    config_dir: Option<&Path>,
) -> Result<(), String> {
    if snapshot_id.starts_with("external-index:") {
        return delete_codex_session_index_entry_with_dirs(snapshot_id, home_dir, config_dir);
    }

    let path = path_from_external_session_snapshot_id(snapshot_id)
        .ok_or_else(|| format!("not an indexed CLI agent session id: {snapshot_id}"))?;
    validate_mutable_session_path_location(&path, home_dir)?;
    match fs::remove_file(&path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            log::info!(
                "CLI agent session file already absent during delete: {}",
                path.display()
            );
        }
        Err(error) => {
            return Err(format!("failed to delete {}: {error}", path.display()));
        }
    }
    if let Some(meta_path) = claude_subagent_meta_path_for_jsonl(&path) {
        if let Err(error) = fs::remove_file(&meta_path) {
            if error.kind() != io::ErrorKind::NotFound {
                log::warn!(
                    "failed to delete companion Claude subagent meta {}: {error}",
                    meta_path.display()
                );
            }
        }
    }
    if let Err(error) = set_session_pinned_with_config(snapshot_id, false, config_dir) {
        log::warn!(
            "delete_current_app_cli_agent_session: failed to clear pinned state for {snapshot_id}: {error}"
        );
    }
    Ok(())
}

/// Returns whether the on-disk JSONL backing an `external:{agent}:{hex}` snapshot
/// still exists. Non file-backed ids are treated as present so callers do not
/// over-prune unrelated restored rows.
pub(crate) fn external_jsonl_session_source_exists(snapshot_id: &str) -> bool {
    match path_from_external_session_snapshot_id(snapshot_id) {
        Some(path) => path.is_file(),
        None => true,
    }
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
    read_session_user_state(None).aliases
}

pub(crate) fn set_session_alias(key: &str, alias: Option<&str>) -> Result<(), String> {
    let key = key.trim();
    if key.is_empty() {
        return Err("session alias key is empty".to_owned());
    }

    let mut state = read_session_user_state(None);
    match alias.map(str::trim).filter(|alias| !alias.is_empty()) {
        Some(alias) => {
            state.aliases.insert(key.to_owned(), alias.to_owned());
        }
        None => {
            state.aliases.remove(key);
        }
    }
    write_session_user_state(&state, None)
}

pub(crate) fn pinned_session_ids() -> HashSet<String> {
    read_session_user_state(None).pinned
}

fn pinned_session_ids_with_config(config_dir: Option<&Path>) -> HashSet<String> {
    read_session_user_state(config_dir).pinned
}

pub(crate) fn set_session_pinned(session_id: &str, pinned: bool) -> Result<(), String> {
    set_session_pinned_with_config(session_id, pinned, None)
}

fn set_session_pinned_with_config(
    session_id: &str,
    pinned: bool,
    config_dir: Option<&Path>,
) -> Result<(), String> {
    let session_id = session_id.trim();
    if session_id.is_empty() {
        return Err("pinned session id is empty".to_owned());
    }

    let mut state = read_session_user_state(config_dir);
    if pinned {
        state.pinned.insert(session_id.to_owned());
    } else {
        state.pinned.remove(session_id);
    }
    write_session_user_state(&state, config_dir)
}

fn system_time_to_unix_ms(time: SystemTime) -> Option<i64> {
    let duration = time.duration_since(UNIX_EPOCH).ok()?;
    i64::try_from(duration.as_millis()).ok()
}

fn session_user_state_path(config_dir: Option<&Path>) -> Option<PathBuf> {
    let config_dir = config_dir
        .map(PathBuf::from)
        .or_else(warp_core::paths::warp_home_config_dir)?;
    Some(config_dir.join("session_state.json"))
}

fn read_session_user_state(config_dir: Option<&Path>) -> SessionUserState {
    let Some(path) = session_user_state_path(config_dir) else {
        return SessionUserState::default();
    };
    let Ok(contents) = fs::read_to_string(path) else {
        return SessionUserState::default();
    };
    let Ok(state) = serde_json::from_str::<SessionUserState>(&contents) else {
        return SessionUserState::default();
    };
    let sanitized = sanitize_session_user_state(state.clone());
    if sanitized != state {
        let _ = write_session_user_state(&sanitized, config_dir);
    }
    sanitized
}

fn sanitize_session_user_state(mut state: SessionUserState) -> SessionUserState {
    state.aliases = state
        .aliases
        .into_iter()
        .filter_map(|(key, alias)| {
            let key = key.trim().to_owned();
            let alias = alias.trim().to_owned();
            (!key.is_empty()
                && !alias.is_empty()
                && !WorkspaceSessionSnapshot::is_volatile_layout_identity_key(&key))
            .then_some((key, alias))
        })
        .collect();
    state.pinned = state
        .pinned
        .into_iter()
        .map(|key| key.trim().to_owned())
        .filter(|key| {
            !key.is_empty() && !WorkspaceSessionSnapshot::is_volatile_layout_identity_key(key)
        })
        .collect();
    state
}

fn write_session_user_state(
    state: &SessionUserState,
    config_dir: Option<&Path>,
) -> Result<(), String> {
    let Some(path) = session_user_state_path(config_dir) else {
        return Err("home directory is unavailable".to_owned());
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    }
    let contents = serde_json::to_string_pretty(&sanitize_session_user_state(state.clone()))
        .map_err(|error| format!("failed to encode session user state: {error}"))?;
    fs::write(&path, contents)
        .map_err(|error| format!("failed to write {}: {error}", path.display()))
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
    // `session_index.jsonl` 是 Codex 会话的外置元数据源，thread_name 不依赖
    // Resume/materialize。它必须先按 session id 与 rollout 聚合，再对逻辑会话
    // 限流；否则两种 source 竞争同一个配额，旧 alias 会直到 Resume 更新
    // `updated_at` 后才偶然进入列表。
    //
    // 保留入选逻辑会话的全部 backing source：UI merge 用它补齐标题，删除流程
    // 也需要同时看到 rollout 与 index，避免只删一侧后会话在 Refresh 中复活。
    sessions.extend(parse_codex_session_index(
        &home_dir.join(".codex/session_index.jsonl"),
    ));
    limit_codex_session_sources(sessions, limit)
}

fn limit_codex_session_sources(sessions: Vec<IndexedSession>, limit: usize) -> Vec<IndexedSession> {
    if limit == 0 {
        return Vec::new();
    }

    let mut sources_by_session_id = HashMap::<String, Vec<IndexedSession>>::new();
    for session in sessions {
        sources_by_session_id
            .entry(session.id.clone())
            .or_default()
            .push(session);
    }

    let mut groups = sources_by_session_id.into_values().collect::<Vec<_>>();
    for sources in &mut groups {
        sources.sort_by(|left, right| {
            right
                .modified
                .cmp(&left.modified)
                .then_with(|| left.path.cmp(&right.path))
        });
    }
    groups.sort_by(|left, right| {
        right[0]
            .modified
            .cmp(&left[0].modified)
            .then_with(|| left[0].id.cmp(&right[0].id))
    });
    groups.truncate(limit);
    groups.into_iter().flatten().collect()
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
        label: None,
        command: CLIAgent::Codex.command_prefix().to_owned(),
        modified,
    })
}

fn parse_codex_session_index(path: &Path) -> Vec<IndexedSession> {
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
        .map(str::to_owned);
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

fn delete_codex_session_index_entry_with_dirs(
    snapshot_id: &str,
    home_dir: &Path,
    config_dir: Option<&Path>,
) -> Result<(), String> {
    remove_codex_session_index_entry(snapshot_id, home_dir)?;
    if let Err(error) = set_session_pinned_with_config(snapshot_id, false, config_dir) {
        log::warn!(
            "delete_codex_session_index_entry: failed to clear pinned state for {snapshot_id}: {error}"
        );
    }
    Ok(())
}

fn remove_codex_session_index_entry(snapshot_id: &str, home_dir: &Path) -> Result<String, String> {
    let (agent, session_id) = session_id_from_external_index_session_snapshot_id(snapshot_id)
        .ok_or_else(|| format!("not an indexed CLI agent session id: {snapshot_id}"))?;
    if !matches!(agent, CLIAgent::Codex) {
        return Err(format!(
            "unsupported indexed CLI agent: {}",
            agent.display_name()
        ));
    }
    let index_path = home_dir.join(".codex/session_index.jsonl");
    let contents = match fs::read_to_string(&index_path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            log::info!(
                "codex session index already absent during delete: {}",
                index_path.display()
            );
            return Ok(String::new());
        }
        Err(error) => {
            return Err(format!("failed to read {}: {error}", index_path.display()));
        }
    };
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
        log::info!(
            "session {session_id} already absent in {} during delete",
            index_path.display()
        );
        return Ok(String::new());
    };
    let mut rewritten = kept_lines.join("\n");
    if !rewritten.is_empty() {
        rewritten.push('\n');
    }
    fs::write(&index_path, rewritten)
        .map_err(|error| format!("failed to write {}: {error}", index_path.display()))?;
    Ok(removed_line)
}

fn validate_mutable_session_path_location(path: &Path, home_dir: &Path) -> Result<(), String> {
    if path
        .extension()
        .is_none_or(|extension| extension != "jsonl")
    {
        return Err(format!(
            "refusing to mutate non-jsonl session file: {}",
            path.display()
        ));
    }

    let canonical_path = canonical_cli_agent_session_path(path)?;
    let allowed_roots = [
        home_dir.join(".claude/projects"),
        home_dir.join(".codex/sessions"),
    ];
    let is_under_allowed_root = allowed_roots.iter().any(|root| {
        root.canonicalize()
            .ok()
            .is_some_and(|root| canonical_path.starts_with(&root))
    });
    if !is_under_allowed_root {
        return Err(format!(
            "refusing to mutate session outside CLI-agent history roots: {}",
            canonical_path.display()
        ));
    }

    Ok(())
}

fn canonical_cli_agent_session_path(path: &Path) -> Result<PathBuf, String> {
    if let Ok(canonical_path) = path.canonicalize() {
        return Ok(canonical_path);
    }

    let parent = path
        .parent()
        .ok_or_else(|| format!("session path has no parent: {}", path.display()))?;
    let file_name = path
        .file_name()
        .ok_or_else(|| format!("session path has no file name: {}", path.display()))?;
    let canonical_parent = parent
        .canonicalize()
        .map_err(|error| format!("failed to resolve {}: {error}", path.display()))?;
    Ok(canonical_parent.join(file_name))
}

fn claude_subagent_meta_path_for_jsonl(jsonl_path: &Path) -> Option<PathBuf> {
    let parent = jsonl_path.parent()?;
    if parent.file_name()? != "subagents" {
        return None;
    }
    let stem = jsonl_path.file_stem()?.to_string_lossy();
    Some(parent.join(format!("{stem}.meta.json")))
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::time::Duration;
    use tempfile::TempDir;

    /// 构造一个隔离的 tempdir,模拟 home 目录结构(.claude/projects, .codex/sessions)。
    /// 所有测试通过 `delete_current_app_cli_agent_session_with_dirs` 传入此目录,
    /// 不再触碰真实 ~/.claude 或 ~/.codex。
    fn test_home() -> TempDir {
        let dir = TempDir::new().expect("create temp home");
        fs::create_dir_all(dir.path().join(".claude/projects")).expect("create claude projects");
        fs::create_dir_all(dir.path().join(".codex/sessions")).expect("create codex sessions");
        dir
    }

    fn codex_source(
        session_id: &str,
        path: &str,
        label: Option<&str>,
        modified_seconds: u64,
        is_index_source: bool,
    ) -> IndexedSession {
        IndexedSession {
            agent: CLIAgent::Codex,
            id: session_id.to_owned(),
            path: PathBuf::from(path),
            snapshot_id: is_index_source
                .then(|| external_index_session_snapshot_id(CLIAgent::Codex, session_id)),
            cwd: None,
            label: label.map(str::to_owned),
            command: CLIAgent::Codex.command_prefix().to_owned(),
            modified: SystemTime::UNIX_EPOCH + Duration::from_secs(modified_seconds),
        }
    }

    #[test]
    fn test_session_navigator_codex_alias_enrichment_is_visible_before_resume() {
        let sources = vec![
            codex_source("session-a", "rollout-a.jsonl", None, 300, false),
            codex_source("session-b", "rollout-b.jsonl", None, 200, false),
            // 这个 index-only 会话比 A/B 的旧别名记录更新。旧实现按 source
            // truncate(2)，只留下两个 rollout，导致别名必须等 Resume 更新
            // index.updated_at 后才显示。
            codex_source("session-c", "session_index.jsonl", Some("C"), 150, true),
            codex_source(
                "session-a",
                "session_index.jsonl",
                Some("外置别名 A"),
                10,
                true,
            ),
            codex_source(
                "session-b",
                "session_index.jsonl",
                Some("外置别名 B"),
                20,
                true,
            ),
        ];

        let limited = limit_codex_session_sources(sources, 2);
        let snapshots = limited
            .into_iter()
            .map(|source| indexed_session_to_snapshot(source, &HashSet::new()))
            .collect::<Vec<_>>();
        let merged =
            WorkspaceSessionSnapshot::merge_for_session_navigator(snapshots, &HashSet::new());
        let labels = merged
            .into_iter()
            .map(|session| (session.cli_agent_session_id.unwrap(), session.label))
            .collect::<HashMap<_, _>>();

        assert_eq!(labels.len(), 2, "配额必须按逻辑会话计算");
        assert_eq!(labels["session-a"].as_deref(), Some("外置别名 A"));
        assert_eq!(labels["session-b"].as_deref(), Some("外置别名 B"));
        assert!(!labels.contains_key("session-c"));
    }

    #[test]
    fn test_codex_cold_scan_preloads_real_session_index_thread_name_without_resume() {
        let home = test_home();
        let provider_session_id = "019f5629-5daf-7381-b33e-00d8efba617f";
        let rollout_dir = home.path().join(".codex/sessions/2026/07/12");
        fs::create_dir_all(&rollout_dir).expect("create rollout date directory");
        let rollout_path = rollout_dir.join(format!(
            "rollout-2026-07-12T19-49-39-{provider_session_id}.jsonl"
        ));
        fs::write(
            &rollout_path,
            format!(
                "{{\"timestamp\":\"2026-07-12T11:49:40.238Z\",\"type\":\"session_meta\",\"payload\":{{\"session_id\":\"{provider_session_id}\",\"id\":\"{provider_session_id}\",\"cwd\":\"/Users/admin/manga_data\"}}}}\n"
            ),
        )
        .expect("write real-shape Codex rollout");
        fs::write(
            home.path().join(".codex/session_index.jsonl"),
            format!(
                "{{\"id\":\"{provider_session_id}\",\"thread_name\":\"打招呼\",\"updated_at\":\"2026-07-12T11:49:43.413457Z\"}}\n"
            ),
        )
        .expect("write real-shape Codex session index");

        let snapshots = scan_current_app_cli_agent_sessions_with_dirs(80, home.path(), None);
        let merged =
            WorkspaceSessionSnapshot::merge_for_session_navigator(snapshots, &HashSet::new());
        let session = merged
            .iter()
            .find(|session| session.cli_agent_session_id.as_deref() == Some(provider_session_id))
            .expect("cold scan should discover the Codex session");

        assert_eq!(session.label.as_deref(), Some("打招呼"));
        assert_eq!(session.cwd.as_deref(), Some("/Users/admin/manga_data"));
        assert!(!session.is_live_container);
    }

    #[test]
    fn delete_current_app_cli_agent_session_treats_missing_jsonl_as_success() {
        let home = test_home();
        let config_dir = TempDir::new().expect("create temp config");
        let projects_dir = home.path().join(".claude/projects/ashide-test-missing");
        fs::create_dir_all(&projects_dir).expect("create projects dir");
        let session_path = projects_dir.join("demo-session.jsonl");
        let snapshot_id = external_session_snapshot_id_for_path(CLIAgent::Claude, &session_path);

        delete_current_app_cli_agent_session_with_dirs(
            &snapshot_id,
            home.path(),
            Some(config_dir.path()),
        )
        .expect("missing jsonl delete succeeds");
        assert!(
            !session_path.exists(),
            "delete should not create the missing jsonl file"
        );
    }

    #[test]
    fn delete_codex_index_entry_treats_missing_session_as_success() {
        let home = test_home();
        let config_dir = TempDir::new().expect("create temp config");
        let session_id = "ashide-test-missing-codex".to_owned();
        let snapshot_id = external_index_session_snapshot_id(CLIAgent::Codex, &session_id);

        delete_current_app_cli_agent_session_with_dirs(
            &snapshot_id,
            home.path(),
            Some(config_dir.path()),
        )
        .expect("deleting a non-existent codex index entry succeeds");
    }

    #[test]
    fn delete_current_app_cli_agent_session_removes_orphan_subagent_meta() {
        let home = test_home();
        let config_dir = TempDir::new().expect("create temp config");
        let subagents_dir = home
            .path()
            .join(".claude/projects/ashide-test-subagent/demo/subagents");
        fs::create_dir_all(&subagents_dir).expect("create subagents dir");
        let jsonl_path = subagents_dir.join("agent-demo.jsonl");
        let meta_path = subagents_dir.join("agent-demo.meta.json");
        {
            let mut jsonl = fs::File::create(&jsonl_path).expect("create jsonl");
            writeln!(jsonl, r#"{{"sessionId":"agent-demo"}}"#).expect("write jsonl");
        }
        fs::write(&meta_path, br#"{"agentType":"general-purpose"}"#).expect("write meta");
        let snapshot_id = external_session_snapshot_id_for_path(CLIAgent::Claude, &jsonl_path);

        delete_current_app_cli_agent_session_with_dirs(
            &snapshot_id,
            home.path(),
            Some(config_dir.path()),
        )
        .expect("delete succeeds");

        assert!(!jsonl_path.exists());
        assert!(!meta_path.exists());
    }

    #[test]
    fn external_jsonl_session_source_exists_reports_missing_backing_file() {
        let home = test_home();
        let session_path = home
            .path()
            .join(".claude/projects/ashide-test-source-exists/demo-session.jsonl");
        let snapshot_id = external_session_snapshot_id_for_path(CLIAgent::Claude, &session_path);

        assert!(!external_jsonl_session_source_exists(&snapshot_id));
    }
}

#[cfg(test)]
mod session_user_state_identity_tests {
    use super::*;

    #[test]
    fn test_session_user_state_drops_volatile_layout_identity_debt() {
        let state = SessionUserState {
            aliases: HashMap::from([
                ("tab:1:leaf:0".to_owned(), "旧坐标".to_owned()),
                (
                    "local::source:tab:1:leaf:0".to_owned(),
                    "旧逻辑坐标".to_owned(),
                ),
                ("local::pane:deadbeef".to_owned(), "稳定 pane".to_owned()),
                (
                    "local::agent:Codex:provider-id".to_owned(),
                    "稳定 agent".to_owned(),
                ),
            ]),
            pinned: HashSet::from(["tab:1:leaf:0".to_owned(), "local::pane:deadbeef".to_owned()]),
        };

        let sanitized = sanitize_session_user_state(state);
        assert_eq!(sanitized.aliases.len(), 2);
        assert!(!sanitized.aliases.contains_key("tab:1:leaf:0"));
        assert!(!sanitized.aliases.contains_key("local::source:tab:1:leaf:0"));
        assert_eq!(sanitized.aliases["local::pane:deadbeef"], "稳定 pane");
        assert_eq!(
            sanitized.pinned,
            HashSet::from(["local::pane:deadbeef".to_owned()])
        );
    }
}
