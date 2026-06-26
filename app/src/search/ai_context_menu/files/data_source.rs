#![cfg_attr(not(feature = "local_fs"), allow(dead_code))]
use super::search_item::FileSearchItem;
use crate::code::opened_files::OpenedFilesModel;
use crate::search::ai_context_menu::mixer::AIContextMenuSearchableAction;
use crate::search::async_snapshot_data_source::AsyncSnapshotDataSource;
use crate::search::data_source::{DataSourceSearchError, Query, QueryResult};
use crate::search::files::model::FileSearchModel;
use crate::search::files::search_item::FileSearchResult;
use crate::search::mixer::{AsyncDataSource, BoxFuture, DataSourceRunErrorWrapper};
use crate::workspace::environment_runtime;
use crate::workspace::ActiveSession;
use futures_lite::future::yield_now;
use fuzzy_match::FuzzyMatchResult;
use itertools::Itertools;
use repo_metadata::repositories::DetectedRepositories;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use warpui::{AppContext, SingletonEntity};

const MAX_RESULTS: usize = 200;

pub(crate) struct FileSnapshot {
    pub(crate) contents: Arc<Vec<FileSearchResult>>,
    pub(crate) git_changed_files: HashSet<String>,
    pub(crate) query_text: String,
    /// Last-opened timestamps for files, keyed by path. Populated from
    /// `OpenedFilesModel` at snapshot time. Used as a secondary recency
    /// signal within each scoring tier.
    pub(crate) last_opened: HashMap<String, instant::Instant>,
}

/// Builds the repository-backed file search source used by the AI context menu.
/// For empty queries, snapshots repo contents with git-change status to prioritize modified files,
/// and for non-empty queries snapshots repo contents only for faster fuzzy matching.
pub fn file_data_source_for_current_repo(
) -> AsyncSnapshotDataSource<FileSnapshot, AIContextMenuSearchableAction> {
    AsyncSnapshotDataSource::new(
        |query: &Query, app: &AppContext| {
            if FileSearchModel::should_skip_overly_broad_query(&query.text) {
                return FileSnapshot {
                    contents: Arc::new(Vec::new()),
                    git_changed_files: HashSet::new(),
                    query_text: query.text.clone(),
                    last_opened: HashMap::new(),
                };
            }

            let file_search_model = FileSearchModel::as_ref(app);
            let last_opened = snapshot_last_opened(app);
            if query.text.is_empty() {
                let (contents, git_changed_files) =
                    file_search_model.get_repo_contents_with_git_status(app);
                FileSnapshot {
                    contents,
                    git_changed_files,
                    query_text: query.text.clone(),
                    last_opened,
                }
            } else {
                let contents = file_search_model.get_repo_contents(app);
                FileSnapshot {
                    contents,
                    git_changed_files: HashSet::new(),
                    query_text: query.text.clone(),
                    last_opened,
                }
            }
        },
        fuzzy_match_files,
    )
}

pub fn file_data_source_for_pwd(
    app: &AppContext,
) -> AsyncSnapshotDataSource<FileSnapshot, AIContextMenuSearchableAction> {
    let file_search_model = FileSearchModel::as_ref(app);
    let mut cached_contents = file_search_model.get_folder_contents(app);
    // Reverse sort to put what you'd expect at the top for zero-state
    cached_contents.sort_by(|a, b| b.path.cmp(&a.path));
    let cached_contents = Arc::new(cached_contents);

    AsyncSnapshotDataSource::new(
        move |query: &Query, _app: &AppContext| {
            if FileSearchModel::should_skip_overly_broad_query(&query.text) {
                return FileSnapshot {
                    contents: Arc::new(Vec::new()),
                    git_changed_files: HashSet::new(),
                    query_text: query.text.clone(),
                    last_opened: HashMap::new(),
                };
            }

            FileSnapshot {
                contents: cached_contents.clone(),
                git_changed_files: HashSet::new(),
                query_text: query.text.clone(),
                last_opened: HashMap::new(),
            }
        },
        fuzzy_match_files,
    )
}

pub(crate) enum ActiveDirectoryFileDataSource {
    CurrentApp(AsyncSnapshotDataSource<FileSnapshot, AIContextMenuSearchableAction>),
    EnvironmentRuntime(EnvironmentDirectoryFileDataSource),
}

