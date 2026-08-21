//! Agent session discovery orchestration.

#[cfg(feature = "local_fs")]
use std::collections::{HashMap, HashSet};
#[cfg(feature = "local_fs")]
use std::fs;
#[cfg(feature = "local_fs")]
use std::io;
#[cfg(feature = "local_fs")]
use std::path::{Path, PathBuf};
#[cfg(feature = "local_fs")]
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[cfg(feature = "local_fs")]
#[cfg(feature = "local_fs")]
use crate::terminal::CLIAgent;
#[cfg(feature = "local_fs")]
use diesel::connection::SimpleConnection;
#[cfg(feature = "local_fs")]
use diesel::prelude::*;
#[cfg(feature = "local_fs")]
use diesel::sql_types::{BigInt, Nullable, Text};
#[cfg(feature = "local_fs")]
use rayon::prelude::*;

#[cfg(feature = "local_fs")]
use super::error::CliAgentSessionScanError;
#[cfg(feature = "local_fs")]
use super::parse::{
    canonical_codex_session_id, claude_session_metadata, codex_session_index_record,
    codex_session_metadata, first_string, read_jsonl_prefix_values, read_jsonl_values_from_path,
    read_jsonl_values_from_path_with_physical_line_limit, string_field,
};
#[cfg(feature = "local_fs")]
use super::policy::{
    limit_cli_agent_session_sources, reserve_discovery_candidate, sort_and_limit_recent_files,
    sort_recent_files, CliAgentSessionSource, RecentJsonlFile,
};
#[cfg(feature = "local_fs")]
use super::roots::{is_omp_session_source, normalize_cli_agent_session_cwd, CliAgentStoreRoots};

/// Agent capability registry 对 session discovery provider 的穷尽声明。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum AgentSessionDiscoveryProvider {
    Claude,
    Codex,
    Droid,
    OpenCode,
    Copilot,
    Pi,
    Cursor,
    Antigravity,
    Omp,
}

/// 一次 discovery generation 要执行的完整共享计划。
#[cfg(feature = "local_fs")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AgentSessionDiscoveryPlan {
    providers: Vec<AgentSessionDiscoveryProvider>,
    logical_limit: usize,
    scope_paths: Vec<PathBuf>,
}

/// provider session source 的稳定类型，不把 path 编码规则泄漏给 transport。
#[cfg(feature = "local_fs")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AgentSessionDiscoverySource {
    Transcript(PathBuf),
    CodexIndexEntry {
        path: PathBuf,
        provider_session_id: String,
    },
    OpenCodeSqliteEntry {
        path: PathBuf,
        provider_session_id: String,
    },
    CursorCliStoreEntry {
        path: PathBuf,
        provider_session_id: String,
    },
}

#[cfg(feature = "local_fs")]
impl AgentSessionDiscoverySource {
    pub(crate) fn transport_reference(&self) -> String {
        match self {
            Self::Transcript(path) => path.to_string_lossy().into_owned(),
            Self::CodexIndexEntry {
                path,
                provider_session_id,
            } => format!("{}:{provider_session_id}", path.to_string_lossy()),
            Self::OpenCodeSqliteEntry {
                path,
                provider_session_id,
            } => format!("{}#{provider_session_id}", path.to_string_lossy()),
            Self::CursorCliStoreEntry {
                path,
                provider_session_id,
            } => format!("{}#{provider_session_id}", path.to_string_lossy()),
        }
    }

    fn physical_key(&self) -> String {
        self.transport_reference()
    }
}

/// 共享 parser 产出的唯一 metadata/identity record。
#[cfg(feature = "local_fs")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AgentSessionDiscoveryRecord {
    pub(crate) agent: CLIAgent,
    pub(crate) provider_session_id: String,
    pub(crate) source: AgentSessionDiscoverySource,
    pub(crate) label: Option<String>,
    pub(crate) cwd: Option<String>,
    pub(crate) modified_epoch_millis: i64,
}

/// delivery 与 canonical collection owner 之间的类型化完成语义。
#[cfg(feature = "local_fs")]
#[derive(Debug)]
pub(crate) enum AgentSessionDiscoveryResult {
    Complete {
        providers: Vec<AgentSessionDiscoveryProvider>,
        records: Vec<AgentSessionDiscoveryRecord>,
    },
    SourceMissing(AgentSessionDiscoveryProvider),
    // 为计划中的远程/异步 delivery 保留：同步文件系统扫描永远不会推断出 provider
    // 被永久删除（消费端对该 transition 显式 unreachable!），但 discovery 生命周期
    // 状态机需要这条 transition 才完整。
    #[allow(dead_code)]
    PermanentlyDeleted(AgentSessionDiscoveryProvider),
    Failed(CliAgentSessionScanError),
    // 为计划中的异步 delivery 保留：同步 delivery 不可能在 execute 之后取消，但
    // discovery 生命周期状态机需要 Cancelled 这条 transition 才完整。
    #[allow(dead_code)]
    Cancelled,
}

/// discovery result 对 canonical collection 的唯一 transition。
#[cfg(feature = "local_fs")]
#[derive(Debug)]
pub(crate) enum AgentSessionDiscoveryTransition {
    Replace {
        providers: Vec<AgentSessionDiscoveryProvider>,
        records: Vec<AgentSessionDiscoveryRecord>,
    },
    RemoveProvider(AgentSessionDiscoveryProvider),
    PreserveSourceMissing(AgentSessionDiscoveryProvider),
    PreserveFailed(CliAgentSessionScanError),
    PreserveCancelled,
}

#[cfg(feature = "local_fs")]
impl AgentSessionDiscoveryProvider {
    pub(crate) fn agent(self) -> CLIAgent {
        match self {
            Self::Claude => CLIAgent::Claude,
            Self::Codex => CLIAgent::Codex,
            Self::Droid => CLIAgent::Droid,
            Self::OpenCode => CLIAgent::OpenCode,
            Self::Copilot => CLIAgent::Copilot,
            Self::Pi => CLIAgent::Pi,
            Self::Cursor => CLIAgent::CursorCli,
            Self::Antigravity => CLIAgent::Antigravity,
            Self::Omp => CLIAgent::Omp,
        }
    }
}

