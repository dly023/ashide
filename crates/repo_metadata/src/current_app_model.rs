#![cfg_attr(not(feature = "local_fs"), allow(dead_code))]
//! Repository metadata model singleton.
//!
//! This module provides a singleton model that manages repository metadata across
//! all repositories tracked by Ashide.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
};

use warp_core::safe_warn;
use warpui::ModelHandle;

/// Represents a filesystem entry in a repository without erasing symlink identity.
#[derive(Debug, Clone)]
pub enum RepoContent<'a> {
    File(&'a FileTreeFileMetadata),
    Directory(&'a FileTreeDirectoryEntryState),
    Symlink(&'a FileTreeSymlinkMetadata),
}

use warp_util::standardized_path::StandardizedPath;

use crate::{
    entry::{Entry, IgnoredPathStrategy},
    gitignores_for_directory, matches_gitignores,
    repository::Repository,
    RepoMetadataError,
};
use std::sync::Arc;
cfg_if::cfg_if! {
    if #[cfg(feature = "local_fs")] {
        use notify_debouncer_full::notify::{RecursiveMode, WatchFilter};
        use crate::repositories::{DetectedRepositories, DetectedRepositoriesEvent};
        use watcher::{BulkFilesystemWatcher, BulkFilesystemWatcherEvent};
        use warpui::SingletonEntity as _;

        /// Duration between filesystem watch events in seconds
        const FILESYSTEM_WATCHER_DEBOUNCE_SECS: u64 = 1;
    }
}

use crate::file_tree_store::{
    FileTreeDirectoryEntryState, FileTreeEntry, FileTreeEntryState, FileTreeFileMetadata,
    FileTreeState, FileTreeSymlinkMetadata,
};
use crate::file_tree_update::{
    flatten_entry_metadata, DirectoryNodeMetadata, FileTreeEntryUpdate, RepoMetadataUpdate,
    RepoNodeMetadata,
};
use ignore::gitignore::Gitignore;
use warpui::ModelContext;

/// Maximum depth to traverse when building file trees
const MAX_TREE_DEPTH: usize = 200;

/// Maximum number of files to index per repository to guard against really large codebases
const MAX_FILES_PER_REPO: usize = 100_000;

/// Returns true when `path` is too broad to be a recursive file-watch root.
///
/// Rejects the user's home directory itself and any of its ancestors
/// (e.g. `/Users`, `/home`, `/`). Registering such a path as a repository
/// root makes the OS push fsevents from unrelated areas (`~/Library/*`,
/// `~/Pictures/Photos Library.photoslibrary/*`, IM caches, …) into the
/// indexer, leaking user data and producing endless `PermissionDenied`
/// build_tree noise.
#[cfg(feature = "local_fs")]
fn is_unsafe_watch_root(path: &Path) -> bool {
    let Some(home) = dirs::home_dir() else {
        return false;
    };
    path == home.as_path() || home.starts_with(path)
}

#[derive(Debug)]
/// Events emitted by the CurrentAppRepoMetadataModel.
pub enum RepositoryMetadataEvent {
    /// A repository was added or updated.
    RepositoryUpdated {
        path: StandardizedPath,
    },
    /// A repository was removed.
    RepositoryRemoved {
        path: StandardizedPath,
    },
    /// The file tree for the repositories were updated.
    FileTreeUpdated {
        paths: Vec<StandardizedPath>,
    },
    /// The file tree's [`Entry`] was updated.
    FileTreeEntryUpdated {
        path: StandardizedPath,
    },
    UpdatingRepositoryFailed {
        path: StandardizedPath,
    },
    /// Emitted after watcher mutations are applied when
    /// `emit_incremental_updates` is enabled, containing a serializable
    /// update suitable for sending to the remote client.
    IncrementalUpdateReady {
        update: RepoMetadataUpdate,
    },
}

/// Represents the state of a repository in the metadata model.
#[derive(Debug)]
pub enum IndexedRepoState {
    /// Repository is currently being indexed.
    Pending,
    /// Repository has been successfully indexed.
    Indexed(FileTreeState),

    /// Repository indexing failed with the given error.
    Failed(RepoMetadataError),
}

/// Singleton model for managing current-app repository metadata.
///
/// This model tracks repositories available to the current app process, using file watchers
/// to stay up to date and subscribing to `DetectedRepositories` for auto-indexing.
///
/// Consumers should access this through the [`RepoMetadataModel`](crate::wrapper_model::RepoMetadataModel)
/// wrapper rather than using this type directly.
pub struct CurrentAppRepoMetadataModel {
    /// Mapping from repository path to its indexed state.
    repositories: HashMap<StandardizedPath, IndexedRepoState>,
    /// Refcounts for lazily-loaded standalone paths tracked in the model.
    lazy_loaded_paths: HashMap<StandardizedPath, usize>,
    /// File system watcher for monitoring changes.
    #[cfg(feature = "local_fs")]
    watcher: Option<ModelHandle<BulkFilesystemWatcher>>,
    /// Symlinks are filesystem mounts: watcher events arrive from their target
    /// namespace but the file-tree identity stays lexical.
    /// This map is the single projection authority from target namespace back
    /// into each repository's link namespace.
    #[cfg(feature = "local_fs")]
    symlink_watch_mounts: HashMap<StandardizedPath, HashMap<StandardizedPath, SymlinkWatchMount>>,
    /// Exact watch roots, requested depth and logical owners. Repository roots
    /// and external symlink mounts share this one registry so unregistering one
    /// owner can never tear down or weaken a watch still required by another owner.
    #[cfg(feature = "local_fs")]
    watch_path_owners: HashMap<StandardizedPath, HashMap<WatchPathOwner, WatchDepth>>,
    /// Watcher batches are applied FIFO per repository. Filesystem reads run on
    /// background threads, so unconstrained `ctx.spawn` callbacks can otherwise
    /// complete out of order and restore stale symlink/tree state.
    #[cfg(feature = "local_fs")]
    pending_repo_updates: HashMap<StandardizedPath, VecDeque<RepoUpdate>>,
    /// Active pipeline token per repository. Re-index/remove invalidates the
    /// token so a callback from an old repository incarnation cannot mutate a
    /// newly inserted tree at the same path.
    #[cfg(feature = "local_fs")]
    repo_update_in_flight: HashMap<StandardizedPath, u64>,
    #[cfg(feature = "local_fs")]
    next_repo_update_token: u64,
    /// When true, emit [`RepositoryMetadataEvent::IncrementalUpdateReady`]
    /// events after applying watcher mutations. Only the remote server
    /// variant enables this.
    emit_incremental_updates: bool,
}

#[derive(Debug, Clone, Default)]
struct RepoUpdate {
    added: Vec<PathBuf>,
    deleted: Vec<PathBuf>,
    moved: HashMap<PathBuf, PathBuf>,
    /// Re-read current lexical state instead of trusting the source event kind.
    /// External target events are enrichment changes: deleting the target must
    /// turn the lexical link into `Missing`, not delete the link inode.
    refreshed: Vec<PathBuf>,
}

#[cfg(feature = "local_fs")]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ProjectedWatchPath {
    repo_path: StandardizedPath,
    path: PathBuf,
    mount_path: Option<StandardizedPath>,
}

