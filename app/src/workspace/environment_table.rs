//! Environment table — the single source of truth for environment state in a
//! workspace.
//!
//! Before this module, environment state was spread across 17 fields on
//! `Workspace`: a `current_environment` snapshot, an `EnvironmentRuntimeRegistry`
//! (remote-only), 7 `pending_environment_runtime_*` intent maps, 3 generation
//! maps, `retained_environment_authorities`, `home_roots`, and 2 indexed-session
//! maps. Every consumer had to know whether to read `current_environment` or the
//! registry, and local/remote fork was a data-storage concern.
//!
//! `EnvironmentTable` consolidates all of that into one `HashMap<authority,
//! EnvironmentEntry>` plus an `active_authority` pointer. Local and remote
//! environments live in the same table; behavioral differences are carried by
//! `EnvironmentEntryBackend`, not by storage forks.

use std::collections::HashMap;
use std::path::PathBuf;

use warp_core::{HostId, SessionId};

use crate::app_state::{EnvironmentLifecycleState, EnvironmentSnapshot, WorkspaceSessionSnapshot};
use crate::pane_group::PaneId;
use crate::workspace::environment_backend::{
    AgentTabEntry, ForkEntry, PendingEnvironmentRuntimeSessionRestore,
};
use crate::workspace::environment_runtime::{
    EnvironmentCliAgentSessionUserState, EnvironmentRuntimeSpawnPlan, EnvironmentRuntimeStatus,
    EnvironmentRuntimeTarget, EnvironmentRuntimeTerminalSpawn, TerminalBootstrapSpawn,
    TerminalBootstrapTarget,
};

/// One row in the environment table. Holds the environment snapshot, runtime
/// handle (if remote), transport generations, home root, pending user intents,
/// and indexed CLI-agent sessions.
///
/// Local entries have `status == Connected`, all runtime fields `None`,
/// generations `0`, and all intent fields empty (local delivery is synchronous).
#[derive(Clone)]
pub(crate) struct EnvironmentEntry {
    /// Environment identity + display metadata.
    pub(crate) snapshot: EnvironmentSnapshot,

    /// Whether this authority is retained by the workspace (lifecycle owned
    /// until explicit disconnect). Replaces `retained_environment_authorities`.
    pub(crate) retained: bool,

    // --- Runtime handle (remote only; local is always Connected / None) ---
    pub(crate) status: EnvironmentRuntimeStatus,
    pub(crate) synthetic_session_id: Option<SessionId>,
    pub(crate) host_id: Option<HostId>,
    pub(crate) control_path: Option<PathBuf>,
    pub(crate) last_error: Option<String>,

    // --- Transport generations (remote only; local is always 0) ---
    /// Monotonic heartbeat generation. Every reconnect increments so late
    /// heartbeat callbacks from old clients are ignored.
    pub(crate) heartbeat_generation: u64,
    /// Monotonic preparation watchdog generation. Connecting/installing must
    /// eventually produce Connected/Error; a stale watchdog must not poison a
    /// newer session.
    pub(crate) preparation_generation: u64,

    // --- Roots ---
    /// Home root discovered at connect time (remote). `None` for local.
    pub(crate) home_root: Option<String>,

    // --- Pending user intents (remote only; local is synchronous) ---
    pub(crate) pending_terminal: bool,
    pub(crate) pending_startup_command: Option<String>,
    pub(crate) pending_agent_view: Option<AgentTabEntry>,
    pub(crate) pending_forked_conversation: Option<ForkEntry>,
    pub(crate) pending_restore: Option<PendingEnvironmentRuntimeSessionRestore>,
    pub(crate) pending_split_pane_loading_id: Option<PaneId>,

    // --- Indexed CLI-agent sessions ---
    /// CLI-agent sessions discovered for this environment. For local, scanned
    /// from terminal models; for remote, scanned from the runtime home.
    pub(crate) indexed_cli_agent_sessions: Vec<WorkspaceSessionSnapshot>,
    /// Aliases/pins owned by this environment. For local, from sidecar; for
    /// remote, from runtime RPC.
    pub(crate) cli_agent_session_user_state: EnvironmentCliAgentSessionUserState,
}