#[cfg(feature = "local_fs")]
impl AgentSessionDiscoveryResult {
    pub(crate) fn transition(self) -> AgentSessionDiscoveryTransition {
        match self {
            Self::Complete { providers, records } => {
                AgentSessionDiscoveryTransition::Replace { providers, records }
            }
            Self::SourceMissing(provider) => {
                AgentSessionDiscoveryTransition::PreserveSourceMissing(provider)
            }
            Self::PermanentlyDeleted(provider) => {
                AgentSessionDiscoveryTransition::RemoveProvider(provider)
            }
            Self::Failed(error) => AgentSessionDiscoveryTransition::PreserveFailed(error),
            Self::Cancelled => AgentSessionDiscoveryTransition::PreserveCancelled,
        }
    }
}

impl AgentSessionDiscoveryPlan {
    #[cfg(test)]
    pub(crate) fn from_registry(logical_limit: usize) -> Self {
        Self::from_enabled_agents(enum_iterator::all::<CLIAgent>(), logical_limit)
    }

    /// 为当前应用与 runtime delivery 构建唯一共享的 discovery plan。provider
    /// 选择由设置 owner 注入；文件系统 reader 不得反向读取 UI 设置。
    pub(crate) fn from_enabled_agents(
        enabled_agents: impl IntoIterator<Item = CLIAgent>,
        logical_limit: usize,
    ) -> Self {
        let providers = enabled_agents
            .into_iter()
            .filter_map(CLIAgent::session_discovery_provider)
            .fold(Vec::new(), |mut providers, provider| {
                if !providers.contains(&provider) {
                    providers.push(provider);
                }
                providers
            });
        Self {
            providers,
            logical_limit,
            scope_paths: Vec::new(),
        }
    }

    pub(crate) fn with_scope_paths(
        mut self,
        scope_paths: impl IntoIterator<Item = PathBuf>,
    ) -> Self {
        self.scope_paths = scope_paths
            .into_iter()
            .filter_map(|path| fs::canonicalize(path).ok())
            .collect();
        self
    }

    #[cfg(test)]
    pub(crate) fn providers(&self) -> &[AgentSessionDiscoveryProvider] {
        &self.providers
    }

    #[cfg(test)]
    pub(crate) fn for_test(
        providers: Vec<AgentSessionDiscoveryProvider>,
        logical_limit: usize,
    ) -> Self {
        Self {
            providers,
            logical_limit,
            scope_paths: Vec::new(),
        }
    }

    pub(crate) fn execute(
        &self,
        roots: &CliAgentStoreRoots,
        previously_observed_providers: &HashSet<AgentSessionDiscoveryProvider>,
    ) -> AgentSessionDiscoveryResult {
        if self.logical_limit == 0 {
            return AgentSessionDiscoveryResult::Complete {
                providers: Vec::new(),
                records: Vec::new(),
            };
        }

        enum ProviderScan {
            Records(Vec<AgentSessionDiscoveryRecord>),
            NotProvisioned,
            SourceMissing(AgentSessionDiscoveryProvider),
        }

        let scan_started_at = Instant::now();
        let provider_scans = self
            .providers
            .par_iter()
            .map(|provider| {
                let provider_started_at = Instant::now();
                let result = match provider_source_exists(*provider, roots) {
                    Err(error) => Err(error),
                    Ok(false) if previously_observed_providers.contains(provider) => {
                        Ok(ProviderScan::SourceMissing(*provider))
                    }
                    Ok(false) => Ok(ProviderScan::NotProvisioned),
                    Ok(true) => scan_agent_session_provider(*provider, roots, self.logical_limit)
                        .map(ProviderScan::Records),
                };
                (*provider, provider_started_at.elapsed(), result)
            })
            .collect::<Vec<_>>();

        let mut observed_providers = Vec::new();
        let mut records = Vec::new();
        for (provider, elapsed, result) in provider_scans {
            match result {
                Ok(ProviderScan::Records(provider_records)) => {
                    observed_providers.push(provider);
                    log::info!(
                        "Session Navigator discovery provider {provider:?} scanned {} records in {} ms",
                        provider_records.len(),
                        elapsed.as_millis(),
                    );
                    records.extend(provider_records);
                }
                Ok(ProviderScan::NotProvisioned) => {
                    log::info!(
                        "Session Navigator discovery provider {provider:?} is not provisioned after {} ms",
                        elapsed.as_millis(),
                    );
                }
                Ok(ProviderScan::SourceMissing(provider)) => {
                    log::info!(
                        "Session Navigator discovery provider {provider:?} source disappeared after {} ms",
                        elapsed.as_millis(),
                    );
                    return AgentSessionDiscoveryResult::SourceMissing(provider);
                }
                Err(error) => {
                    log::warn!(
                        "Session Navigator discovery provider {provider:?} failed after {} ms: {error}",
                        elapsed.as_millis(),
                    );
                    return AgentSessionDiscoveryResult::Failed(error);
                }
            }
        }
        log::info!(
            "Session Navigator discovery scanned {} providers in {} ms",
            self.providers.len(),
            scan_started_at.elapsed().as_millis(),
        );

        let mut scope_records = records
            .iter()
            .filter(|record| {
                record.cwd.as_deref().is_some_and(|cwd| {
                    let cwd = Path::new(cwd);
                    self.scope_paths
                        .iter()
                        .any(|scope| cwd == scope || cwd.starts_with(scope))
                })
            })
            .cloned()
            .collect::<Vec<_>>();
        let mut limited = limit_cli_agent_session_sources(records, self.logical_limit);
        let mut selected = limited
            .iter()
            .map(|record| (record.agent, record.provider_session_id.clone()))
            .collect::<HashSet<_>>();
        for record in scope_records.drain(..) {
            let identity = (record.agent, record.provider_session_id.clone());
            if selected.insert(identity) {
                limited.push(record);
            }
        }
        AgentSessionDiscoveryResult::Complete {
            providers: observed_providers,
            records: limited,
        }
    }
}

#[cfg(feature = "local_fs")]
impl CliAgentSessionSource for AgentSessionDiscoveryRecord {
    fn agent_key(&self) -> String {
        self.agent.to_serialized_name()
    }

    fn provider_session_id(&self) -> &str {
        &self.provider_session_id
    }

    fn physical_source_key(&self) -> String {
        self.source.physical_key()
    }

    fn modified_epoch_millis(&self) -> i64 {
        self.modified_epoch_millis
    }
}

