//! Best-effort current-app index for CLI-agent history files.
//!
//! This turns already persisted Claude/Codex session metadata into Ashide
//! workspace-session rows without executing any provider resume command.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use tempfile::NamedTempFile;

use crate::app_state::{CliAgentSessionOrigin, WorkspaceSessionKind, WorkspaceSessionSnapshot};
use crate::cli_agent_jsonl::{
    mutate_cli_agent_session_source, resolve_current_process_cli_agent_store_roots,
    AgentSessionDiscoveryPlan, AgentSessionDiscoveryRecord, AgentSessionDiscoverySource,
    AgentSessionDiscoveryTransition, CliAgentSessionScanError, CliAgentSessionSourceMutation,
    CliAgentStoreRoots,
};
#[cfg(test)]
use crate::cli_agent_jsonl::{require_cli_agent_home, AgentSessionDiscoveryResult};
use crate::terminal::CLIAgent;
use crate::workspace::environment_table::IndexedCliAgentSessionScanOutcome;

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

pub(crate) fn try_scan_current_app_cli_agent_session_discovery(
    logical_limit: usize,
    enabled_agents: impl IntoIterator<Item = CLIAgent>,
    previously_observed_agents: &HashSet<CLIAgent>,
    scope_paths: impl IntoIterator<Item = PathBuf>,
) -> Result<IndexedCliAgentSessionScanOutcome, CliAgentSessionScanError> {
    let roots = resolve_current_process_cli_agent_store_roots()?;
    let plan = AgentSessionDiscoveryPlan::from_enabled_agents(enabled_agents, logical_limit)
        .with_scope_paths(scope_paths);
    scan_current_app_cli_agent_session_discovery_with_plan(plan, &roots, previously_observed_agents)
}

#[cfg(test)]
fn current_app_cli_agent_home(home: Option<PathBuf>) -> Result<PathBuf, CliAgentSessionScanError> {
    require_cli_agent_home(home)
}

#[cfg(test)]
fn scan_current_app_cli_agent_sessions_with_dirs(
    logical_limit: usize,
    home_dir: &Path,
) -> Result<Vec<WorkspaceSessionSnapshot>, CliAgentSessionScanError> {
    let roots = CliAgentStoreRoots::for_home(home_dir.to_path_buf());
    scan_current_app_cli_agent_sessions_with_roots(logical_limit, &roots)
}

#[cfg(test)]
pub(crate) fn scan_current_app_cli_agent_sessions_with_roots(
    logical_limit: usize,
    roots: &CliAgentStoreRoots,
) -> Result<Vec<WorkspaceSessionSnapshot>, CliAgentSessionScanError> {
    let plan = AgentSessionDiscoveryPlan::from_registry(logical_limit);
    scan_current_app_cli_agent_sessions_with_plan(plan, roots)
}

pub(crate) fn scan_current_app_cli_agent_session_discovery_with_plan(
    plan: AgentSessionDiscoveryPlan,
    roots: &CliAgentStoreRoots,
    previously_observed_agents: &HashSet<CLIAgent>,
) -> Result<IndexedCliAgentSessionScanOutcome, CliAgentSessionScanError> {
    let previously_observed_providers = previously_observed_agents
        .iter()
        .filter_map(|agent| agent.session_discovery_provider())
        .collect::<HashSet<_>>();
    match plan
        .execute(roots, &previously_observed_providers)
        .transition()
    {
        AgentSessionDiscoveryTransition::Replace { providers, records } => {
            Ok(IndexedCliAgentSessionScanOutcome::Complete {
                observed_agents: providers
                    .into_iter()
                    .map(|provider| provider.agent())
                    .collect(),
                sessions: records
                    .into_iter()
                    .map(indexed_session_to_snapshot)
                    .collect(),
            })
        }
        AgentSessionDiscoveryTransition::RemoveProvider(provider) => Ok(
            IndexedCliAgentSessionScanOutcome::PermanentlyDeleted(provider.agent()),
        ),
        AgentSessionDiscoveryTransition::PreserveSourceMissing(provider) => Ok(
            IndexedCliAgentSessionScanOutcome::SourceMissing(provider.agent()),
        ),
        AgentSessionDiscoveryTransition::PreserveFailed(error) => Err(error),
        AgentSessionDiscoveryTransition::PreserveCancelled => {
            Ok(IndexedCliAgentSessionScanOutcome::Cancelled)
        }
    }
}

#[cfg(test)]
pub(crate) fn scan_current_app_cli_agent_sessions_with_plan(
    plan: AgentSessionDiscoveryPlan,
    roots: &CliAgentStoreRoots,
) -> Result<Vec<WorkspaceSessionSnapshot>, CliAgentSessionScanError> {
    match scan_current_app_cli_agent_session_discovery_with_plan(plan, roots, &HashSet::new())? {
        IndexedCliAgentSessionScanOutcome::Complete { sessions, .. } => Ok(sessions),
        IndexedCliAgentSessionScanOutcome::SourceMissing(_) => {
            Err(CliAgentSessionScanError::source_missing())
        }
        IndexedCliAgentSessionScanOutcome::PermanentlyDeleted(_) => {
            unreachable!("filesystem scan does not infer permanent provider deletion")
        }
        IndexedCliAgentSessionScanOutcome::Cancelled => {
            unreachable!("synchronous filesystem delivery cannot cancel after execution")
        }
    }
}