impl EnvironmentEntry {
    /// Create a remote entry in the Dormant state.
    pub(crate) fn dormant(environment: EnvironmentSnapshot) -> Self {
        Self {
            snapshot: environment,
            retained: false,
            status: EnvironmentRuntimeStatus::Dormant,
            synthetic_session_id: None,
            host_id: None,
            control_path: None,
            last_error: None,
            heartbeat_generation: 0,
            preparation_generation: 0,
            home_root: None,
            pending_terminal: false,
            pending_startup_command: None,
            pending_agent_view: None,
            pending_forked_conversation: None,
            pending_restore: None,
            pending_split_pane_loading_id: None,
            indexed_cli_agent_sessions: Vec::new(),
            cli_agent_session_user_state: EnvironmentCliAgentSessionUserState::default(),
        }
    }

    /// True when this entry has any pending user intent queued for the next
    /// environment-owned terminal.
    pub(crate) fn has_pending_entry(&self) -> bool {
        self.pending_terminal
            || self.pending_startup_command.is_some()
            || self.pending_agent_view.is_some()
            || self.pending_forked_conversation.is_some()
            || self.pending_restore.is_some()
    }

    /// Clear all pending intents for this entry.
    pub(crate) fn clear_pending_intents(&mut self) {
        self.pending_terminal = false;
        self.pending_startup_command = None;
        self.pending_agent_view = None;
        self.pending_forked_conversation = None;
        self.pending_restore = None;
        self.pending_split_pane_loading_id = None;
    }
}

/// The single environment-state container for a workspace. Replaces 17
/// scattered fields on `Workspace`.
#[derive(Default)]
pub(crate) struct EnvironmentTable {
    entries: HashMap<String, EnvironmentEntry>,
    /// Authority of the currently active environment. Replaces
    /// `current_environment: Option<EnvironmentSnapshot>`.
    active_authority: Option<String>,
    /// Reverse lookup: synthetic session → authority (remote only).
    session_to_authority: HashMap<SessionId, String>,
    /// Monotonic allocator for runtime synthetic session IDs.
    next_runtime_session_id: u64,
}

impl EnvironmentTable {
    // --- Active environment ---

    pub(crate) fn set_active_authority(&mut self, authority: Option<String>) {
        self.active_authority = authority;
    }

    pub(crate) fn current_snapshot(&self) -> Option<EnvironmentSnapshot> {
        self.active_authority
            .as_ref()
            .and_then(|authority| self.entries.get(authority))
            .map(|entry| entry.snapshot.clone())
    }

    // --- Entry access ---

    pub(crate) fn entry_for_authority(&self, authority: &str) -> Option<&EnvironmentEntry> {
        self.entries.get(authority)
    }

    pub(crate) fn entry_for_authority_mut(
        &mut self,
        authority: &str,
    ) -> Option<&mut EnvironmentEntry> {
        self.entries.get_mut(authority)
    }

    pub(crate) fn snapshot_for_authority(&self, authority: &str) -> Option<EnvironmentSnapshot> {
        self.entries.get(authority).map(|e| e.snapshot.clone())
    }

    /// Mutably patch the snapshot for an authority. Returns `false` if the
    /// authority is not present.
    pub(crate) fn patch_snapshot_for_authority(
        &mut self,
        authority: &str,
        patch: impl FnOnce(&mut EnvironmentSnapshot),
    ) -> bool {
        if let Some(entry) = self.entries.get_mut(authority) {
            patch(&mut entry.snapshot);
            true
        } else {
            false
        }
    }

    /// All entries, iterator.
    pub(crate) fn entries(&self) -> impl Iterator<Item = (&String, &EnvironmentEntry)> {
        self.entries.iter()
    }

