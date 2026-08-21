//! Provider-owned CLI-agent session source mutation.
//!
//! Local Session Navigator and the remote helper both enter through this
//! module. The caller supplies one target-owned roots snapshot and the typed
//! Agent identity carried by discovery; this module is the sole filesystem /
//! database side-effect boundary.

use std::fs;
use std::path::{Path, PathBuf};

use diesel::connection::SimpleConnection;
use diesel::prelude::*;
use diesel::sql_types::{BigInt, Text};
use serde_json::Value;
use tempfile::NamedTempFile;

use super::CliAgentStoreRoots;
use crate::terminal::CLIAgent;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CliAgentSessionSourceMutation {
    Archive,
    Delete,
}

pub(crate) fn mutate_cli_agent_session_source(
    source: &str,
    agent: CLIAgent,
    mutation: CliAgentSessionSourceMutation,
    roots: &CliAgentStoreRoots,
) -> Result<(), String> {
    if let Some((path, session_id)) = split_codex_index_source(source, roots) {
        if !matches!(agent, CLIAgent::Codex) {
            return Err(format!(
                "{} session cannot mutate the Codex session index",
                agent.display_name()
            ));
        }
        return mutate_codex_index_entry(&path, &session_id, mutation, roots);
    }
    if let Some((path, session_id)) = split_store_entry_source(source) {
        return match agent {
            CLIAgent::OpenCode => mutate_opencode_sqlite_entry(&path, &session_id, mutation, roots),
            CLIAgent::CursorCli => mutate_cursor_store_entry(&path, &session_id, mutation, roots),
            CLIAgent::Claude
            | CLIAgent::Codex
            | CLIAgent::Amp
            | CLIAgent::Droid
            | CLIAgent::Copilot
            | CLIAgent::Pi
            | CLIAgent::Auggie
            | CLIAgent::Goose
            | CLIAgent::DeepSeek
            | CLIAgent::Antigravity
            | CLIAgent::Omp
            | CLIAgent::Unknown => Err(format!(
                "{} session cannot mutate a database-backed source",
                agent.display_name()
            )),
        };
    }
    mutate_transcript_file(source, agent, mutation, roots)
}

fn split_codex_index_source(source: &str, roots: &CliAgentStoreRoots) -> Option<(PathBuf, String)> {
    let (path, session_id) = source.rsplit_once(':')?;
    if session_id.trim().is_empty() {
        return None;
    }
    let path = resolved_source_path(path, &roots.home_dir);
    (path == resolved_path(&roots.codex_index())).then(|| (path, session_id.to_owned()))
}

fn split_store_entry_source(source: &str) -> Option<(String, String)> {
    let (path, session_id) = source.rsplit_once('#')?;
    (!path.trim().is_empty() && !session_id.trim().is_empty())
        .then(|| (path.to_owned(), session_id.to_owned()))
}

fn mutate_transcript_file(
    source: &str,
    agent: CLIAgent,
    mutation: CliAgentSessionSourceMutation,
    roots: &CliAgentStoreRoots,
) -> Result<(), String> {
    let path = resolved_source_path(source, &roots.home_dir);
    if !roots.is_authoritative_session_transcript(agent, &path) {
        return Err(format!(
            "refusing to mutate a non-authoritative {} session transcript: {}",
            agent.display_name(),
            path.display()
        ));
    }
    if !path.is_file() {
        return Ok(());
    }
    match mutation {
        CliAgentSessionSourceMutation::Delete => fs::remove_file(&path)
            .map_err(|error| format!("failed to delete {}: {error}", path.display())),
        CliAgentSessionSourceMutation::Archive => {
            let destination = archive_path_for(&path)?;
            fs::rename(&path, &destination).map_err(|error| {
                format!(
                    "failed to archive {} -> {}: {error}",
                    path.display(),
                    destination.display()
                )
            })
        }
    }
}