#[cfg(feature = "local_fs")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum WatchDepth {
    Direct,
    Recursive,
}

#[cfg(feature = "local_fs")]
#[derive(Debug, Clone, PartialEq, Eq)]
struct SymlinkWatchMount {
    /// Canonical target namespace used to project content events back into the
    /// lexical link namespace. Missing targets are reconstructed from their
    /// nearest existing ancestor and do not need to exist yet.
    target_path: StandardizedPath,
    /// Lexical target identities whose inode lifecycle can change without any
    /// event under `target_path` (for example `link -> alias -> target`). Such
    /// events refresh the link root and rebuild the mount from current state.
    lifecycle_targets: HashSet<StandardizedPath>,
    /// Concrete watcher roots. Loaded directories own a recursive content
    /// watch plus a direct parent lifecycle watch; missing targets own a direct
    /// watch on their nearest existing ancestor.
    watch_paths: HashMap<StandardizedPath, WatchDepth>,
}

#[cfg(feature = "local_fs")]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum WatchPathOwner {
    Repository {
        repo_path: StandardizedPath,
    },
    SymlinkMount {
        repo_path: StandardizedPath,
        lexical_path: StandardizedPath,
    },
}

/// Describes a single file-tree mutation computed on a background thread.
/// These are produced by `compute_file_tree_mutations` (filesystem I/O) and
/// consumed by `apply_file_tree_mutations` (tree-only, main thread).
#[derive(Debug)]
pub(crate) enum FileTreeMutation {
    /// Remove a path from the tree.
    Remove(PathBuf),
    /// Add or replace an entry without collapsing symlinks into their targets.
    AddEntry { path: PathBuf, entry: Entry },
    /// Fallback: add a bare (unloaded) directory entry when `build_tree` fails.
    AddEmptyDirectory { path: PathBuf, is_ignored: bool },
}

/// A filter function for filtering repo contents during traversal.
type RepoContentFilter = dyn for<'a> Fn(&RepoContent<'a>) -> bool + Send + Sync;

pub struct GetContentsArgs {
    pub include_folders: bool,
    pub include_ignored: bool,
    /// Optional filter applied during traversal to skip entries early.
    /// Return `true` to include the entry, `false` to skip it.
    pub filter: Option<Arc<RepoContentFilter>>,
}

impl Default for GetContentsArgs {
    fn default() -> Self {
        Self {
            include_folders: true,
            include_ignored: false,
            filter: None,
        }
    }
}

impl GetContentsArgs {
    pub fn include_ignored(mut self) -> Self {
        self.include_ignored = true;
        self
    }

    pub fn exclude_folders(mut self) -> Self {
        self.include_folders = false;
        self
    }

    /// Sets a filter closure to be applied during traversal.
    /// Only entries for which the filter returns `true` will be included.
    pub fn with_filter<F>(self, filter: F) -> Self
    where
        F: for<'a> Fn(&RepoContent<'a>) -> bool + Send + Sync + 'static,
    {
        Self {
            include_folders: self.include_folders,
            include_ignored: self.include_ignored,
            filter: Some(Arc::new(filter)),
        }
    }
}

impl CurrentAppRepoMetadataModel {
    /// Creates a new CurrentAppRepoMetadataModel.
    #[cfg_attr(not(feature = "local_fs"), allow(unused_variables), allow(unused_mut))]
    pub fn new(ctx: &mut ModelContext<Self>) -> Self {
        let mut model = Self {
            repositories: HashMap::new(),
            lazy_loaded_paths: HashMap::new(),
            #[cfg(feature = "local_fs")]
            watcher: None,
            #[cfg(feature = "local_fs")]
            symlink_watch_mounts: HashMap::new(),
            #[cfg(feature = "local_fs")]
            watch_path_owners: HashMap::new(),
            #[cfg(feature = "local_fs")]
            pending_repo_updates: HashMap::new(),
            #[cfg(feature = "local_fs")]
            repo_update_in_flight: HashMap::new(),
            #[cfg(feature = "local_fs")]
            next_repo_update_token: 1,
            emit_incremental_updates: false,
        };
        cfg_if::cfg_if! {
            if #[cfg(feature = "local_fs")] {
                let watcher = ctx.add_model(|ctx| {
                    BulkFilesystemWatcher::new(
                        std::time::Duration::from_secs(FILESYSTEM_WATCHER_DEBOUNCE_SECS),
                        ctx,
                    )
                });
                ctx.subscribe_to_model(&watcher, Self::handle_watcher_event);
                model.watcher = Some(watcher);

                ctx.subscribe_to_model(&DetectedRepositories::handle(ctx), |me, event, ctx| {
                    let DetectedRepositoriesEvent::DetectedGitRepo { repository, .. } = event;
                    let repo_path = repository.as_ref(ctx).root_dir().clone();
                    if let Err(e) = me.index_directory(repository.clone(), ctx) {
                        log::warn!(
                            "Failed to index directory {repo_path}: {e}"
                        );
                    }
                });
            }
        }

