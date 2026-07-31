use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::FairMutex;
use warpui::{
    AppContext, Entity, EntityId, ModelHandle, SingletonEntity, WeakViewHandle, WindowId,
};

use crate::session_management::SessionNavigationData;
use crate::terminal::{
    model::terminal_model::{ExitReason, TerminalModel},
    CLIAgent,
    TerminalManager,
};
use crate::workspace::environment_backend::EnvironmentSessionRefreshIntent;
use crate::workspace::environment_table::IndexedCliAgentSessionScanToken;

use super::{PaneViewLocator, Workspace};

/// A registry that tracks all workspace views by their window ID.
///
/// This provides O(1) lookup of workspaces instead of the O(n) linear scan
/// that `views_of_type::<Workspace>` performs.
pub struct WorkspaceRegistry {
    workspaces: HashMap<WindowId, WeakViewHandle<Workspace>>,
    /// Canonical committed Session Navigator documents, partitioned by window.
    /// Search only consumes this projection; it must never rediscover membership
    /// by walking Workspace views on each keystroke.
    session_search_documents: HashMap<WindowId, Vec<SessionNavigationData>>,
    session_search_generation: u64,
    retiring_session_owners: HashMap<String, Vec<RetiringWorkspaceSessionOwner>>,
    /// Process-scoped local provider-store scan transactions. The native source
    /// is shared by every local Workspace, but each recipient still commits the
    /// result through its own EnvironmentTable token.
    local_cli_agent_session_scans: HashMap<LocalCliAgentSessionScanKey, LocalCliAgentSessionScan>,
}

/// Notification emitted after the canonical per-window Session Navigator
/// projection changes. Consumers may rebuild derived, non-persistent indexes,
/// but must not rediscover Workspace membership themselves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspaceRegistryEvent {
    SessionSearchProjectionChanged { generation: u64 },
}

/// A source-read key, not a Session Navigator identity. Scope, enabled
/// providers, and observed-provider semantics must all agree before two local
/// Workspaces may share one filesystem scan.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct LocalCliAgentSessionScanKey {
    enabled_agents: Vec<String>,
    observed_agents: Vec<String>,
    scope_paths: Vec<PathBuf>,
}