    /// All environment snapshots from runtime (non-local) entries, sorted by
    /// label then authority (matches the old `environment_snapshots()` order).
    pub(crate) fn runtime_snapshots(&self) -> Vec<EnvironmentSnapshot> {
        let mut snapshots: Vec<_> = self
            .entries
            .values()
            .filter(|e| {
                e.status != EnvironmentRuntimeStatus::Connected || e.synthetic_session_id.is_some()
            })
            .map(|e| e.snapshot.clone())
            .collect();
        snapshots.sort_by(|left, right| {
            left.label
                .to_lowercase()
                .cmp(&right.label.to_lowercase())
                .then_with(|| left.authority_key.cmp(&right.authority_key))
        });
        snapshots
    }

    // --- Insert / remove ---

    /// Insert or update an environment. If the authority doesn't exist, create a
    /// dormant (remote) or connected (local) entry.
    pub(crate) fn upsert(&mut self, environment: EnvironmentSnapshot) {
        let authority = environment.authority_key.clone();
        self.entries
            .entry(authority)
            .and_modify(|entry| entry.snapshot = environment.clone())
            .or_insert_with(|| EnvironmentEntry::dormant(environment));
    }

    pub(crate) fn remove(&mut self, authority: &str) -> Option<EnvironmentEntry> {
        let entry = self.entries.remove(authority)?;
        self.session_to_authority.retain(|_, a| a != authority);
        if self.active_authority.as_deref() == Some(authority) {
            self.active_authority = None;
        }
        Some(entry)
    }

    // --- Retained ---

    pub(crate) fn retain(&mut self, authority: &str) {
        if let Some(entry) = self.entries.get_mut(authority) {
            entry.retained = true;
        }
    }

    pub(crate) fn release(&mut self, authority: &str) {
        if let Some(entry) = self.entries.get_mut(authority) {
            entry.retained = false;
        }
    }

    pub(crate) fn is_retained(&self, authority: &str) -> bool {
        self.entries.get(authority).is_some_and(|e| e.retained)
    }

    // --- Lifecycle / status (remote) ---

    pub(crate) fn lifecycle_for_authority(
        &self,
        authority: &str,
    ) -> Option<EnvironmentLifecycleState> {
        self.entries
            .get(authority)
            .map(|e| e.status.lifecycle_state())
    }

    pub(crate) fn mark_connecting(
        &mut self,
        mut environment: EnvironmentSnapshot,
        session_id: SessionId,
        control_path: PathBuf,
    ) {
        environment.lifecycle_state = EnvironmentLifecycleState::Connecting;
        let authority = environment.authority_key.clone();
        let prev = self.entries.remove(&authority);
        if let Some(prev_session) = prev.as_ref().and_then(|e| e.synthetic_session_id) {
            self.session_to_authority.remove(&prev_session);
        }
        let retained = prev.as_ref().is_some_and(|e| e.retained);
        let heartbeat_generation = prev.as_ref().map(|e| e.heartbeat_generation).unwrap_or(0);
        let preparation_generation = prev.as_ref().map(|e| e.preparation_generation).unwrap_or(0);
        let home_root = prev.as_ref().and_then(|e| e.home_root.clone());
        let pending_terminal = prev.as_ref().is_some_and(|e| e.pending_terminal);
        let pending_startup_command = prev
            .as_ref()
            .and_then(|e| e.pending_startup_command.clone());
        let pending_agent_view = prev.as_ref().and_then(|e| e.pending_agent_view.clone());
        let pending_forked_conversation = prev
            .as_ref()
            .and_then(|e| e.pending_forked_conversation.clone());
        let pending_restore = prev.as_ref().and_then(|e| e.pending_restore.clone());
        let pending_split_pane_loading_id =
            prev.as_ref().and_then(|e| e.pending_split_pane_loading_id);
        let indexed_cli_agent_sessions = prev
            .as_ref()
            .map(|e| e.indexed_cli_agent_sessions.clone())
            .unwrap_or_default();
        let cli_agent_session_user_state = prev
            .as_ref()
            .map(|e| e.cli_agent_session_user_state.clone())
            .unwrap_or_default();
        self.session_to_authority
            .insert(session_id, authority.clone());
        self.entries.insert(
            authority,
            EnvironmentEntry {
                snapshot: environment,
                retained,
                status: EnvironmentRuntimeStatus::Connecting,
                synthetic_session_id: Some(session_id),
                host_id: None,
                control_path: Some(control_path),
                last_error: None,
                heartbeat_generation,
                preparation_generation,
                home_root,
                pending_terminal,
                pending_startup_command,
                pending_agent_view,
                pending_forked_conversation,
                pending_restore,
                pending_split_pane_loading_id,
                indexed_cli_agent_sessions,
                cli_agent_session_user_state,
            },
        );
    }

