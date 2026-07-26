//! Shared persisted-session discovery primitives for CLI agents.
//!
//! Consumers parse the same on-disk transcript/index formats:
//!
//! - [`crate::session_bridge::cli_agent_reader`] runs locally and parses a
//!   transcript into a full `SessionIr` (every user/assistant message).
//! - [`crate::environment_runtime_transport::cli_agent_sessions`] runs natively
//!   inside the daemon on the remote host and scans / reads / mutates the stores.
//!
//! JSONL discovery, decoding and lightweight session metadata extraction are
//! shared here because local and remote scans must either observe the same
//! complete store snapshot or fail without replacing their previous cache.
//! An agent whose native store has not been provisioned yet contributes a
//! successful empty set; a missing store is not a transient scan failure.
//! Path allow-listing stays at the mutation/read boundaries because it encodes
//! a different security responsibility.

#[cfg(feature = "local_fs")]
use std::collections::{HashMap, HashSet};
#[cfg(feature = "local_fs")]
use std::fmt;
#[cfg(feature = "local_fs")]
use std::fs;
#[cfg(feature = "local_fs")]
use std::io::{self, Read};
#[cfg(feature = "local_fs")]
use std::path::{Path, PathBuf};
#[cfg(feature = "local_fs")]
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(feature = "local_fs")]
use serde::{de::IgnoredAny, Deserialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use chrono::DateTime;

#[cfg(feature = "local_fs")]
use crate::terminal::CLIAgent;

/// Agent capability registry 对 session discovery provider 的穷尽声明。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum AgentSessionDiscoveryProvider {
    Claude,
    Codex,
    Jcode,
    Omp,
}

/// 一次 discovery generation 要执行的完整共享计划。
#[cfg(feature = "local_fs")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AgentSessionDiscoveryPlan {
    providers: Vec<AgentSessionDiscoveryProvider>,
    logical_limit: usize,
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
    PermanentlyDeleted(AgentSessionDiscoveryProvider),
    Failed(CliAgentSessionScanError),
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
impl AgentSessionDiscoveryTransition {
    pub(crate) fn apply_to(
        self,
        current: Vec<AgentSessionDiscoveryRecord>,
    ) -> Result<Vec<AgentSessionDiscoveryRecord>, CliAgentSessionScanError> {
        match self {
            Self::Replace { records, .. } => Ok(records),
            Self::RemoveProvider(provider) => Ok(current
                .into_iter()
                .filter(|record| record.agent != provider.agent())
                .collect()),
            Self::PreserveSourceMissing(_) | Self::PreserveCancelled => Ok(current),
            Self::PreserveFailed(error) => Err(error),
        }
    }
}

