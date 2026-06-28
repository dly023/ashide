//! Unified repository metadata model.
//!
//! [`RepoMetadataModel`] is the singleton entry point for all repository metadata
//! queries. It holds handles to current-app native and
//! remote repository metadata backends and dispatches operations based on
//! [`RepositoryIdentifier`].

#[cfg(feature = "local_fs")]
use std::path::Path;

use warp_core::HostId;
use warp_util::standardized_path::StandardizedPath;
use warpui::{AppContext, ModelContext, ModelHandle, SingletonEntity};

use crate::current_app_model::{
    CurrentAppRepoMetadataModel, GetContentsArgs, IndexedRepoState, RepoContent,
    RepositoryMetadataEvent,
};
use crate::file_tree_store::FileTreeState;
use crate::file_tree_update::RepoMetadataUpdate;
use crate::remote_model::{RemoteRepoMetadataModel, RemoteRepositoryMetadataEvent};
use crate::repository_identifier::{RemoteRepositoryIdentifier, RepositoryIdentifier};
use crate::RepoMetadataError;

/// Unified events emitted by the [`RepoMetadataModel`] wrapper.
///
/// These are mapped from the sub-model events into a common enum keyed by
/// [`RepositoryIdentifier`].
#[derive(Debug)]
pub enum RepoMetadataEvent {
    /// A repository was added or updated.
    RepositoryUpdated { id: RepositoryIdentifier },
    /// A repository was removed.
    RepositoryRemoved { id: RepositoryIdentifier },
    /// File trees for repositories were updated.
    FileTreeUpdated { ids: Vec<RepositoryIdentifier> },
    /// A file tree entry was updated.
    FileTreeEntryUpdated { id: RepositoryIdentifier },
    /// Updating a repository failed.
    UpdatingRepositoryFailed { id: RepositoryIdentifier },
    /// An incremental file tree update is ready to be sent to the remote
    /// client. Only emitted when the current-app native backend has
    /// `emit_incremental_updates` enabled.
    IncrementalUpdateReady { update: RepoMetadataUpdate },
}

/// Singleton wrapper that provides a unified API over current-app and remote
/// repository metadata models.
///
/// All consumers should interact with this type rather than accessing the
/// sub-models directly. The wrapper does **not** expose backend accessors —
/// encapsulation keeps consumers decoupled from the current-app/remote split.
pub struct RepoMetadataModel {
    current_app: ModelHandle<CurrentAppRepoMetadataModel>,
    remote: ModelHandle<RemoteRepoMetadataModel>,
}

impl RepoMetadataModel {
    /// Creates a new `RepoMetadataModel`, instantiating both sub-models and
    /// subscribing to their events for forwarding.
    pub fn new(ctx: &mut ModelContext<Self>) -> Self {
        let current_app = ctx.add_model(CurrentAppRepoMetadataModel::new);
        let remote = ctx.add_model(RemoteRepoMetadataModel::new);

        ctx.subscribe_to_model(&current_app, Self::forward_current_app_event);
        ctx.subscribe_to_model(&remote, Self::forward_remote_event);

        Self {
            current_app,
            remote,
        }
    }

    /// Creates a new `RepoMetadataModel` with incremental update emission
    /// enabled on the local sub-model. Used by the remote server.
    pub fn new_with_incremental_updates(ctx: &mut ModelContext<Self>) -> Self {
        let current_app = ctx.add_model(|ctx| {
            let mut model = CurrentAppRepoMetadataModel::new(ctx);
            model.set_emit_incremental_updates(true);
            model
        });
        let remote = ctx.add_model(RemoteRepoMetadataModel::new);

        ctx.subscribe_to_model(&current_app, Self::forward_current_app_event);
        ctx.subscribe_to_model(&remote, Self::forward_remote_event);

        Self {
            current_app,
            remote,
        }
    }

    // ── Event forwarding ─────────────────────────────────────────────

    fn forward_current_app_event(
        &mut self,
        event: &RepositoryMetadataEvent,
        ctx: &mut ModelContext<Self>,
    ) {
        let unified = match event {
            RepositoryMetadataEvent::RepositoryUpdated { path } => {
                RepoMetadataEvent::RepositoryUpdated {
                    id: RepositoryIdentifier::local(path.clone()),
                }
            }
            RepositoryMetadataEvent::RepositoryRemoved { path } => {
                RepoMetadataEvent::RepositoryRemoved {
                    id: RepositoryIdentifier::local(path.clone()),
                }
            }
            RepositoryMetadataEvent::FileTreeUpdated { paths } => {
                RepoMetadataEvent::FileTreeUpdated {
                    ids: paths
                        .iter()
                        .map(|p| RepositoryIdentifier::local(p.clone()))
                        .collect(),
                }
            }
            RepositoryMetadataEvent::FileTreeEntryUpdated { path } => {
                RepoMetadataEvent::FileTreeEntryUpdated {
                    id: RepositoryIdentifier::local(path.clone()),
                }
            }
            RepositoryMetadataEvent::UpdatingRepositoryFailed { path } => {
                RepoMetadataEvent::UpdatingRepositoryFailed {
                    id: RepositoryIdentifier::local(path.clone()),
                }
            }
            RepositoryMetadataEvent::IncrementalUpdateReady { update } => {
                RepoMetadataEvent::IncrementalUpdateReady {
                    update: update.clone(),
                }
            }
        };
        ctx.emit(unified);
    }