#[cfg(feature = "local_fs")]
pub(crate) fn provider_source_exists(
    provider: AgentSessionDiscoveryProvider,
    roots: &CliAgentStoreRoots,
) -> Result<bool, CliAgentSessionScanError> {
    let paths = roots.provider_session_discovery_roots(provider.agent());
    paths.into_iter().try_fold(false, |found, path| {
        if found {
            return Ok(true);
        }
        match fs::symlink_metadata(&path) {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(CliAgentSessionScanError::io(
                &path,
                "读取 CLI-agent session source metadata",
                error,
            )),
        }
    })
}

#[cfg(feature = "local_fs")]
pub(crate) fn scan_agent_session_provider(
    provider: AgentSessionDiscoveryProvider,
    roots: &CliAgentStoreRoots,
    logical_limit: usize,
) -> Result<Vec<AgentSessionDiscoveryRecord>, CliAgentSessionScanError> {
    match provider {
        AgentSessionDiscoveryProvider::Claude => recent_jsonl_files(
            &roots.claude_projects(),
            logical_limit,
            Some(crate::app_state::WORKSPACE_SESSION_NAVIGATOR_PHYSICAL_SOURCE_LIMIT_PER_PROVIDER),
        )?
        .into_iter()
        .map(|file| parse_claude_discovery_record(file, roots))
        .collect(),
        AgentSessionDiscoveryProvider::Codex => {
            let physical_limit =
                crate::app_state::WORKSPACE_SESSION_NAVIGATOR_PHYSICAL_SOURCE_LIMIT_PER_PROVIDER;
            let mut records =
                recent_jsonl_files(&roots.codex_sessions(), logical_limit, Some(physical_limit))?
                    .into_iter()
                    .map(|file| parse_codex_discovery_record(file, roots))
                    .collect::<Result<Vec<_>, _>>()?;
            records.extend(parse_codex_discovery_index(
                &roots.codex_index(),
                roots,
                physical_limit,
            )?);
            Ok(records)
        }
        AgentSessionDiscoveryProvider::Droid => scan_jsonl_provider_roots(
            CLIAgent::Droid,
            &[roots.droid_sessions(), roots.droid_projects()],
            logical_limit,
            roots,
            parse_droid_discovery_record,
        ),
        AgentSessionDiscoveryProvider::OpenCode => {
            scan_opencode_discovery_records(roots, logical_limit)
        }
        AgentSessionDiscoveryProvider::Copilot => scan_jsonl_provider_roots(
            CLIAgent::Copilot,
            &[roots.copilot_sessions()],
            logical_limit,
            roots,
            parse_copilot_discovery_record,
        ),
        AgentSessionDiscoveryProvider::Pi => scan_jsonl_provider_roots(
            CLIAgent::Pi,
            &[roots.pi_sessions()],
            logical_limit,
            roots,
            parse_pi_discovery_record,
        ),
        AgentSessionDiscoveryProvider::Cursor => {
            scan_cursor_discovery_records(roots, logical_limit)
        }
        AgentSessionDiscoveryProvider::Antigravity => scan_jsonl_provider_roots(
            CLIAgent::Antigravity,
            &[roots.antigravity_brain()],
            logical_limit,
            roots,
            parse_antigravity_discovery_record,
        ),
        AgentSessionDiscoveryProvider::Omp => recent_omp_session_files(
            &roots.omp_sessions(),
            logical_limit,
            crate::app_state::WORKSPACE_SESSION_NAVIGATOR_PHYSICAL_SOURCE_LIMIT_PER_PROVIDER,
        )?
        .into_iter()
        .filter_map(|file| parse_omp_discovery_record(file, roots).transpose())
        .collect(),
    }
}

#[cfg(feature = "local_fs")]
fn scan_jsonl_provider_roots(
    _agent: CLIAgent,
    provider_roots: &[PathBuf],
    logical_limit: usize,
    roots: &CliAgentStoreRoots,
    parser: fn(
        RecentJsonlFile,
        &CliAgentStoreRoots,
    ) -> Result<Option<AgentSessionDiscoveryRecord>, CliAgentSessionScanError>,
) -> Result<Vec<AgentSessionDiscoveryRecord>, CliAgentSessionScanError> {
    let physical_limit =
        crate::app_state::WORKSPACE_SESSION_NAVIGATOR_PHYSICAL_SOURCE_LIMIT_PER_PROVIDER;
    let mut files = Vec::new();
    for provider_root in provider_roots {
        files.extend(recent_jsonl_files(
            provider_root,
            physical_limit,
            Some(physical_limit),
        )?);
    }
    sort_recent_files(&mut files);
    let mut records = Vec::new();
    for file in files {
        if let Some(record) = parser(file, roots)? {
            records.push(record);
        }
    }
    Ok(limit_cli_agent_session_sources(records, logical_limit))
}

#[cfg(feature = "local_fs")]
fn generic_session_record(
    agent: CLIAgent,
    file: RecentJsonlFile,
    roots: &CliAgentStoreRoots,
    provider_session_id: Option<String>,
    label: Option<String>,
    cwd: Option<String>,
) -> AgentSessionDiscoveryRecord {
    let fallback_id = file
        .path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    AgentSessionDiscoveryRecord {
        agent,
        provider_session_id: provider_session_id.unwrap_or(fallback_id),
        source: AgentSessionDiscoverySource::Transcript(file.path),
        label: label.and_then(|label| super::parse::first_message_excerpt(&label)),
        cwd: normalize_cli_agent_session_cwd(cwd.as_deref(), roots),
        modified_epoch_millis: system_time_to_epoch_millis(file.modified),
    }
}

#[cfg(feature = "local_fs")]
fn json_content_text(value: &serde_json::Value) -> Option<String> {
    value.as_str().map(str::to_owned).or_else(|| {
        value.as_array().and_then(|items| {
            items.iter().find_map(|item| {
                first_string(&[string_field(item, "text"), string_field(item, "content")])
            })
        })
    })
}

#[cfg(feature = "local_fs")]
fn json_message_text(value: &serde_json::Value) -> Option<String> {
    first_string(&[
        string_field(value, "text"),
        string_field(value, "content"),
        super::parse::nested_string(value, &["message", "content"]),
        super::parse::nested_string(value, &["data", "content"]),
        super::parse::nested_string(value, &["data", "transformedContent"]),
    ])
    .or_else(|| value.get("content").and_then(json_content_text))
    .or_else(|| {
        value
            .get("message")
            .and_then(|message| message.get("content"))
            .and_then(json_content_text)
    })
    .or_else(|| {
        value
            .get("data")
            .and_then(|data| data.get("content"))
            .and_then(json_content_text)
    })
}