pub(crate) fn file_data_source_for_active_directory(
    app: &AppContext,
) -> ActiveDirectoryFileDataSource {
    let active_session = ActiveSession::as_ref(app);
    let uses_environment_runtime = app
        .windows()
        .state()
        .active_window
        .and_then(|window_id| active_session.session(window_id))
        .is_some_and(|session| session.session_type().uses_environment_runtime());

    if uses_environment_runtime {
        ActiveDirectoryFileDataSource::EnvironmentRuntime(EnvironmentDirectoryFileDataSource)
    } else {
        ActiveDirectoryFileDataSource::CurrentApp(file_data_source_for_pwd(app))
    }
}

impl AsyncDataSource for ActiveDirectoryFileDataSource {
    type Action = AIContextMenuSearchableAction;

    fn run_query(
        &self,
        query: &Query,
        app: &AppContext,
    ) -> BoxFuture<'static, Result<Vec<QueryResult<Self::Action>>, DataSourceRunErrorWrapper>> {
        match self {
            ActiveDirectoryFileDataSource::CurrentApp(data_source) => {
                data_source.run_query(query, app)
            }
            ActiveDirectoryFileDataSource::EnvironmentRuntime(data_source) => {
                data_source.run_query(query, app)
            }
        }
    }
}

pub(crate) struct EnvironmentDirectoryFileDataSource;

impl AsyncDataSource for EnvironmentDirectoryFileDataSource {
    type Action = AIContextMenuSearchableAction;

    fn run_query(
        &self,
        query: &Query,
        app: &AppContext,
    ) -> BoxFuture<'static, Result<Vec<QueryResult<Self::Action>>, DataSourceRunErrorWrapper>> {
        if FileSearchModel::should_skip_overly_broad_query(&query.text) {
            return Box::pin(async { Ok(Vec::new()) });
        }

        let active_session = ActiveSession::as_ref(app);
        let active_environment_directory =
            app.windows().state().active_window.and_then(|window_id| {
                let session = active_session.session(window_id)?;
                if !session.session_type().uses_environment_runtime() {
                    return None;
                }
                let current_working_directory = active_session
                    .current_working_directory(window_id)?
                    .to_owned();
                let client = environment_runtime::client_for_session(session.id(), app)?;
                Some((client, current_working_directory))
            });

        let query_text = query.text.clone();
        Box::pin(async move {
            let Some((client, current_working_directory)) = active_environment_directory else {
                return Ok(Vec::new());
            };

            let listing = environment_runtime::list_directory(&client, current_working_directory)
                .await
                .map_err(|error| {
                    Box::new(DataSourceSearchError {
                        message: format!("Failed to list remote directory: {error}"),
                    }) as DataSourceRunErrorWrapper
                })?;
            let project_directory = listing.canonical_path.clone();
            let mut contents = listing
                .entries
                .into_iter()
                .filter(|entry| entry.name != "." && entry.name != "..")
                .map(|entry| FileSearchResult {
                    path: entry.name,
                    project_directory: project_directory.clone(),
                    is_directory: entry.is_dir,
                })
                .collect_vec();

            // 对齐本地当前目录 source:zero-state 需要稳定、接近底部 append 的体感,
            // 而不是 repo-wide 的最近打开排序。
            contents.sort_by(|left, right| right.path.cmp(&left.path));

            fuzzy_match_files(FileSnapshot {
                contents: Arc::new(contents),
                git_changed_files: HashSet::new(),
                query_text,
                last_opened: HashMap::new(),
            })
            .await
        })
    }
}

/// Captures last-opened timestamps from `OpenedFilesModel` for the active
/// repo at snapshot time. Returns an empty map when no repo is active.
fn snapshot_last_opened(app: &AppContext) -> HashMap<String, instant::Instant> {
    let git_repo_path = app
        .windows()
        .state()
        .active_window
        .and_then(|window_id| ActiveSession::as_ref(app).current_app_path(window_id))
        .and_then(|current_dir| {
            DetectedRepositories::as_ref(app).get_root_for_path(Path::new(current_dir))
        });

    let Some(repo_path) = git_repo_path else {
        return HashMap::new();
    };

    let opened_files_model = OpenedFilesModel::as_ref(app);
    let Some(opened_in_repo) = opened_files_model.opened_files_for_repo(&repo_path) else {
        return HashMap::new();
    };

    // Convert PathBuf keys to String keys matching FileSearchResult.path
    // (relative paths from repo root).
    opened_in_repo
        .iter()
        .map(|(path, ts)| (path.to_string_lossy().to_string(), *ts))
        .collect()
}

/// Routes file matching to zero-state ranking or query-based fuzzy scoring.
pub(crate) fn fuzzy_match_files(
    snapshot: FileSnapshot,
) -> BoxFuture<
    'static,
    Result<Vec<QueryResult<AIContextMenuSearchableAction>>, DataSourceRunErrorWrapper>,