    pub(crate) fn mark_installing_session(&mut self, session_id: SessionId) -> Option<String> {
        let authority = self.current_authority_for_session(session_id)?.to_owned();
        let entry = self.entries.get_mut(&authority)?;
        entry.status = EnvironmentRuntimeStatus::Installing;
        entry.snapshot.lifecycle_state = EnvironmentLifecycleState::Installing;
        self.session_to_authority
            .insert(session_id, authority.clone());
        Some(authority)
    }

    pub(crate) fn mark_connected_session(
        &mut self,
        session_id: SessionId,
        host_id: HostId,
    ) -> Option<String> {
        let authority = self.current_authority_for_session(session_id)?.to_owned();
        let entry = self.entries.get_mut(&authority)?;
        entry.status = EnvironmentRuntimeStatus::Connected;
        entry.host_id = Some(host_id);
        entry.last_error = None;
        entry.snapshot.lifecycle_state = EnvironmentLifecycleState::Connected;
        self.session_to_authority
            .insert(session_id, authority.clone());
        Some(authority)
    }

    pub(crate) fn mark_error_for_session(
        &mut self,
        session_id: SessionId,
        error: String,
    ) -> Option<String> {
        let authority = self.current_authority_for_session(session_id)?.to_owned();
        self.mark_error_for_authority(&authority, error);
        self.session_to_authority
            .insert(session_id, authority.clone());
        Some(authority)
    }

    pub(crate) fn mark_error_for_authority(&mut self, authority: &str, error: String) {
        if let Some(entry) = self.entries.get_mut(authority) {
            entry.status = EnvironmentRuntimeStatus::Error;
            entry.last_error = Some(error);
            entry.snapshot.lifecycle_state = EnvironmentLifecycleState::Error;
        }
    }

    // --- Session ↔ authority ---

    pub(crate) fn authority_for_session(&self, session_id: SessionId) -> Option<&str> {
        self.session_to_authority
            .get(&session_id)
            .map(String::as_str)
    }

    pub(crate) fn authority_for_session_or_synthetic(&self, session_id: SessionId) -> Option<&str> {
        self.authority_for_session(session_id).or_else(|| {
            self.entries.iter().find_map(|(authority, entry)| {
                (entry.synthetic_session_id == Some(session_id)).then_some(authority.as_str())
            })
        })
    }

    pub(crate) fn current_authority_for_session(&self, session_id: SessionId) -> Option<&str> {
        let authority = self.authority_for_session_or_synthetic(session_id)?;
        let entry = self.entries.get(authority)?;
        (entry.synthetic_session_id == Some(session_id)).then_some(authority)
    }

    // --- Runtime queries ---

    pub(crate) fn session_for_authority(&self, authority: &str) -> Option<SessionId> {
        self.entries.get(authority)?.synthetic_session_id
    }

    pub(crate) fn has_bootstrap_session(&self, authority: &str) -> bool {
        let Some(entry) = self.entries.get(authority) else {
            return false;
        };
        match entry.status {
            EnvironmentRuntimeStatus::Connecting | EnvironmentRuntimeStatus::Installing => {
                entry.synthetic_session_id.is_some()
            }
            EnvironmentRuntimeStatus::Dormant
            | EnvironmentRuntimeStatus::Connected
            | EnvironmentRuntimeStatus::Error => false,
        }
    }

