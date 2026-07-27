//! Native CLI-agent session history operations.
//!
//! The daemon runs natively on the remote host, so it can scan, read and
//! mutate the agent session stores under the target process configuration roots directly
//! via `std::fs` — one round trip per operation, with no remote Python.
//!
//! Every function here is a native replacement for the Python heredocs that
//! used to live in `app/src/workspace/environment_runtime.rs`. Shared JSONL
//! metadata parsing and cwd normalization live in `cli_agent_jsonl`; this
//! module owns only remote store access, allow-listing and atomic mutation.

#![cfg(feature = "local_fs")]

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

use crate::cli_agent_jsonl::{
    canonical_codex_session_id, codex_session_metadata, read_jsonl_values_from_path,
    recent_jsonl_files, require_cli_agent_home, sha256_hex, AgentSessionDiscoveryPlan,
    AgentSessionDiscoveryTransition, CliAgentSessionScanError, CliAgentStoreRoots,
};

/// Default number of records the scan returns (mirrors the Python `LIMIT`).
const DEFAULT_SCAN_LIMIT: usize = 40;

/// A scanned session record, mirroring the Python JSON rows.
pub struct ScannedSession {
    pub agent: crate::terminal::CLIAgent,
    pub id: String,
    pub source: String,
    pub label: Option<String>,
    pub cwd: Option<String>,
    pub modified_epoch_millis: Option<i64>,
}

/// Remote transport projection of the shared discovery plan. The daemon never
/// turns a source-missing provider into an empty successful record list.
pub enum ScannedSessionDiscovery {
    Complete {
        observed_agents: Vec<crate::terminal::CLIAgent>,
        sessions: Vec<ScannedSession>,
    },
    SourceMissing {
        agent: crate::terminal::CLIAgent,
    },
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

fn remote_cli_agent_home(home: Option<PathBuf>) -> Result<PathBuf, CliAgentSessionScanError> {
    require_cli_agent_home(home)
}

/// Read the first `limit` non-empty JSONL lines of `path`, yielding parsed
/// JSON values (skipping unparseable lines). Mirrors `read_jsonl_prefix`.
fn read_jsonl_prefix(path: &Path, limit: usize) -> Result<Vec<Value>, CliAgentSessionScanError> {
    read_jsonl_values_from_path(path, Some(limit))
}

fn str_field<'a>(item: &'a Value, key: &str) -> Option<&'a str> {
    item.get(key).and_then(Value::as_str)
}