fn indexed_session_to_snapshot(session: AgentSessionDiscoveryRecord) -> WorkspaceSessionSnapshot {
    let id = match &session.source {
        AgentSessionDiscoverySource::Transcript(path) => {
            external_session_snapshot_id(session.agent, path)
        }
        AgentSessionDiscoverySource::CodexIndexEntry {
            provider_session_id,
            ..
        } => external_index_session_snapshot_id(session.agent, provider_session_id),
        AgentSessionDiscoverySource::OpenCodeSqliteEntry {
            path,
            provider_session_id,
        }
        | AgentSessionDiscoverySource::CursorCliStoreEntry {
            path,
            provider_session_id,
        } => external_store_entry_session_snapshot_id(session.agent, path, provider_session_id),
    };
    WorkspaceSessionSnapshot {
        id,
        container_uuid: None,
        kind: WorkspaceSessionKind::AgentTerminal,
        label: session.label,
        environment_authority_key: Some("local".to_owned()),
        cwd: session.cwd,
        startup_directory: None,
        cli_agent: Some(session.agent.to_serialized_name()),
        cli_command: Some(session.agent.command_prefix().to_owned()),
        cli_agent_origin: Some(CliAgentSessionOrigin::PluginObserved),
        conversation_ids: Vec::new(),
        active_conversation_id: None,
        cli_agent_session_id: Some(session.provider_session_id),
        is_active: false,
        is_pinned: false,
        updated_at_unix_ms: Some(session.modified_epoch_millis),
        is_live_container: false,
    }
}

#[cfg(any(not(feature = "local_fs"), target_family = "wasm"))]
pub(crate) fn delete_current_app_cli_agent_session(snapshot_id: &str) -> Result<(), String> {
    let roots =
        resolve_current_process_cli_agent_store_roots().map_err(|error| error.to_string())?;
    let config_dir = warp_core::paths::warp_home_config_dir();
    delete_current_app_cli_agent_session_with_dirs(snapshot_id, &roots, config_dir.as_deref())
}

#[cfg(test)]
fn delete_current_app_cli_agent_session_with_home(
    snapshot_id: &str,
    home: Option<PathBuf>,
) -> Result<(), String> {
    let home_dir = require_cli_agent_home(home).map_err(|error| error.to_string())?;
    let roots = CliAgentStoreRoots::for_home(home_dir);
    delete_current_app_cli_agent_session_with_dirs(snapshot_id, &roots, None)
}

pub(crate) fn delete_current_app_cli_agent_session_with_roots(
    snapshot_id: &str,
    expected_agent: CLIAgent,
    roots: &CliAgentStoreRoots,
) -> Result<(), String> {
    let actual_agent = cli_agent_from_external_session_snapshot_id(snapshot_id)
        .ok_or_else(|| format!("not an indexed CLI agent session id: {snapshot_id}"))?;
    if actual_agent != expected_agent {
        return Err(format!(
            "session source agent {} does not match expected {}",
            actual_agent.display_name(),
            expected_agent.display_name()
        ));
    }
    let config_dir = warp_core::paths::warp_home_config_dir();
    delete_current_app_cli_agent_session_with_dirs(snapshot_id, roots, config_dir.as_deref())
}