#[cfg(feature = "local_fs")]
fn record_role(value: &serde_json::Value) -> Option<&str> {
    string_field(value, "role")
        .or_else(|| super::parse::nested_string(value, &["message", "role"]))
        .or_else(|| super::parse::nested_string(value, &["data", "role"]))
}

#[cfg(feature = "local_fs")]
fn parse_generic_jsonl_metadata(
    path: &Path,
) -> Result<(Option<String>, Option<String>, Option<String>), CliAgentSessionScanError> {
    let values = read_jsonl_values_from_path(path, Some(300))?;
    let mut session_id = None;
    let mut cwd = None;
    let mut provider_title = None;
    let mut first_user_message = None;
    for value in &values {
        if session_id.is_none() {
            session_id = first_string(&[
                string_field(value, "session_id"),
                string_field(value, "sessionId"),
                string_field(value, "id"),
                super::parse::nested_string(value, &["data", "sessionId"]),
            ]);
        }
        if cwd.is_none() {
            cwd = first_string(&[
                string_field(value, "cwd"),
                string_field(value, "directory"),
                string_field(value, "working_directory"),
                super::parse::nested_string(value, &["data", "cwd"]),
            ]);
        }
        if provider_title.is_none() {
            provider_title = first_string(&[
                string_field(value, "title"),
                string_field(value, "customTitle"),
                super::parse::nested_string(value, &["data", "title"]),
            ]);
        }
        if first_user_message.is_none() && record_role(value) == Some("user") {
            first_user_message = json_message_text(value);
        }
        if first_user_message.is_none() && string_field(value, "type") == Some("user.message") {
            first_user_message = json_message_text(value);
        }
    }
    Ok((session_id, cwd, provider_title.or(first_user_message)))
}

#[cfg(feature = "local_fs")]
fn parse_droid_discovery_record(
    file: RecentJsonlFile,
    roots: &CliAgentStoreRoots,
) -> Result<Option<AgentSessionDiscoveryRecord>, CliAgentSessionScanError> {
    let (session_id, cwd, title) = parse_generic_jsonl_metadata(&file.path)?;
    Ok(Some(generic_session_record(
        CLIAgent::Droid,
        file,
        roots,
        session_id,
        title,
        cwd,
    )))
}

#[cfg(feature = "local_fs")]
fn parse_copilot_discovery_record(
    file: RecentJsonlFile,
    roots: &CliAgentStoreRoots,
) -> Result<Option<AgentSessionDiscoveryRecord>, CliAgentSessionScanError> {
    let (session_id, cwd, title) = parse_generic_jsonl_metadata(&file.path)?;
    Ok(Some(generic_session_record(
        CLIAgent::Copilot,
        file,
        roots,
        session_id,
        title,
        cwd,
    )))
}

#[cfg(feature = "local_fs")]
fn parse_pi_discovery_record(
    file: RecentJsonlFile,
    roots: &CliAgentStoreRoots,
) -> Result<Option<AgentSessionDiscoveryRecord>, CliAgentSessionScanError> {
    let (session_id, cwd, title) = parse_generic_jsonl_metadata(&file.path)?;
    Ok(Some(generic_session_record(
        CLIAgent::Pi,
        file,
        roots,
        session_id,
        title,
        cwd,
    )))
}