> {
    Box::pin(async move {
        if snapshot.query_text.is_empty() {
            Ok(fuzzy_match_files_zero_state(snapshot).await)
        } else {
            Ok(fuzzy_match_files_query(snapshot).await)
        }
    })
}

/// Build a recency index: sort files by last-opened timestamp (ascending,
/// `None` first) and return a map from path to sort position.
fn build_recency_index(
    contents: &[FileSearchResult],
    last_opened: &HashMap<String, instant::Instant>,
) -> HashMap<String, usize> {
    let mut opened: Vec<_> = contents
        .iter()
        .filter_map(|item| last_opened.get(&item.path).map(|ts| (&item.path, ts)))
        .collect();
    opened.sort_by_key(|(_, ts)| *ts);
    opened
        .into_iter()
        .enumerate()
        .map(|(rank, (path, _))| (path.clone(), rank + 1))
        .collect()
}

/// Returns zero-state file results with two scoring tiers and recency
/// as a secondary sort within each tier.
async fn fuzzy_match_files_zero_state(
    snapshot: FileSnapshot,
) -> Vec<QueryResult<AIContextMenuSearchableAction>> {
    let recency_index = build_recency_index(&snapshot.contents, &snapshot.last_opened);
    let max_recency = recency_index.len();
    let mut results: Vec<QueryResult<AIContextMenuSearchableAction>> = Vec::new();

    // Pass 1: git-changed or recently-opened files (guaranteed inclusion)
    for chunk in snapshot.contents.chunks(512) {
        for item in chunk {
            let is_git_changed = snapshot.git_changed_files.contains(&item.path);
            let is_recently_opened = snapshot.last_opened.contains_key(&item.path);

            if is_git_changed || is_recently_opened {
                let rank = recency_index.get(&item.path).copied().unwrap_or(0);
                let recency_bonus = if max_recency > 0 {
                    (30 * rank / max_recency) as i64
                } else {
                    0
                };
                let base_score = if is_git_changed { 10000 } else { 0 };
                let match_result = FuzzyMatchResult {
                    score: base_score + recency_bonus,
                    matched_indices: vec![],
                };
                let search_item = FileSearchItem {
                    path: PathBuf::from(&item.path),
                    match_result,
                    is_directory: item.is_directory,
                };
                results.push(QueryResult::from(search_item));
            }
        }
        yield_now().await;
    }

    // Pass 2: fill remaining capacity with untouched files
    for chunk in snapshot.contents.chunks(512) {
        for item in chunk {
            if !snapshot.git_changed_files.contains(&item.path)
                && !snapshot.last_opened.contains_key(&item.path)
                && results.len() < MAX_RESULTS
            {
                let match_result = FuzzyMatchResult {
                    score: 0,
                    matched_indices: vec![],
                };
                let search_item = FileSearchItem {
                    path: PathBuf::from(&item.path),
                    match_result,
                    is_directory: item.is_directory,
                };
                results.push(QueryResult::from(search_item));
            }
        }
        yield_now().await;
    }

    results
}

/// Returns fuzzy-ranked file results for non-empty queries.
async fn fuzzy_match_files_query(
    snapshot: FileSnapshot,
) -> Vec<QueryResult<AIContextMenuSearchableAction>> {
    let recency_index = build_recency_index(&snapshot.contents, &snapshot.last_opened);
    let max_recency = recency_index.len();
    let mut results = Vec::new();

    for chunk in snapshot.contents.chunks(512) {
        for item in chunk {
            if let Some(mut match_result) =
                FileSearchModel::fuzzy_match_path(&item.path, &snapshot.query_text)
            {
                // Give files a slight boost over directories to prioritize them when names are similar
                if !item.is_directory {
                    match_result.score += 100;
                }

                // Add a recency bonus, capped at 30.
                let rank = recency_index.get(&item.path).copied().unwrap_or(0);
                let recency_bonus = if max_recency > 0 {
                    (30 * rank / max_recency) as i64
                } else {
                    0
                };

                match_result.score += recency_bonus;

                let search_item = FileSearchItem {
                    path: PathBuf::from(&item.path),
                    match_result,
                    is_directory: item.is_directory,
                };
                results.push(QueryResult::from(search_item));
            }
        }
        yield_now().await;
    }

    results
        .into_iter()
        .k_largest_relaxed_by_key(MAX_RESULTS, |item| item.score())
        .collect()
}