#[cfg(feature = "local_fs")]
impl AgentSessionDiscoveryProvider {
    pub(crate) fn agent(self) -> CLIAgent {
        match self {
            Self::Claude => CLIAgent::Claude,
            Self::Codex => CLIAgent::Codex,
            Self::Jcode => CLIAgent::Jcode,
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

#[cfg(feature = "local_fs")]
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
        }
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
        }
    }

    pub(crate) fn execute(
        &self,
        roots: &CliAgentStoreRoots,
        previously_observed_providers: &HashSet<AgentSessionDiscoveryProvider>,
    ) -> AgentSessionDiscoveryResult {
        if self.logical_limit == 0 {
            return AgentSessionDiscoveryResult::Complete {
                providers: self.providers.clone(),
                records: Vec::new(),
            };
        }

        let mut records = Vec::new();
        for provider in &self.providers {
            let source_exists = match provider_source_exists(*provider, roots) {
                Ok(source_exists) => source_exists,
                Err(error) => return AgentSessionDiscoveryResult::Failed(error),
            };
            if previously_observed_providers.contains(provider) && !source_exists {
                return AgentSessionDiscoveryResult::SourceMissing(*provider);
            }
            match scan_agent_session_provider(*provider, roots, self.logical_limit) {
                Ok(provider_records) => records.extend(provider_records),
                Err(error) => return AgentSessionDiscoveryResult::Failed(error),
            }
        }
        AgentSessionDiscoveryResult::Complete {
            providers: self.providers.clone(),
            records: limit_cli_agent_session_sources(records, self.logical_limit),
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
fn provider_source_exists(
    provider: AgentSessionDiscoveryProvider,
    roots: &CliAgentStoreRoots,
) -> Result<bool, CliAgentSessionScanError> {
    let paths = match provider {
        AgentSessionDiscoveryProvider::Claude => vec![roots.claude_projects()],
        AgentSessionDiscoveryProvider::Codex => vec![roots.codex_sessions(), roots.codex_index()],
        AgentSessionDiscoveryProvider::Jcode => vec![roots.jcode_sessions()],
        AgentSessionDiscoveryProvider::Omp => vec![roots.omp_sessions()],
    };
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
fn scan_agent_session_provider(
    provider: AgentSessionDiscoveryProvider,
    roots: &CliAgentStoreRoots,
    logical_limit: usize,
) -> Result<Vec<AgentSessionDiscoveryRecord>, CliAgentSessionScanError> {
    match provider {
        AgentSessionDiscoveryProvider::Claude => {
            recent_jsonl_files(&roots.claude_projects(), logical_limit)?
                .into_iter()
                .map(|file| parse_claude_discovery_record(file, roots))
                .collect()
        }
        AgentSessionDiscoveryProvider::Codex => {
            let mut records = recent_jsonl_files(&roots.codex_sessions(), logical_limit)?
                .into_iter()
                .map(|file| parse_codex_discovery_record(file, roots))
                .collect::<Result<Vec<_>, _>>()?;
            records.extend(parse_codex_discovery_index(&roots.codex_index(), roots)?);
            Ok(records)
        }
        AgentSessionDiscoveryProvider::Jcode => {
            let mut records = Vec::new();
            for file in recent_jcode_session_files(&roots.jcode_sessions())? {
                if records.len() == logical_limit {
                    break;
                }
                if let Some(record) = parse_jcode_discovery_record(file, roots)? {
                    records.push(record);
                }
            }
            Ok(records)
        }
        AgentSessionDiscoveryProvider::Omp => {
            recent_omp_session_files(&roots.omp_sessions(), logical_limit)?
                .into_iter()
                .map(|file| parse_omp_discovery_record(file, roots))
                .collect()
        }
    }
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
fn parse_jcode_discovery_record(
    file: RecentJsonlFile,
    roots: &CliAgentStoreRoots,
) -> Result<Option<AgentSessionDiscoveryRecord>, CliAgentSessionScanError> {
    let session_file = fs::File::open(&file.path)
        .map_err(|error| CliAgentSessionScanError::io(&file.path, "读取 Jcode session", error))?;
    let metadata =
        serde_json::from_reader::<_, JcodeDiscoveryMetadata>(session_file).map_err(|error| {
            CliAgentSessionScanError::io(
                &file.path,
                "解析 Jcode session metadata",
                io::Error::new(io::ErrorKind::InvalidData, error),
            )
        })?;
    let provider_session_id = metadata
        .id
        .as_deref()
        .filter(|id| !id.trim().is_empty())
        .map(str::to_owned)
        .ok_or_else(|| invalid_session_record(&file.path, "Jcode session 缺少非空 id"))?;
    let file_stem = file
        .path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .ok_or_else(|| invalid_session_record(&file.path, "Jcode session filename 无法解析"))?;
    if !provider_session_id.starts_with("session_") || file_stem != provider_session_id {
        return Err(invalid_session_record(
            &file.path,
            "Jcode session id 必须与 session_*.json filename 一致",
        ));
    }
    if !jcode_session_is_default_discoverable(&metadata) {
        return Ok(None);
    }
    let modified_epoch_millis = metadata
        .updated_at
        .as_deref()
        .and_then(|updated_at| DateTime::parse_from_rfc3339(updated_at).ok())
        .map(|updated_at| updated_at.timestamp_millis())
        .unwrap_or_else(|| system_time_to_epoch_millis(file.modified));

    Ok(Some(AgentSessionDiscoveryRecord {
        agent: CLIAgent::Jcode,
        provider_session_id,
        source: AgentSessionDiscoverySource::Transcript(file.path),
        label: first_string(&[metadata.title.as_deref(), metadata.short_name.as_deref()]),
        cwd: normalize_cli_agent_session_cwd(metadata.working_dir.as_deref(), roots),
        modified_epoch_millis,
    }))
}

/// 只反序列化 Jcode default picker 可见性所需的 root metadata。messages
/// 的每项都由 `IgnoredAny` 消耗，不会将 message content 保留到内存。
#[cfg(feature = "local_fs")]
#[derive(Deserialize)]
struct JcodeDiscoveryMetadata {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    short_name: Option<String>,
    #[serde(default)]
    working_dir: Option<String>,
    #[serde(default)]
    updated_at: Option<String>,
    #[serde(default)]
    is_debug: bool,
    #[serde(default)]
    messages: Option<Vec<IgnoredAny>>,
}

/// Jcode 的默认 picker 会隐藏 self-dev/swarm debug 会话，且不展示没有
/// 结构化 messages 的空 session。这里仅检查 root metadata 与数组长度；
/// `parent_id` 不是可见性条件。
#[cfg(feature = "local_fs")]
fn jcode_session_is_default_discoverable(metadata: &JcodeDiscoveryMetadata) -> bool {
    !metadata.is_debug && jcode_session_has_structural_messages(metadata.messages.as_deref())
}

#[cfg(feature = "local_fs")]
fn jcode_session_has_structural_messages(messages: Option<&[IgnoredAny]>) -> bool {
    messages.is_some_and(|messages| !messages.is_empty())
}

#[cfg(feature = "local_fs")]
fn parse_omp_discovery_record(
    file: RecentJsonlFile,
    roots: &CliAgentStoreRoots,
) -> Result<AgentSessionDiscoveryRecord, CliAgentSessionScanError> {
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
    let header = header
        .ok_or_else(|| invalid_session_record(&file.path, "Omp session 缺少 session header"))?;
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

    Ok(AgentSessionDiscoveryRecord {
        agent: CLIAgent::Omp,
        provider_session_id,
        source: AgentSessionDiscoverySource::Transcript(file.path),
        label: title_slot.or_else(|| string_field(&header, "title").map(str::to_owned)),
        cwd: normalize_cli_agent_session_cwd(string_field(&header, "cwd"), roots),
        modified_epoch_millis: system_time_to_epoch_millis(file.modified),
    })
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
fn parse_codex_discovery_index(
    path: &Path,
    roots: &CliAgentStoreRoots,
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
    Ok(read_jsonl_values_from_path(path, None)?
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
                modified_epoch_millis: record.updated_at_epoch_millis.unwrap_or(fallback_modified),
            })
        })
        .collect())
}

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

#[cfg(feature = "local_fs")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CliAgentSessionScanError {
    path: Option<PathBuf>,
    operation: &'static str,
    message: String,
}

#[cfg(feature = "local_fs")]
impl CliAgentSessionScanError {
    pub(crate) fn io(path: &Path, operation: &'static str, error: io::Error) -> Self {
        Self {
            path: Some(path.to_path_buf()),
            operation,
            message: error.to_string(),
        }
    }

    fn walk(root: &Path, error: walkdir::Error) -> Self {
        Self {
            path: error
                .path()
                .map(Path::to_path_buf)
                .or_else(|| Some(root.to_path_buf())),
            operation: "遍历 CLI-agent 会话目录",
            message: error.to_string(),
        }
    }

    fn expected_directory(path: &Path) -> Self {
        Self {
            path: Some(path.to_path_buf()),
            operation: "读取 CLI-agent 会话目录",
            message: "路径存在但不是目录".to_owned(),
        }
    }

    fn home_directory_unavailable() -> Self {
        Self {
            path: None,
            operation: "解析 CLI-agent home directory",
            message: "当前用户 home directory 不可用".to_owned(),
        }
    }

    pub(crate) fn source_missing() -> Self {
        Self {
            path: None,
            operation: "扫描 CLI-agent session discovery source",
            message: "provider stores are temporarily unavailable".to_owned(),
        }
    }
}

#[cfg(feature = "local_fs")]
impl fmt::Display for CliAgentSessionScanError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(path) = &self.path {
            write!(
                formatter,
                "{} {} 失败：{}",
                self.operation,
                path.display(),
                self.message
            )
        } else {
            write!(formatter, "{}失败：{}", self.operation, self.message)
        }
    }
}

