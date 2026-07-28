//! Physical discovery candidate gates and logical session quotas.

#[cfg(feature = "local_fs")]
use std::collections::{HashMap, HashSet};
#[cfg(feature = "local_fs")]
use std::path::{Path, PathBuf};
#[cfg(feature = "local_fs")]
use std::time::SystemTime;

#[cfg(feature = "local_fs")]
use super::error::CliAgentSessionScanError;

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
pub(super) fn reserve_discovery_candidate(
    root: &Path,
    candidate_count: &mut usize,
    physical_limit: Option<usize>,
) -> Result<(), CliAgentSessionScanError> {
    *candidate_count += 1;
    if physical_limit.is_some_and(|limit| *candidate_count > limit) {
        return Err(CliAgentSessionScanError::discovery_candidate_limit(
            root,
            physical_limit.expect("physical limit was checked above"),
        ));
    }
    Ok(())
}

#[cfg(feature = "local_fs")]
pub(super) fn sort_and_limit_recent_files(files: &mut Vec<RecentJsonlFile>, limit: usize) {
    sort_recent_files(files);
    files.truncate(limit);
}

#[cfg(feature = "local_fs")]
pub(super) fn sort_recent_files(files: &mut [RecentJsonlFile]) {
    files.sort_by(|left, right| {
        right
            .modified
            .cmp(&left.modified)
            .then_with(|| left.path.cmp(&right.path))
    });
}