#[cfg(feature = "local_fs")]
fn parse_cursor_discovery_record(
    file: RecentJsonlFile,
    roots: &CliAgentStoreRoots,
) -> Result<Option<AgentSessionDiscoveryRecord>, CliAgentSessionScanError> {
    if file
        .path
        .components()
        .any(|component| component.as_os_str() == "subagents")
    {
        return Ok(None);
    }
    if !file
        .path
        .components()
        .any(|component| component.as_os_str() == "agent-transcripts")
    {
        return Ok(None);
    }
    let (mut session_id, cwd, title) = parse_generic_jsonl_metadata(&file.path)?;
    if session_id.is_none() {
        session_id = file
            .path
            .parent()
            .and_then(|parent| parent.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .filter(|name| !name.is_empty());
    }
    Ok(Some(generic_session_record(
        CLIAgent::CursorCli,
        file,
        roots,
        session_id,
        title,
        cwd,
    )))
}

#[cfg(feature = "local_fs")]
#[derive(serde::Deserialize)]
struct CursorChatMeta {
    title: Option<String>,
    cwd: Option<String>,
    #[serde(rename = "updatedAtMs")]
    updated_at_ms: Option<i64>,
    #[serde(rename = "createdAtMs")]
    created_at_ms: Option<i64>,
}

#[cfg(feature = "local_fs")]
fn scan_cursor_discovery_records(
    roots: &CliAgentStoreRoots,
    logical_limit: usize,
) -> Result<Vec<AgentSessionDiscoveryRecord>, CliAgentSessionScanError> {
    let physical_limit =
        crate::app_state::WORKSPACE_SESSION_NAVIGATOR_PHYSICAL_SOURCE_LIMIT_PER_PROVIDER;
    let mut records = scan_cursor_chats_store_records(roots, physical_limit)?;
    let mut indexed_ids: HashSet<String> = records
        .iter()
        .map(|record| record.provider_session_id.clone())
        .collect();
    let jsonl_records = scan_jsonl_provider_roots(
        CLIAgent::CursorCli,
        &[roots.cursor_projects()],
        logical_limit,
        roots,
        parse_cursor_discovery_record,
    )?;
    for record in jsonl_records {
        if indexed_ids.insert(record.provider_session_id.clone()) {
            records.push(record);
        }
    }
    Ok(limit_cli_agent_session_sources(records, logical_limit))
}

#[cfg(feature = "local_fs")]
fn scan_cursor_chats_store_records(
    roots: &CliAgentStoreRoots,
    physical_limit: usize,
) -> Result<Vec<AgentSessionDiscoveryRecord>, CliAgentSessionScanError> {
    let chats_root = roots.cursor_chats();
    let root_metadata = match fs::symlink_metadata(&chats_root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(CliAgentSessionScanError::io(
                &chats_root,
                "读取 Cursor CLI chats 目录",
                error,
            ));
        }
    };
    if !root_metadata.file_type().is_dir() {
        return Err(CliAgentSessionScanError::expected_directory(&chats_root));
    }

    let mut records = Vec::new();
    let mut candidate_count = 0;
    let project_entries = fs::read_dir(&chats_root).map_err(|error| {
        CliAgentSessionScanError::io(&chats_root, "枚举 Cursor CLI project bucket", error)
    })?;
    for project_entry in project_entries {
        let project_entry = project_entry.map_err(|error| {
            CliAgentSessionScanError::io(&chats_root, "读取 Cursor CLI project bucket", error)
        })?;
        if !project_entry
            .file_type()
            .map(|file_type| file_type.is_dir())
            .unwrap_or(false)
        {
            continue;
        }
        let project_path = project_entry.path();
        let session_entries = fs::read_dir(&project_path).map_err(|error| {
            CliAgentSessionScanError::io(&project_path, "枚举 Cursor CLI session", error)
        })?;
        for session_entry in session_entries {
            let session_entry = session_entry.map_err(|error| {
                CliAgentSessionScanError::io(&project_path, "读取 Cursor CLI session", error)
            })?;
            if !session_entry
                .file_type()
                .map(|file_type| file_type.is_dir())
                .unwrap_or(false)
            {
                continue;
            }
            reserve_discovery_candidate(&chats_root, &mut candidate_count, Some(physical_limit))?;
            let session_dir = session_entry.path();
            let store_db = session_dir.join("store.db");
            let meta_path = session_dir.join("meta.json");
            if !store_db.is_file() || !meta_path.is_file() {
                continue;
            }
            let session_id = session_dir
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .filter(|id| !id.is_empty());
            let Some(session_id) = session_id else {
                continue;
            };
            let meta_text = fs::read_to_string(&meta_path).map_err(|error| {
                CliAgentSessionScanError::io(&meta_path, "读取 Cursor CLI session meta", error)
            })?;
            let meta: CursorChatMeta = serde_json::from_str(&meta_text).map_err(|error| {
                CliAgentSessionScanError::parse(
                    &meta_path,
                    "解析 Cursor CLI session meta",
                    error.to_string(),
                )
            })?;
            let modified = fs::metadata(&store_db)
                .and_then(|metadata| metadata.modified())
                .map(system_time_to_epoch_millis)
                .unwrap_or(0);
            let modified_epoch_millis = meta
                .updated_at_ms
                .or(meta.created_at_ms)
                .unwrap_or(modified);
            records.push(AgentSessionDiscoveryRecord {
                agent: CLIAgent::CursorCli,
                provider_session_id: session_id.clone(),
                source: AgentSessionDiscoverySource::CursorCliStoreEntry {
                    path: store_db,
                    provider_session_id: session_id,
                },
                label: meta
                    .title
                    .as_deref()
                    .and_then(super::parse::first_message_excerpt),
                cwd: normalize_cli_agent_session_cwd(meta.cwd.as_deref(), roots),
                modified_epoch_millis,
            });
        }
    }
    records.sort_by(|left, right| {
        right
            .modified_epoch_millis
            .cmp(&left.modified_epoch_millis)
            .then_with(|| left.provider_session_id.cmp(&right.provider_session_id))
    });
    Ok(records)
}

#[cfg(feature = "local_fs")]
fn parse_antigravity_discovery_record(
    file: RecentJsonlFile,
    roots: &CliAgentStoreRoots,
) -> Result<Option<AgentSessionDiscoveryRecord>, CliAgentSessionScanError> {
    if file.path.file_name().and_then(|name| name.to_str()) != Some("transcript.jsonl") {
        return Ok(None);
    }
    let session_id = file
        .path
        .ancestors()
        .nth(3)
        .and_then(Path::file_name)
        .map(|name| name.to_string_lossy().into_owned());
    let values = read_jsonl_values_from_path(&file.path, Some(300))?;
    let title = values.iter().find_map(|value| {
        let source = string_field(value, "source");
        let kind = string_field(value, "type");
        if matches!(source, Some("USER_EXPLICIT") | Some("USER"))
            && matches!(kind, Some("USER_INPUT") | Some("REQUEST"))
        {
            let content = string_field(value, "content")?;
            let content = content
                .split_once("<USER_REQUEST>")
                .map(|(_, content)| content.split("</USER_REQUEST>").next().unwrap_or(content))
                .unwrap_or(content);
            return super::parse::first_message_excerpt(content);
        }
        None
    });
    Ok(Some(generic_session_record(
        CLIAgent::Antigravity,
        file,
        roots,
        session_id,
        title,
        None,
    )))
}

#[cfg(feature = "local_fs")]
#[derive(QueryableByName)]
struct SqliteColumnName {
    #[diesel(sql_type = Text)]
    name: String,
}

#[cfg(feature = "local_fs")]
#[derive(QueryableByName)]
struct OpenCodeSqliteSessionRow {
    #[diesel(sql_type = Text)]
    id: String,
    #[diesel(sql_type = Nullable<Text>)]
    title: Option<String>,
    #[diesel(sql_type = Nullable<Text>)]
    directory: Option<String>,
    #[diesel(sql_type = BigInt)]
    time_created: i64,
    #[diesel(sql_type = BigInt)]
    time_updated: i64,
}

#[cfg(feature = "local_fs")]
fn scan_opencode_discovery_records(
    roots: &CliAgentStoreRoots,
    logical_limit: usize,
) -> Result<Vec<AgentSessionDiscoveryRecord>, CliAgentSessionScanError> {
    let physical_limit =
        crate::app_state::WORKSPACE_SESSION_NAVIGATOR_PHYSICAL_SOURCE_LIMIT_PER_PROVIDER;
    let mut sqlite_records = scan_opencode_sqlite_records(roots, physical_limit)?;
    let sqlite_ids = sqlite_records
        .iter()
        .map(|record| record.provider_session_id.clone())
        .collect::<HashSet<_>>();
    let mut legacy_records =
        recent_files_with_extensions(&roots.opencode_legacy_sessions(), &["json"], physical_limit)?
            .into_iter()
            .map(|file| parse_opencode_legacy_record(file, roots))
            .collect::<Result<Vec<_>, _>>()?;
    legacy_records.retain(|record| !sqlite_ids.contains(&record.provider_session_id));
    sqlite_records.extend(legacy_records);
    Ok(limit_cli_agent_session_sources(
        sqlite_records,
        logical_limit,
    ))
}