#[cfg(feature = "local_fs")]
impl std::error::Error for CliAgentSessionScanError {}

#[cfg(feature = "local_fs")]
pub(crate) fn require_cli_agent_home(
    home: Option<PathBuf>,
) -> Result<PathBuf, CliAgentSessionScanError> {
    home.ok_or_else(CliAgentSessionScanError::home_directory_unavailable)
}

#[cfg(feature = "local_fs")]
pub(crate) fn current_cli_agent_home() -> Result<PathBuf, CliAgentSessionScanError> {
    require_cli_agent_home(dirs::home_dir())
}

#[cfg(feature = "local_fs")]
pub(crate) fn resolve_current_process_cli_agent_store_roots(
) -> Result<CliAgentStoreRoots, CliAgentSessionScanError> {
    Ok(CliAgentStoreRoots::for_current_process(
        current_cli_agent_home()?,
    ))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CliAgentStoreRoots {
    pub(crate) home_dir: PathBuf,
    pub(crate) claude_config_dir: PathBuf,
    pub(crate) codex_home: PathBuf,
    pub(crate) jcode_home: PathBuf,
    pub(crate) omp_agent_home: PathBuf,
}

impl CliAgentStoreRoots {
    pub(crate) fn for_home(home_dir: PathBuf) -> Self {
        Self {
            claude_config_dir: home_dir.join(".claude"),
            codex_home: home_dir.join(".codex"),
            jcode_home: home_dir.join(".jcode"),
            omp_agent_home: home_dir.join(".omp/agent"),
            home_dir,
        }
    }

    pub(crate) fn for_current_process(home_dir: PathBuf) -> Self {
        let mut roots = Self::for_home(home_dir);
        if let Some(path) = std::env::var_os("CLAUDE_CONFIG_DIR") {
            roots.claude_config_dir = process_config_root(PathBuf::from(path));
        }
        if let Some(path) = std::env::var_os("CODEX_HOME") {
            roots.codex_home = process_config_root(PathBuf::from(path));
        }
        roots
    }

    pub(crate) fn from_explicit_target_paths(
        home_dir: PathBuf,
        claude_config_dir: PathBuf,
        codex_home: PathBuf,
    ) -> Result<Self, String> {
        for (label, path) in [
            ("home_dir", &home_dir),
            ("claude_config_dir", &claude_config_dir),
            ("codex_home", &codex_home),
        ] {
            if path.as_os_str().is_empty() {
                return Err(format!("CLI-agent target store root {label} is empty"));
            }
            if !path.is_absolute() {
                return Err(format!(
                    "CLI-agent target store root {label} is not absolute: {}",
                    path.display()
                ));
            }
        }
        let jcode_home = home_dir.join(".jcode");
        let omp_agent_home = home_dir.join(".omp/agent");
        Ok(Self {
            home_dir,
            claude_config_dir,
            codex_home,
            jcode_home,
            omp_agent_home,
        })
    }

    pub(crate) fn claude_projects(&self) -> PathBuf {
        self.claude_config_dir.join("projects")
    }

    pub(crate) fn codex_sessions(&self) -> PathBuf {
        self.codex_home.join("sessions")
    }

    pub(crate) fn codex_index(&self) -> PathBuf {
        self.codex_home.join("session_index.jsonl")
    }

    pub(crate) fn jcode_sessions(&self) -> PathBuf {
        self.jcode_home.join("sessions")
    }

    pub(crate) fn omp_sessions(&self) -> PathBuf {
        self.omp_agent_home.join("sessions")
    }
}

fn process_config_root(path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        return path;
    }
    std::env::current_dir()
        .map(|cwd| cwd.join(&path))
        .unwrap_or(path)
}

#[cfg(feature = "local_fs")]
pub(crate) fn normalize_cli_agent_session_cwd(
    value: Option<&str>,
    roots: &CliAgentStoreRoots,
) -> Option<String> {
    let home = &roots.home_dir;
    let value = value?.trim();
    if value.is_empty() || value.starts_with("remote:") || value.starts_with("ssh:") {
        return None;
    }
    let expanded = if value == "~" {
        home.to_path_buf()
    } else if let Some(relative) = value.strip_prefix("~/") {
        home.join(relative)
    } else {
        PathBuf::from(value)
    };
    if !expanded.is_absolute() || !expanded.is_dir() {
        return None;
    }

    let real_value = fs::canonicalize(&expanded).unwrap_or_else(|_| expanded.clone());
    for store in [
        roots.claude_config_dir.clone(),
        roots.codex_sessions(),
        roots.codex_index(),
        roots.jcode_sessions(),
        roots.omp_sessions(),
    ] {
        let real_store = fs::canonicalize(&store).unwrap_or(store);
        if real_value == real_store || real_value.starts_with(&real_store) {
            return None;
        }
    }
    Some(expanded.to_string_lossy().into_owned())
}

#[cfg(feature = "local_fs")]
pub(crate) trait CliAgentSessionSource {
    fn agent_key(&self) -> String;
    fn provider_session_id(&self) -> &str;
    fn physical_source_key(&self) -> String;
    fn modified_epoch_millis(&self) -> i64;
}