fn mutate_codex_index_entry(
    path: &Path,
    session_id: &str,
    mutation: CliAgentSessionSourceMutation,
    roots: &CliAgentStoreRoots,
) -> Result<(), String> {
    if path != resolved_path(&roots.codex_index()) {
        return Err(format!(
            "refusing to mutate a non-authoritative Codex index: {}",
            path.display()
        ));
    }
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("failed to read {}: {error}", path.display())),
    };
    let mut removed = Vec::new();
    let mut kept = Vec::new();
    for (index, line) in contents.lines().enumerate() {
        let value = serde_json::from_str::<Value>(line).map_err(|error| {
            format!(
                "invalid Codex index JSONL at {} line {}: {error}",
                path.display(),
                index + 1
            )
        })?;
        if value.get("id").and_then(Value::as_str) == Some(session_id) {
            removed.push(line);
        } else {
            kept.push(line);
        }
    }
    if removed.is_empty() {
        return Ok(());
    }
    if matches!(mutation, CliAgentSessionSourceMutation::Archive) {
        let archive_dir = path
            .parent()
            .unwrap_or_else(|| Path::new(""))
            .join(".ashide-archive");
        fs::create_dir_all(&archive_dir).map_err(|error| {
            format!(
                "failed to create archive dir {}: {error}",
                archive_dir.display()
            )
        })?;
        let archive_path =
            unique_path(&archive_dir.join(format!("session_index-{session_id}.jsonl")))?;
        let mut archived = removed.join("\n");
        archived.push('\n');
        fs::write(&archive_path, archived).map_err(|error| {
            format!(
                "failed to write archive {}: {error}",
                archive_path.display()
            )
        })?;
    }
    let mut rewritten = kept.join("\n");
    if !rewritten.is_empty() {
        rewritten.push('\n');
    }
    atomic_write(path, rewritten.as_bytes())
}

#[derive(QueryableByName)]
struct SqliteColumnName {
    #[diesel(sql_type = Text)]
    name: String,
}

#[derive(QueryableByName)]
struct SqliteCount {
    #[diesel(sql_type = BigInt)]
    count: i64,
}

fn mutate_opencode_sqlite_entry(
    source: &str,
    session_id: &str,
    mutation: CliAgentSessionSourceMutation,
    roots: &CliAgentStoreRoots,
) -> Result<(), String> {
    if !matches!(mutation, CliAgentSessionSourceMutation::Delete) {
        return Err("OpenCode SQLite sessions do not support Ashide-side archive".to_owned());
    }
    let path = resolved_source_path(source, &roots.home_dir);
    let data_root = resolved_path(&roots.opencode_databases_dir());
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if path.parent() != Some(data_root.as_path())
        || !file_name.starts_with("opencode")
        || !file_name.ends_with(".db")
    {
        return Err(format!(
            "refusing to mutate a non-authoritative OpenCode database: {}",
            path.display()
        ));
    }
    if !path.is_file() {
        return Ok(());
    }

    let mut connection = diesel::sqlite::SqliteConnection::establish(&path.to_string_lossy())
        .map_err(|error| {
            format!(
                "failed to open OpenCode database {}: {error}",
                path.display()
            )
        })?;
    connection
        .batch_execute("PRAGMA foreign_keys = ON")
        .map_err(|error| {
            format!(
                "failed to enable OpenCode foreign keys for {}: {error}",
                path.display()
            )
        })?;
    let columns = diesel::sql_query("PRAGMA table_info(session)")
        .load::<SqliteColumnName>(&mut connection)
        .map_err(|error| {
            format!(
                "failed to inspect OpenCode schema {}: {error}",
                path.display()
            )
        })?
        .into_iter()
        .map(|column| column.name)
        .collect::<std::collections::HashSet<_>>();
    if !columns.contains("id") {
        return Err(format!(
            "OpenCode database {} has no canonical session.id column",
            path.display()
        ));
    }
    let supports_children = columns.contains("parent_id");
    connection
        .transaction::<_, diesel::result::Error, _>(|connection| {
            if supports_children {
                diesel::sql_query(
                    "WITH RECURSIVE descendants(id) AS (
                       SELECT id FROM session WHERE id = ?
                       UNION ALL
                       SELECT child.id FROM session child
                       JOIN descendants parent ON child.parent_id = parent.id
                     )
                     DELETE FROM session WHERE id IN (SELECT id FROM descendants)",
                )
                .bind::<Text, _>(session_id)
                .execute(connection)?;
            } else {
                diesel::sql_query("DELETE FROM session WHERE id = ?")
                    .bind::<Text, _>(session_id)
                    .execute(connection)?;
            }
            let remaining = diesel::sql_query("SELECT COUNT(*) AS count FROM session WHERE id = ?")
                .bind::<Text, _>(session_id)
                .get_result::<SqliteCount>(connection)?;
            if remaining.count != 0 {
                return Err(diesel::result::Error::RollbackTransaction);
            }
            Ok(())
        })
        .map_err(|error| {
            format!(
                "failed to delete OpenCode session {session_id} from {}: {error}",
                path.display()
            )
        })
}