        model
    }

    /// Enables or disables emission of
    /// [`RepositoryMetadataEvent::IncrementalUpdateReady`] events after
    /// applying watcher mutations. Only the remote server variant should
    /// enable this.
    pub fn set_emit_incremental_updates(&mut self, enabled: bool) {
        self.emit_incremental_updates = enabled;
    }

    /// Handles events from the BulkFilesystemWatcher.
    #[cfg(feature = "local_fs")]
    fn handle_watcher_event(
        &mut self,
        event: &BulkFilesystemWatcherEvent,
        ctx: &mut ModelContext<Self>,
    ) {
        let mut repo_updates: HashMap<StandardizedPath, RepoUpdate> = HashMap::new();

        for path in event.added_or_updated_iter() {
            for projected in self.project_watch_path(path) {
                let update = repo_updates.entry(projected.repo_path).or_default();
                if projected.mount_path.is_some() {
                    update.refreshed.push(projected.path);
                } else {
                    update.added.push(projected.path);
                }
            }
        }

        for path in &event.deleted {
            let projections = self.project_watch_path(path);
            if projections.is_empty() {
                log::warn!("Deleted file not found in any repo: {path:?} not found in any repo");
            }
            for projected in projections {
                let update = repo_updates.entry(projected.repo_path).or_default();
                if projected.mount_path.is_some() {
                    update.refreshed.push(projected.path);
                } else {
                    update.deleted.push(projected.path);
                }
            }
        }

        for (to_path, from_path) in &event.moved {
            let (to_mount_projections, to_projections): (Vec<_>, Vec<_>) = self
                .project_watch_path(to_path)
                .into_iter()
                .partition(|projection| projection.mount_path.is_some());
            let (from_mount_projections, from_projections): (Vec<_>, Vec<_>) = self
                .project_watch_path(from_path)
                .into_iter()
                .partition(|projection| projection.mount_path.is_some());

            for projected in to_mount_projections
                .into_iter()
                .chain(from_mount_projections)
            {
                repo_updates
                    .entry(projected.repo_path)
                    .or_default()
                    .refreshed
                    .push(projected.path);
            }
            let mut matched_from = HashSet::new();

            for to_projection in to_projections {
                if let Some((from_index, from_projection)) = from_projections
                    .iter()
                    .enumerate()
                    .find(|(index, from_projection)| {
                        !matched_from.contains(index)
                            && from_projection.repo_path == to_projection.repo_path
                            && from_projection.mount_path == to_projection.mount_path
                    })
                {
                    matched_from.insert(from_index);
                    repo_updates
                        .entry(to_projection.repo_path)
                        .or_default()
                        .moved
                        .insert(to_projection.path, from_projection.path.clone());
                } else {
                    repo_updates
                        .entry(to_projection.repo_path)
                        .or_default()
                        .added
                        .push(to_projection.path);
                }
            }

            for (index, from_projection) in from_projections.into_iter().enumerate() {
                if !matched_from.contains(&index) {
                    repo_updates
                        .entry(from_projection.repo_path)
                        .or_default()
                        .deleted
                        .push(from_projection.path);
                }
            }
        }

        // Collect all paths that have been updated and emit an event.
        ctx.emit(RepositoryMetadataEvent::FileTreeUpdated {
            paths: repo_updates.keys().cloned().collect(),
        });
        for (repo_path, repo_scoped_update) in repo_updates {
            self.enqueue_repo_update(repo_path, repo_scoped_update, ctx);
        }
    }

    #[cfg(feature = "local_fs")]
    fn enqueue_repo_update(
        &mut self,
        repo_path: StandardizedPath,
        update: RepoUpdate,
        ctx: &mut ModelContext<Self>,
    ) {
        self.pending_repo_updates
            .entry(repo_path.clone())
            .or_default()
            .push_back(update);
        self.start_next_repo_update(repo_path, ctx);
    }

    #[cfg(feature = "local_fs")]
    fn start_next_repo_update(
        &mut self,
        repo_path: StandardizedPath,
        ctx: &mut ModelContext<Self>,
    ) {
        if self.repo_update_in_flight.contains_key(&repo_path) {
            return;
        }
        let Some(update) = self
            .pending_repo_updates
            .get_mut(&repo_path)
            .and_then(VecDeque::pop_front)
        else {
            self.pending_repo_updates.remove(&repo_path);
            return;
        };
        if self
            .pending_repo_updates
            .get(&repo_path)
            .is_some_and(VecDeque::is_empty)
        {
            self.pending_repo_updates.remove(&repo_path);
        }
        let Some(IndexedRepoState::Indexed(state)) = self.repositories.get(&repo_path) else {
            self.pending_repo_updates.remove(&repo_path);
            return;
        };

        let token = self.next_repo_update_token;
        self.next_repo_update_token = self
            .next_repo_update_token
            .checked_add(1)
            .expect("repository update token space exhausted");
        self.repo_update_in_flight.insert(repo_path.clone(), token);
        let gitignores = state.gitignores.clone();
        let lazy_load = self.lazy_loaded_paths.contains_key(&repo_path);
        ctx.spawn(
            async move {
                let mutations = Self::compute_file_tree_mutations(&update, &gitignores).await;
                (mutations, repo_path, lazy_load, token)
            },
            |model, (mutations, repo_path, lazy_load, token), ctx| {
                if model.repo_update_in_flight.get(&repo_path) != Some(&token) {
                    return;
                }
                model.repo_update_in_flight.remove(&repo_path);
                let update = model.repositories.get_mut(&repo_path).and_then(|state| {
                    let IndexedRepoState::Indexed(state) = state else {
                        return None;
                    };
                    Some(Self::apply_file_tree_mutations(
                        &mut state.entry,
                        mutations,
                        lazy_load,
                        model.emit_incremental_updates,
                    ))
                });
                if let Some(update) = update {
                    model.refresh_symlink_watch_mounts(&repo_path, ctx);
                    ctx.emit(RepositoryMetadataEvent::FileTreeEntryUpdated {
                        path: repo_path.clone(),
                    });
                    if let Some(update) = update {
                        ctx.emit(RepositoryMetadataEvent::IncrementalUpdateReady { update });
                    }
                }
                model.start_next_repo_update(repo_path, ctx);
            },
        );
    }

    #[cfg(feature = "local_fs")]
    fn invalidate_repo_update_pipeline(&mut self, repo_path: &StandardizedPath) {
        self.pending_repo_updates.remove(repo_path);
        self.repo_update_in_flight.remove(repo_path);
    }

    #[cfg(feature = "local_fs")]
    fn project_watch_path(&self, path: &Path) -> Vec<ProjectedWatchPath> {
        let mut projections = HashSet::new();
        if let Some(repo_path) = self.find_repository_for_path(path) {
            projections.insert(ProjectedWatchPath {
                repo_path,
                path: path.to_path_buf(),
                mount_path: None,
            });
        }

        for (repo_path, mounts) in &self.symlink_watch_mounts {
            for (lexical_mount, mount) in mounts {
                let Some(target_path) = mount.target_path.to_local_path() else {
                    continue;
                };
                let Some(lexical_mount_path) = lexical_mount.to_local_path() else {
                    continue;
                };
                let projected_path = match path.strip_prefix(&target_path) {
                    Ok(relative_path) if relative_path.as_os_str().is_empty() => lexical_mount_path,
                    Ok(relative_path) => lexical_mount_path.join(relative_path),
                    Err(_)
                        if target_path.starts_with(path)
                            || mount
                                .lifecycle_targets
                                .iter()
                                .filter_map(StandardizedPath::to_local_path)
                                .any(|target| target == path || target.starts_with(path)) =>
                    {
                        // A missing multi-component target advances one
                        // ancestor at a time. Creation/removal of an
                        // intermediate component must refresh the link root so
                        // the lifecycle watch can move to the new nearest
                        // existing ancestor.
                        lexical_mount_path
                    }
                    Err(_) => continue,
                };
                projections.insert(ProjectedWatchPath {
                    repo_path: repo_path.clone(),
                    path: projected_path,
                    mount_path: Some(lexical_mount.clone()),
                });
            }
        }

        projections.into_iter().collect()
    }

    #[cfg(feature = "local_fs")]
    fn find_repository_for_standardized_path(
        &self,
        path: &StandardizedPath,
    ) -> Option<StandardizedPath> {
        self.repositories
            .iter()
            .filter(|(repo_path, state)| {
                path.starts_with(repo_path) && matches!(state, IndexedRepoState::Indexed(_))
            })
            .max_by_key(|(repo_path, _)| repo_path.as_str().len())
            .map(|(repo_path, _)| repo_path.clone())
    }

    #[cfg(feature = "local_fs")]
    pub fn find_repository_for_path(&self, path: &Path) -> Option<StandardizedPath> {
        StandardizedPath::try_from_local(path)
            .ok()
            .and_then(|path| self.find_repository_for_standardized_path(&path))
            .or_else(|| {
                StandardizedPath::from_local_canonicalized(path)
                    .ok()
                    .and_then(|path| self.find_repository_for_standardized_path(&path))
            })
    }

    #[cfg(feature = "local_fs")]
    fn symlink_watch_mount(symlink: &FileTreeSymlinkMetadata) -> Option<SymlinkWatchMount> {
        use crate::SymlinkTargetKind;

        if symlink.target_kind == SymlinkTargetKind::Other {
            return None;
        }
        let lexical_path = symlink.path.to_local_path()?;
        let raw_target = std::fs::read_link(&lexical_path).ok()?;
        let absolute_target = if raw_target.is_absolute() {
            raw_target
        } else {
            lexical_path.parent()?.join(raw_target)
        };
        let lexical_target = StandardizedPath::try_from_local(&absolute_target).ok()?;
        let lexical_target_path = lexical_target.to_local_path()?;

        let mut lifecycle_targets = HashSet::from([lexical_target.clone()]);
        let (target_path, lifecycle_anchors) = match symlink.target_kind {
            SymlinkTargetKind::File | SymlinkTargetKind::Directory => {
                let target_path = StandardizedPath::from_local_canonicalized(&lexical_path).ok()?;
                lifecycle_targets.insert(target_path.clone());
                let mut anchors = HashSet::from([target_path.parent()?]);
                if let Some(raw_parent) = lexical_target.parent() {
                    if let Some(raw_parent_path) = raw_parent.to_local_path() {
                        if let Ok(raw_parent) =
                            StandardizedPath::from_local_canonicalized(&raw_parent_path)
                        {
                            anchors.insert(raw_parent);
                        }
                    }
                }
                (target_path, anchors)
            }
            SymlinkTargetKind::Missing => {
                let (existing_ancestor, canonical_ancestor) =
                    lexical_target.ancestors().skip(1).find_map(|ancestor| {
                        let local_path = ancestor.to_local_path()?;
                        if !local_path.is_dir() {
                            return None;
                        }
                        let canonical =
                            StandardizedPath::from_local_canonicalized(&local_path).ok()?;
                        Some((ancestor, canonical))
                    })?;
                let existing_path = existing_ancestor.to_local_path()?;
                let relative = lexical_target_path.strip_prefix(existing_path).ok()?;
                let canonical_path = canonical_ancestor.to_local_path()?.join(relative);
                let target_path = StandardizedPath::try_from_local(&canonical_path).ok()?;
                lifecycle_targets.insert(target_path.clone());
                (target_path, HashSet::from([canonical_ancestor]))
            }
            SymlinkTargetKind::Other => return None,
        };

        let mut watch_paths = HashMap::new();
        for lifecycle_anchor in lifecycle_anchors {
            watch_paths.insert(lifecycle_anchor, WatchDepth::Direct);
        }
        if symlink.target_kind == SymlinkTargetKind::Directory && symlink.loaded {
            watch_paths.insert(target_path.clone(), WatchDepth::Recursive);
        }

        Some(SymlinkWatchMount {
            target_path,
            lifecycle_targets,
            watch_paths,
        })
    }

    #[cfg(feature = "local_fs")]
    fn collect_symlink_watch_mounts(
        tree: &FileTreeEntry,
        path: &StandardizedPath,
        mounts: &mut HashMap<StandardizedPath, SymlinkWatchMount>,
    ) {
        let Some(entry) = tree.get(path) else {
            return;
        };
        match entry {
            FileTreeEntryState::File(_) => return,
            FileTreeEntryState::Directory(directory) => {
                if !directory.loaded {
                    return;
                }
            }
            FileTreeEntryState::Symlink(symlink) => {
                if let Some(mount) = Self::symlink_watch_mount(symlink) {
                    if mount.target_path != *symlink.path {
                        mounts.insert(symlink.path.as_ref().clone(), mount);
                    }
                }
                if symlink.target_kind != crate::SymlinkTargetKind::Directory || !symlink.loaded {
                    return;
                }
            }
        }

        let children: Vec<_> = tree.child_paths(path).cloned().collect();
        for child in children {
            Self::collect_symlink_watch_mounts(tree, &child, mounts);
        }
    }

    #[cfg(feature = "local_fs")]
    fn symlink_watch_mounts(state: &FileTreeState) -> HashMap<StandardizedPath, SymlinkWatchMount> {
        let mut mounts = HashMap::new();
        Self::collect_symlink_watch_mounts(&state.entry, state.entry.root_directory(), &mut mounts);
        mounts
    }

    #[cfg(feature = "local_fs")]
    fn watch_depth_is_unsafe(path: &Path, depth: WatchDepth) -> bool {
        if depth == WatchDepth::Recursive {
            return is_unsafe_watch_root(path);
        }
        let Some(home) = dirs::home_dir() else {
            return false;
        };
        path != home && home.starts_with(path)
    }

    #[cfg(feature = "local_fs")]
    fn set_watch_path_registration(
        &mut self,
        path: &StandardizedPath,
        previous_depth: Option<WatchDepth>,
        desired_depth: Option<WatchDepth>,
        ctx: &mut ModelContext<Self>,
    ) {
        let Some(local_path) = path.to_local_path() else {
            return;
        };
        let Some(watcher) = &self.watcher else {
            return;
        };
        watcher.update(ctx, |watcher, _ctx| {
            if previous_depth.is_some() {
                std::mem::drop(watcher.unregister_path(&local_path));
            }
            let Some(depth) = desired_depth else {
                return;
            };
            use crate::entry::should_ignore_git_path;
            let watch_filter = WatchFilter::with_filter(Arc::new(move |watch_path| {
                !should_ignore_git_path(watch_path)
            }));
            let recursive_mode = match depth {
                WatchDepth::Direct => RecursiveMode::NonRecursive,
                WatchDepth::Recursive => RecursiveMode::Recursive,
            };
            std::mem::drop(watcher.register_path(&local_path, watch_filter, recursive_mode));
        });
    }

    #[cfg(feature = "local_fs")]
    fn acquire_watch_path(
        &mut self,
        path: &StandardizedPath,
        owner: WatchPathOwner,
        depth: WatchDepth,
        ctx: &mut ModelContext<Self>,
    ) -> bool {
        let Some(local_path) = path.to_local_path() else {
            return false;
        };
        if Self::watch_depth_is_unsafe(&local_path, depth) {
            log::warn!("Refusing overly broad {depth:?} watch root {path} owned by {owner:?}");
            return false;
        }

        let owners = self.watch_path_owners.entry(path.clone()).or_default();
        let previous_depth = owners.values().copied().max();
        let requested_depth = owners
            .get(&owner)
            .copied()
            .max(Some(depth))
            .unwrap_or(depth);
        owners.insert(owner, requested_depth);
        let desired_depth = owners.values().copied().max();
        if previous_depth != desired_depth {
            self.set_watch_path_registration(path, previous_depth, desired_depth, ctx);
        }
        true
    }

    #[cfg(feature = "local_fs")]
    fn release_watch_path(
        &mut self,
        path: &StandardizedPath,
        owner: &WatchPathOwner,
        ctx: &mut ModelContext<Self>,
    ) {
        let Some(owners) = self.watch_path_owners.get_mut(path) else {
            return;
        };
        let previous_depth = owners.values().copied().max();
        if owners.remove(owner).is_none() {
            return;
        }
        let desired_depth = owners.values().copied().max();
        if owners.is_empty() {
            self.watch_path_owners.remove(path);
        }
        if previous_depth != desired_depth {
            self.set_watch_path_registration(path, previous_depth, desired_depth, ctx);
        }
    }

    #[cfg(feature = "local_fs")]
    fn acquire_symlink_watch_mount(
        &mut self,
        repo_path: &StandardizedPath,
        lexical_path: &StandardizedPath,
        mount: &SymlinkWatchMount,
        ctx: &mut ModelContext<Self>,
    ) {
        if mount.target_path.starts_with(repo_path) {
            return;
        }
        let owner = WatchPathOwner::SymlinkMount {
            repo_path: repo_path.clone(),
            lexical_path: lexical_path.clone(),
        };
        for (watch_path, depth) in &mount.watch_paths {
            self.acquire_watch_path(watch_path, owner.clone(), *depth, ctx);
        }
    }

    #[cfg(feature = "local_fs")]
    fn release_symlink_watch_mount(
        &mut self,
        repo_path: &StandardizedPath,
        lexical_path: &StandardizedPath,
        mount: &SymlinkWatchMount,
        ctx: &mut ModelContext<Self>,
    ) {
        if mount.target_path.starts_with(repo_path) {
            return;
        }
        let owner = WatchPathOwner::SymlinkMount {
            repo_path: repo_path.clone(),
            lexical_path: lexical_path.clone(),
        };
        for watch_path in mount.watch_paths.keys() {
            self.release_watch_path(watch_path, &owner, ctx);
        }
    }

    #[cfg(feature = "local_fs")]
    fn refresh_symlink_watch_mounts(
        &mut self,
        repo_path: &StandardizedPath,
        ctx: &mut ModelContext<Self>,
    ) {
        let desired = match self.repositories.get(repo_path) {
            Some(IndexedRepoState::Indexed(state)) => Self::symlink_watch_mounts(state),
            Some(IndexedRepoState::Pending | IndexedRepoState::Failed(_)) | None => HashMap::new(),
        };
        let previous = self
            .symlink_watch_mounts
            .remove(repo_path)
            .unwrap_or_default();

        for (lexical_path, previous_mount) in &previous {
            if desired.get(lexical_path) != Some(previous_mount) {
                self.release_symlink_watch_mount(repo_path, lexical_path, previous_mount, ctx);
            }
        }
        for (lexical_path, desired_mount) in &desired {
            if previous.get(lexical_path) != Some(desired_mount) {
                self.acquire_symlink_watch_mount(repo_path, lexical_path, desired_mount, ctx);
            }
        }

        if !desired.is_empty() {
            self.symlink_watch_mounts.insert(repo_path.clone(), desired);
        }
    }

    #[cfg(feature = "local_fs")]
    fn clear_symlink_watch_mounts(
        &mut self,
        repo_path: &StandardizedPath,
        ctx: &mut ModelContext<Self>,
    ) {
        let mounts = self
            .symlink_watch_mounts
            .remove(repo_path)
            .unwrap_or_default();
        for (lexical_path, target) in &mounts {
            self.release_symlink_watch_mount(repo_path, lexical_path, target, ctx);
        }
    }

    /// Adds or updates a repository's file tree state.
    fn add_repository_internal(
        &mut self,
        repo_path: StandardizedPath,
        state: FileTreeState,
        ctx: &mut ModelContext<Self>,
    ) -> Result<(), RepoMetadataError> {
        let local_path = repo_path
            .to_local_path()
            .ok_or_else(|| RepoMetadataError::PathEncodingMismatch(repo_path.clone()))?;

        // Validate the repository path exists
        if !local_path.exists() {
            return Err(RepoMetadataError::RepoNotFound(repo_path.to_string()));
        }

        if !local_path.is_dir() {
            return Err(RepoMetadataError::InvalidPath(
                "Repository path must be a directory".to_string(),
            ));
        }

        #[cfg(feature = "local_fs")]
        self.invalidate_repo_update_pipeline(&repo_path);
        #[cfg(feature = "local_fs")]
        self.acquire_watch_path(
            &repo_path,
            WatchPathOwner::Repository {
                repo_path: repo_path.clone(),
            },
            WatchDepth::Recursive,
            ctx,
        );

        // Insert the repository state into the map
        let repo_path_for_event = repo_path.clone();
        self.repositories
            .insert(repo_path, IndexedRepoState::Indexed(state));
        #[cfg(feature = "local_fs")]
        self.refresh_symlink_watch_mounts(&repo_path_for_event, ctx);

        ctx.emit(RepositoryMetadataEvent::RepositoryUpdated {
            path: repo_path_for_event,
        });

        Ok(())
    }

    /// Removes a repository from tracking.
    pub fn remove_repository(
        &mut self,
        repo_path: &StandardizedPath,
        ctx: &mut ModelContext<Self>,
    ) -> Result<(), RepoMetadataError> {
        if self.repositories.remove(repo_path).is_some() {
            #[cfg(feature = "local_fs")]
            self.invalidate_repo_update_pipeline(repo_path);
            #[cfg(feature = "local_fs")]
            self.clear_symlink_watch_mounts(repo_path, ctx);
            #[cfg(feature = "local_fs")]
            self.release_watch_path(
                repo_path,
                &WatchPathOwner::Repository {
                    repo_path: repo_path.clone(),
                },
                ctx,
            );

            ctx.emit(RepositoryMetadataEvent::RepositoryRemoved {
                path: repo_path.clone(),
            });

            Ok(())
        } else {
            Err(RepoMetadataError::RepoNotFound(repo_path.to_string()))
        }
    }

    pub fn get_repository(&self, repo_path: &StandardizedPath) -> Option<&FileTreeState> {
        match self.repositories.get(repo_path)? {
            IndexedRepoState::Indexed(state) => Some(state),
            IndexedRepoState::Pending => None,
            IndexedRepoState::Failed(_) => None,
        }
    }

    /// Returns the current [`IndexedRepoState`] for the specified repository or `None` if the
    /// repository is not being tracked.
    pub fn repository_state(&self, repo_path: &StandardizedPath) -> Option<&IndexedRepoState> {
        self.repositories.get(repo_path)
    }

    /// Checks if a repository is being tracked and indexed.
    pub fn has_repository(&self, repo_path: &StandardizedPath) -> bool {
        matches!(
            self.repositories.get(repo_path),
            Some(IndexedRepoState::Indexed(_))
        )
    }

    /// Returns whether the given path is tracked as a lazily-loaded standalone path.
    pub fn is_lazy_loaded_path(&self, path: &StandardizedPath) -> bool {
        self.lazy_loaded_paths.contains_key(path)
    }

    /// Lazily indexes a standalone path with only the first level of children.
    /// Registers the path with the file watcher for live updates.
    /// No-ops if the path is already tracked.
    #[cfg(feature = "local_fs")]
    pub fn index_lazy_loaded_path(
        &mut self,
        path: &StandardizedPath,
        ctx: &mut ModelContext<Self>,
    ) -> Result<(), RepoMetadataError> {
        // Already tracked as a lazy-loaded path — increase the refcount and keep the
        // existing watcher/model entry alive.
        if let Some(refcount) = self.lazy_loaded_paths.get_mut(path) {
            *refcount += 1;
            return Ok(());
        }

        // Already tracked as a real repo — don't overwrite it.
        if matches!(
            self.repositories.get(path),
            Some(IndexedRepoState::Indexed(_) | IndexedRepoState::Pending)
        ) {
            return Ok(());
        }

        let local_path = path
            .to_local_path()
            .ok_or_else(|| RepoMetadataError::PathEncodingMismatch(path.clone()))?;
        if !local_path.exists() {
            return Err(RepoMetadataError::RepoNotFound(path.to_string()));
        }
        if !local_path.is_dir() {
            return Err(RepoMetadataError::InvalidPath(
                "Path must be a directory".to_string(),
            ));
        }

        // Build first-level-only tree.
        let mut files = Vec::new();
        let mut file_limit = MAX_FILES_PER_REPO;
        let root_entry = Entry::build_tree(
            &local_path,
            &mut files,
            &mut vec![],
            Some(&mut file_limit),
            1, // max_depth — only first level
            0,
            &IgnoredPathStrategy::Include,
        )
        .map_err(RepoMetadataError::BuildTree)?;

        let state = FileTreeState::new_lazy_loaded(root_entry);
        self.add_repository_internal(path.clone(), state, ctx)?;
        self.lazy_loaded_paths.insert(path.clone(), 1);
        Ok(())
    }

    /// Removes a lazily-loaded standalone path from tracking and unregisters the file watcher.
    #[cfg(feature = "local_fs")]
    pub fn remove_lazy_loaded_path(
        &mut self,
        path: &StandardizedPath,
        ctx: &mut ModelContext<Self>,
    ) {
        let Some(refcount) = self.lazy_loaded_paths.get_mut(path) else {
            return;
        };
        if *refcount > 1 {
            *refcount -= 1;
            return;
        }
        self.lazy_loaded_paths.remove(path);
        // remove_repository unregisters the watcher and emits RepositoryRemoved.
        let _ = self.remove_repository(path, ctx);
    }

    /// Loads a specific directory inside an already-tracked tree.
    /// Emits `FileTreeEntryUpdated` so subscribers can sync.
    #[cfg(feature = "local_fs")]
    pub fn load_directory(
        &mut self,
        repo_root: &StandardizedPath,
        dir_path: &StandardizedPath,
        ctx: &mut ModelContext<Self>,
    ) -> Result<(), RepoMetadataError> {
        let Some(IndexedRepoState::Indexed(state)) = self.repositories.get_mut(repo_root) else {
            return Err(RepoMetadataError::RepoNotFound(repo_root.to_string()));
        };

        let mut gitignores = state.gitignores.clone();
        state
            .entry
            .load_at_path(dir_path, &mut gitignores)
            .map_err(RepoMetadataError::BuildTree)?;
        self.refresh_symlink_watch_mounts(repo_root, ctx);

        ctx.emit(RepositoryMetadataEvent::FileTreeEntryUpdated {
            path: repo_root.clone(),
        });
        Ok(())
    }

    /// Checks whether the parent directory of `path` is loaded in the given entry.
    fn is_parent_loaded_in_entry(entry: &FileTreeEntry, path: &StandardizedPath) -> bool {
        let Some(parent) = path.parent() else {
            return true;
        };
        entry.get(&parent).is_some_and(|state| state.loaded())
    }

    /// Phase 1: Computes file-tree mutations on a background thread.
    ///
    /// Performs all filesystem I/O (`exists()`, `is_dir()`, `build_tree()`,
    /// gitignore checks) and returns a lightweight list of mutations that can
    /// be applied to the tree on the main thread without cloning it.
    async fn compute_file_tree_mutations(
        update: &RepoUpdate,
        gitignores: &[Gitignore],
    ) -> Vec<FileTreeMutation> {
        let mut mutations = Vec::new();

        // Removals for deleted and moved-from paths
        for path_to_remove in update.deleted.iter().chain(update.moved.values()) {
            mutations.push(FileTreeMutation::Remove(path_to_remove.clone()));
        }

        // Additions, moved-to paths and mount projections all converge on one
        // state-based refresh. Event kind is not filesystem truth: an external
        // target deletion still leaves the lexical symlink inode present.
        let paths_to_refresh: HashSet<_> = update
            .added
            .iter()
            .chain(update.moved.keys())
            .chain(&update.refreshed)
            .cloned()
            .collect();
        for path_to_add in paths_to_refresh {
            let path_metadata = match std::fs::symlink_metadata(&path_to_add) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    mutations.push(FileTreeMutation::Remove(path_to_add));
                    continue;
                }
                Err(error) => {
                    log::debug!("Failed to refresh filesystem entry {path_to_add:?}: {error:?}");
                    continue;
                }
            };

            let is_ignored = Self::path_is_ignored(&path_to_add, gitignores);

            let mut files = Vec::new();
            let mut gitignores = gitignores.to_owned();
            let mut file_limit = MAX_FILES_PER_REPO;
            match Entry::build_tree(
                &path_to_add,
                &mut files,
                &mut gitignores,
                Some(&mut file_limit),
                MAX_TREE_DEPTH,
                0,
                &IgnoredPathStrategy::IncludeLazy,
            ) {
                Ok(entry) => mutations.push(FileTreeMutation::AddEntry {
                    path: path_to_add,
                    entry,
                }),
                Err(error) if path_metadata.is_dir() => {
                    log::debug!("Failed to build directory {path_to_add:?}: {error:?}");
                    mutations.push(FileTreeMutation::AddEmptyDirectory {
                        path: path_to_add,
                        is_ignored,
                    });
                }
                Err(error) => {
                    log::debug!("Failed to build filesystem entry {path_to_add:?}: {error:?}");
                }
            }
        }

        mutations
    }

    /// Phase 2: Applies pre-computed mutations to the file tree on the main thread.
    ///
    /// No filesystem I/O — only tree-structure operations. When `lazy_load` is
    /// true, additions are skipped if the parent directory has not been expanded.
    ///
    /// When `emit_updates` is true,
    /// from the mutations that were actually applied (filtering out any skipped
    /// by `lazy_load`), suitable for sending to the remote client. When false,
    /// no update tracking is performed and the function returns `None`.
    pub(crate) fn apply_file_tree_mutations(
        root_entry: &mut FileTreeEntry,
        mutations: Vec<FileTreeMutation>,
        lazy_load: bool,
        emit_updates: bool,
    ) -> Option<RepoMetadataUpdate> {
        let emit = emit_updates;
        let mut remove_entries: Vec<StandardizedPath> = Vec::new();
        let mut update_entries: Vec<FileTreeEntryUpdate> = Vec::new();

        for mutation in mutations {
            match mutation {
                FileTreeMutation::Remove(ref path) => {
                    let Some(std_path) = StandardizedPath::try_from_local(path).ok() else {
                        continue;
                    };
                    root_entry.remove(&std_path);
                    if emit {
                        remove_entries.push(std_path);
                    }
                }
                FileTreeMutation::AddEntry {
                    ref path,
                    ref entry,
                } => {
                    let Some(std_path) = StandardizedPath::try_from_local(path).ok() else {
                        continue;
                    };
                    if lazy_load && !Self::is_parent_loaded_in_entry(root_entry, &std_path) {
                        continue;
                    }
                    let Some(parent) = std_path.parent() else {
                        continue;
                    };
                    Self::ensure_parent_directories_exist(root_entry, &parent);

                    let Some(parent_dir) = root_entry.find_parent_directory(&std_path) else {
                        continue;
                    };

                    match (root_entry.get_mut(&std_path), entry) {
                        (Some(FileTreeEntryState::File(existing)), Entry::File(new_file)) => {
                            existing.extension = new_file.extension.clone();
                            existing.ignored = new_file.ignored;
                        }
                        _ => {
                            root_entry.remove(&std_path);
                            root_entry
                                .insert_entry_at_path(Arc::new(std_path.clone()), entry.clone());
                        }
                    }
                    if let Some(FileTreeEntryState::Directory(directory)) =
                        root_entry.get_mut(&parent_dir)
                    {
                        directory.loaded = true;
                    }
                    if emit {
                        update_entries.push(FileTreeEntryUpdate {
                            parent_path_to_replace: parent,
                            subtree_metadata: flatten_entry_metadata(entry),
                        });
                    }
                }
                FileTreeMutation::AddEmptyDirectory {
                    ref path,
                    is_ignored,
                } => {
                    let Some(std_path) = StandardizedPath::try_from_local(path).ok() else {
                        continue;
                    };
                    if lazy_load && !Self::is_parent_loaded_in_entry(root_entry, &std_path) {
                        continue;
                    }
                    let Some(parent) = std_path.parent() else {
                        continue;
                    };
                    Self::ensure_parent_directories_exist(root_entry, &parent);

                    let Some(parent_dir) = root_entry.find_parent_directory(&std_path) else {
                        continue;
                    };

                    let dir_state = FileTreeEntryState::Directory(FileTreeDirectoryEntryState {
                        path: Arc::new(std_path.clone()),
                        ignored: is_ignored,
                        loaded: false,
                    });
                    root_entry.insert_child_state(&parent_dir, dir_state);
                    if emit {
                        update_entries.push(FileTreeEntryUpdate {
                            parent_path_to_replace: parent.clone(),
                            subtree_metadata: vec![RepoNodeMetadata::Directory(
                                DirectoryNodeMetadata {
                                    path: std_path,
                                    ignored: is_ignored,
                                    loaded: false,
                                },
                            )],
                        });
                    }
                }
            }
        }

        if !emit {
            return None;
        }

        Some(RepoMetadataUpdate {
            repo_path: root_entry.root_directory().as_ref().clone(),
            remove_entries,
            update_entries,
        })
    }

    /// Delegates to [`FileTreeEntry::ensure_parent_directories_exist`].
    fn ensure_parent_directories_exist(
        root_entry: &mut FileTreeEntry,
        target_parent: &StandardizedPath,
    ) {
        root_entry.ensure_parent_directories_exist(target_parent);
    }

    /// Checks if a path matches any of the gitignore patterns
    fn path_is_ignored(path: &Path, gitignores: &[Gitignore]) -> bool {
        // Check if any component of the path is .git
        if path
            .components()
            .any(|component| component.as_os_str() == ".git")
        {
            return true;
        }

        // Check if path matches any gitignore patterns
        let is_dir = path.is_dir();
        matches_gitignores(path, is_dir, gitignores, true)
    }

    /// Indexes a repository from the given repository handle.
    pub fn index_directory(
        &mut self,
        repository: ModelHandle<Repository>,
        ctx: &mut ModelContext<'_, Self>,
    ) -> Result<(), RepoMetadataError> {
        let std_path = repository.as_ref(ctx).root_dir().clone();
        let local_path = std_path
            .to_local_path()
            .ok_or_else(|| RepoMetadataError::PathEncodingMismatch(std_path.clone()))?;

        // Validate the repository path exists and is a directory
        if !local_path.exists() {
            return Err(RepoMetadataError::RepoNotFound(std_path.to_string()));
        }

        if !local_path.is_dir() {
            return Err(RepoMetadataError::InvalidPath(
                "Repository path must be a directory".to_string(),
            ));
        }

        let repo_path_str = std_path.to_string();

        // Check if the repository is already indexed or currently being indexed.
        // Allow re-indexing if the existing entry was a lazily-loaded path placeholder.
        match self.repositories.get(&std_path) {
            Some(IndexedRepoState::Indexed(_))
                if !self.lazy_loaded_paths.contains_key(&std_path) =>
            {
                log::debug!("Repository already indexed: {std_path}");
                return Ok(());
            }
            Some(IndexedRepoState::Indexed(_)) => {
                // Was a lazy-loaded path – allow upgrading to a real repo.
                log::info!("Upgrading lazy-loaded path to git repo: {repo_path_str}");
                self.lazy_loaded_paths.remove(&std_path);
            }
            Some(IndexedRepoState::Pending) => {
                log::debug!("Repository already being indexed: {repo_path_str}");
                return Ok(());
            }
            Some(IndexedRepoState::Failed(error)) => {
                log::debug!(
                    "Repository indexing previously failed: {repo_path_str}, error: {error}"
                );
                log::info!("Retrying indexing for previously failed repository: {repo_path_str}");
                // Continue to retry indexing
            }
            None => {
                // Repository is not indexed and not pending, proceed with indexing
            }
        }

        // Collect gitignore files from the repository
        let gitignores = gitignores_for_directory(&local_path);

        // Mark the repository as pending to prevent duplicate work
        self.repositories
            .insert(std_path.clone(), IndexedRepoState::Pending);

        // Use the provided repository handle instead of creating a new one
        let repository_handle = repository;

        // Build the complete file tree for the repository asynchronously
        let repo_path_for_build = local_path;
        let gitignores_for_build = gitignores.clone();
        let repo_path_str_for_log = std_path.to_string();
        let std_path_for_completion = std_path;
        let repository_handle_for_completion = repository_handle.clone();

        ctx.spawn(
            async move {
                let mut files: Vec<crate::entry::FileMetadata> = Vec::new();
                let mut gitignores_for_build = gitignores_for_build;

                let mut file_limit = MAX_FILES_PER_REPO;

                let build_result = Entry::build_tree(
                    &repo_path_for_build,
                    &mut files,
                    &mut gitignores_for_build,
                    Some(&mut file_limit),
                    MAX_TREE_DEPTH,        // max_depth
                    0,                 // current_depth
                    &IgnoredPathStrategy::IncludeLazy,
                );
                (
                    build_result,
                    files,
                    gitignores_for_build,
                    repo_path_str_for_log,
                    std_path_for_completion,
                    repository_handle_for_completion,
                )
            },
            move |model: &mut CurrentAppRepoMetadataModel,
                  (
                      build_result,
                      files,
                      gitignores_for_build,
                      repo_path_str,
                      std_repo_path,
                      repository_handle,
                  ): (Result<Entry, _>, Vec<crate::entry::FileMetadata>, _, String, StandardizedPath, ModelHandle<Repository>),
                  ctx| {
                match build_result {
                    Ok(root_entry) => {
                        let state =
                            FileTreeState::new(root_entry, gitignores_for_build, Some(repository_handle));

                        if let Err(e) =
                            model.add_repository_internal(std_repo_path.clone(), state, ctx)
                        {
                            log::warn!("Failed to add repository {repo_path_str}: {e:?}");
                            // On failure, mark the repository as failed
                            model
                                .repositories
                                .insert(std_repo_path, IndexedRepoState::Failed(e));
                        } else {
                            log::info!(
                                "Successfully indexed repository: {} with {} files",
                                repo_path_str,
                                files.len()
                            );
                        }
                    }
                    Err(e) => {
                        safe_warn!(
                            safe: ("Failed to build file tree for repository: {e:?}"),
                            full: ("Failed to build file tree for repository {repo_path_str}: {e:?}")
                        );
                        ctx.emit(RepositoryMetadataEvent::UpdatingRepositoryFailed { path: std_repo_path.clone() });
                        model.repositories.insert(
                            std_repo_path,
                            IndexedRepoState::Failed(RepoMetadataError::BuildTree(e)),
                        );
                    }
                }
            },
        );

        Ok(())
    }

    /// Returns repository contents (files and optionally directories) in a given repository.
    pub fn get_repo_contents(
        &self,
        repo_path: &StandardizedPath,
        args: GetContentsArgs,
    ) -> Option<Vec<RepoContent<'_>>> {
        let state = match self.repositories.get(repo_path)? {
            IndexedRepoState::Indexed(state) => state,
            IndexedRepoState::Pending => return None,
            IndexedRepoState::Failed(_) => return None,
        };
        let mut contents = Vec::new();
        collect_contents_recursive(
            &state.entry,
            state.entry.root_directory(),
            &mut contents,
            &args,
        );
        Some(contents)
    }
}