fn delete_current_app_cli_agent_session_with_dirs(
    snapshot_id: &str,
    roots: &CliAgentStoreRoots,
    config_dir: Option<&Path>,
) -> Result<(), String> {
    let (agent, source, transcript_path) =
        mutation_source_from_external_session_snapshot_id(snapshot_id, roots)
            .ok_or_else(|| format!("not an indexed CLI agent session id: {snapshot_id}"))?;
    mutate_cli_agent_session_source(&source, agent, CliAgentSessionSourceMutation::Delete, roots)?;
    if let Some(meta_path) = transcript_path
        .as_deref()
        .and_then(claude_subagent_meta_path_for_jsonl)
    {
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
    match agent_and_path_from_external_session_snapshot_id(snapshot_id) {
        Some((_, path)) => path.is_file(),
        None => true,
    }
}

pub(crate) fn current_app_cli_agent_session_source_target_from_id(
    snapshot_id: &str,
    cli_agent: Option<&str>,
    provider_session_id: Option<String>,
) -> Result<Option<CurrentAppCliAgentSessionSourceTarget>, String> {
    let roots =
        resolve_current_process_cli_agent_store_roots().map_err(|error| error.to_string())?;
    current_app_cli_agent_session_source_target_from_id_with_roots(
        snapshot_id,
        cli_agent,
        provider_session_id,
        &roots,
    )
}

#[cfg(test)]
fn current_app_cli_agent_session_source_target_from_id_with_home(
    snapshot_id: &str,
    cli_agent: Option<&str>,
    provider_session_id: Option<String>,
    home: Option<PathBuf>,
) -> Result<Option<CurrentAppCliAgentSessionSourceTarget>, String> {
    let home_dir = require_cli_agent_home(home).map_err(|error| error.to_string())?;
    let roots = CliAgentStoreRoots::for_home(home_dir);
    current_app_cli_agent_session_source_target_from_id_with_roots(
        snapshot_id,
        cli_agent,
        provider_session_id,
        &roots,
    )
}

pub(crate) fn current_app_cli_agent_session_source_target_from_id_with_roots(
    snapshot_id: &str,
    cli_agent: Option<&str>,
    provider_session_id: Option<String>,
    roots: &CliAgentStoreRoots,
) -> Result<Option<CurrentAppCliAgentSessionSourceTarget>, String> {
    if let Some((encoded_agent, path)) =
        agent_and_path_from_external_session_snapshot_id(snapshot_id)
    {
        let metadata_agent = cli_agent
            .map(CLIAgent::from_serialized_name)
            .filter(|agent| !matches!(agent, CLIAgent::Unknown));
        if metadata_agent.is_some_and(|agent| agent != encoded_agent) {
            return Err(
                "local CLI session source id agent does not match snapshot metadata".to_owned(),
            );
        }
        return Ok(Some(CurrentAppCliAgentSessionSourceTarget {
            source: path.display().to_string(),
            agent: Some(encoded_agent),
            provider_session_id,
        }));
    }

    if let Some((encoded_agent, path, encoded_session_id)) =
        store_entry_from_external_session_snapshot_id(snapshot_id)
    {
        let metadata_agent = cli_agent
            .map(CLIAgent::from_serialized_name)
            .filter(|agent| !matches!(agent, CLIAgent::Unknown));
        if metadata_agent.is_some_and(|agent| agent != encoded_agent) {
            return Err(
                "local CLI session source id agent does not match snapshot metadata".to_owned(),
            );
        }
        if provider_session_id
            .as_deref()
            .is_some_and(|session_id| session_id != encoded_session_id)
        {
            return Err(
                "local CLI session source id does not match provider session metadata".to_owned(),
            );
        }
        return Ok(Some(CurrentAppCliAgentSessionSourceTarget {
            source: format!("{}#{encoded_session_id}", path.display()),
            agent: Some(encoded_agent),
            provider_session_id: Some(encoded_session_id),
        }));
    }

    let Some((agent, session_id)) = session_id_from_external_index_session_snapshot_id(snapshot_id)
    else {
        return Ok(None);
    };
    if !matches!(agent, CLIAgent::Codex) {
        return Err(format!(
            "unsupported legacy indexed CLI agent source: {}",
            agent.display_name()
        ));
    }
    Ok(Some(CurrentAppCliAgentSessionSourceTarget {
        source: format!("{}:{session_id}", roots.codex_index().display()),
        agent: Some(agent),
        provider_session_id: provider_session_id.or(Some(session_id)),
    }))
}

pub(crate) fn session_aliases() -> HashMap<String, String> {
    read_session_user_state(None).aliases
}

pub(crate) fn mutate_session_user_state(
    keys: &[String],
    alias: Option<Option<&str>>,
    pinned: Option<bool>,
) -> Result<(), String> {
    mutate_session_user_state_with_config(keys, alias, pinned, None)
}

fn mutate_session_user_state_with_config(
    keys: &[String],
    alias: Option<Option<&str>>,
    pinned: Option<bool>,
    config_dir: Option<&Path>,
) -> Result<(), String> {
    let keys = keys
        .iter()
        .map(|key| key.trim())
        .filter(|key| !key.is_empty())
        .collect::<Vec<_>>();
    if keys.is_empty() {
        return Err("session user-state mutation has no keys".to_owned());
    }
    let mut state = read_session_user_state(config_dir);
    for key in keys {
        if let Some(alias) = alias {
            match alias.map(str::trim).filter(|alias| !alias.is_empty()) {
                Some(alias) => {
                    state.aliases.insert(key.to_owned(), alias.to_owned());
                }
                None => {
                    state.aliases.remove(key);
                }
            }
        }
        if let Some(pinned) = pinned {
            if pinned {
                state.pinned.insert(key.to_owned());
            } else {
                state.pinned.remove(key);
            }
        }
    }
    write_session_user_state(&state, config_dir)
}

pub(crate) fn pinned_session_ids() -> HashSet<String> {
    read_session_user_state(None).pinned
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
    fn is_persistable_local_agent_history_user_state_key(key: &str) -> bool {
        !key.starts_with("local::pane:")
    }

    state.aliases = state
        .aliases
        .into_iter()
        .filter_map(|(key, alias)| {
            let key = key.trim().to_owned();
            let alias = alias.trim().to_owned();
            (!key.is_empty()
                && !alias.is_empty()
                && !WorkspaceSessionSnapshot::is_volatile_layout_identity_key(&key)
                && is_persistable_local_agent_history_user_state_key(&key))
            .then_some((key, alias))
        })
        .collect();
    state.pinned = state
        .pinned
        .into_iter()
        .map(|key| key.trim().to_owned())
        .filter(|key| {
            !key.is_empty()
                && !WorkspaceSessionSnapshot::is_volatile_layout_identity_key(key)
                && is_persistable_local_agent_history_user_state_key(key)
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
    let parent = path
        .parent()
        .ok_or_else(|| format!("session user-state path has no parent: {}", path.display()))?;
    let mut temp_file = NamedTempFile::new_in(parent)
        .map_err(|error| format!("failed to create temp file for {}: {error}", path.display()))?;
    temp_file
        .write_all(contents.as_bytes())
        .and_then(|_| temp_file.flush())
        .map_err(|error| format!("failed to write temp file for {}: {error}", path.display()))?;
    temp_file.persist(&path).map(|_| ()).map_err(|error| {
        format!(
            "failed to atomically replace {}: {}",
            path.display(),
            error.error
        )
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

fn external_store_entry_session_snapshot_id(
    agent: CLIAgent,
    path: &Path,
    session_id: &str,
) -> String {
    format!(
        "external-store:{}:{}:{}",
        agent.to_serialized_name(),
        hex_encode(path.to_string_lossy().as_bytes()),
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
    if parts.next().is_some() {
        return None;
    }
    let bytes = hex_decode(encoded_id)?;
    let session_id = String::from_utf8(bytes).ok()?;
    (!session_id.is_empty()).then_some((agent, session_id))
}

fn agent_and_path_from_external_session_snapshot_id(
    snapshot_id: &str,
) -> Option<(CLIAgent, PathBuf)> {
    let mut parts = snapshot_id.split(':');
    if parts.next()? != "external" {
        return None;
    }
    let agent = CLIAgent::from_serialized_name(parts.next()?);
    if matches!(agent, CLIAgent::Unknown) {
        return None;
    }
    let encoded_path = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    let bytes = hex_decode(encoded_path)?;
    let path = PathBuf::from(String::from_utf8(bytes).ok()?);
    (!path.as_os_str().is_empty()).then_some((agent, path))
}

fn store_entry_from_external_session_snapshot_id(
    snapshot_id: &str,
) -> Option<(CLIAgent, PathBuf, String)> {
    let mut parts = snapshot_id.split(':');
    if parts.next()? != "external-store" {
        return None;
    }
    let agent = CLIAgent::from_serialized_name(parts.next()?);
    if matches!(agent, CLIAgent::Unknown) {
        return None;
    }
    let path = PathBuf::from(String::from_utf8(hex_decode(parts.next()?)?).ok()?);
    let session_id = String::from_utf8(hex_decode(parts.next()?)?).ok()?;
    if parts.next().is_some() {
        return None;
    }
    (!path.as_os_str().is_empty() && !session_id.is_empty()).then_some((agent, path, session_id))
}

fn mutation_source_from_external_session_snapshot_id(
    snapshot_id: &str,
    roots: &CliAgentStoreRoots,
) -> Option<(CLIAgent, String, Option<PathBuf>)> {
    if let Some((agent, path)) = agent_and_path_from_external_session_snapshot_id(snapshot_id) {
        return Some((agent, path.to_string_lossy().into_owned(), Some(path)));
    }
    if let Some((agent, path, session_id)) =
        store_entry_from_external_session_snapshot_id(snapshot_id)
    {
        return Some((agent, format!("{}#{session_id}", path.display()), None));
    }
    let (agent, session_id) = session_id_from_external_index_session_snapshot_id(snapshot_id)?;
    matches!(agent, CLIAgent::Codex).then(|| {
        (
            agent,
            format!("{}:{session_id}", roots.codex_index().display()),
            None,
        )
    })
}

fn cli_agent_from_external_session_snapshot_id(snapshot_id: &str) -> Option<CLIAgent> {
    agent_and_path_from_external_session_snapshot_id(snapshot_id)
        .map(|(agent, _)| agent)
        .or_else(|| {
            session_id_from_external_index_session_snapshot_id(snapshot_id).map(|(agent, _)| agent)
        })
        .or_else(|| {
            store_entry_from_external_session_snapshot_id(snapshot_id).map(|(agent, _, _)| agent)
        })
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
    use diesel::connection::SimpleConnection;
    use diesel::prelude::*;
    use diesel::sql_types::BigInt;
    use std::fs::FileTimes;
    use std::io::Write;
    use std::time::{Duration, UNIX_EPOCH};
    use tempfile::TempDir;

    #[test]
    fn local_cli_agent_scan_requires_resolved_home() {
        assert!(current_app_cli_agent_home(None).is_err());
    }

    #[test]
    fn session_user_state_mutation_commits_all_stable_keys_together() {
        let temp = TempDir::new().unwrap();
        let keys = vec![
            "container:first".to_owned(),
            "agent:codex:session-1".to_owned(),
        ];

        mutate_session_user_state_with_config(
            &keys,
            Some(Some("Renamed")),
            Some(true),
            Some(temp.path()),
        )
        .unwrap();

        let state = read_session_user_state(Some(temp.path()));
        assert_eq!(state.aliases.len(), 2);
        assert_eq!(state.pinned.len(), 2);
        for key in &keys {
            assert_eq!(state.aliases.get(key).map(String::as_str), Some("Renamed"));
            assert!(state.pinned.contains(key));
        }

        mutate_session_user_state_with_config(&keys, Some(None), Some(false), Some(temp.path()))
            .unwrap();

        let state = read_session_user_state(Some(temp.path()));
        assert!(state.aliases.is_empty());
        assert!(state.pinned.is_empty());
    }

    #[test]
    fn local_cli_agent_delete_requires_resolved_home() {
        let error = delete_current_app_cli_agent_session_with_home(
            "external-index:codex:019f5629-5daf-7381-b33e-00d8efba617f",
            None,
        )
        .expect_err("delete must fail before resolving a CLI-agent store");

        assert!(error.contains("home directory"));
    }

    #[test]
    fn local_cli_agent_source_target_requires_resolved_home() {
        let error = current_app_cli_agent_session_source_target_from_id_with_home(
            "external-index:codex:019f5629-5daf-7381-b33e-00d8efba617f",
            Some("codex"),
            Some("019f5629-5daf-7381-b33e-00d8efba617f".to_owned()),
            None,
        )
        .expect_err("source resolution must fail before constructing an indexed store path");

        assert!(error.contains("home directory"));
    }

    #[test]
    fn local_cli_agent_source_target_uses_shared_custom_codex_root() {
        let home = TempDir::new().expect("create temp home");
        let custom_codex_home = home.path().join("custom-codex");
        let mut roots = CliAgentStoreRoots::for_home(home.path().to_path_buf());
        roots.codex_home = custom_codex_home.clone();
        let session_id = "019f5629-5daf-7381-b33e-00d8efba617f";
        let snapshot_id = external_index_session_snapshot_id(CLIAgent::Codex, session_id);

        let target = current_app_cli_agent_session_source_target_from_id_with_roots(
            &snapshot_id,
            Some("codex"),
            Some(session_id.to_owned()),
            &roots,
        )
        .expect("resolve source target")
        .expect("indexed Codex target");

        assert_eq!(
            target.source,
            format!(
                "{}:{session_id}",
                custom_codex_home.join("session_index.jsonl").display()
            )
        );
    }

    /// 构造一个隔离的 tempdir,模拟 home 目录结构(.claude/projects, .codex/sessions)。
    /// 所有测试通过 `delete_current_app_cli_agent_session_with_dirs` 传入此目录,
    /// 不再触碰真实 ~/.claude 或 ~/.codex。
    fn test_home() -> TempDir {
        let dir = TempDir::new().expect("create temp home");
        fs::create_dir_all(dir.path().join(".claude/projects")).expect("create claude projects");
        fs::create_dir_all(dir.path().join(".codex/sessions")).expect("create codex sessions");
        dir
    }

    fn set_test_file_modified(path: &Path, seconds: u64) {
        let file = fs::OpenOptions::new()
            .write(true)
            .open(path)
            .expect("open fixture for mtime update");
        file.set_times(FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(seconds)))
            .expect("set fixture mtime");
    }

    #[test]
    fn local_omp_cold_scan_projects_stable_rows_without_layout_identity() {
        let home = test_home();
        let project = home.path().join("project");
        fs::create_dir(&project).expect("create indexed project");
        let canonical_project = fs::canonicalize(&project).expect("canonicalize indexed project");

        let omp_id = "019f0a0b-1111-4222-8333-444444444444";
        let omp_path = home
            .path()
            .join(".omp/agent/sessions/-ashide")
            .join(format!("1784897000000_{omp_id}.jsonl"));
        fs::create_dir_all(omp_path.parent().expect("Omp session parent"))
            .expect("create Omp session directory");
        fs::write(
            &omp_path,
            format!(
                "{}\n{}\n",
                serde_json::json!({"type": "title", "title": "继续 Omp 会话"}),
                serde_json::json!({"type": "session", "id": omp_id, "cwd": project})
            ),
        )
        .expect("write Omp session");

        let merged = WorkspaceSessionSnapshot::merge_for_session_navigator(
            scan_current_app_cli_agent_sessions_with_dirs(80, home.path())
                .expect("scan local Omp sessions"),
        );

        for (agent, session_id, label, path) in
            [(CLIAgent::Omp, omp_id, "继续 Omp 会话", &omp_path)]
        {
            let session = merged
                .iter()
                .find(|session| session.cli_agent_session_id.as_deref() == Some(session_id))
                .expect("indexed agent session should project into Navigator");
            assert_eq!(
                session.id,
                external_session_snapshot_id_for_path(agent, path)
            );
            assert_eq!(
                session.cli_agent.as_deref(),
                Some(agent.to_serialized_name().as_str())
            );
            assert_eq!(session.cli_agent_session_id.as_deref(), Some(session_id));
            assert_eq!(session.cli_command.as_deref(), Some(agent.command_prefix()));
            assert_eq!(session.label.as_deref(), Some(label));
            assert_eq!(session.cwd.as_deref(), canonical_project.to_str());
            assert_eq!(session.kind, WorkspaceSessionKind::AgentTerminal);
            assert_eq!(
                session.cli_agent_origin,
                Some(CliAgentSessionOrigin::PluginObserved)
            );
            assert!(session.container_uuid.is_none());
            assert!(!session.is_live_container);
        }
    }

    #[test]
    fn test_codex_cold_scan_preloads_real_session_index_thread_name_without_resume() {
        let home = test_home();
        let cwd = home.path().join("manga_data");
        fs::create_dir(&cwd).expect("create Codex cwd");
        let canonical_cwd = fs::canonicalize(&cwd).expect("canonicalize Codex cwd");
        let provider_session_id = "019f5629-5daf-7381-b33e-00d8efba617f";
        let rollout_dir = home.path().join(".codex/sessions/2026/07/12");
        fs::create_dir_all(&rollout_dir).expect("create rollout date directory");
        let rollout_path = rollout_dir.join(format!(
            "rollout-2026-07-12T19-49-39-{provider_session_id}.jsonl"
        ));
        fs::write(
            &rollout_path,
            format!(
                "{}\n",
                serde_json::json!({
                    "timestamp": "2026-07-12T11:49:40.238Z",
                    "type": "session_meta",
                    "payload": {
                        "session_id": provider_session_id,
                        "id": provider_session_id,
                        "cwd": cwd,
                    },
                })
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

        let snapshots = scan_current_app_cli_agent_sessions_with_dirs(80, home.path())
            .expect("scan current-app sessions");
        let merged = WorkspaceSessionSnapshot::merge_for_session_navigator(snapshots);
        let session = merged
            .iter()
            .find(|session| session.cli_agent_session_id.as_deref() == Some(provider_session_id))
            .expect("cold scan should discover the Codex session");

        assert_eq!(session.label.as_deref(), Some("打招呼"));
        assert_eq!(session.cwd.as_deref(), canonical_cwd.to_str());
        assert!(!session.is_live_container);
    }

    #[test]
    fn test_local_codex_index_accepts_shared_session_id_fallback() {
        let home = test_home();
        let project = home.path().join("index-project");
        let provider_session_id = "019f5f34-b6b7-70b3-8e50-e98504691ca1";
        fs::create_dir(&project).expect("create index cwd");
        let canonical_project = fs::canonicalize(&project).expect("canonicalize index cwd");
        fs::write(
            home.path().join(".codex/session_index.jsonl"),
            format!(
                "{}\n",
                serde_json::json!({
                    "session_id": provider_session_id,
                    "thread_name": "本地共享 Index Parser",
                    "cwd": "~/index-project",
                    "updated_at_unix_ms": 1234,
                })
            ),
        )
        .expect("write Codex session index");

        let AgentSessionDiscoveryResult::Complete {
            records: sessions, ..
        } = AgentSessionDiscoveryPlan::from_registry(40).execute(
            &CliAgentStoreRoots::for_home(home.path().to_path_buf()),
            &HashSet::new(),
        )
        else {
            panic!("scan local Codex index must complete");
        };
        let session = sessions
            .iter()
            .find(|session| session.provider_session_id == provider_session_id)
            .expect("session_id fallback must be visible locally");

        assert_eq!(session.label.as_deref(), Some("本地共享 Index Parser"));
        assert_eq!(session.cwd.as_deref(), canonical_project.to_str());
        assert_eq!(session.modified_epoch_millis, 1234);
    }

    #[test]
    fn test_local_cli_agent_scan_uses_shared_cwd_normalization() {
        let home = test_home();
        let project = home.path().join("project");
        fs::create_dir(&project).expect("create project cwd");
        let canonical_project = fs::canonicalize(&project).expect("canonicalize project cwd");

        let valid_session_id = "019f5f34-b6b7-70b3-8e50-e98504691ca2";
        fs::write(
            home.path()
                .join(".codex/sessions")
                .join(format!("rollout-{valid_session_id}.jsonl")),
            format!(
                "{}\n",
                serde_json::json!({
                    "type": "session_meta",
                    "payload": {"id": valid_session_id, "cwd": "~/project"},
                })
            ),
        )
        .expect("write valid cwd transcript");

        let store_session_id = "019f5f34-b6b7-70b3-8e50-e98504691ca3";
        fs::write(
            home.path()
                .join(".codex/sessions")
                .join(format!("rollout-{store_session_id}.jsonl")),
            format!(
                "{}\n",
                serde_json::json!({
                    "type": "session_meta",
                    "payload": {"id": store_session_id, "cwd": "~/.codex/sessions"},
                })
            ),
        )
        .expect("write session-store cwd transcript");

        let AgentSessionDiscoveryResult::Complete {
            records: sessions, ..
        } = AgentSessionDiscoveryPlan::from_registry(40).execute(
            &CliAgentStoreRoots::for_home(home.path().to_path_buf()),
            &HashSet::new(),
        )
        else {
            panic!("scan local Codex sessions must complete");
        };
        let valid = sessions
            .iter()
            .find(|session| session.provider_session_id == valid_session_id)
            .expect("valid cwd session");
        let store = sessions
            .iter()
            .find(|session| session.provider_session_id == store_session_id)
            .expect("session-store cwd session");

        assert_eq!(valid.cwd.as_deref(), canonical_project.to_str());
        assert_eq!(store.cwd, None);
    }

    #[test]
    fn test_local_cli_agent_scan_applies_global_logical_limit() {
        let home = test_home();
        for (index, session_id) in [
            "019f5f34-b6b7-70b3-8e50-e98504691ca4",
            "019f5f34-b6b7-70b3-8e50-e98504691ca5",
        ]
        .into_iter()
        .enumerate()
        {
            fs::write(
                home.path()
                    .join(".codex/sessions")
                    .join(format!("rollout-{session_id}.jsonl")),
                format!(
                    "{}\n",
                    serde_json::json!({
                        "type": "session_meta",
                        "payload": {"id": session_id},
                    })
                ),
            )
            .expect("write Codex limit fixture");

            let session_id = format!("local-claude-limit-{index}");
            fs::write(
                home.path()
                    .join(".claude/projects")
                    .join(format!("{session_id}.jsonl")),
                format!(
                    "{}\n",
                    serde_json::json!({
                        "type": "user",
                        "sessionId": session_id,
                        "message": {"role": "user", "content": "limit fixture"},
                    })
                ),
            )
            .expect("write Claude limit fixture");
        }

        let snapshots = scan_current_app_cli_agent_sessions_with_dirs(2, home.path())
            .expect("scan local mixed-provider sessions");
        let logical_ids = snapshots
            .iter()
            .filter_map(|session| session.cli_agent_session_id.as_deref())
            .collect::<HashSet<_>>();

        assert_eq!(
            logical_ids.len(),
            2,
            "quota must be global across providers"
        );
    }

    #[test]
    fn test_local_cli_agent_scan_preserves_shared_logical_order() {
        let home = test_home();
        let old_codex = "019f5f34-b6b7-70b3-8e50-e98504691cc1";
        let new_codex = "019f5f34-b6b7-70b3-8e50-e98504691cc2";
        let middle_claude = "local-remote-order-middle";

        let old_codex_path = home
            .path()
            .join(".codex/sessions")
            .join(format!("rollout-{old_codex}.jsonl"));
        let new_codex_path = home
            .path()
            .join(".codex/sessions")
            .join(format!("rollout-{new_codex}.jsonl"));
        let middle_claude_path = home
            .path()
            .join(".claude/projects")
            .join(format!("{middle_claude}.jsonl"));

        for (path, session_id) in [(&old_codex_path, old_codex), (&new_codex_path, new_codex)] {
            fs::write(
                path,
                format!(
                    "{}\n",
                    serde_json::json!({
                        "type": "session_meta",
                        "payload": {"id": session_id},
                    })
                ),
            )
            .expect("write Codex order fixture");
        }
        fs::write(
            &middle_claude_path,
            format!(
                "{}\n",
                serde_json::json!({
                    "type": "user",
                    "sessionId": middle_claude,
                    "message": {"role": "user", "content": "order fixture"},
                })
            ),
        )
        .expect("write Claude order fixture");

        set_test_file_modified(&old_codex_path, 100);
        set_test_file_modified(&middle_claude_path, 200);
        set_test_file_modified(&new_codex_path, 300);

        let snapshots = scan_current_app_cli_agent_sessions_with_dirs(2, home.path())
            .expect("scan ordered local mixed-provider sessions");
        let logical_ids = snapshots
            .iter()
            .filter_map(|session| session.cli_agent_session_id.as_deref())
            .collect::<Vec<_>>();

        assert_eq!(logical_ids, vec![new_codex, middle_claude]);
    }

    #[test]
    fn test_local_cli_agent_scan_uses_shared_first_user_message_title_fallback() {
        let home = test_home();
        let codex_session_id = "019f5f34-b6b7-70b3-8e50-e98504691ca6";
        let codex_path = home
            .path()
            .join(".codex/sessions")
            .join(format!("rollout-{codex_session_id}.jsonl"));
        fs::write(
            &codex_path,
            format!(
                concat!(
                    "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{}\",\"cwd\":\"/tmp/project\"}}}}\n",
                    "{{\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"user\",\"content\":[{{\"type\":\"input_text\",\"text\":\"# AGENTS.md instructions\"}}]}}}}\n",
                    "{{\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"user\",\"content\":[{{\"type\":\"input_text\",\"text\":\"修复本地 Codex 标题\\n第二行不进入标题\"}}]}}}}\n"
                ),
                codex_session_id
            ),
        )
        .expect("write Codex rollout");

        let claude_session_id = "local-claude-first-user-title";
        let claude_path = home
            .path()
            .join(".claude/projects")
            .join(format!("{claude_session_id}.jsonl"));
        fs::write(
            &claude_path,
            format!(
                "{{\"type\":\"user\",\"sessionId\":\"{claude_session_id}\",\"cwd\":\"/tmp/project\",\"message\":{{\"role\":\"user\",\"content\":\"继续本地 Claude 会话\"}}}}\n"
            ),
        )
        .expect("write Claude transcript");

        let snapshots = scan_current_app_cli_agent_sessions_with_dirs(80, home.path())
            .expect("scan current-app sessions");
        let merged = WorkspaceSessionSnapshot::merge_for_session_navigator(snapshots);
        let labels = merged
            .into_iter()
            .filter_map(|session| Some((session.cli_agent_session_id?, session.label?)))
            .collect::<HashMap<_, _>>();

        assert_eq!(
            labels.get(codex_session_id).map(String::as_str),
            Some("修复本地 Codex 标题")
        );
        assert_eq!(
            labels.get(claude_session_id).map(String::as_str),
            Some("继续本地 Claude 会话")
        );
    }

    #[test]
    fn delete_current_app_cli_agent_session_treats_missing_jsonl_as_success() {
        let home = test_home();
        let config_dir = TempDir::new().expect("create temp config");
        let projects_dir = home.path().join(".claude/projects/ashide-test-missing");
        fs::create_dir_all(&projects_dir).expect("create projects dir");
        let session_path = projects_dir.join("demo-session.jsonl");
        let snapshot_id = external_session_snapshot_id_for_path(CLIAgent::Claude, &session_path);
        let roots = CliAgentStoreRoots::for_home(home.path().to_path_buf());

        delete_current_app_cli_agent_session_with_dirs(
            &snapshot_id,
            &roots,
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
        let roots = CliAgentStoreRoots::for_home(home.path().to_path_buf());

        delete_current_app_cli_agent_session_with_dirs(
            &snapshot_id,
            &roots,
            Some(config_dir.path()),
        )
        .expect("deleting a non-existent codex index entry succeeds");
    }

    #[test]
    fn delete_codex_index_entry_uses_shared_custom_codex_root() {
        let home = test_home();
        let config_dir = TempDir::new().expect("create temp config");
        let custom_codex_home = home.path().join("custom-codex");
        fs::create_dir_all(&custom_codex_home).expect("create custom Codex root");
        let session_id = "019f5629-5daf-7381-b33e-00d8efba617f";
        let kept_session_id = "119f5629-5daf-7381-b33e-00d8efba617f";
        let index_path = custom_codex_home.join("session_index.jsonl");
        fs::write(
            &index_path,
            format!(
                "{{\"id\":\"{session_id}\",\"thread_name\":\"delete\"}}\n{{\"id\":\"{kept_session_id}\",\"thread_name\":\"keep\"}}\n"
            ),
        )
        .expect("write custom Codex index");
        let mut roots = CliAgentStoreRoots::for_home(home.path().to_path_buf());
        roots.codex_home = custom_codex_home;
        let snapshot_id = external_index_session_snapshot_id(CLIAgent::Codex, session_id);

        delete_current_app_cli_agent_session_with_dirs(
            &snapshot_id,
            &roots,
            Some(config_dir.path()),
        )
        .expect("delete custom-root Codex index entry");

        let rewritten = fs::read_to_string(index_path).expect("read rewritten index");
        assert!(!rewritten.contains(session_id));
        assert!(rewritten.contains(kept_session_id));
    }

    #[test]
    fn delete_jsonl_uses_shared_custom_claude_root_allowlist() {
        let home = test_home();
        let config_dir = TempDir::new().expect("create temp config");
        let custom_claude_root = home.path().join("custom-claude");
        let session_path = custom_claude_root
            .join("projects/project")
            .join("session.jsonl");
        fs::create_dir_all(session_path.parent().expect("session parent"))
            .expect("create custom Claude root");
        fs::write(&session_path, "{}\n").expect("write custom Claude session");
        let mut roots = CliAgentStoreRoots::for_home(home.path().to_path_buf());
        roots.claude_config_dir = custom_claude_root;
        let snapshot_id = external_session_snapshot_id_for_path(CLIAgent::Claude, &session_path);

        delete_current_app_cli_agent_session_with_dirs(
            &snapshot_id,
            &roots,
            Some(config_dir.path()),
        )
        .expect("delete custom-root Claude transcript");

        assert!(!session_path.exists());
    }

    #[test]
    fn delete_antigravity_session_accepts_shared_discovery_root() {
        let home = test_home();
        let config_dir = TempDir::new().expect("create temp config");
        let roots = CliAgentStoreRoots::for_home(home.path().to_path_buf());
        let session_path = roots
            .antigravity_brain()
            .join("agy-session/.system_generated/logs/transcript.jsonl");
        fs::create_dir_all(
            session_path
                .parent()
                .expect("Antigravity transcript parent"),
        )
        .expect("create Antigravity transcript directory");
        fs::write(&session_path, "{}\n").expect("write Antigravity transcript");
        let snapshot_id =
            external_session_snapshot_id_for_path(CLIAgent::Antigravity, &session_path);

        delete_current_app_cli_agent_session_with_dirs(
            &snapshot_id,
            &roots,
            Some(config_dir.path()),
        )
        .expect("delete Antigravity transcript from its discovery root");

        assert!(!session_path.exists());
    }

    #[derive(QueryableByName)]
    struct SqliteCount {
        #[diesel(sql_type = BigInt)]
        count: i64,
    }

    #[test]
    fn delete_opencode_sqlite_session_uses_provider_native_store_entry() {
        let home = test_home();
        let config_dir = TempDir::new().expect("create temp config");
        let roots = CliAgentStoreRoots::for_home(home.path().to_path_buf());
        fs::create_dir_all(roots.opencode_databases_dir()).expect("create OpenCode data dir");
        let database_path = roots.opencode_databases_dir().join("opencode.db");
        let mut connection =
            diesel::sqlite::SqliteConnection::establish(&database_path.to_string_lossy())
                .expect("open OpenCode fixture database");
        connection
            .batch_execute(
                "PRAGMA foreign_keys = ON;
                 CREATE TABLE session (
                   id TEXT PRIMARY KEY,
                   parent_id TEXT,
                   title TEXT,
                   directory TEXT,
                   time_created INTEGER NOT NULL,
                   time_updated INTEGER NOT NULL,
                   time_archived INTEGER
                 );
                 CREATE TABLE message (
                   id TEXT PRIMARY KEY,
                   session_id TEXT NOT NULL REFERENCES session(id) ON DELETE CASCADE
                 );
                 INSERT INTO session VALUES ('target', NULL, 'target', '/work', 1, 2, NULL);
                 INSERT INTO session VALUES ('child', 'target', 'child', '/work', 1, 2, NULL);
                 INSERT INTO session VALUES ('unrelated', NULL, 'unrelated', '/work', 1, 2, NULL);
                 INSERT INTO message VALUES ('target-message', 'target');
                 INSERT INTO message VALUES ('child-message', 'child');
                 INSERT INTO message VALUES ('unrelated-message', 'unrelated');",
            )
            .expect("seed OpenCode fixture database");
        drop(connection);
        let snapshot_id = scan_current_app_cli_agent_sessions_with_roots(10, &roots)
            .expect("discover OpenCode SQLite-backed sessions")
            .into_iter()
            .find(|snapshot| snapshot.cli_agent_session_id.as_deref() == Some("target"))
            .expect("project OpenCode target session")
            .id;
        assert!(snapshot_id.starts_with(&format!(
            "external-store:{}:",
            CLIAgent::OpenCode.to_serialized_name()
        )));

        delete_current_app_cli_agent_session_with_dirs(
            &snapshot_id,
            &roots,
            Some(config_dir.path()),
        )
        .expect("delete OpenCode SQLite-backed session");

        let mut connection =
            diesel::sqlite::SqliteConnection::establish(&database_path.to_string_lossy())
                .expect("reopen OpenCode fixture database");
        let remaining = diesel::sql_query(
            "SELECT COUNT(*) AS count FROM session WHERE id IN ('target', 'child')",
        )
        .get_result::<SqliteCount>(&mut connection)
        .expect("count deleted OpenCode sessions");
        let unrelated =
            diesel::sql_query("SELECT COUNT(*) AS count FROM session WHERE id = 'unrelated'")
                .get_result::<SqliteCount>(&mut connection)
                .expect("count unrelated OpenCode session");
        assert_eq!(remaining.count, 0);
        assert_eq!(unrelated.count, 1);
    }

    #[test]
    fn external_source_snapshot_ids_reject_trailing_or_empty_components() {
        assert!(session_id_from_external_index_session_snapshot_id(
            "external-index:codex:746172676574:unexpected"
        )
        .is_none());
        assert!(agent_and_path_from_external_session_snapshot_id("external:claude:").is_none());
        assert!(store_entry_from_external_session_snapshot_id(
            "external-store:opencode:2f746d702f6f70656e636f64652e6462:"
        )
        .is_none());
        assert!(store_entry_from_external_session_snapshot_id(
            "external-store:opencode:2f746d702f6f70656e636f64652e6462:746172676574:unexpected"
        )
        .is_none());
    }

    #[test]
    fn delete_cursor_store_entry_removes_only_exact_session_directory() {
        let home = test_home();
        let config_dir = TempDir::new().expect("create temp config");
        let roots = CliAgentStoreRoots::for_home(home.path().to_path_buf());
        let session_dir = roots.cursor_chats().join("project/cursor-session");
        let store_path = session_dir.join("store.db");
        fs::create_dir_all(&session_dir).expect("create Cursor session directory");
        fs::write(&store_path, "store").expect("write Cursor store");
        fs::write(session_dir.join("meta.json"), "{}").expect("write Cursor metadata");
        let unrelated_dir = roots.cursor_chats().join("project/unrelated");
        fs::create_dir_all(&unrelated_dir).expect("create unrelated Cursor session");
        fs::write(unrelated_dir.join("store.db"), "store").expect("write unrelated store");
        fs::write(unrelated_dir.join("meta.json"), "{}").expect("write unrelated metadata");
        let snapshot_id = external_store_entry_session_snapshot_id(
            CLIAgent::CursorCli,
            &store_path,
            "cursor-session",
        );

        delete_current_app_cli_agent_session_with_dirs(
            &snapshot_id,
            &roots,
            Some(config_dir.path()),
        )
        .expect("delete canonical Cursor store entry");

        assert!(!session_dir.exists());
        assert!(unrelated_dir.exists());
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
        let roots = CliAgentStoreRoots::for_home(home.path().to_path_buf());

        delete_current_app_cli_agent_session_with_dirs(
            &snapshot_id,
            &roots,
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
    fn local_session_user_state_drops_generic_pane_identity_debt() {
        let state = SessionUserState {
            aliases: HashMap::from([
                ("tab:1:leaf:0".to_owned(), "旧坐标".to_owned()),
                (
                    "local::source:tab:1:leaf:0".to_owned(),
                    "旧逻辑坐标".to_owned(),
                ),
                (
                    "local::pane:deadbeef".to_owned(),
                    "错误终端 pane".to_owned(),
                ),
                (
                    "local::agent:Codex:provider-id".to_owned(),
                    "稳定 agent".to_owned(),
                ),
            ]),
            pinned: HashSet::from(["tab:1:leaf:0".to_owned(), "local::pane:deadbeef".to_owned()]),
        };

        let sanitized = sanitize_session_user_state(state);
        assert_eq!(sanitized.aliases.len(), 1);
        assert!(!sanitized.aliases.contains_key("tab:1:leaf:0"));
        assert!(!sanitized.aliases.contains_key("local::source:tab:1:leaf:0"));
        assert!(!sanitized.aliases.contains_key("local::pane:deadbeef"));
        assert_eq!(
            sanitized.aliases["local::agent:Codex:provider-id"],
            "稳定 agent"
        );
        assert!(sanitized.pinned.is_empty());
    }
}