#[cfg(feature = "local_fs")]
fn scan_opencode_sqlite_records(
    roots: &CliAgentStoreRoots,
    physical_limit: usize,
) -> Result<Vec<AgentSessionDiscoveryRecord>, CliAgentSessionScanError> {
    let data_dir = roots.opencode_databases_dir();
    let entries = match fs::read_dir(&data_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(CliAgentSessionScanError::io(
                &data_dir,
                "读取 OpenCode 数据目录",
                error,
            ));
        }
    };
    let mut database_paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| {
            CliAgentSessionScanError::io(&data_dir, "遍历 OpenCode 数据目录", error)
        })?;
        let path = entry.path();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        if entry
            .file_type()
            .map_err(|error| {
                CliAgentSessionScanError::io(&path, "读取 OpenCode 数据库文件类型", error)
            })?
            .is_file()
            && name.starts_with("opencode")
            && name.ends_with(".db")
        {
            database_paths.push(path);
        }
    }
    database_paths.sort();

    let mut records_by_id = HashMap::<String, AgentSessionDiscoveryRecord>::new();
    for path in database_paths {
        let mut connection = diesel::sqlite::SqliteConnection::establish(&path.to_string_lossy())
            .map_err(|error| {
            invalid_session_record_owned(&path, format!("打开 OpenCode SQLite: {error}"))
        })?;
        connection
            .batch_execute("PRAGMA query_only = ON")
            .map_err(|error| {
                invalid_session_record_owned(&path, format!("设置 OpenCode SQLite 只读: {error}"))
            })?;
        let columns = diesel::sql_query("PRAGMA table_info(session)")
            .load::<SqliteColumnName>(&mut connection)
            .map_err(|error| {
                invalid_session_record_owned(
                    &path,
                    format!("读取 OpenCode session schema: {error}"),
                )
            })?
            .into_iter()
            .map(|column| column.name)
            .collect::<HashSet<_>>();
        if !["id", "time_created", "time_updated"]
            .iter()
            .all(|column| columns.contains(*column))
        {
            continue;
        }
        let title = columns
            .contains("title")
            .then_some("title")
            .unwrap_or("NULL");
        let directory = columns
            .contains("directory")
            .then_some("directory")
            .unwrap_or("NULL");
        let parent = columns
            .contains("parent_id")
            .then_some("AND parent_id IS NULL")
            .unwrap_or("");
        let archived = columns
            .contains("time_archived")
            .then_some("AND time_archived IS NULL")
            .unwrap_or("");
        let query = format!(
            "SELECT id, {title} AS title, {directory} AS directory, time_created, time_updated \
             FROM session WHERE 1=1 {parent} {archived} \
             ORDER BY CASE WHEN time_updated > 0 THEN time_updated ELSE time_created END DESC \
             LIMIT {physical_limit}"
        );
        let rows = diesel::sql_query(query)
            .load::<OpenCodeSqliteSessionRow>(&mut connection)
            .map_err(|error| {
                invalid_session_record_owned(&path, format!("读取 OpenCode sessions: {error}"))
            })?;
        for row in rows {
            let modified_epoch_millis = if row.time_updated > 0 {
                row.time_updated
            } else {
                row.time_created
            };
            let record = AgentSessionDiscoveryRecord {
                agent: CLIAgent::OpenCode,
                provider_session_id: row.id.clone(),
                source: AgentSessionDiscoverySource::OpenCodeSqliteEntry {
                    path: path.clone(),
                    provider_session_id: row.id.clone(),
                },
                label: row
                    .title
                    .and_then(|title| super::parse::first_message_excerpt(&title)),
                cwd: normalize_cli_agent_session_cwd(row.directory.as_deref(), roots),
                modified_epoch_millis,
            };
            if records_by_id
                .get(&row.id)
                .is_none_or(|existing| existing.modified_epoch_millis < modified_epoch_millis)
            {
                records_by_id.insert(row.id, record);
            }
        }
    }
    Ok(records_by_id.into_values().collect())
}

#[cfg(feature = "local_fs")]
fn parse_opencode_legacy_record(
    file: RecentJsonlFile,
    roots: &CliAgentStoreRoots,
) -> Result<AgentSessionDiscoveryRecord, CliAgentSessionScanError> {
    let value = serde_json::from_reader::<_, serde_json::Value>(
        fs::File::open(&file.path).map_err(|error| {
            CliAgentSessionScanError::io(&file.path, "读取 OpenCode legacy session", error)
        })?,
    )
    .map_err(|error| {
        invalid_session_record_owned(&file.path, format!("解析 OpenCode legacy session: {error}"))
    })?;
    let provider_session_id = string_field(&value, "id")
        .map(str::to_owned)
        .or_else(|| {
            file.path
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
        })
        .ok_or_else(|| invalid_session_record(&file.path, "OpenCode legacy session 缺少 id"))?;
    Ok(AgentSessionDiscoveryRecord {
        agent: CLIAgent::OpenCode,
        provider_session_id,
        source: AgentSessionDiscoverySource::Transcript(file.path),
        label: string_field(&value, "title").and_then(super::parse::first_message_excerpt),
        cwd: normalize_cli_agent_session_cwd(string_field(&value, "directory"), roots),
        modified_epoch_millis: value
            .get("time")
            .and_then(|time| time.get("updated").or_else(|| time.get("created")))
            .and_then(serde_json::Value::as_i64)
            .unwrap_or_else(|| system_time_to_epoch_millis(file.modified)),
    })
}