impl warpui::Entity for CurrentAppRepoMetadataModel {
    type Event = RepositoryMetadataEvent;
}

/// Helper function to recursively collect contents (files and optionally directories) from an Entry tree.
pub(crate) fn collect_contents_recursive<'a>(
    entry: &'a FileTreeEntry,
    current_path: &'a StandardizedPath,
    contents: &mut Vec<RepoContent<'a>>,
    args: &GetContentsArgs,
) {
    if !args.include_ignored && entry.ignored(current_path) {
        return;
    }

    match entry.get(current_path) {
        Some(FileTreeEntryState::File(metadata)) => {
            let content = RepoContent::File(metadata);
            if args.filter.as_ref().is_none_or(|f| f(&content)) {
                contents.push(content);
            }
        }
        Some(FileTreeEntryState::Directory(dir)) => {
            if args.include_folders {
                let content = RepoContent::Directory(dir);
                if args.filter.as_ref().is_none_or(|f| f(&content)) {
                    contents.push(content);
                }
            }

            for child in entry.child_paths(current_path) {
                collect_contents_recursive(entry, child, contents, args);
            }
        }
        Some(FileTreeEntryState::Symlink(symlink)) => {
            let content = RepoContent::Symlink(symlink);
            if args.filter.as_ref().is_none_or(|f| f(&content)) {
                contents.push(content);
            }

            if symlink.target_kind == crate::SymlinkTargetKind::Directory {
                for child in entry.child_paths(current_path) {
                    collect_contents_recursive(entry, child, contents, args);
                }
            }
        }
        None => {}
    }
}