fn mutate_cursor_store_entry(
    source: &str,
    session_id: &str,
    mutation: CliAgentSessionSourceMutation,
    roots: &CliAgentStoreRoots,
) -> Result<(), String> {
    let store_path = resolved_source_path(source, &roots.home_dir);
    let chats_root = resolved_path(&roots.cursor_chats());
    let session_dir = store_path.parent().ok_or_else(|| {
        format!(
            "Cursor store has no session directory: {}",
            store_path.display()
        )
    })?;
    let project_dir = session_dir.parent().ok_or_else(|| {
        format!(
            "Cursor session has no project directory: {}",
            session_dir.display()
        )
    })?;
    let source_is_canonical = store_path.file_name().and_then(|name| name.to_str())
        == Some("store.db")
        && session_dir.file_name().and_then(|name| name.to_str()) == Some(session_id)
        && project_dir.parent() == Some(chats_root.as_path());
    if !source_is_canonical {
        return Err(format!(
            "refusing to mutate a non-authoritative Cursor session store: {}",
            store_path.display()
        ));
    }
    if !session_dir.exists() {
        return Ok(());
    }
    let metadata_path = session_dir.join("meta.json");
    if !store_path.is_file() || !metadata_path.is_file() {
        return Err(format!(
            "refusing to mutate incomplete Cursor session store: {}",
            session_dir.display()
        ));
    }
    match mutation {
        CliAgentSessionSourceMutation::Delete => fs::remove_dir_all(session_dir).map_err(|error| {
            format!(
                "failed to delete Cursor session directory {}: {error}",
                session_dir.display()
            )
        }),
        CliAgentSessionSourceMutation::Archive => {
            let archive_dir = project_dir.join(".ashide-archive");
            fs::create_dir_all(&archive_dir).map_err(|error| {
                format!(
                    "failed to create Cursor archive dir {}: {error}",
                    archive_dir.display()
                )
            })?;
            let destination = unique_path(&archive_dir.join(session_id))?;
            fs::rename(session_dir, &destination).map_err(|error| {
                format!(
                    "failed to archive Cursor session {} -> {}: {error}",
                    session_dir.display(),
                    destination.display()
                )
            })
        }
    }
}

fn resolved_source_path(path: &str, home: &Path) -> PathBuf {
    let expanded = if path == "~" {
        home.to_path_buf()
    } else if let Some(relative) = path.strip_prefix("~/") {
        home.join(relative)
    } else {
        PathBuf::from(path)
    };
    resolved_path(&expanded)
}

fn resolved_path(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| {
        path.parent()
            .and_then(|parent| fs::canonicalize(parent).ok())
            .zip(path.file_name())
            .map(|(parent, file_name)| parent.join(file_name))
            .unwrap_or_else(|| path.to_path_buf())
    })
}

fn archive_path_for(path: &Path) -> Result<PathBuf, String> {
    let archive_dir = path
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .join(".ashide-archive");
    fs::create_dir_all(&archive_dir).map_err(|error| {
        format!(
            "failed to create archive dir {}: {error}",
            archive_dir.display()
        )
    })?;
    unique_path(&archive_dir.join(path.file_name().unwrap_or_default()))
}

fn unique_path(path: &Path) -> Result<PathBuf, String> {
    if !path.exists() {
        return Ok(path.to_path_buf());
    }
    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (stem, extension) = match file_name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem.to_owned(), format!(".{extension}")),
        _ => (file_name, String::new()),
    };
    for index in 1..1000 {
        let candidate = parent.join(format!("{stem}-{index}{extension}"));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(format!(
        "could not allocate archive path for {}",
        path.display()
    ))
}

fn atomic_write(path: &Path, contents: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("path has no parent: {}", path.display()))?;
    let mut temp = NamedTempFile::new_in(parent).map_err(|error| {
        format!(
            "failed to create temp file in {}: {error}",
            parent.display()
        )
    })?;
    std::io::Write::write_all(&mut temp, contents)
        .and_then(|()| std::io::Write::flush(&mut temp))
        .map_err(|error| format!("failed to write temp file for {}: {error}", path.display()))?;
    temp.persist(path).map(|_| ()).map_err(|error| {
        format!(
            "failed to atomically replace {}: {}",
            path.display(),
            error.error
        )
    })
}