#[cfg(feature = "local_fs")]
fn recent_files_with_extensions(
    root: &Path,
    extensions: &[&str],
    physical_limit: usize,
) -> Result<Vec<RecentJsonlFile>, CliAgentSessionScanError> {
    let metadata = match fs::symlink_metadata(root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(CliAgentSessionScanError::io(
                root,
                "读取 session 目录",
                error,
            ))
        }
    };
    if !metadata.file_type().is_dir() {
        return Err(CliAgentSessionScanError::expected_directory(root));
    }
    let mut files = Vec::new();
    let mut candidate_count = 0;
    for entry in walkdir::WalkDir::new(root).follow_links(false) {
        let entry = entry.map_err(|error| CliAgentSessionScanError::walk(root, error))?;
        if !entry.file_type().is_file()
            || !entry
                .path()
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extensions.contains(&extension))
        {
            continue;
        }
        reserve_discovery_candidate(root, &mut candidate_count, Some(physical_limit))?;
        let metadata = fs::metadata(entry.path()).map_err(|error| {
            CliAgentSessionScanError::io(entry.path(), "读取 session metadata", error)
        })?;
        let modified = metadata.modified().map_err(|error| {
            CliAgentSessionScanError::io(entry.path(), "读取 session mtime", error)
        })?;
        files.push(RecentJsonlFile {
            path: entry.path().to_path_buf(),
            modified,
        });
    }
    sort_recent_files(&mut files);
    Ok(files)
}

#[cfg(feature = "local_fs")]
fn system_time_to_epoch_millis(modified: SystemTime) -> i64 {
    modified
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(feature = "local_fs")]
fn parse_claude_discovery_record(
    file: RecentJsonlFile,
    roots: &CliAgentStoreRoots,
) -> Result<AgentSessionDiscoveryRecord, CliAgentSessionScanError> {
    let metadata = claude_session_metadata(&read_jsonl_values_from_path(&file.path, Some(200))?);
    let fallback_id = file
        .path
        .file_stem()
        .expect("recent JSONL candidate must have a file stem")
        .to_string_lossy()
        .into_owned();
    let label = metadata.display_title();
    let cwd = normalize_cli_agent_session_cwd(metadata.cwd.as_deref(), roots);
    Ok(AgentSessionDiscoveryRecord {
        agent: CLIAgent::Claude,
        provider_session_id: metadata.session_id.unwrap_or(fallback_id),
        source: AgentSessionDiscoverySource::Transcript(file.path),
        label,
        cwd,
        modified_epoch_millis: system_time_to_epoch_millis(file.modified),
    })
}

#[cfg(feature = "local_fs")]
fn parse_codex_discovery_record(
    file: RecentJsonlFile,
    roots: &CliAgentStoreRoots,
) -> Result<AgentSessionDiscoveryRecord, CliAgentSessionScanError> {
    let metadata = codex_session_metadata(&read_jsonl_values_from_path(&file.path, Some(200))?);
    let file_stem = file
        .path
        .file_stem()
        .expect("recent JSONL candidate must have a file stem")
        .to_string_lossy()
        .into_owned();
    let label = metadata.display_title();
    let cwd = normalize_cli_agent_session_cwd(metadata.cwd.as_deref(), roots);
    let provider_session_id = metadata
        .session_id
        .or_else(|| canonical_codex_session_id(&file_stem))
        .ok_or_else(|| {
            CliAgentSessionScanError::io(
                &file.path,
                "解析 Codex provider session id",
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "transcript has no canonical Codex session UUID",
                ),
            )
        })?;
    Ok(AgentSessionDiscoveryRecord {
        agent: CLIAgent::Codex,
        provider_session_id,
        source: AgentSessionDiscoverySource::Transcript(file.path),
        label,
        cwd,
        modified_epoch_millis: system_time_to_epoch_millis(file.modified),
    })
}

#[cfg(feature = "local_fs")]
fn parse_omp_discovery_record(
    file: RecentJsonlFile,
    roots: &CliAgentStoreRoots,
) -> Result<Option<AgentSessionDiscoveryRecord>, CliAgentSessionScanError> {
    let values = read_jsonl_prefix_values(&file.path, 8 * 1024)?;
    let mut title_slot = None;
    let mut header = None;
    for value in values {
        match string_field(&value, "type") {
            Some("title") if title_slot.is_none() && header.is_none() => {
                title_slot = string_field(&value, "title").map(str::to_owned);
            }
            Some("session") if header.is_none() => header = Some(value),
            Some(_) | None => {}
        }
    }
    let Some(header) = header else {
        return Ok(None);
    };
    let provider_session_id = string_field(&header, "id")
        .map(str::to_owned)
        .ok_or_else(|| invalid_session_record(&file.path, "Omp session header 缺少非空 id"))?;
    let file_stem = file
        .path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .ok_or_else(|| invalid_session_record(&file.path, "Omp session filename 无法解析"))?;
    if file_stem.rsplit_once('_').map(|(_, id)| id) != Some(provider_session_id.as_str()) {
        return Err(invalid_session_record(
            &file.path,
            "Omp session id 必须与 <timestamp>_<id>.jsonl filename 一致",
        ));
    }

    Ok(Some(AgentSessionDiscoveryRecord {
        agent: CLIAgent::Omp,
        provider_session_id,
        source: AgentSessionDiscoverySource::Transcript(file.path),
        label: title_slot.or_else(|| string_field(&header, "title").map(str::to_owned)),
        cwd: normalize_cli_agent_session_cwd(string_field(&header, "cwd"), roots),
        modified_epoch_millis: system_time_to_epoch_millis(file.modified),
    }))
}

#[cfg(feature = "local_fs")]
fn invalid_session_record(path: &Path, message: &'static str) -> CliAgentSessionScanError {
    CliAgentSessionScanError::io(
        path,
        "解析 CLI-agent session metadata",
        io::Error::new(io::ErrorKind::InvalidData, message),
    )
}

#[cfg(feature = "local_fs")]
fn invalid_session_record_owned(path: &Path, message: String) -> CliAgentSessionScanError {
    CliAgentSessionScanError::io(
        path,
        "解析 CLI-agent session metadata",
        io::Error::new(io::ErrorKind::InvalidData, message),
    )
}