    fn forward_remote_event(
        &mut self,
        event: &RemoteRepositoryMetadataEvent,
        ctx: &mut ModelContext<Self>,
    ) {
        let unified = match event {
            RemoteRepositoryMetadataEvent::RepositoryUpdated { id } => {
                RepoMetadataEvent::RepositoryUpdated {
                    id: RepositoryIdentifier::Remote(id.clone()),
                }
            }
            RemoteRepositoryMetadataEvent::RepositoryRemoved { id } => {
                RepoMetadataEvent::RepositoryRemoved {
                    id: RepositoryIdentifier::Remote(id.clone()),
                }
            }
            RemoteRepositoryMetadataEvent::FileTreeUpdated { ids } => {
                RepoMetadataEvent::FileTreeUpdated {
                    ids: ids
                        .iter()
                        .cloned()
                        .map(RepositoryIdentifier::Remote)
                        .collect(),
                }
            }
            RemoteRepositoryMetadataEvent::FileTreeEntryUpdated { id } => {
                RepoMetadataEvent::FileTreeEntryUpdated {
                    id: RepositoryIdentifier::Remote(id.clone()),
                }
            }
        };
        ctx.emit(unified);
    }

    // ── Unified query API ────────────────────────────────────────────

    /// Returns the [`FileTreeState`] for a repository identified by `id`.
    pub fn get_repository<'a>(
        &self,
        id: &RepositoryIdentifier,
        ctx: &'a AppContext,
    ) -> Option<&'a FileTreeState> {
        match id {
            RepositoryIdentifier::Local(path) => self.current_app.as_ref(ctx).get_repository(path),
            RepositoryIdentifier::Remote(remote_id) => {
                self.remote.as_ref(ctx).get_repository(remote_id)
            }
        }
    }

    /// Returns whether the given repository is indexed.
    pub fn has_repository(&self, id: &RepositoryIdentifier, ctx: &AppContext) -> bool {
        match id {
            RepositoryIdentifier::Local(path) => self.current_app.as_ref(ctx).has_repository(path),
            RepositoryIdentifier::Remote(remote_id) => {
                self.remote.as_ref(ctx).has_repository(remote_id)
            }
        }
    }

    /// Returns the current [`IndexedRepoState`] for a repository.
    pub fn repository_state<'a>(
        &self,
        id: &RepositoryIdentifier,
        ctx: &'a AppContext,
    ) -> Option<&'a IndexedRepoState> {
        match id {
            RepositoryIdentifier::Local(path) => {
                self.current_app.as_ref(ctx).repository_state(path)
            }
            RepositoryIdentifier::Remote(remote_id) => {
                self.remote.as_ref(ctx).repository_state(remote_id)
            }
        }
    }

    /// Returns repository contents for the specified repository.
    pub fn get_repo_contents<'a>(
        &self,
        id: &RepositoryIdentifier,
        args: GetContentsArgs,
        ctx: &'a AppContext,
    ) -> Option<Vec<RepoContent<'a>>> {
        match id {
            RepositoryIdentifier::Local(path) => {
                self.current_app.as_ref(ctx).get_repo_contents(path, args)
            }
            RepositoryIdentifier::Remote(remote_id) => {
                self.remote.as_ref(ctx).get_repo_contents(remote_id, args)
            }
        }
    }

    /// Finds the repository root that contains the given local path.
    #[cfg(feature = "local_fs")]
    pub fn find_repository_for_path(
        &self,
        path: &Path,
        ctx: &AppContext,
    ) -> Option<StandardizedPath> {
        self.current_app.as_ref(ctx).find_repository_for_path(path)
    }

    // ── Current-app native operations ────────────────────────────────────
    // These delegate to the current-app native backend. Remote equivalents will be
    // added once the remote client ↔ server sync layer is in place.

    /// Indexes a current-app repository from the given repository handle.
    #[cfg(feature = "local_fs")]
    pub fn index_directory(
        &self,
        repository: ModelHandle<crate::repository::Repository>,
        ctx: &mut ModelContext<Self>,
    ) -> Result<(), RepoMetadataError> {
        self.current_app
            .update(ctx, |local, ctx| local.index_directory(repository, ctx))
    }

    /// Lazily indexes a current-app standalone path with only the first level of children.
    #[cfg(feature = "local_fs")]
    pub fn index_lazy_loaded_path(
        &self,
        path: &StandardizedPath,
        ctx: &mut ModelContext<Self>,
    ) -> Result<(), RepoMetadataError> {
        let path = path.clone();
        self.current_app
            .update(ctx, |local, ctx| local.index_lazy_loaded_path(&path, ctx))
    }

    /// Loads a specific directory inside an already-tracked current-app tree.
    #[cfg(feature = "local_fs")]
    pub fn load_directory(
        &self,
        repo_root: &StandardizedPath,
        dir_path: &StandardizedPath,
        ctx: &mut ModelContext<Self>,
    ) -> Result<(), RepoMetadataError> {
        let repo_root = repo_root.clone();
        let dir_path = dir_path.clone();
        self.current_app.update(ctx, |local, ctx| {
            local.load_directory(&repo_root, &dir_path, ctx)
        })
    }

    /// Removes a lazily-loaded current-app standalone path from tracking.
    #[cfg(feature = "local_fs")]
    pub fn remove_lazy_loaded_path(&self, path: &StandardizedPath, ctx: &mut ModelContext<Self>) {
        let path = path.clone();
        self.current_app
            .update(ctx, |local, ctx| local.remove_lazy_loaded_path(&path, ctx));
    }

    // ── Remote-specific operations ─────────────────────────────────
    // These delegate to the remote sub-model and are called by the
    // environment runtime event subscription in the app layer.

    /// Inserts or replaces a remote repository from a snapshot push event.
    pub fn insert_remote_snapshot(
        &self,
        host_id: HostId,
        update: &RepoMetadataUpdate,
        ctx: &mut ModelContext<Self>,
    ) {
        self.remote.update(ctx, |remote, ctx| {
            remote.insert_from_snapshot(host_id, update, ctx);
        });
    }

    /// Applies an incremental remote repo metadata update.
    pub fn apply_remote_incremental_update(
        &self,
        host_id: &HostId,
        update: &RepoMetadataUpdate,
        ctx: &mut ModelContext<Self>,
    ) {
        let host_id = host_id.clone();
        self.remote.update(ctx, |remote, ctx| {
            remote.apply_incremental_update(&host_id, update, ctx);
        });
    }

    /// Removes all remote repositories for the given host (e.g. on disconnect).
    pub fn remove_remote_repositories_for_host(
        &self,
        host_id: &HostId,
        ctx: &mut ModelContext<Self>,
    ) {
        let host_id = host_id.clone();
        self.remote.update(ctx, |remote, ctx| {
            remote.remove_repositories_for_host(&host_id, ctx);
        });
    }

    /// Removes a repository (local or remote) from tracking.
    pub fn remove_repository(
        &self,
        id: &RepositoryIdentifier,
        ctx: &mut ModelContext<Self>,
    ) -> Result<(), RepoMetadataError> {
        match id {
            RepositoryIdentifier::Local(path) => {
                let path = path.clone();
                self.current_app
                    .update(ctx, |local, ctx| local.remove_repository(&path, ctx))
            }
            RepositoryIdentifier::Remote(remote_id) => {
                let remote_id = remote_id.clone();
                self.remote
                    .update(ctx, |remote, ctx| remote.remove_repository(&remote_id, ctx));
                Ok(())
            }
        }
    }

    /// Returns all tracked remote repository identifiers.
    pub fn remote_repository_ids<'a>(
        &self,
        ctx: &'a AppContext,
    ) -> impl Iterator<Item = &'a RemoteRepositoryIdentifier> {
        self.remote.as_ref(ctx).remote_repository_ids()
    }

    /// Returns whether the given local path is tracked as a lazily-loaded standalone path.
    pub fn is_lazy_loaded_path(&self, path: &StandardizedPath, ctx: &AppContext) -> bool {
        self.current_app.as_ref(ctx).is_lazy_loaded_path(path)
    }
}

impl warpui::Entity for RepoMetadataModel {
    type Event = RepoMetadataEvent;
}

impl SingletonEntity for RepoMetadataModel {}

#[cfg(any(test, feature = "test-util"))]
impl RepoMetadataModel {
    /// Inserts repository state directly into the local sub-model for testing.
    pub fn insert_test_state(
        &self,
        repo_path: StandardizedPath,
        state: FileTreeState,
        ctx: &mut ModelContext<Self>,
    ) {
        self.current_app.update(ctx, |local, _ctx| {
            local.insert_test_state(repo_path, state);
        });
    }
}