    pub(crate) fn connected_target_for_authority(
        &self,
        authority: &str,
    ) -> Option<EnvironmentRuntimeTarget> {
        let entry = self.entries.get(authority)?;
        if entry.status != EnvironmentRuntimeStatus::Connected {
            return None;
        }
        Some(EnvironmentRuntimeTarget {
            authority: authority.to_owned(),
            session_id: entry.synthetic_session_id?,
            host_id: entry.host_id.clone()?,
            root: entry.snapshot.active_workspace_root.clone(),
        })
    }

    pub(crate) fn connected_session_for_host(
        &self,
        host_id: &HostId,
    ) -> Option<(String, SessionId)> {
        self.entries.iter().find_map(|(authority, entry)| {
            if entry.status == EnvironmentRuntimeStatus::Connected
                && entry.host_id.as_ref() == Some(host_id)
            {
                Some((authority.clone(), entry.synthetic_session_id?))
            } else {
                None
            }
        })
    }

    pub(crate) fn control_path_for_session(&self, session_id: SessionId) -> Option<PathBuf> {
        let authority = self.current_authority_for_session(session_id)?;
        self.entries.get(authority)?.control_path.clone()
    }

    pub(crate) fn connection_ref_for_authority(&self, authority: &str) -> Option<String> {
        self.entries
            .get(authority)?
            .snapshot
            .runtime_connection_ref()
            .map(str::to_owned)
    }

    // --- Generations ---

    pub(crate) fn heartbeat_generation(&self, authority: &str) -> u64 {
        self.entries
            .get(authority)
            .map(|e| e.heartbeat_generation)
            .unwrap_or(0)
    }

    pub(crate) fn bump_heartbeat_generation(&mut self, authority: &str) -> Option<u64> {
        let entry = self.entries.get_mut(authority)?;
        entry.heartbeat_generation += 1;
        Some(entry.heartbeat_generation)
    }

    pub(crate) fn preparation_generation(&self, authority: &str) -> u64 {
        self.entries
            .get(authority)
            .map(|e| e.preparation_generation)
            .unwrap_or(0)
    }

    pub(crate) fn bump_preparation_generation(&mut self, authority: &str) -> Option<u64> {
        let entry = self.entries.get_mut(authority)?;
        entry.preparation_generation += 1;
        Some(entry.preparation_generation)
    }

    /// Reset the preparation generation for an authority back to 0 (clears the
    /// watchdog). Called when the runtime reaches a terminal state (Connected
    /// or Error).
    pub(crate) fn clear_preparation_generation(&mut self, authority: &str) {
        if let Some(entry) = self.entries.get_mut(authority) {
            entry.preparation_generation = 0;
        }
    }

    // --- Home roots ---

    pub(crate) fn home_root(&self, authority: &str) -> Option<String> {
        self.entries
            .get(authority)
            .and_then(|e| e.home_root.clone())
    }

    pub(crate) fn set_home_root(&mut self, authority: String, home_root: String) {
        if let Some(entry) = self.entries.get_mut(&authority) {
            entry.home_root = Some(home_root);
        }
    }

    pub(crate) fn clear_home_root(&mut self, authority: &str) {
        if let Some(entry) = self.entries.get_mut(authority) {
            entry.home_root = None;
        }
    }

    // --- Workspace root ---

    // --- Pending intents ---

    pub(crate) fn queue_terminal(&mut self, authority: &str) {
        if let Some(entry) = self.entries.get_mut(authority) {
            entry.pending_terminal = true;
        }
    }

    pub(crate) fn queue_startup_command(&mut self, authority: &str, command: String) {
        if let Some(entry) = self.entries.get_mut(authority) {
            entry.pending_startup_command = Some(command);
        }
    }

