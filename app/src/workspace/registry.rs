use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::FairMutex;
use warpui::{
    AppContext, Entity, EntityId, ModelHandle, SingletonEntity, WeakViewHandle, WindowId,
};

use crate::terminal::{model::terminal_model::TerminalModel, TerminalManager};

use super::{PaneViewLocator, Workspace};

/// A registry that tracks all workspace views by their window ID.
///
/// This provides O(1) lookup of workspaces instead of the O(n) linear scan
/// that `views_of_type::<Workspace>` performs.
pub struct WorkspaceRegistry {
    workspaces: HashMap<WindowId, WeakViewHandle<Workspace>>,
    retiring_session_owners: HashMap<String, Vec<RetiringWorkspaceSessionOwner>>,
}

struct RetiringWorkspaceSessionOwner {
    window_id: WindowId,
    terminal_view_id: EntityId,
    terminal_model: Arc<FairMutex<TerminalModel>>,
    terminal_manager: ModelHandle<Box<dyn TerminalManager>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct WorkspaceSessionOwner {
    pub(crate) window_id: WindowId,
    pub(crate) locator: PaneViewLocator,
}

impl Default for WorkspaceRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl WorkspaceRegistry {
    pub fn new() -> Self {
        Self {
            workspaces: HashMap::new(),
            retiring_session_owners: HashMap::new(),
        }
    }

    /// Registers a workspace for the given window.
    pub fn register(&mut self, window_id: WindowId, workspace: WeakViewHandle<Workspace>) {
        self.workspaces.insert(window_id, workspace);
    }

    /// Unregisters the workspace for the given window.
    pub fn unregister(&mut self, window_id: WindowId) {
        self.workspaces.remove(&window_id);
    }

    /// Retains process-level ownership after a pane/window has disappeared.
    ///
    /// Closing a terminal is asynchronous: `shutdown_pty` requests teardown,
    /// but the shell/agent process may remain alive until the terminal event
    /// loop observes and handles exit. Releasing durable ownership at UI detach
    /// time would allow another window to start the same provider session while
    /// the old process can still write to it.
    pub(crate) fn begin_retiring_session_owner(
        &mut self,
        durable_identity_key: String,
        window_id: WindowId,
        terminal_view_id: EntityId,
        terminal_model: Arc<FairMutex<TerminalModel>>,
        terminal_manager: ModelHandle<Box<dyn TerminalManager>>,
    ) {
        let owners = self
            .retiring_session_owners
            .entry(durable_identity_key.clone())
            .or_default();
        if owners
            .iter()
            .any(|owner| owner.terminal_view_id == terminal_view_id)
        {
            return;
        }
        if !owners.is_empty() {
            log::error!(
                "durable workspace session {durable_identity_key} entered retiring state with multiple terminal owners"
            );
        }
        owners.push(RetiringWorkspaceSessionOwner {
            window_id,
            terminal_view_id,
            terminal_model,
            terminal_manager,
        });
    }

    /// Returns an undo-retained owner to live ownership.
    ///
    /// Both the durable identity and the terminal entity must match. Removing
    /// by identity alone could release a different process after a duplicate
    /// owner invariant violation; removing by entity alone could release an
    /// unrelated provider session whose entity ID was observed under stale
    /// state.
    pub(crate) fn cancel_retiring_session_owner(
        &mut self,
        durable_identity_key: &str,
        terminal_view_id: EntityId,
    ) {
        let Some(owners) = self.retiring_session_owners.get_mut(durable_identity_key) else {
            return;
        };
        owners.retain(|owner| owner.terminal_view_id != terminal_view_id);
        if owners.is_empty() {
            self.retiring_session_owners.remove(durable_identity_key);
        }
    }

    /// Permanently discards every undo-retained terminal from a closed window.
    /// The leases remain registered until each model reports process exit.
    pub(crate) fn shutdown_retiring_session_owners_for_window(
        &self,
        window_id: WindowId,
        ctx: &mut AppContext,
    ) {
        let terminal_managers = self
            .retiring_session_owners
            .values()
            .flatten()
            .filter(|owner| owner.window_id == window_id)
            .map(|owner| owner.terminal_manager.clone())
            .collect::<Vec<_>>();
        for terminal_manager in terminal_managers {
            terminal_manager.update(ctx, |terminal_manager, ctx| {
                terminal_manager.shutdown_pty(ctx);
            });
        }
    }

    /// Returns whether process-level ownership is still retiring.
    ///
    /// `try_lock` is intentionally fail-closed. The UI thread must never block
    /// on `TerminalModel`; if the event loop currently owns the lock, the old
    /// process is conservatively still treated as an owner.
    pub(crate) fn is_session_owner_retiring(&mut self, durable_identity_key: &str) -> bool {
        let Some(owners) = self.retiring_session_owners.get_mut(durable_identity_key) else {
            return false;
        };

        owners.retain(|owner| {
            owner
                .terminal_model
                .try_lock()
                .is_none_or(|model| !model.has_exited())
        });
        let is_retiring = !owners.is_empty();
        if !is_retiring {
            self.retiring_session_owners.remove(durable_identity_key);
        }
        is_retiring
    }

    /// Returns the workspace for the given window, if it is still alive.
    pub fn get(
        &self,
        window_id: WindowId,
        app: &AppContext,
    ) -> Option<warpui::ViewHandle<Workspace>> {
        self.workspaces.get(&window_id)?.upgrade(app)
    }

    /// Returns all registered workspaces that are still alive.
    /// The returned vector contains tuples of (WindowId, ViewHandle<Workspace>).
    pub fn all_workspaces(
        &self,
        app: &AppContext,
    ) -> Vec<(WindowId, warpui::ViewHandle<Workspace>)> {
        self.workspaces
            .iter()
            .filter_map(|(window_id, weak_handle)| {
                weak_handle.upgrade(app).map(|handle| (*window_id, handle))
            })
            .collect()
    }

    /// Finds the app-wide live or materializing owner for a durable session identity,
    /// excluding the Workspace that is currently handling the activation. Ownership is
    /// derived from pane state instead of copied into another mutable registry.
    pub(crate) fn other_workspace_session_owner(
        &self,
        current_window_id: WindowId,
        durable_identity_key: &str,
        app: &AppContext,
    ) -> Option<WorkspaceSessionOwner> {
        let mut owners = self
            .all_workspaces(app)
            .into_iter()
            .filter(|(window_id, _)| *window_id != current_window_id)
            .filter_map(|(window_id, workspace)| {
                workspace
                    .try_as_ref(app)?
                    .live_or_pending_workspace_session_locator(durable_identity_key, app)
                    .map(|locator| WorkspaceSessionOwner { window_id, locator })
            })
            .collect::<Vec<_>>();
        owners.sort_by_key(|owner| owner.window_id);
        if owners.len() > 1 {
            log::error!(
                "durable workspace session {durable_identity_key} has multiple app-wide owners: {owners:?}"
            );
        }
        owners.into_iter().next()
    }
}

impl Entity for WorkspaceRegistry {
    type Event = ();
}

impl SingletonEntity for WorkspaceRegistry {}