impl LocalCliAgentSessionScanKey {
    pub(crate) fn new(
        enabled_agents: &[CLIAgent],
        observed_agents: &HashSet<CLIAgent>,
        scope_paths: &[PathBuf],
    ) -> Self {
        let mut enabled_agents = enabled_agents
            .iter()
            .map(|agent| agent.to_serialized_name().to_owned())
            .collect::<Vec<_>>();
        enabled_agents.sort_unstable();
        enabled_agents.dedup();

        let mut observed_agents = observed_agents
            .iter()
            .map(|agent| agent.to_serialized_name().to_owned())
            .collect::<Vec<_>>();
        observed_agents.sort_unstable();
        observed_agents.dedup();

        let mut scope_paths = scope_paths.to_vec();
        scope_paths.sort_unstable();
        scope_paths.dedup();

        Self {
            enabled_agents,
            observed_agents,
            scope_paths,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct LocalCliAgentSessionScanParticipant {
    pub(crate) authority: String,
    pub(crate) scan_token: IndexedCliAgentSessionScanToken,
    pub(crate) refresh_generation: Option<u64>,
}

#[derive(Clone, Debug)]
struct LocalCliAgentSessionScan {
    generation: u64,
    participants: HashMap<WindowId, LocalCliAgentSessionScanParticipant>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LocalCliAgentSessionScanRequest {
    Started { generation: u64 },
    Joined,
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
            session_search_documents: HashMap::new(),
            session_search_generation: 0,
            retiring_session_owners: HashMap::new(),
            local_cli_agent_session_scans: HashMap::new(),
        }
    }

    /// Registers one local Workspace as a recipient of a process-scoped source
    /// scan. An explicit refresh replaces the worker generation while retaining
    /// passive followers, so no EnvironmentTable token is stranded when a user
    /// refresh overtakes startup reconciliation.
    pub(crate) fn request_local_cli_agent_session_scan(
        &mut self,
        key: LocalCliAgentSessionScanKey,
        window_id: WindowId,
        participant: LocalCliAgentSessionScanParticipant,
        intent: EnvironmentSessionRefreshIntent,
    ) -> LocalCliAgentSessionScanRequest {
        let Some(scan) = self.local_cli_agent_session_scans.get_mut(&key) else {
            self.local_cli_agent_session_scans.insert(
                key,
                LocalCliAgentSessionScan {
                    generation: 1,
                    participants: HashMap::from([(window_id, participant)]),
                },
            );
            return LocalCliAgentSessionScanRequest::Started { generation: 1 };
        };

        scan.participants.insert(window_id, participant);
        if matches!(
            intent,
            EnvironmentSessionRefreshIntent::UserInitiated { .. }
        ) {
            scan.generation = scan
                .generation
                .checked_add(1)
                .expect("local CLI-agent session scan generation exhausted");
            LocalCliAgentSessionScanRequest::Started {
                generation: scan.generation,
            }
        } else {
            LocalCliAgentSessionScanRequest::Joined
        }
    }

    /// Returns every still-live Workspace transaction that belongs to the
    /// completing generation. A stale worker cannot clear or deliver a newer
    /// explicit refresh generation.
    pub(crate) fn complete_local_cli_agent_session_scan(
        &mut self,
        key: &LocalCliAgentSessionScanKey,
        generation: u64,
    ) -> Vec<(WindowId, LocalCliAgentSessionScanParticipant)> {
        let Some(scan) = self.local_cli_agent_session_scans.get(key) else {
            return Vec::new();
        };
        if scan.generation != generation {
            return Vec::new();
        }
        self.local_cli_agent_session_scans
            .remove(key)
            .expect("current local CLI-agent scan must remain present")
            .participants
            .into_iter()
            .collect()
    }

    /// Registers a workspace for the given window.
    pub fn register(&mut self, window_id: WindowId, workspace: WeakViewHandle<Workspace>) {
        self.workspaces.insert(window_id, workspace);
    }

    /// Unregisters the workspace for the given window.
    pub fn unregister(&mut self, window_id: WindowId) -> Option<u64> {
        self.workspaces.remove(&window_id);
        if self.session_search_documents.remove(&window_id).is_some() {
            self.session_search_generation = self.session_search_generation.wrapping_add(1);
            Some(self.session_search_generation)
        } else {
            None
        }
    }

    /// Replaces one Workspace's already-committed Navigator search projection.
    /// `WindowId` scopes this transient in-memory index only; stable RowId and
    /// container identity remain owned by the Navigator model itself.
    pub(crate) fn replace_session_search_documents(
        &mut self,
        window_id: WindowId,
        documents: Vec<SessionNavigationData>,
    ) -> u64 {
        self.session_search_documents.insert(window_id, documents);
        self.session_search_generation = self.session_search_generation.wrapping_add(1);
        self.session_search_generation
    }

    /// Returns a clone suitable for a query-only search path. The clone is made
    /// at projection commit time, not by discovering Workspace/session sources
    /// during each palette keystroke.
    pub(crate) fn session_search_snapshot(&self) -> (u64, Vec<SessionNavigationData>) {
        (
            self.session_search_generation(),
            self.session_search_documents
                .values()
                .flat_map(|documents| documents.iter().cloned())
                .collect(),
        )
    }

    pub(crate) fn session_search_generation(&self) -> u64 {
        self.session_search_generation
    }

    pub(crate) fn session_search_documents(&self) -> impl Iterator<Item = &SessionNavigationData> {
        self.session_search_documents.values().flatten()
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
    /// 在指定 Environment authority 的远端进程已确定退出后，标记其全部
    /// durable terminal owner 已退出。
    ///
    /// 远端 transport 已永久消失，不会再发送 `PtyExited`。因此这里补齐
    /// `TerminalModel` 的退出事实，让 retiring lease 可以释放。界面线程
    /// 禁止阻塞等待 `TerminalModel` 锁；锁暂不可用时交给短生命周期后台线程，
    /// lease 在下一次查询时按既有惰性清理路径收敛。
    pub(crate) fn exit_retiring_session_owners_for_authority(&mut self, authority: &str) {
        let agent_prefix = format!("{authority}::agent:");
        let matching_keys = self
            .retiring_session_owners
            .keys()
            .filter(|key| key.starts_with(&agent_prefix))
            .cloned()
            .collect::<Vec<_>>();

        for key in matching_keys {
            let Some(owners) = self.retiring_session_owners.get_mut(&key) else {
                continue;
            };
            for owner in owners.iter() {
                let terminal_model = owner.terminal_model.clone();
                if let Some(mut model) = terminal_model.try_lock() {
                    model.exit(ExitReason::PtyDisconnected);
                    continue;
                }

                let terminal_model = owner.terminal_model.clone();
                let durable_identity_key = key.clone();
                let worker_identity_key = durable_identity_key.clone();
                if let Err(error) = std::thread::Builder::new()
                    .name("remote-terminal-exit".to_owned())
                    .spawn(move || {
                        terminal_model.lock().exit(ExitReason::PtyDisconnected);
                        log::info!(
                            "Remote process exited; completed deferred retiring lease release for durable session {worker_identity_key}"
                        );
                    })
                {
                    log::error!(
                        "Remote process exited but the retiring lease release worker for {durable_identity_key} could not start: {error}"
                    );
                }
            }
        }

        // 已拿到锁的 model 会在此立即移除；交给后台线程的 model 继续留在
        // registry，并由 `is_session_owner_retiring` 在观察到 exit 后惰性清理。
        self.retiring_session_owners.retain(|key, owners| {
            if !key.starts_with(&agent_prefix) {
                return true;
            }
            owners.retain(|owner| {
                owner
                    .terminal_model
                    .try_lock()
                    .is_none_or(|model| !model.has_exited())
            });
            !owners.is_empty()
        });
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
    type Event = WorkspaceRegistryEvent;
}

impl SingletonEntity for WorkspaceRegistry {}

#[cfg(test)]
#[path = "registry_tests.rs"]
mod tests;