    pub(crate) fn queue_agent_view(&mut self, authority: &str, entry: AgentTabEntry) {
        if let Some(e) = self.entries.get_mut(authority) {
            e.pending_agent_view = Some(entry);
        }
    }

    pub(crate) fn queue_forked_conversation(&mut self, authority: &str, entry: ForkEntry) {
        if let Some(e) = self.entries.get_mut(authority) {
            e.pending_forked_conversation = Some(entry);
        }
    }

    pub(crate) fn queue_restore(
        &mut self,
        authority: &str,
        restore: PendingEnvironmentRuntimeSessionRestore,
    ) {
        if let Some(entry) = self.entries.get_mut(authority) {
            entry.pending_restore = Some(restore);
        }
    }

    pub(crate) fn set_split_pane_loading_id(&mut self, authority: &str, pane_id: PaneId) {
        if let Some(entry) = self.entries.get_mut(authority) {
            entry.pending_split_pane_loading_id = Some(pane_id);
        }
    }

    pub(crate) fn clear_terminal(&mut self, authority: &str) {
        if let Some(entry) = self.entries.get_mut(authority) {
            entry.pending_terminal = false;
        }
    }

    pub(crate) fn clear_startup_command(&mut self, authority: &str) {
        if let Some(entry) = self.entries.get_mut(authority) {
            entry.pending_startup_command = None;
        }
    }

    pub(crate) fn clear_agent_view(&mut self, authority: &str) {
        if let Some(entry) = self.entries.get_mut(authority) {
            entry.pending_agent_view = None;
        }
    }

    pub(crate) fn clear_forked_conversation(&mut self, authority: &str) {
        if let Some(entry) = self.entries.get_mut(authority) {
            entry.pending_forked_conversation = None;
        }
    }

    pub(crate) fn clear_split_pane_loading_id(&mut self, authority: &str) {
        if let Some(entry) = self.entries.get_mut(authority) {
            entry.pending_split_pane_loading_id = None;
        }
    }

    pub(crate) fn clear_pending_intents(&mut self, authority: &str) {
        if let Some(entry) = self.entries.get_mut(authority) {
            entry.clear_pending_intents();
        }
    }

    pub(crate) fn has_pending_entry(&self, authority: &str) -> bool {
        self.entries
            .get(authority)
            .is_some_and(|e| e.has_pending_entry())
    }

    // --- Indexed CLI-agent sessions ---

    pub(crate) fn indexed_cli_agent_sessions_for_authority(
        &self,
        authority: &str,
    ) -> Vec<WorkspaceSessionSnapshot> {
        self.entries
            .get(authority)
            .map(|e| e.indexed_cli_agent_sessions.clone())
            .unwrap_or_default()
    }

    pub(crate) fn set_indexed_cli_agent_sessions(
        &mut self,
        authority: String,
        sessions: Vec<WorkspaceSessionSnapshot>,
    ) {
        if let Some(entry) = self.entries.get_mut(&authority) {
            entry.indexed_cli_agent_sessions = sessions;
        }
    }

    pub(crate) fn clear_indexed_cli_agent_sessions(&mut self, authority: &str) {
        if let Some(entry) = self.entries.get_mut(authority) {
            entry.indexed_cli_agent_sessions.clear();
        }
    }

    pub(crate) fn cli_agent_session_user_state(
        &self,
        authority: &str,
    ) -> EnvironmentCliAgentSessionUserState {
        self.entries
            .get(authority)
            .map(|e| e.cli_agent_session_user_state.clone())
            .unwrap_or_default()
    }

    pub(crate) fn set_cli_agent_session_user_state(
        &mut self,
        authority: String,
        state: EnvironmentCliAgentSessionUserState,
    ) {
        if let Some(entry) = self.entries.get_mut(&authority) {
            entry.cli_agent_session_user_state = state;
        }
    }

    // --- Session ID allocation ---

    pub(crate) fn next_runtime_session_id(&mut self) -> SessionId {
        let session_id = SessionId::from(self.next_runtime_session_id);
        self.next_runtime_session_id += 1;
        session_id
    }