#[cfg(feature = "local_fs")]
pub(crate) fn parse_codex_discovery_index(
    path: &Path,
    roots: &CliAgentStoreRoots,
    physical_limit: usize,
) -> Result<Vec<AgentSessionDiscoveryRecord>, CliAgentSessionScanError> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(CliAgentSessionScanError::io(
                path,
                "读取 Codex session index metadata",
                error,
            ));
        }
    };
    if !metadata.is_file() {
        return Err(CliAgentSessionScanError::io(
            path,
            "读取 Codex session index",
            io::Error::new(io::ErrorKind::InvalidData, "路径存在但不是普通文件"),
        ));
    }
    let modified = metadata.modified().map_err(|error| {
        CliAgentSessionScanError::io(path, "读取 Codex session index mtime", error)
    })?;
    let fallback_modified = system_time_to_epoch_millis(modified);
    Ok(
        read_jsonl_values_from_path_with_physical_line_limit(path, physical_limit)?
            .into_iter()
            .filter_map(|value| {
                let record = codex_session_index_record(&value)?;
                Some(AgentSessionDiscoveryRecord {
                    agent: CLIAgent::Codex,
                    provider_session_id: record.session_id.clone(),
                    source: AgentSessionDiscoverySource::CodexIndexEntry {
                        path: path.to_path_buf(),
                        provider_session_id: record.session_id,
                    },
                    label: record.title,
                    cwd: normalize_cli_agent_session_cwd(record.cwd.as_deref(), roots),
                    modified_epoch_millis: record
                        .updated_at_epoch_millis
                        .unwrap_or(fallback_modified),
                })
            })
            .collect(),
    )
}

/// 完整发现 `root` 下最近的普通 JSONL 文件。
///
/// 不存在的 provider store 是合法空集；store 一旦存在，任何遍历、metadata
/// 或 mtime 错误都会使整次扫描失败，禁止把 partial result 当成 Success。
#[cfg(feature = "local_fs")]
pub(crate) fn recent_jsonl_files(
    root: &Path,
    limit: usize,
    physical_limit: Option<usize>,
) -> Result<Vec<RecentJsonlFile>, CliAgentSessionScanError> {
    let root_metadata = match fs::symlink_metadata(root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(CliAgentSessionScanError::io(
                root,
                "读取 CLI-agent 会话目录",
                error,
            ));
        }
    };
    if !root_metadata.file_type().is_dir() {
        return Err(CliAgentSessionScanError::expected_directory(root));
    }

    let mut files = Vec::new();
    let mut candidate_count = 0;
    for entry in walkdir::WalkDir::new(root).follow_links(false) {
        let entry = entry.map_err(|error| CliAgentSessionScanError::walk(root, error))?;
        if !entry.file_type().is_file()
            || entry
                .path()
                .extension()
                .is_none_or(|extension| extension != "jsonl")
        {
            continue;
        }
        reserve_discovery_candidate(root, &mut candidate_count, physical_limit)?;
        let metadata = fs::metadata(entry.path()).map_err(|error| {
            CliAgentSessionScanError::io(entry.path(), "读取 CLI-agent 会话文件 metadata", error)
        })?;
        let modified = metadata.modified().map_err(|error| {
            CliAgentSessionScanError::io(entry.path(), "读取 CLI-agent 会话文件 mtime", error)
        })?;
        files.push(RecentJsonlFile {
            path: entry.path().to_path_buf(),
            modified,
        });
    }

    files.sort_by(|left, right| {
        right
            .modified
            .cmp(&left.modified)
            .then_with(|| left.path.cmp(&right.path))
    });
    files.truncate(limit);
    Ok(files)
}

/// 发现 Omp 默认 storage 中恰好一层 project bucket 下的 `.jsonl` session。
///
/// Omp 可能在同名目录中保存 tool log；只扫描 `sessions/*/*.jsonl`，不能递归
/// 扩展成 `sessions/**/*.jsonl`。
#[cfg(feature = "local_fs")]
pub(crate) fn recent_omp_session_files(
    root: &Path,
    limit: usize,
    physical_limit: usize,
) -> Result<Vec<RecentJsonlFile>, CliAgentSessionScanError> {
    let root_metadata = match fs::symlink_metadata(root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(CliAgentSessionScanError::io(
                root,
                "读取 Omp session 目录",
                error,
            ));
        }
    };
    if !root_metadata.file_type().is_dir() {
        return Err(CliAgentSessionScanError::expected_directory(root));
    }

    let mut files = Vec::new();
    let mut candidate_count = 0;
    let project_entries = fs::read_dir(root)
        .map_err(|error| CliAgentSessionScanError::io(root, "读取 Omp session 目录", error))?;
    for project_entry in project_entries {
        let project_entry = project_entry
            .map_err(|error| CliAgentSessionScanError::io(root, "遍历 Omp session 目录", error))?;
        let file_type = project_entry.file_type().map_err(|error| {
            CliAgentSessionScanError::io(
                &project_entry.path(),
                "读取 Omp project bucket metadata",
                error,
            )
        })?;
        if !file_type.is_dir() {
            continue;
        }
        files.extend(direct_regular_files(
            &project_entry.path(),
            "读取 Omp project session 目录",
            physical_limit,
            &mut candidate_count,
            |path| is_omp_session_source(root, path),
        )?);
    }
    sort_and_limit_recent_files(&mut files, limit);
    Ok(files)
}

#[cfg(feature = "local_fs")]
fn direct_regular_files(
    root: &Path,
    operation: &'static str,
    physical_limit: usize,
    candidate_count: &mut usize,
    mut is_candidate: impl FnMut(&Path) -> bool,
) -> Result<Vec<RecentJsonlFile>, CliAgentSessionScanError> {
    let root_metadata = fs::symlink_metadata(root)
        .map_err(|error| CliAgentSessionScanError::io(root, operation, error))?;
    if !root_metadata.file_type().is_dir() {
        return Err(CliAgentSessionScanError::expected_directory(root));
    }
    let mut files = Vec::new();
    let entries =
        fs::read_dir(root).map_err(|error| CliAgentSessionScanError::io(root, operation, error))?;
    for entry in entries {
        let entry = entry.map_err(|error| CliAgentSessionScanError::io(root, operation, error))?;
        let file_type = entry.file_type().map_err(|error| {
            CliAgentSessionScanError::io(&entry.path(), "读取 CLI-agent 会话文件类型", error)
        })?;
        let path = entry.path();
        if !file_type.is_file() || !is_candidate(&path) {
            continue;
        }
        reserve_discovery_candidate(root, candidate_count, Some(physical_limit))?;
        let metadata = entry.metadata().map_err(|error| {
            CliAgentSessionScanError::io(&entry.path(), "读取 CLI-agent 会话文件 metadata", error)
        })?;
        let modified = metadata.modified().map_err(|error| {
            CliAgentSessionScanError::io(&entry.path(), "读取 CLI-agent 会话文件 mtime", error)
        })?;
        files.push(RecentJsonlFile { path, modified });
    }
    Ok(files)
}