/// 对所有 provider 的 physical sources 应用唯一的逻辑会话 quota。
///
/// 同一 physical source 只保留最新记录；随后按 `(agent, provider session id)`
/// 聚合 backing sources，以逻辑会话最新时间全局排序并截断。入选会话的全部
/// backing sources 都会保留，避免 Codex rollout 与 session index enrichment
/// 竞争用户可见 quota。
#[cfg(feature = "local_fs")]
pub(crate) fn limit_cli_agent_session_sources<T: CliAgentSessionSource>(
    sources: Vec<T>,
    logical_limit: usize,
) -> Vec<T> {
    if logical_limit == 0 {
        return Vec::new();
    }

    struct KeyedSource<T> {
        value: T,
        agent: String,
        session_id: String,
        physical_source: String,
        modified_epoch_millis: i64,
    }

    let mut sources = sources
        .into_iter()
        .map(|source| KeyedSource {
            agent: source.agent_key(),
            session_id: source.provider_session_id().to_owned(),
            physical_source: source.physical_source_key(),
            modified_epoch_millis: source.modified_epoch_millis(),
            value: source,
        })
        .collect::<Vec<_>>();
    sources.sort_by(|left, right| {
        right
            .modified_epoch_millis
            .cmp(&left.modified_epoch_millis)
            .then_with(|| left.agent.cmp(&right.agent))
            .then_with(|| left.session_id.cmp(&right.session_id))
            .then_with(|| left.physical_source.cmp(&right.physical_source))
    });

    let mut seen_physical_sources = HashSet::new();
    let mut sources_by_logical_session = HashMap::<(String, String), Vec<KeyedSource<T>>>::new();
    for source in sources {
        if !seen_physical_sources.insert((source.agent.clone(), source.physical_source.clone())) {
            continue;
        }
        sources_by_logical_session
            .entry((source.agent.clone(), source.session_id.clone()))
            .or_default()
            .push(source);
    }

    let mut logical_sessions = sources_by_logical_session.into_values().collect::<Vec<_>>();
    logical_sessions.sort_by(|left, right| {
        right[0]
            .modified_epoch_millis
            .cmp(&left[0].modified_epoch_millis)
            .then_with(|| left[0].agent.cmp(&right[0].agent))
            .then_with(|| left[0].session_id.cmp(&right[0].session_id))
    });
    logical_sessions.truncate(logical_limit);
    logical_sessions
        .into_iter()
        .flatten()
        .map(|source| source.value)
        .collect()
}

#[cfg(feature = "local_fs")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RecentJsonlFile {
    pub(crate) path: PathBuf,
    pub(crate) modified: SystemTime,
}

#[cfg(feature = "local_fs")]
fn collect_complete_scan_entries<T, E>(
    entries: impl IntoIterator<Item = Result<T, E>>,
) -> Result<Vec<T>, E> {
    entries.into_iter().collect()
}

/// 完整发现 `root` 下最近的普通 JSONL 文件。
///
/// 不存在的 provider store 是合法空集；store 一旦存在，任何遍历、metadata
/// 或 mtime 错误都会使整次扫描失败，禁止把 partial result 当成 Success。
#[cfg(feature = "local_fs")]
pub(crate) fn recent_jsonl_files(
    root: &Path,
    limit: usize,
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

    let entries = collect_complete_scan_entries(
        walkdir::WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .map(|entry| entry.map_err(|error| CliAgentSessionScanError::walk(root, error))),
    )?;
    let mut files = Vec::new();
    for entry in entries {
        if !entry.file_type().is_file()
            || !entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "jsonl")
        {
            continue;
        }
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

/// 发现 Jcode 正本目录中最近的 `session_*.json` 文件。
///
/// Jcode 的 `cache/session-picker-list-v1.json` 只是 picker cache，不是会话
/// authority；`.bak` 同样不能代表当前 session state，因此两者均不在这里读取。
#[cfg(feature = "local_fs")]
fn recent_jcode_session_files(
    root: &Path,
) -> Result<Vec<RecentJsonlFile>, CliAgentSessionScanError> {
    match fs::symlink_metadata(root) {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(CliAgentSessionScanError::io(
                root,
                "读取 Jcode session 目录",
                error,
            ));
        }
    }
    let entries = direct_regular_files(root, "读取 Jcode session 目录")?;
    let mut files = entries
        .into_iter()
        .filter(|entry| {
            entry
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("session_") && name.ends_with(".json"))
        })
        .collect::<Vec<_>>();
    sort_recent_files(&mut files);
    Ok(files)
}

/// 发现 Omp 默认 storage 中恰好一层 project bucket 下的 `.jsonl` session。
///
/// Omp 可能在同名目录中保存 tool log；只扫描 `sessions/*/*.jsonl`，不能递归
/// 扩展成 `sessions/**/*.jsonl`。
#[cfg(feature = "local_fs")]
fn recent_omp_session_files(
    root: &Path,
    limit: usize,
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

    let project_entries = collect_complete_scan_entries(
        fs::read_dir(root)
            .map_err(|error| CliAgentSessionScanError::io(root, "读取 Omp session 目录", error))?
            .map(|entry| {
                entry.map_err(|error| {
                    CliAgentSessionScanError::io(root, "遍历 Omp session 目录", error)
                })
            }),
    )?;
    let mut files = Vec::new();
    for project_entry in project_entries {
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
        files.extend(
            direct_regular_files(&project_entry.path(), "读取 Omp project session 目录")?
                .into_iter()
                .filter(|entry| {
                    entry
                        .path
                        .extension()
                        .is_some_and(|extension| extension == "jsonl")
                }),
        );
    }
    sort_and_limit_recent_files(&mut files, limit);
    Ok(files)
}

#[cfg(feature = "local_fs")]
fn direct_regular_files(
    root: &Path,
    operation: &'static str,
) -> Result<Vec<RecentJsonlFile>, CliAgentSessionScanError> {
    let root_metadata = fs::symlink_metadata(root)
        .map_err(|error| CliAgentSessionScanError::io(root, operation, error))?;
    if !root_metadata.file_type().is_dir() {
        return Err(CliAgentSessionScanError::expected_directory(root));
    }
    let entries = collect_complete_scan_entries(
        fs::read_dir(root)
            .map_err(|error| CliAgentSessionScanError::io(root, operation, error))?
            .map(|entry| {
                entry.map_err(|error| CliAgentSessionScanError::io(root, operation, error))
            }),
    )?;
    let mut files = Vec::new();
    for entry in entries {
        let file_type = entry.file_type().map_err(|error| {
            CliAgentSessionScanError::io(&entry.path(), "读取 CLI-agent 会话文件类型", error)
        })?;
        if !file_type.is_file() {
            continue;
        }
        let metadata = entry.metadata().map_err(|error| {
            CliAgentSessionScanError::io(&entry.path(), "读取 CLI-agent 会话文件 metadata", error)
        })?;
        let modified = metadata.modified().map_err(|error| {
            CliAgentSessionScanError::io(&entry.path(), "读取 CLI-agent 会话文件 mtime", error)
        })?;
        files.push(RecentJsonlFile {
            path: entry.path(),
            modified,
        });
    }
    Ok(files)
}

#[cfg(feature = "local_fs")]
fn sort_and_limit_recent_files(files: &mut Vec<RecentJsonlFile>, limit: usize) {
    sort_recent_files(files);
    files.truncate(limit);
}

