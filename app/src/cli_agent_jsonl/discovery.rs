//! Agent session discovery orchestration.

#[cfg(feature = "local_fs")]
use std::collections::HashSet;
#[cfg(feature = "local_fs")]
use std::fs;
#[cfg(feature = "local_fs")]
use std::io;
#[cfg(feature = "local_fs")]
use std::path::{Path, PathBuf};
#[cfg(feature = "local_fs")]
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(feature = "local_fs")]
use chrono::DateTime;
#[cfg(feature = "local_fs")]
use serde::{de::IgnoredAny, Deserialize};

#[cfg(feature = "local_fs")]
use crate::terminal::CLIAgent;

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
use super::roots::{normalize_cli_agent_session_cwd, CliAgentStoreRoots};

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
        AgentSessionDiscoveryProvider::Jcode => {
            let mut records = Vec::new();
            for file in recent_jcode_session_files(
                &roots.jcode_sessions(),
                crate::app_state::WORKSPACE_SESSION_NAVIGATOR_PHYSICAL_SOURCE_LIMIT_PER_PROVIDER,
            )? {
                if records.len() == logical_limit {
                    break;
                }
                if let Some(record) = parse_jcode_discovery_record(file, roots)? {
                    records.push(record);
                }
            }
            Ok(records)
        }
        AgentSessionDiscoveryProvider::Omp => recent_omp_session_files(
            &roots.omp_sessions(),
            logical_limit,
            crate::app_state::WORKSPACE_SESSION_NAVIGATOR_PHYSICAL_SOURCE_LIMIT_PER_PROVIDER,
        )?
        .into_iter()
        .map(|file| parse_omp_discovery_record(file, roots))
        .collect(),
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

/// 发现 Jcode 正本目录中最近的 `session_*.json` 文件。
///
/// Jcode 的 `cache/session-picker-list-v1.json` 只是 picker cache，不是会话
/// authority；`.bak` 同样不能代表当前 session state，因此两者均不在这里读取。
#[cfg(feature = "local_fs")]
pub(crate) fn recent_jcode_session_files(
    root: &Path,
    physical_limit: usize,
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
    let mut candidate_count = 0;
    let mut files = direct_regular_files(
        root,
        "读取 Jcode session 目录",
        physical_limit,
        &mut candidate_count,
        |path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("session_") && name.ends_with(".json"))
        },
    )?;
    sort_recent_files(&mut files);
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
            |path| {
                path.extension()
                    .is_some_and(|extension| extension == "jsonl")
            },
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