fn file_stem(path: &Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// 执行由 desktop 显式选择的共享 plan，并保留 typed provider identity。
///
/// RPC projection 只允许在 `server_model` 完成，以免 command prefix 穿过 transport。
pub fn scan_sessions(
    roots: &CliAgentStoreRoots,
    limit: usize,
    enabled_agents: impl IntoIterator<Item = crate::terminal::CLIAgent>,
    previously_observed_agents: impl IntoIterator<Item = crate::terminal::CLIAgent>,
) -> Result<ScannedSessionDiscovery, CliAgentSessionScanError> {
    let previously_observed_providers = previously_observed_agents
        .into_iter()
        .filter_map(|agent| agent.session_discovery_provider())
        .collect::<HashSet<_>>();
    match AgentSessionDiscoveryPlan::from_enabled_agents(enabled_agents, limit)
        .execute(roots, &previously_observed_providers)
        .transition()
    {
        AgentSessionDiscoveryTransition::Replace { providers, records } => {
            Ok(ScannedSessionDiscovery::Complete {
                observed_agents: providers
                    .into_iter()
                    .map(|provider| provider.agent())
                    .collect(),
                sessions: records
                    .into_iter()
                    .map(|record| ScannedSession {
                        agent: record.agent,
                        id: record.provider_session_id,
                        source: record.source.transport_reference(),
                        label: record.label,
                        cwd: record.cwd,
                        modified_epoch_millis: Some(record.modified_epoch_millis),
                    })
                    .collect(),
            })
        }
        AgentSessionDiscoveryTransition::RemoveProvider(_) => {
            unreachable!("runtime scan does not infer permanent provider deletion")
        }
        AgentSessionDiscoveryTransition::PreserveSourceMissing(provider) => {
            Ok(ScannedSessionDiscovery::SourceMissing {
                agent: provider.agent(),
            })
        }
        AgentSessionDiscoveryTransition::PreserveFailed(error) => Err(error),
        AgentSessionDiscoveryTransition::PreserveCancelled => {
            unreachable!("synchronous runtime filesystem delivery cannot cancel after execution")
        }
    }
}

#[cfg(test)]
fn scan_sessions_for_home(
    home: &Path,
    limit: usize,
) -> Result<Vec<ScannedSession>, CliAgentSessionScanError> {
    let roots = CliAgentStoreRoots::for_home(home.to_path_buf());
    match scan_sessions(
        &roots,
        limit,
        enum_iterator::all::<crate::terminal::CLIAgent>(),
        [],
    )? {
        ScannedSessionDiscovery::Complete { sessions, .. } => Ok(sessions),
        ScannedSessionDiscovery::SourceMissing { .. } => {
            Err(CliAgentSessionScanError::source_missing())
        }
    }
}

// ── Path allow-list + resolution ──────────────────────────────────

fn expand_user(path: &str, home: &Path) -> PathBuf {
    if path == "~" {
        return home.to_path_buf();
    }
    if let Some(stripped) = path.strip_prefix("~/") {
        return home.join(stripped);
    }
    PathBuf::from(path)
}

/// `os.path.realpath(os.path.expanduser(path))`. Falls back to the expanded
/// (lexical) path when canonicalize fails (e.g. the path does not exist),
/// matching realpath's best-effort behaviour for missing leaves.
fn real(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn real_str(path: &str, home: &Path) -> PathBuf {
    real(&expand_user(path, home))
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
fn ensure_allowed(path: &str, roots: &CliAgentStoreRoots) -> Result<PathBuf, String> {
    let resolved = real_str(path, &roots.home_dir);
    let allowed = [
        real(&roots.claude_projects()),
        real(&roots.codex_sessions()),
    ];
    let allowed_index = real(&roots.codex_index());
    if resolved == allowed_index {
        return Ok(resolved);
    }
    for root in &allowed {
        if resolved == *root || resolved.starts_with(path_with_sep(root)) {
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
fn split_index_source(source: &str, roots: &CliAgentStoreRoots) -> Option<(String, String)> {
    let (path, sid) = source.rsplit_once(':')?;
    if sid.trim().is_empty() {
        return None;
    }
    let codex_index = roots.codex_index();
    if real_str(path, &roots.home_dir) == real(&codex_index) {
        Some((path.to_owned(), sid.to_owned()))
    } else {
        None
    }
}

// ── Read path ─────────────────────────────────────────────────────

fn codex_session_id_for_file(path: &Path) -> Result<String, CliAgentSessionScanError> {
    let stem = file_stem(path);
    codex_session_metadata(&read_jsonl_prefix(path, 80)?)
        .session_id
        .or_else(|| canonical_codex_session_id(&stem))
        .ok_or_else(|| {
            CliAgentSessionScanError::io(
                path,
                "解析 Codex provider session id",
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "transcript has no canonical Codex session UUID",
                ),
            )
        })
}

/// Find the codex transcript whose session id matches `session_id` by scanning
/// `~/.codex/sessions` newest-first. Mirrors `find_codex_transcript`.
fn find_codex_transcript(
    roots: &CliAgentStoreRoots,
    session_id: &str,
) -> Result<Option<PathBuf>, CliAgentSessionScanError> {
    let root = roots.codex_sessions();
    for file in recent_jsonl_files(&root, usize::MAX, None)? {
        if codex_session_id_for_file(&file.path)? == session_id {
            return Ok(Some(file.path));
        }
    }
    Ok(None)
}

/// Resolve a source to an allowed transcript path. Mirrors `source_path`.
fn resolve_source_path(source: &str, roots: &CliAgentStoreRoots) -> Result<PathBuf, String> {
    if let Some((path, sid)) = split_index_source(source, roots) {
        ensure_allowed(&path, roots)?;
        let transcript = find_codex_transcript(roots, &sid)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("Codex transcript not found for indexed session {sid}"))?;
        return ensure_allowed(&transcript.to_string_lossy(), roots);
    }
    ensure_allowed(source, roots)
}

/// Resolve `source` and read the resulting transcript bytes. Mirrors the Python
/// read heredoc; `content` is returned raw (the proto carries bytes directly,
/// so no base64 wrapping is needed on the wire).
pub fn read_session(source: &str, roots: &CliAgentStoreRoots) -> Result<ReadSession, String> {
    let path = resolve_source_path(source, roots)?;
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
    std::fs::create_dir_all(&archive_dir).map_err(|err| {
        format!(
            "failed to create archive dir {}: {err}",
            archive_dir.display()
        )
    })?;
    let file_name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    unique_path(&archive_dir.join(file_name))
}

/// Rewrite the codex index jsonl removing the entry with `sid`, archiving the
/// removed lines when `mutation == Archive`. Atomic via temp+rename. Mirrors
/// `mutate_index_entry`.
fn mutate_index_entry(
    path: &str,
    sid: &str,
    mutation: Mutation,
    roots: &CliAgentStoreRoots,
) -> Result<(), String> {
    let path = ensure_allowed(path, roots)?;
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
        std::fs::write(&archive_path, removed.concat())
            .map_err(|err| format!("failed to write archive {}: {err}", archive_path.display()))?;
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
fn mutate_file(source: &str, mutation: Mutation, roots: &CliAgentStoreRoots) -> Result<(), String> {
    let path = ensure_allowed(source, roots)?;
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
pub fn mutate_session(
    source: &str,
    mutation: Mutation,
    roots: &CliAgentStoreRoots,
) -> Result<(), String> {
    if let Some((path, sid)) = split_index_source(source, roots) {
        mutate_index_entry(&path, &sid, mutation, roots)
    } else {
        mutate_file(source, mutation, roots)
    }
}

#[cfg(test)]
mod uireq014_first_message_tests {
    use super::*;
    use crate::cli_agent_jsonl::{
        claude_user_message_from_item, codex_session_index_record, codex_title_from_item,
        codex_user_message_from_item, first_message_excerpt, AgentSessionDiscoveryProvider,
        AgentSessionDiscoveryRecord, AgentSessionDiscoveryResult,
    };
    use serde_json::json;
    use std::fs::FileTimes;
    use std::time::{Duration, UNIX_EPOCH};

    fn set_test_file_modified(path: &Path, seconds: u64) {
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .expect("open fixture for mtime update");
        file.set_times(FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(seconds)))
            .expect("set fixture mtime");
    }

    fn scan_remote_codex_alias_enrichment_fixture() -> (
        tempfile::TempDir,
        Vec<AgentSessionDiscoveryRecord>,
        [String; 3],
    ) {
        let home = tempfile::tempdir().expect("create temp home");
        let sessions_dir = home.path().join(".codex/sessions");
        std::fs::create_dir_all(&sessions_dir).expect("create Codex sessions store");
        std::fs::create_dir_all(home.path().join(".claude/projects"))
            .expect("create Claude sessions store");
        let session_ids = [
            "019f5f34-b6b7-70b3-8e50-e98504691ca1".to_owned(),
            "019f5f34-b6b7-70b3-8e50-e98504691ca2".to_owned(),
            "019f5f34-b6b7-70b3-8e50-e98504691ca3".to_owned(),
        ];
        for (index, session_id) in session_ids.iter().enumerate() {
            let path = sessions_dir.join(format!("rollout-{session_id}.jsonl"));
            std::fs::write(
                &path,
                format!(
                    "{}\n",
                    json!({"type": "session_meta", "payload": {"id": session_id}})
                ),
            )
            .expect("write Codex rollout");
            set_test_file_modified(&path, 300 - index as u64 * 100);
        }
        std::fs::write(
            home.path().join(".codex/session_index.jsonl"),
            format!(
                "{}\n{}\n",
                json!({"id": &session_ids[0], "thread_name": "远程别名 A"}),
                json!({"id": &session_ids[1], "thread_name": "远程别名 B"}),
            ),
        )
        .expect("write Codex session index");

        let roots = CliAgentStoreRoots::for_home(home.path().to_path_buf());
        let limited = match AgentSessionDiscoveryPlan::for_test(
            vec![AgentSessionDiscoveryProvider::Codex],
            2,
        )
        .execute(&roots, &std::collections::HashSet::new())
        {
            AgentSessionDiscoveryResult::Complete { records, .. } => records,
            result => panic!("expected complete Codex fixture discovery, got {result:?}"),
        };
        (home, limited, session_ids)
    }

    #[test]
    fn test_codex_alias_enrichment_does_not_compete_for_logical_session_limit() {
        let (_home, limited, session_ids) = scan_remote_codex_alias_enrichment_fixture();
        let ids = limited
            .iter()
            .map(|session| session.provider_session_id.as_str())
            .collect::<std::collections::HashSet<_>>();

        assert_eq!(
            ids,
            std::collections::HashSet::from([session_ids[0].as_str(), session_ids[1].as_str(),])
        );
    }

    #[test]
    fn test_remote_session_navigator_initial_scan_projects_codex_alias_enrichment() {
        let (_home, limited, session_ids) = scan_remote_codex_alias_enrichment_fixture();
        let labels = limited
            .iter()
            .filter_map(|session| {
                session
                    .label
                    .as_deref()
                    .map(|label| (session.provider_session_id.as_str(), label))
            })
            .collect::<std::collections::HashMap<_, _>>();
        assert_eq!(labels[session_ids[0].as_str()], "远程别名 A");
        assert_eq!(labels[session_ids[1].as_str()], "远程别名 B");
    }

    #[test]
    fn test_remote_scan_respects_explicit_enabled_agents() {
        let home = tempfile::tempdir().expect("create remote scan home");
        let jcode_id = "session_remote_enabled_agent";
        let jcode_path = home
            .path()
            .join(".jcode/sessions")
            .join(format!("{jcode_id}.json"));
        std::fs::create_dir_all(jcode_path.parent().expect("Jcode parent"))
            .expect("create Jcode store");
        std::fs::write(
            &jcode_path,
            json!({
                "id": jcode_id,
                "short_name": "Jcode remote",
                "is_debug": false,
                "messages": [{"role": "user"}],
            })
            .to_string(),
        )
        .expect("write Jcode session");

        let omp_id = "019f0a0b-1111-4222-8333-444444444444";
        let omp_path = home
            .path()
            .join(".omp/agent/sessions/-ashide")
            .join(format!("1784897000000_{omp_id}.jsonl"));
        std::fs::create_dir_all(omp_path.parent().expect("Omp parent")).expect("create Omp store");
        std::fs::write(
            &omp_path,
            json!({"type": "session", "id": omp_id, "title": "Omp remote"}).to_string(),
        )
        .expect("write Omp session");

        let sessions = scan_sessions(
            &CliAgentStoreRoots::for_home(home.path().to_path_buf()),
            40,
            [crate::terminal::CLIAgent::Jcode],
            [],
        )
        .expect("scan explicitly enabled remote agent");
        let ScannedSessionDiscovery::Complete { sessions, .. } = sessions else {
            panic!("explicitly enabled remote agent should complete");
        };

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].agent, crate::terminal::CLIAgent::Jcode);
        assert_eq!(sessions[0].id, jcode_id);
    }

    #[test]
    fn remote_discovery_keeps_previously_observed_missing_provider_typed() {
        let home = tempfile::tempdir().expect("create remote discovery home");
        let roots = CliAgentStoreRoots::for_home(home.path().to_path_buf());
        std::fs::create_dir_all(roots.jcode_sessions()).expect("provision Jcode source");

        assert!(matches!(
            scan_sessions(&roots, 40, [crate::terminal::CLIAgent::Jcode], [])
                .expect("first discovery completes"),
            ScannedSessionDiscovery::Complete { observed_agents, sessions }
                if observed_agents == vec![crate::terminal::CLIAgent::Jcode] && sessions.is_empty()
        ));

        std::fs::remove_dir_all(roots.jcode_sessions()).expect("remove observed Jcode source");
        assert!(matches!(
            scan_sessions(
                &roots,
                40,
                [crate::terminal::CLIAgent::Jcode],
                [crate::terminal::CLIAgent::Jcode],
            )
            .expect("missing provider is a typed completion"),
            ScannedSessionDiscovery::SourceMissing {
                agent: crate::terminal::CLIAgent::Jcode
            }
        ));
    }

    #[test]
    fn codex_session_index_thread_name_is_a_title_source() {
        assert_eq!(
            codex_title_from_item(&json!({"thread_name": "外置 Codex 别名"})),
            Some("外置 Codex 别名".to_owned())
        );
    }

    #[test]
    fn codex_session_index_rfc3339_timestamp_drives_logical_recency() {
        let record = codex_session_index_record(&json!({
            "id": "019f5629-5daf-7381-b33e-00d8efba617f",
            "updated_at": "2026-07-12T08:00:00Z",
        }))
        .expect("shared Codex index record");
        assert_eq!(record.updated_at_epoch_millis, Some(1_783_843_200_000));
    }

    #[test]
    fn test_remote_codex_index_accepts_shared_session_id_fallback() {
        let home = tempfile::tempdir().expect("create temp home");
        let project = home.path().join("index-project");
        let provider_session_id = "019f5f34-b6b7-70b3-8e50-e98504691cb1";
        std::fs::create_dir_all(home.path().join(".codex/sessions"))
            .expect("create Codex sessions store");
        std::fs::create_dir_all(home.path().join(".claude/projects"))
            .expect("create Claude sessions store");
        std::fs::create_dir(&project).expect("create index cwd");
        std::fs::write(
            home.path().join(".codex/session_index.jsonl"),
            format!(
                "{}\n",
                serde_json::json!({
                    "session_id": provider_session_id,
                    "thread_name": "远程共享 Index Parser",
                    "cwd": "~/index-project",
                    "updated_at_unix_ms": 1234,
                })
            ),
        )
        .expect("write Codex session index");

        let sessions = scan_sessions_for_home(home.path(), 40).expect("scan remote Codex index");
        let session = sessions
            .iter()
            .find(|session| session.id == provider_session_id)
            .expect("session_id fallback must be visible remotely");

        assert_eq!(session.label.as_deref(), Some("远程共享 Index Parser"));
        assert_eq!(
            session.cwd.as_deref(),
            Some(project.to_string_lossy().as_ref())
        );
        assert_eq!(session.modified_epoch_millis, Some(1234));
    }

    #[test]
    fn remote_cli_agent_scan_failure_is_an_error_result() {
        let home = tempfile::tempdir().expect("create temp home");
        std::fs::create_dir_all(home.path().join(".claude")).expect("create provider dir");
        std::fs::write(home.path().join(".claude/projects"), b"not a directory")
            .expect("create invalid provider store");

        let result = scan_sessions_for_home(home.path(), 40);

        assert!(result.is_err(), "incomplete traversal must not be Success");
    }

    #[test]
    fn remote_cli_agent_paths_require_resolved_home() {
        assert!(remote_cli_agent_home(None).is_err());
    }

    #[test]
    fn test_remote_cli_agent_scan_uses_shared_cwd_normalization() {
        let home = tempfile::tempdir().expect("create temp home");
        let sessions_dir = home.path().join(".codex/sessions");
        let project = home.path().join("project");
        std::fs::create_dir_all(&sessions_dir).expect("create Codex sessions store");
        std::fs::create_dir_all(home.path().join(".claude/projects"))
            .expect("create Claude sessions store");
        std::fs::create_dir(&project).expect("create project cwd");

        let valid_session_id = "019f5f34-b6b7-70b3-8e50-e98504691cb2";
        std::fs::write(
            sessions_dir.join(format!("rollout-{valid_session_id}.jsonl")),
            format!(
                "{}\n",
                serde_json::json!({
                    "type": "session_meta",
                    "payload": {"id": valid_session_id, "cwd": "~/project"},
                })
            ),
        )
        .expect("write valid cwd transcript");

        let store_session_id = "019f5f34-b6b7-70b3-8e50-e98504691cb3";
        std::fs::write(
            sessions_dir.join(format!("rollout-{store_session_id}.jsonl")),
            format!(
                "{}\n",
                serde_json::json!({
                    "type": "session_meta",
                    "payload": {"id": store_session_id, "cwd": "~/.codex/sessions"},
                })
            ),
        )
        .expect("write session-store cwd transcript");

        let sessions = scan_sessions_for_home(home.path(), 40).expect("scan remote sessions");
        let valid = sessions
            .iter()
            .find(|session| session.id == valid_session_id)
            .expect("valid cwd session");
        let store = sessions
            .iter()
            .find(|session| session.id == store_session_id)
            .expect("session-store cwd session");

        assert_eq!(
            valid.cwd.as_deref(),
            Some(project.to_string_lossy().as_ref())
        );
        assert_eq!(store.cwd, None);
    }

    #[test]
    fn test_remote_cli_agent_scan_uses_shared_first_user_message_title_fallback() {
        let home = tempfile::tempdir().expect("create temp home");
        let codex_dir = home.path().join(".codex/sessions");
        let claude_dir = home.path().join(".claude/projects");
        std::fs::create_dir_all(&codex_dir).expect("create Codex sessions store");
        std::fs::create_dir_all(&claude_dir).expect("create Claude sessions store");

        let codex_session_id = "019f5f34-b6b7-70b3-8e50-e98504691ca6";
        std::fs::write(
            codex_dir.join(format!("rollout-{codex_session_id}.jsonl")),
            format!(
                concat!(
                    "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{}\",\"cwd\":\"/tmp/project\"}}}}\n",
                    "{{\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"user\",\"content\":[{{\"type\":\"input_text\",\"text\":\"# AGENTS.md instructions\"}}]}}}}\n",
                    "{{\"type\":\"event_msg\",\"payload\":{{\"type\":\"user_message\",\"message\":\"修复远程 Codex 标题\\n第二行不进入标题\"}}}}\n"
                ),
                codex_session_id
            ),
        )
        .expect("write remote Codex rollout");

        let claude_session_id = "remote-claude-first-user-title";
        std::fs::write(
            claude_dir.join(format!("{claude_session_id}.jsonl")),
            format!(
                "{{\"type\":\"user\",\"sessionId\":\"{claude_session_id}\",\"cwd\":\"/tmp/project\",\"message\":{{\"role\":\"user\",\"content\":\"继续远程 Claude 会话\"}}}}\n"
            ),
        )
        .expect("write remote Claude transcript");

        let sessions = scan_sessions_for_home(home.path(), 80).expect("scan remote sessions");
        let labels = sessions
            .into_iter()
            .filter_map(|session| Some((session.id, session.label?)))
            .collect::<std::collections::HashMap<_, _>>();

        assert_eq!(
            labels.get(codex_session_id).map(String::as_str),
            Some("修复远程 Codex 标题")
        );
        assert_eq!(
            labels.get(claude_session_id).map(String::as_str),
            Some("继续远程 Claude 会话")
        );
    }

    #[test]
    fn test_remote_cli_agent_scan_applies_global_logical_limit() {
        let home = tempfile::tempdir().expect("create temp home");
        let codex_dir = home.path().join(".codex/sessions");
        let claude_dir = home.path().join(".claude/projects");
        std::fs::create_dir_all(&codex_dir).expect("create Codex sessions store");
        std::fs::create_dir_all(&claude_dir).expect("create Claude sessions store");

        for (index, session_id) in [
            "019f5f34-b6b7-70b3-8e50-e98504691cb4",
            "019f5f34-b6b7-70b3-8e50-e98504691cb5",
        ]
        .into_iter()
        .enumerate()
        {
            std::fs::write(
                codex_dir.join(format!("rollout-{session_id}.jsonl")),
                format!(
                    "{}\n",
                    serde_json::json!({
                        "type": "session_meta",
                        "payload": {"id": session_id},
                    })
                ),
            )
            .expect("write Codex limit fixture");

            let session_id = format!("remote-claude-limit-{index}");
            std::fs::write(
                claude_dir.join(format!("{session_id}.jsonl")),
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

        let sessions = scan_sessions_for_home(home.path(), 2).expect("scan remote sessions");
        let logical_ids = sessions
            .iter()
            .map(|session| (session.agent, session.id.as_str()))
            .collect::<std::collections::HashSet<_>>();

        assert_eq!(
            logical_ids.len(),
            2,
            "quota must be global across providers"
        );
    }

    #[test]
    fn test_remote_cli_agent_scan_preserves_shared_logical_order() {
        let home = tempfile::tempdir().expect("create temp home");
        let codex_dir = home.path().join(".codex/sessions");
        let claude_dir = home.path().join(".claude/projects");
        std::fs::create_dir_all(&codex_dir).expect("create Codex sessions store");
        std::fs::create_dir_all(&claude_dir).expect("create Claude sessions store");

        let old_codex = "019f5f34-b6b7-70b3-8e50-e98504691cc1";
        let new_codex = "019f5f34-b6b7-70b3-8e50-e98504691cc2";
        let middle_claude = "local-remote-order-middle";
        let old_codex_path = codex_dir.join(format!("rollout-{old_codex}.jsonl"));
        let new_codex_path = codex_dir.join(format!("rollout-{new_codex}.jsonl"));
        let middle_claude_path = claude_dir.join(format!("{middle_claude}.jsonl"));

        for (path, session_id) in [(&old_codex_path, old_codex), (&new_codex_path, new_codex)] {
            std::fs::write(
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
        std::fs::write(
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

        let sessions = scan_sessions_for_home(home.path(), 2)
            .expect("scan ordered remote mixed-provider sessions");
        let logical_ids = sessions
            .iter()
            .map(|session| session.id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(logical_ids, vec![new_codex, middle_claude]);
    }

    #[test]
    fn remote_cli_agent_operations_use_explicit_target_store_roots() {
        let home = tempfile::tempdir().expect("create target home");
        let default_codex_sessions = home.path().join(".codex/sessions");
        let custom_codex_home = home.path().join("target-config/codex");
        let custom_codex_sessions = custom_codex_home.join("sessions");
        let custom_claude_config = home.path().join("target-config/claude");
        std::fs::create_dir_all(&default_codex_sessions).expect("create decoy default store");
        std::fs::create_dir_all(&custom_codex_sessions).expect("create custom Codex store");
        std::fs::create_dir_all(custom_claude_config.join("projects"))
            .expect("create custom Claude store");

        let decoy_id = "019f5f34-b6b7-70b3-8e50-e98504691dd0";
        std::fs::write(
            default_codex_sessions.join(format!("rollout-{decoy_id}.jsonl")),
            format!(
                "{}\n",
                serde_json::json!({
                    "type": "session_meta",
                    "payload": {"id": decoy_id},
                })
            ),
        )
        .expect("write default-root decoy");

        let target_id = "019f5f34-b6b7-70b3-8e50-e98504691dd1";
        let target_path = custom_codex_sessions.join(format!("rollout-{target_id}.jsonl"));
        std::fs::write(
            &target_path,
            format!(
                "{}\n{}\n",
                serde_json::json!({
                    "type": "session_meta",
                    "payload": {"id": target_id},
                }),
                serde_json::json!({
                    "type": "event_msg",
                    "payload": {"type": "user_message", "message": "explicit roots"},
                })
            ),
        )
        .expect("write target-root transcript");

        let roots = CliAgentStoreRoots::from_explicit_target_paths(
            home.path().to_path_buf(),
            custom_claude_config,
            custom_codex_home,
        )
        .expect("valid explicit target roots");

        let scanned = scan_sessions(
            &roots,
            40,
            enum_iterator::all::<crate::terminal::CLIAgent>(),
            [],
        )
        .expect("scan explicit target roots");
        let ScannedSessionDiscovery::Complete {
            sessions: scanned, ..
        } = scanned
        else {
            panic!("explicit target roots should complete");
        };
        assert!(scanned.iter().any(|session| session.id == target_id));
        assert!(!scanned.iter().any(|session| session.id == decoy_id));

        let read = read_session(&target_path.to_string_lossy(), &roots)
            .expect("read transcript through explicit roots");
        assert!(String::from_utf8_lossy(&read.content).contains("explicit roots"));

        mutate_session(&target_path.to_string_lossy(), Mutation::Delete, &roots)
            .expect("delete transcript through explicit roots");
        assert!(!target_path.exists());
        assert!(
            default_codex_sessions
                .join(format!("rollout-{decoy_id}.jsonl"))
                .exists(),
            "explicit target operations must not touch the daemon/default store"
        );
    }

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