#[cfg(feature = "local_fs")]
fn sort_recent_files(files: &mut [RecentJsonlFile]) {
    files.sort_by(|left, right| {
        right
            .modified
            .cmp(&left.modified)
            .then_with(|| left.path.cmp(&right.path))
    });
}

#[cfg(feature = "local_fs")]
fn read_jsonl_prefix_values(
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
    let bytes = fs::read(path)
        .map_err(|error| CliAgentSessionScanError::io(path, "读取 CLI-agent 会话文件", error))?;
    Ok(parse_jsonl_values(&String::from_utf8_lossy(&bytes), limit))
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

fn first_string(values: &[Option<&str>]) -> Option<String> {
    values
        .iter()
        .flatten()
        .map(|value| value.trim())
        .find(|value| !value.is_empty())
        .map(str::to_owned)
}

fn string_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "local_fs")]
    struct TestSessionSource {
        agent: &'static str,
        session_id: &'static str,
        physical_source: &'static str,
        modified_epoch_millis: i64,
    }

    #[cfg(feature = "local_fs")]
    impl CliAgentSessionSource for TestSessionSource {
        fn agent_key(&self) -> String {
            self.agent.to_owned()
        }

        fn provider_session_id(&self) -> &str {
            self.session_id
        }

        fn physical_source_key(&self) -> String {
            self.physical_source.to_owned()
        }

        fn modified_epoch_millis(&self) -> i64 {
            self.modified_epoch_millis
        }
    }

    #[cfg(feature = "local_fs")]
    #[test]
    fn discovery_plan_distinguishes_source_missing_from_successful_empty_store() {
        let home = tempfile::tempdir().expect("create discovery home");
        let roots = CliAgentStoreRoots::for_home(home.path().to_path_buf());
        let plan =
            AgentSessionDiscoveryPlan::for_test(vec![AgentSessionDiscoveryProvider::Jcode], 40);

        assert!(matches!(
            plan.execute(&roots, &HashSet::new()).transition(),
            AgentSessionDiscoveryTransition::Replace { records, .. } if records.is_empty()
        ));

        assert!(matches!(
            plan.execute(
                &roots,
                &HashSet::from([AgentSessionDiscoveryProvider::Jcode]),
            )
            .transition(),
            AgentSessionDiscoveryTransition::PreserveSourceMissing(
                AgentSessionDiscoveryProvider::Jcode
            )
        ));

        fs::create_dir_all(roots.jcode_sessions()).expect("provision observed Jcode store");
        assert!(matches!(
            plan.execute(
                &roots,
                &HashSet::from([AgentSessionDiscoveryProvider::Jcode]),
            )
            .transition(),
            AgentSessionDiscoveryTransition::Replace { records, .. } if records.is_empty()
        ));
    }

    #[cfg(feature = "local_fs")]
    #[test]
    fn discovery_plan_uses_only_enabled_unique_file_backed_agents() {
        let plan = AgentSessionDiscoveryPlan::from_enabled_agents(
            [
                CLIAgent::Jcode,
                CLIAgent::Unknown,
                CLIAgent::Omp,
                CLIAgent::Jcode,
                CLIAgent::Claude,
            ],
            40,
        );

        assert_eq!(
            plan.providers(),
            [
                AgentSessionDiscoveryProvider::Jcode,
                AgentSessionDiscoveryProvider::Omp,
                AgentSessionDiscoveryProvider::Claude,
            ],
            "discovery selection must be an ordered, duplicate-free projection of the setting",
        );
    }

    #[cfg(feature = "local_fs")]
    #[test]
    fn discovery_plan_rejects_partial_collection_on_scan_failure() {
        let home = tempfile::tempdir().expect("create scan home");
        fs::create_dir_all(home.path().join(".claude/projects")).expect("create Claude store");
        fs::write(
            home.path().join(".claude/projects/observed.jsonl"),
            serde_json::json!({"sessionId": "target"}).to_string(),
        )
        .expect("write valid target");
        fs::create_dir_all(home.path().join(".codex/sessions")).expect("create Codex store");
        fs::create_dir_all(home.path().join(".codex/session_index.jsonl"))
            .expect("make index path invalid");

        assert!(matches!(
            AgentSessionDiscoveryPlan::for_test(
                vec![
                    AgentSessionDiscoveryProvider::Claude,
                    AgentSessionDiscoveryProvider::Codex,
                ],
                40,
            )
            .execute(
                &CliAgentStoreRoots::for_home(home.path().to_path_buf()),
                &HashSet::new(),
            )
            .transition(),
            AgentSessionDiscoveryTransition::PreserveFailed(_)
        ));
    }

    #[cfg(feature = "local_fs")]
    #[test]
    fn discovery_cancel_has_explicit_preserve_transition() {
        assert!(matches!(
            AgentSessionDiscoveryResult::Cancelled.transition(),
            AgentSessionDiscoveryTransition::PreserveCancelled
        ));
    }

    #[cfg(feature = "local_fs")]
    fn discovery_record(
        agent: CLIAgent,
        provider_session_id: &str,
        modified_epoch_millis: i64,
    ) -> AgentSessionDiscoveryRecord {
        AgentSessionDiscoveryRecord {
            agent,
            provider_session_id: provider_session_id.to_owned(),
            source: AgentSessionDiscoverySource::Transcript(PathBuf::from(format!(
                "/fixtures/{provider_session_id}.jsonl"
            ))),
            label: None,
            cwd: None,
            modified_epoch_millis,
        }
    }

    #[cfg(feature = "local_fs")]
    #[test]
    fn provider_outcomes_preserve_identity_and_order_until_explicit_permanent_deletion() {
        let current = vec![
            discovery_record(CLIAgent::Claude, "unrelated-claude-a", 30),
            discovery_record(CLIAgent::Codex, "target-codex", 20),
            discovery_record(CLIAgent::Claude, "unrelated-claude-b", 10),
        ];
        let identities = |records: &[AgentSessionDiscoveryRecord]| {
            records
                .iter()
                .map(|record| (record.agent, record.provider_session_id.clone()))
                .collect::<Vec<_>>()
        };
        let expected = identities(&current);

        let missing =
            AgentSessionDiscoveryResult::SourceMissing(AgentSessionDiscoveryProvider::Codex)
                .transition()
                .apply_to(current.clone())
                .expect("source missing preserves collection");
        assert_eq!(identities(&missing), expected);

        let cancelled = AgentSessionDiscoveryResult::Cancelled
            .transition()
            .apply_to(missing)
            .expect("cancel preserves collection");
        assert_eq!(identities(&cancelled), expected);

        let deleted =
            AgentSessionDiscoveryResult::PermanentlyDeleted(AgentSessionDiscoveryProvider::Codex)
                .transition()
                .apply_to(cancelled)
                .expect("permanent deletion removes only its provider");
        assert_eq!(
            identities(&deleted),
            vec![
                (CLIAgent::Claude, "unrelated-claude-a".to_owned()),
                (CLIAgent::Claude, "unrelated-claude-b".to_owned()),
            ]
        );
    }

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

    #[test]
    fn shared_codex_metadata_ignores_injected_user_content() {
        let provider_session_id = "019f5f34-b6b7-70b3-8e50-e98504691ca7";
        let values = vec![
            serde_json::json!({
                "type": "session_meta",
                "payload": {"id": provider_session_id, "cwd": "/repo"}
            }),
            serde_json::json!({
                "type": "response_item",
                "payload": {"type": "message", "role": "user", "content": [
                    {"type": "input_text", "text": "# AGENTS.md instructions"}
                ]}
            }),
            serde_json::json!({
                "type": "event_msg",
                "payload": {"type": "user_message", "message": "修复本地标题\n不要显示 Codex"}
            }),
        ];

        let metadata = codex_session_metadata(&values);
        assert_eq!(metadata.session_id.as_deref(), Some(provider_session_id));
        assert_eq!(metadata.cwd.as_deref(), Some("/repo"));
        assert_eq!(metadata.title, None);
        assert_eq!(metadata.display_title().as_deref(), Some("修复本地标题"));
    }

    #[test]
    fn shared_codex_metadata_never_promotes_response_item_message_id() {
        let values = vec![
            serde_json::json!({
                "type": "session_meta",
                "payload": {
                    "id": "019f5f34-b6b7-70b3-8e50-e98504691ca7",
                    "cwd": "/Users/admin/manga_data"
                }
            }),
            serde_json::json!({
                "type": "response_item",
                "payload": {
                    "type": "message",
                    "id": "msg_06435be93b11cbcc016a55deda46808197a7e8894330ebe948",
                    "role": "assistant",
                    "content": []
                }
            }),
            serde_json::json!({
                "type": "response_item",
                "payload": {
                    "type": "function_call",
                    "id": "fc_0cbbc521aa68e6da016a57017553e081909e7762be87374e44"
                }
            }),
        ];

        let metadata = codex_session_metadata(&values);
        assert_eq!(
            metadata.session_id.as_deref(),
            Some("019f5f34-b6b7-70b3-8e50-e98504691ca7")
        );
    }

    #[test]
    fn canonical_codex_session_id_rejects_runtime_object_ids() {
        let provider_session_id = "019f5f34-b6b7-70b3-8e50-e98504691ca7";
        assert_eq!(
            canonical_codex_session_id(provider_session_id).as_deref(),
            Some(provider_session_id)
        );
        assert_eq!(
            canonical_codex_session_id(&format!(
                "rollout-2026-07-14T13-58-38-{provider_session_id}"
            ))
            .as_deref(),
            Some(provider_session_id)
        );
        assert_eq!(
            canonical_codex_session_id("msg_06435be93b11cbcc016a55deda46808197a7e8894330ebe948"),
            None
        );
        assert_eq!(
            canonical_codex_session_id("fc_0cbbc521aa68e6da016a57017553e081909e7762be87374e44"),
            None
        );
    }

    #[test]
    fn shared_provider_title_wins_over_first_user_message() {
        let values = vec![
            serde_json::json!({"thread_name": "正式标题"}),
            serde_json::json!({
                "type": "event_msg",
                "payload": {"type": "user_message", "message": "首条消息"}
            }),
        ];

        let metadata = codex_session_metadata(&values);
        assert_eq!(metadata.display_title().as_deref(), Some("正式标题"));
    }

    #[test]
    fn codex_session_index_record_parser_is_shared() {
        let provider_session_id = "019f5f34-b6b7-70b3-8e50-e98504691ca7";
        let record = codex_session_index_record(&serde_json::json!({
            "session_id": provider_session_id,
            "thread_name": "共享 Index 标题",
            "cwd": "~/project",
            "updated_at": "2026-07-12T08:00:00Z",
        }))
        .expect("shared index record");

        assert_eq!(record.session_id, provider_session_id);
        assert_eq!(record.title.as_deref(), Some("共享 Index 标题"));
        assert_eq!(record.cwd.as_deref(), Some("~/project"));
        assert_eq!(record.updated_at_epoch_millis, Some(1_783_843_200_000));

        let camel_case = codex_session_index_record(&serde_json::json!({
            "sessionId": "019f5629-5daf-7381-b33e-00d8efba617f",
            "updated_at_unix_ms": 1234,
        }))
        .expect("camel-case session id fallback");
        assert_eq!(
            camel_case.session_id,
            "019f5629-5daf-7381-b33e-00d8efba617f"
        );
        assert_eq!(camel_case.updated_at_epoch_millis, Some(1234));

        assert!(codex_session_index_record(&serde_json::json!({
            "id": "msg_06435be93b11cbcc016a55deda46808197a7e8894330ebe948"
        }))
        .is_none());
    }

    #[cfg(feature = "local_fs")]
    #[test]
    fn recent_jsonl_scan_error_is_not_silently_dropped() {
        let entries = [Ok("first"), Err("traversal failed"), Ok("last")];

        let result = collect_complete_scan_entries(entries);

        assert_eq!(result, Err("traversal failed"));
    }

    #[cfg(feature = "local_fs")]
    #[test]
    fn cli_agent_home_resolution_never_falls_back_to_filesystem_root() {
        let result = require_cli_agent_home(None);

        assert!(result.is_err(), "unknown home must remain an error");
    }

    #[cfg(feature = "local_fs")]
    #[test]
    fn cli_agent_session_cwd_normalization_is_shared() {
        let home = tempfile::tempdir().expect("create temp home");
        let project = home.path().join("project");
        fs::create_dir(&project).expect("create project");
        fs::create_dir_all(home.path().join(".codex/sessions")).expect("create session store");

        assert_eq!(
            normalize_cli_agent_session_cwd(
                Some("~/project"),
                &CliAgentStoreRoots::for_home(home.path().to_path_buf()),
            )
            .as_deref(),
            Some(project.to_string_lossy().as_ref())
        );
        assert_eq!(
            normalize_cli_agent_session_cwd(
                Some("relative/project"),
                &CliAgentStoreRoots::for_home(home.path().to_path_buf()),
            ),
            None
        );
        assert_eq!(
            normalize_cli_agent_session_cwd(
                Some("~/missing"),
                &CliAgentStoreRoots::for_home(home.path().to_path_buf()),
            ),
            None
        );
        assert_eq!(
            normalize_cli_agent_session_cwd(
                Some("~/.codex/sessions"),
                &CliAgentStoreRoots::for_home(home.path().to_path_buf()),
            ),
            None
        );
    }

    #[cfg(feature = "local_fs")]
    #[test]
    fn cli_agent_logical_limit_is_shared_across_providers() {
        let limited = limit_cli_agent_session_sources(
            vec![
                TestSessionSource {
                    agent: "claude",
                    session_id: "claude-new",
                    physical_source: "claude-new.jsonl",
                    modified_epoch_millis: 400,
                },
                TestSessionSource {
                    agent: "codex",
                    session_id: "codex-new",
                    physical_source: "codex-new-rollout.jsonl",
                    modified_epoch_millis: 300,
                },
                TestSessionSource {
                    agent: "codex",
                    session_id: "codex-new",
                    physical_source: "session_index.jsonl:codex-new",
                    modified_epoch_millis: 100,
                },
                TestSessionSource {
                    agent: "claude",
                    session_id: "claude-old",
                    physical_source: "claude-old.jsonl",
                    modified_epoch_millis: 200,
                },
                TestSessionSource {
                    agent: "codex",
                    session_id: "codex-old",
                    physical_source: "codex-old.jsonl",
                    modified_epoch_millis: 50,
                },
            ],
            2,
        );
        let logical_ids = limited
            .iter()
            .map(|source| (source.agent, source.session_id))
            .collect::<HashSet<_>>();
        let codex_new_backing_sources = limited
            .iter()
            .filter(|source| source.session_id == "codex-new")
            .count();

        assert_eq!(
            logical_ids,
            HashSet::from([("claude", "claude-new"), ("codex", "codex-new")])
        );
        assert_eq!(codex_new_backing_sources, 2);
    }

    #[cfg(feature = "local_fs")]
    #[test]
    fn jcode_discovery_reads_authoritative_session_json_and_uses_updated_at() {
        let home = tempfile::tempdir().expect("create Jcode home");
        let project = home.path().join("project");
        let sessions = home.path().join(".jcode/sessions");
        fs::create_dir_all(&project).expect("create Jcode project");
        fs::create_dir_all(&sessions).expect("create Jcode sessions");

        let session_id = "session_bug_1784897411999_c3c24cb8ea67c6a2";
        let session_path = sessions.join(format!("{session_id}.json"));
        fs::write(
            &session_path,
            serde_json::json!({
                "id": session_id,
                "short_name": "bug",
                "working_dir": project,
                "updated_at": "2026-07-24T13:28:15.634345Z",
                "is_debug": false,
                "messages": [{"role": "user"}],
            })
            .to_string(),
        )
        .expect("write authoritative Jcode session");
        fs::write(
            sessions.join(format!("{session_id}.json.bak")),
            "not a session",
        )
        .expect("write ignored Jcode backup");
        fs::create_dir_all(home.path().join(".jcode/cache")).expect("create Jcode cache");
        fs::write(
            home.path().join(".jcode/cache/session-picker-list-v1.json"),
            serde_json::json!([{"id": "session_stale"}]).to_string(),
        )
        .expect("write ignored Jcode picker cache");

        let records = scan_agent_session_provider(
            AgentSessionDiscoveryProvider::Jcode,
            &CliAgentStoreRoots::for_home(home.path().to_path_buf()),
            10,
        )
        .expect("scan Jcode authoritative sessions");

        assert_eq!(records.len(), 1);
        assert_eq!(records[0].agent, CLIAgent::Jcode);
        assert_eq!(records[0].provider_session_id, session_id);
        assert_eq!(records[0].label.as_deref(), Some("bug"));
        assert_eq!(records[0].cwd.as_deref(), project.to_str());
        assert_eq!(records[0].modified_epoch_millis, 1_784_899_695_634);
        assert_eq!(
            records[0].source,
            AgentSessionDiscoverySource::Transcript(session_path)
        );
    }

    #[cfg(feature = "local_fs")]
    #[test]
    fn jcode_discovery_filters_debug_and_empty_sessions_before_recency_limit() {
        let home = tempfile::tempdir().expect("create Jcode home");
        let sessions = home.path().join(".jcode/sessions");
        fs::create_dir_all(&sessions).expect("create Jcode sessions");

        let parent_session_id = "session_a_parent";
        fs::write(
            sessions.join(format!("{parent_session_id}.json")),
            serde_json::json!({
                "id": parent_session_id,
                "parent_id": "session_upstream_parent",
                "short_name": "visible parent session",
                "is_debug": false,
                "messages": [{"role": "user"}],
            })
            .to_string(),
        )
        .expect("write visible Jcode parent session");
        fs::write(
            sessions.join("session_y_empty.json"),
            serde_json::json!({
                "id": "session_y_empty",
                "is_debug": false,
                "messages": [],
            })
            .to_string(),
        )
        .expect("write empty Jcode session");
        fs::write(
            sessions.join("session_z_debug.json"),
            serde_json::json!({
                "id": "session_z_debug",
                "is_debug": true,
                "messages": [{"role": "user"}],
            })
            .to_string(),
        )
        .expect("write debug Jcode session");

        let records = scan_agent_session_provider(
            AgentSessionDiscoveryProvider::Jcode,
            &CliAgentStoreRoots::for_home(home.path().to_path_buf()),
            1,
        )
        .expect("scan Jcode sessions");

        assert_eq!(records.len(), 1);
        assert_eq!(records[0].provider_session_id, parent_session_id);
        assert_eq!(records[0].label.as_deref(), Some("visible parent session"));
    }

    #[cfg(feature = "local_fs")]
    #[test]
    fn jcode_discovery_rejects_filename_id_mismatch() {
        let home = tempfile::tempdir().expect("create Jcode home");
        let sessions = home.path().join(".jcode/sessions");
        fs::create_dir_all(&sessions).expect("create Jcode sessions");
        fs::write(
            sessions.join("session_filename.json"),
            serde_json::json!({"id": "session_payload"}).to_string(),
        )
        .expect("write mismatched Jcode session");

        assert!(scan_agent_session_provider(
            AgentSessionDiscoveryProvider::Jcode,
            &CliAgentStoreRoots::for_home(home.path().to_path_buf()),
            10,
        )
        .is_err());
    }

    #[cfg(feature = "local_fs")]
    #[test]
    fn omp_discovery_reads_one_project_level_and_prefers_title_slot() {
        let home = tempfile::tempdir().expect("create Omp home");
        let project = home.path().join("project");
        let bucket = home.path().join(".omp/agent/sessions/-ashide");
        fs::create_dir_all(&project).expect("create Omp project");
        fs::create_dir_all(bucket.join("tool-logs")).expect("create Omp nested tool logs");

        let session_id = "019f0a0b-1111-4222-8333-444444444444";
        let session_path = bucket.join(format!("1784897000000_{session_id}.jsonl"));
        fs::write(
            &session_path,
            format!(
                "{}\n{}\n",
                serde_json::json!({"type": "title", "title": "标题槽优先"}),
                serde_json::json!({
                    "type": "session",
                    "id": session_id,
                    "cwd": project,
                    "title": "header title",
                })
            ),
        )
        .expect("write Omp session");
        fs::write(
            bucket.join("tool-logs/1784897000001_nested.jsonl"),
            serde_json::json!({"type": "session", "id": "nested"}).to_string(),
        )
        .expect("write ignored nested Omp log");

        let records = scan_agent_session_provider(
            AgentSessionDiscoveryProvider::Omp,
            &CliAgentStoreRoots::for_home(home.path().to_path_buf()),
            10,
        )
        .expect("scan Omp session");

        assert_eq!(records.len(), 1);
        assert_eq!(records[0].agent, CLIAgent::Omp);
        assert_eq!(records[0].provider_session_id, session_id);
        assert_eq!(records[0].label.as_deref(), Some("标题槽优先"));
        assert_eq!(records[0].cwd.as_deref(), project.to_str());
        assert_eq!(
            records[0].source,
            AgentSessionDiscoverySource::Transcript(session_path)
        );
    }

    #[cfg(feature = "local_fs")]
    #[test]
    fn omp_discovery_orders_by_mtime_and_rejects_filename_id_mismatch() {
        let home = tempfile::tempdir().expect("create Omp home");
        let bucket = home.path().join(".omp/agent/sessions/-ashide");
        fs::create_dir_all(&bucket).expect("create Omp bucket");
        let old_id = "019f0a0b-1111-4222-8333-555555555555";
        let new_id = "019f0a0b-1111-4222-8333-666666666666";
        let old_path = bucket.join(format!("1000_{old_id}.jsonl"));
        let new_path = bucket.join(format!("2000_{new_id}.jsonl"));
        for (path, id, title) in [(&old_path, old_id, "old"), (&new_path, new_id, "new")] {
            fs::write(
                path,
                serde_json::json!({"type": "session", "id": id, "title": title}).to_string(),
            )
            .expect("write Omp ordering fixture");
        }
        fs::File::options()
            .write(true)
            .open(&old_path)
            .expect("open old Omp fixture")
            .set_times(
                fs::FileTimes::new().set_modified(UNIX_EPOCH + std::time::Duration::from_secs(100)),
            )
            .expect("set old Omp mtime");
        fs::File::options()
            .write(true)
            .open(&new_path)
            .expect("open new Omp fixture")
            .set_times(
                fs::FileTimes::new().set_modified(UNIX_EPOCH + std::time::Duration::from_secs(200)),
            )
            .expect("set new Omp mtime");

        let records = scan_agent_session_provider(
            AgentSessionDiscoveryProvider::Omp,
            &CliAgentStoreRoots::for_home(home.path().to_path_buf()),
            1,
        )
        .expect("scan latest Omp session");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].provider_session_id, new_id);
        assert_eq!(records[0].label.as_deref(), Some("new"));

        fs::write(
            bucket.join("3000_filename.jsonl"),
            serde_json::json!({"type": "session", "id": "payload"}).to_string(),
        )
        .expect("write mismatched Omp session");
        assert!(scan_agent_session_provider(
            AgentSessionDiscoveryProvider::Omp,
            &CliAgentStoreRoots::for_home(home.path().to_path_buf()),
            10,
        )
        .is_err());
    }

    #[cfg(feature = "local_fs")]
    #[test]
    fn explicit_target_roots_keep_jcode_and_omp_under_target_home() {
        let home = PathBuf::from("/target/home");
        let roots = CliAgentStoreRoots::from_explicit_target_paths(
            home.clone(),
            PathBuf::from("/target/claude"),
            PathBuf::from("/target/codex"),
        )
        .expect("construct explicit target roots");

        assert_eq!(roots.jcode_home, home.join(".jcode"));
        assert_eq!(roots.omp_agent_home, home.join(".omp/agent"));
    }

    #[cfg(feature = "local_fs")]
    #[test]
    fn jcode_and_omp_are_file_backed_session_index_providers() {
        let plan = AgentSessionDiscoveryPlan::from_registry(40);

        assert_eq!(
            CLIAgent::Jcode.session_discovery_provider(),
            Some(AgentSessionDiscoveryProvider::Jcode)
        );
        assert_eq!(
            CLIAgent::Omp.session_discovery_provider(),
            Some(AgentSessionDiscoveryProvider::Omp)
        );
        assert_eq!(
            plan.providers(),
            [
                AgentSessionDiscoveryProvider::Jcode,
                AgentSessionDiscoveryProvider::Claude,
                AgentSessionDiscoveryProvider::Codex,
                AgentSessionDiscoveryProvider::Omp,
            ],
            "all read-only persisted-session adapters must use the shared discovery plan",
        );
    }
}