// Test helpers
#[cfg(any(test, feature = "test-util"))]
impl CurrentAppRepoMetadataModel {
    /// Insert a repository state directly for testing purposes.
    pub fn insert_test_state(&mut self, repo_path: StandardizedPath, state: FileTreeState) {
        self.repositories
            .insert(repo_path, IndexedRepoState::Indexed(state));
    }
}

#[cfg(test)]
#[path = "current_app_model_tests.rs"]
mod tests;

#[cfg(all(test, feature = "local_fs"))]
mod is_unsafe_watch_root_tests {
    use super::is_unsafe_watch_root;
    use std::path::Path;

    #[test]
    fn rejects_home_and_its_ancestors() {
        let Some(home) = dirs::home_dir() else {
            // No $HOME (sandboxed CI etc.) — guard is a no-op there by design.
            return;
        };

        assert!(
            is_unsafe_watch_root(&home),
            "home directory itself ({}) must be rejected",
            home.display()
        );

        assert!(
            is_unsafe_watch_root(Path::new("/")),
            "filesystem root must be rejected",
        );

        if let Some(parent) = home.parent() {
            assert!(
                is_unsafe_watch_root(parent),
                "home's parent ({}) must be rejected",
                parent.display(),
            );
        }
    }

    #[test]
    fn allows_directories_inside_home() {
        let Some(home) = dirs::home_dir() else {
            return;
        };

        let repo_inside_home = home.join("__ashide_test_repo_path__");
        assert!(
            !is_unsafe_watch_root(&repo_inside_home),
            "{} (a directory inside home) must NOT be rejected",
            repo_inside_home.display(),
        );
    }

    #[test]
    fn allows_unrelated_paths() {
        let Some(home) = dirs::home_dir() else {
            return;
        };

        let tmp_path = std::env::temp_dir().join("__ashide_test_unsafe_watch_root__");
        // Skip the case where tmp_path happens to be an ancestor of home
        // (vanishingly unlikely, but keeps the assertion meaningful).
        if !home.starts_with(&tmp_path) {
            assert!(
                !is_unsafe_watch_root(&tmp_path),
                "{} (unrelated tmp path) must NOT be rejected",
                tmp_path.display(),
            );
        }
    }
}