    // --- Spawn plan (unified) ---

    pub(crate) fn terminal_bootstrap_target_for_environment(
        &self,
        environment: &EnvironmentSnapshot,
    ) -> Option<TerminalBootstrapTarget> {
        if !crate::workspace::environment_runtime::uses_terminal_bootstrap(environment) {
            return None;
        }
        Some(TerminalBootstrapTarget {
            authority: environment.authority_key.clone(),
            root: environment.active_workspace_root.clone(),
        })
    }

    pub(crate) fn terminal_bootstrap_spawn_for_target(
        &self,
        target: TerminalBootstrapTarget,
    ) -> TerminalBootstrapSpawn {
        let initial_directory = target
            .root
            .as_ref()
            .filter(|root| !root.trim().is_empty())
            .map(PathBuf::from);
        TerminalBootstrapSpawn {
            target,
            initial_directory,
        }
    }

    pub(crate) fn terminal_spawn_for_target(
        &self,
        target: EnvironmentRuntimeTarget,
        root: impl Into<String>,
    ) -> EnvironmentRuntimeTerminalSpawn {
        EnvironmentRuntimeTerminalSpawn {
            target,
            root: root.into(),
        }
    }

    pub(crate) fn spawn_plan_for_environment(
        &self,
        environment: &EnvironmentSnapshot,
    ) -> EnvironmentRuntimeSpawnPlan {
        if let Some(target) = self.terminal_bootstrap_target_for_environment(environment) {
            return EnvironmentRuntimeSpawnPlan::TerminalBootstrap(target);
        }
        self.connected_target_for_authority(&environment.authority_key)
            .map(|mut target| {
                target.root = environment.active_workspace_root.clone();
                EnvironmentRuntimeSpawnPlan::RuntimeTarget(target)
            })
            .unwrap_or(EnvironmentRuntimeSpawnPlan::RuntimeBootstrap)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use warp_core::{HostId, SessionId};

    use super::*;
    use crate::app_state::{EnvironmentKind, EnvironmentLifecycleState};

    fn ssh_environment(authority_key: &str) -> EnvironmentSnapshot {
        EnvironmentSnapshot::runtime_transport(
            EnvironmentKind::Ssh,
            authority_key.to_owned(),
            authority_key.to_owned(),
            Some(authority_key.to_owned()),
            Some("/".to_owned()),
            EnvironmentLifecycleState::Dormant,
        )
    }

    #[test]
    fn rejects_stale_session_transitions_after_reconnect() {
        let mut table = EnvironmentTable::default();
        let first_session = SessionId::from(1);
        let second_session = SessionId::from(2);

        table.mark_connecting(
            ssh_environment("ssh:example"),
            first_session,
            PathBuf::from("/tmp/first.sock"),
        );
        table.mark_connecting(
            ssh_environment("ssh:example"),
            second_session,
            PathBuf::from("/tmp/second.sock"),
        );

        assert_eq!(table.authority_for_session(first_session), None);
        assert_eq!(table.current_authority_for_session(first_session), None);
        assert_eq!(table.control_path_for_session(first_session), None);
        assert_eq!(table.mark_installing_session(first_session), None);
        assert_eq!(
            table.mark_connected_session(first_session, HostId::new("old-host".to_owned())),
            None
        );
        assert_eq!(
            table.mark_error_for_session(first_session, "old failure".to_owned()),
            None
        );

        assert_eq!(
            table.current_authority_for_session(second_session),
            Some("ssh:example")
        );
        assert_eq!(
            table.control_path_for_session(second_session),
            Some(PathBuf::from("/tmp/second.sock"))
        );
        assert_eq!(
            table.lifecycle_for_authority("ssh:example"),
            Some(EnvironmentLifecycleState::Connecting)
        );

        table.remove("ssh:example");
        assert_eq!(table.authority_for_session(second_session), None);
    }
}
