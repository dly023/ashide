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
//! `EnvironmentBackend`, not by storage forks.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;

use warp_core::{HostId, SessionId};
use warpui::EntityId;

use crate::app_state::{
    EnvironmentKind, EnvironmentLifecycleState, EnvironmentSnapshot, WorkspaceSessionSnapshot,
};
use crate::environment_authority::ParsedEnvironmentAuthority;
use crate::pane_group::PaneId;
use crate::terminal::CLIAgent;
use crate::workspace::environment_backend::{
    AgentTabEntry, EnvironmentEntryIntent, ForkEntry, PlainTerminalEntry, SessionRestoreEntry,
};
use crate::workspace::environment_runtime::{
    EnvironmentCliAgentSessionUserState, EnvironmentRuntimeSpawnPlan, EnvironmentRuntimeStatus,
    EnvironmentRuntimeTarget, EnvironmentRuntimeTerminalSpawn, TerminalBootstrapSpawn,
    TerminalBootstrapTarget,
};
use crate::workspace::view::session_navigator_reducer::SessionNavigatorModel;
#[cfg(test)]
use crate::workspace::view::session_navigator_reducer::SessionNavigatorState;

#[derive(Clone)]
pub(crate) struct PendingMaterialization {
    pub(crate) stage: PendingMaterializationStage,
    pub(crate) intent: EnvironmentEntryIntent,
    generation: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MaterializationTransition {
    pub(crate) authority: String,
    pub(crate) pane_id: PaneId,
    pub(crate) generation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MaterializationOutcome {
    Success,
    RetryableFailure { retryable_pane_id: PaneId },
    Cancelled,
    CarrierMissing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MaterializationError {
    CarrierMissing,
}

#[derive(Clone)]
pub(crate) enum MaterializationCompletion {
    Applied(PendingMaterialization),
    Stale,
    Failed(MaterializationError),
}

impl MaterializationCompletion {
    #[cfg(test)]
    fn is_carrier_missing(&self) -> bool {
        matches!(self, Self::Failed(MaterializationError::CarrierMissing))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PendingMaterializationStage {
    /// Intent is bound to a placeholder (or a failed terminal that is safe to replace).
    Queued { pane_id: PaneId },
    /// A runtime terminal exists, but remote PTY bootstrap has not committed yet.
    Materializing { pane_id: PaneId },
}

impl PendingMaterialization {
    pub(crate) fn pane_id(&self) -> PaneId {
        match self.stage {
            PendingMaterializationStage::Queued { pane_id }
            | PendingMaterializationStage::Materializing { pane_id } => pane_id,
        }
    }

    pub(crate) fn is_queued(&self) -> bool {
        matches!(self.stage, PendingMaterializationStage::Queued { .. })
    }
}

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
    /// FIFO of pane-owned materialization requests. Pane identity and payload
    /// are one value so concurrent restores cannot overwrite or cross-bind.
    pub(crate) pending_materializations: VecDeque<PendingMaterialization>,
    next_materialization_generation: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct IndexedCliAgentSessionScanToken {
    navigation_key: String,
    session_id: Option<SessionId>,
    generation: u64,
    observed_agents: HashSet<CLIAgent>,
}

impl IndexedCliAgentSessionScanToken {
    pub(crate) fn observed_agents(&self) -> &HashSet<CLIAgent> {
        &self.observed_agents
    }
}

/// 已索引 CLI-agent discovery 对 canonical Environment collection 的唯一提交语义。
///
/// scan generation、source observation 与 session projection 必须由
/// [`EnvironmentTable`] 原子持有；delivery adapter 只能提供这个结果，不能自行
/// 替换、过滤或推断删除。
#[derive(Clone, Debug)]
pub(crate) enum IndexedCliAgentSessionScanOutcome {
    Complete {
        observed_agents: HashSet<CLIAgent>,
        sessions: Vec<WorkspaceSessionSnapshot>,
    },
    SourceMissing(CLIAgent),
    PermanentlyDeleted(CLIAgent),
    Cancelled,
}

impl EnvironmentEntry {
    /// Create an entry whose lifecycle is derived from backend capability and
    /// the persisted failure boundary. Current-app/local is intrinsically
    /// connected. Runtime-backed entries without a live transport start
    /// dormant, except that a persisted Error remains terminal until an
    /// explicit user reconnect.
    pub(crate) fn from_snapshot(mut environment: EnvironmentSnapshot) -> Self {
        let status = if crate::workspace::environment_runtime::uses_terminal_bootstrap(&environment)
        {
            environment.lifecycle_state = EnvironmentLifecycleState::Connected;
            EnvironmentRuntimeStatus::Connected
        } else {
            match environment.lifecycle_state {
                EnvironmentLifecycleState::Error => EnvironmentRuntimeStatus::Error,
                EnvironmentLifecycleState::Dormant
                | EnvironmentLifecycleState::Connecting
                | EnvironmentLifecycleState::Installing
                | EnvironmentLifecycleState::Connected => {
                    environment.lifecycle_state = EnvironmentLifecycleState::Dormant;
                    EnvironmentRuntimeStatus::Dormant
                }
            }
        };
        Self {
            snapshot: environment,
            retained: false,
            status,
            synthetic_session_id: None,
            host_id: None,
            control_path: None,
            last_error: None,
            heartbeat_generation: 0,
            preparation_generation: 0,
            home_root: None,
            pending_materializations: VecDeque::new(),
            next_materialization_generation: 0,
        }
    }

    /// True when this entry has any pending user intent queued for the next
    /// environment-owned terminal.
    pub(crate) fn has_pending_entry(&self) -> bool {
        !self.pending_materializations.is_empty()
    }

    /// Clear all pending intents for this entry.
    pub(crate) fn drain_pending_materializations(
        &mut self,
    ) -> impl Iterator<Item = PendingMaterialization> + '_ {
        self.pending_materializations.drain(..)
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
    /// 每个逻辑 Environment 独立记忆最近激活 tab 的稳定实体身份。
    last_active_tab_by_navigation_key: HashMap<String, EntityId>,
    /// Session Navigator 是逻辑 Environment 的 UI 模型，而不是某次 authority/
    /// transport 连接的附属字段。`local:/path` 等 authority alias 必须共享同一份
    /// 状态，重连或 workspace root 变化也不能制造新的 selection/order 分区。
    session_navigator_model_by_navigation_key: HashMap<String, SessionNavigatorModel>,
    /// 已索引会话与 Navigator model 使用完全相同的逻辑 Environment 分区。
    /// transport authority 变化或 local root alias 不得制造第二份可见 projection。
    indexed_cli_agent_sessions_by_navigation_key: HashMap<String, Vec<WorkspaceSessionSnapshot>>,
    /// Latest indexed-session scan transaction for each logical Environment.
    /// The token and collection share this owner so stale async success cannot
    /// destructively replace a newer canonical projection.
    indexed_cli_agent_session_scan_by_navigation_key:
        HashMap<String, IndexedCliAgentSessionScanToken>,
    /// 成功观察过的 provider 归属 canonical collection owner。空 provider store
    /// 也必须记录，才能把下一代 source disappearance 与首次未 provision 区分。
    indexed_cli_agent_session_observed_agents_by_navigation_key: HashMap<String, HashSet<CLIAgent>>,
    /// Alias/Pin 是 canonical projection 的输入，也必须与 indexed sessions/model
    /// 原子地归属同一 navigation key，不能挂在可替换的 transport entry 上。
    cli_agent_session_user_state_by_navigation_key:
        HashMap<String, EnvironmentCliAgentSessionUserState>,
    /// Alias/Pin optimistic projection 与其异步 completion 必须共享 owner。
    /// generation 使旧 success/failure 无法覆盖较新的 mutation 或 refresh。
    cli_agent_session_user_state_generation_by_navigation_key: HashMap<String, u64>,
}

impl EnvironmentTable {
    // --- Active environment ---

    fn projection_navigation_key(authority: &str) -> String {
        ParsedEnvironmentAuthority::parse(authority)
            .navigation_key()
            .to_owned()
    }

    fn ensure_projection_partition(&mut self, authority: &str) {
        let navigation_key = Self::projection_navigation_key(authority);
        self.session_navigator_model_by_navigation_key
            .entry(navigation_key.clone())
            .or_default();
        self.indexed_cli_agent_sessions_by_navigation_key
            .entry(navigation_key.clone())
            .or_default();
        self.indexed_cli_agent_session_observed_agents_by_navigation_key
            .entry(navigation_key.clone())
            .or_default();
        self.cli_agent_session_user_state_by_navigation_key
            .entry(navigation_key)
            .or_default();
    }

    pub(crate) fn set_active_authority(&mut self, authority: Option<String>) {
        if let Some(authority) = authority.as_deref() {
            self.ensure_projection_partition(authority);
        }
        self.active_authority = authority;
    }

    pub(crate) fn current_snapshot(&self) -> Option<EnvironmentSnapshot> {
        self.active_authority
            .as_ref()
            .and_then(|authority| self.entries.get(authority))
            .map(|entry| entry.snapshot.clone())
    }

    pub(crate) fn active_session_navigator_model(&self) -> Option<&SessionNavigatorModel> {
        let authority = self.active_authority.as_deref()?;
        self.session_navigator_model_by_navigation_key
            .get(ParsedEnvironmentAuthority::parse(authority).navigation_key())
    }

    pub(crate) fn active_session_navigator_model_mut(
        &mut self,
    ) -> Option<&mut SessionNavigatorModel> {
        let authority = self.active_authority.as_deref()?;
        self.session_navigator_model_by_navigation_key
            .get_mut(ParsedEnvironmentAuthority::parse(authority).navigation_key())
    }

    #[cfg(test)]
    pub(crate) fn active_session_navigator_state(&self) -> Option<&SessionNavigatorState> {
        Some(&self.active_session_navigator_model()?.state)
    }

    #[cfg(test)]
    pub(crate) fn active_session_navigator_state_mut(
        &mut self,
    ) -> Option<&mut SessionNavigatorState> {
        Some(&mut self.active_session_navigator_model_mut()?.state)
    }

    pub(crate) fn remember_active_tab(&mut self, navigation_key: String, tab_id: EntityId) {
        self.last_active_tab_by_navigation_key
            .insert(navigation_key, tab_id);
    }

    pub(crate) fn last_active_tab(&self, navigation_key: &str) -> Option<EntityId> {
        self.last_active_tab_by_navigation_key
            .get(navigation_key)
            .copied()
    }

    pub(crate) fn forget_navigation_context(&mut self, navigation_key: &str) {
        self.last_active_tab_by_navigation_key
            .remove(navigation_key);
        self.session_navigator_model_by_navigation_key
            .remove(navigation_key);
        self.indexed_cli_agent_sessions_by_navigation_key
            .remove(navigation_key);
        self.indexed_cli_agent_session_scan_by_navigation_key
            .remove(navigation_key);
        self.indexed_cli_agent_session_observed_agents_by_navigation_key
            .remove(navigation_key);
        self.cli_agent_session_user_state_by_navigation_key
            .remove(navigation_key);
        self.cli_agent_session_user_state_generation_by_navigation_key
            .remove(navigation_key);
    }

    // --- Entry access ---

    #[cfg(test)]
    pub(crate) fn entry_for_authority(&self, authority: &str) -> Option<&EnvironmentEntry> {
        self.entries.get(authority)
    }

    pub(crate) fn snapshot_for_authority(&self, authority: &str) -> Option<EnvironmentSnapshot> {
        self.entries.get(authority).map(|e| e.snapshot.clone())
    }

    /// Resolve the semantic Environment targeted by an entry intent, creating
    /// the same dormant table row that pending materialization uses when the
    /// authority has not been opened in this workspace yet. Product actions
    /// must not reconstruct local/runtime snapshots or branch on backend kind.
    pub(crate) fn entry_target_snapshot(&mut self, authority: &str) -> EnvironmentSnapshot {
        self.ensure_entry_for_authority(authority);
        self.entries
            .get(authority)
            .expect("entry target authority must exist after ensure")
            .snapshot
            .clone()
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

    /// All environment snapshots from runtime (non-local) entries, sorted by
    /// label then authority (matches the old `environment_snapshots()` order).
    pub(crate) fn runtime_snapshots(&self) -> Vec<EnvironmentSnapshot> {
        let mut snapshots: Vec<_> = self
            .entries
            .values()
            .filter(|e| {
                crate::workspace::environment_runtime::supports_runtime_entry(&e.snapshot)
                    && (e.status != EnvironmentRuntimeStatus::Connected
                        || e.synthetic_session_id.is_some())
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
        self.ensure_projection_partition(&authority);
        if let Some(entry) = self.entries.get_mut(&authority) {
            entry.snapshot = environment;
            if crate::workspace::environment_runtime::uses_terminal_bootstrap(&entry.snapshot) {
                entry.status = EnvironmentRuntimeStatus::Connected;
                entry.snapshot.lifecycle_state = EnvironmentLifecycleState::Connected;
            }
        } else {
            self.entries
                .insert(authority, EnvironmentEntry::from_snapshot(environment));
        }
    }

    pub(crate) fn remove(&mut self, authority: &str) -> Option<EnvironmentEntry> {
        let entry = self.entries.remove(authority)?;
        self.session_to_authority.retain(|_, a| a != authority);
        if self.active_authority.as_deref() == Some(authority) {
            self.active_authority = None;
        }
        Some(entry)
    }

    /// Drop the runtime transport handle without removing the table row.
    /// Preserves pending intents, retained flag, indexed sessions, and snapshot identity
    /// so reconnect (`clear_user_intents: false`) does not wipe user queue state.
    pub(crate) fn clear_runtime_handle(&mut self, authority: &str) {
        let Some(entry) = self.entries.get_mut(authority) else {
            return;
        };
        if let Some(session_id) = entry.synthetic_session_id.take() {
            self.session_to_authority.remove(&session_id);
        }
        entry.status = EnvironmentRuntimeStatus::Dormant;
        entry.host_id = None;
        entry.control_path = None;
        entry.last_error = None;
        entry.snapshot.lifecycle_state = EnvironmentLifecycleState::Dormant;
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
        let pending_materializations = prev
            .as_ref()
            .map(|e| e.pending_materializations.clone())
            .unwrap_or_default();
        let next_materialization_generation = prev
            .as_ref()
            .map(|entry| entry.next_materialization_generation)
            .unwrap_or(0);
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
                pending_materializations,
                next_materialization_generation,
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

    /// Ensure a dormant table row exists so queue_* can create-on-write (pre-table HashMap semantics).
    fn ensure_entry_for_authority(&mut self, authority: &str) {
        if self.entries.contains_key(authority) {
            return;
        }
        let parsed_authority = ParsedEnvironmentAuthority::parse(authority);
        let kind = if parsed_authority.uses_terminal_bootstrap() {
            EnvironmentKind::Local
        } else {
            EnvironmentKind::Ssh
        };
        let label = parsed_authority
            .display_label()
            .unwrap_or(authority)
            .to_owned();
        let connection_ref = parsed_authority.runtime_connection_ref().map(str::to_owned);
        let snapshot = EnvironmentSnapshot::runtime_transport(
            kind,
            label,
            authority.to_owned(),
            connection_ref,
            None,
            EnvironmentLifecycleState::Dormant,
        );
        self.entries.insert(
            authority.to_owned(),
            EnvironmentEntry::from_snapshot(snapshot),
        );
    }

    pub(crate) fn queue_terminal(
        &mut self,
        authority: &str,
        entry: PlainTerminalEntry,
        materialization_pane_id: PaneId,
    ) {
        self.queue_materialization(
            authority,
            materialization_pane_id,
            EnvironmentEntryIntent::PlainTerminal(entry),
        );
    }

    pub(crate) fn queue_startup_command(
        &mut self,
        authority: &str,
        command: String,
        materialization_pane_id: PaneId,
    ) {
        self.queue_materialization(
            authority,
            materialization_pane_id,
            EnvironmentEntryIntent::StartupCommand(command),
        );
    }

    pub(crate) fn queue_agent_view(
        &mut self,
        authority: &str,
        entry: AgentTabEntry,
        materialization_pane_id: PaneId,
    ) {
        self.queue_materialization(
            authority,
            materialization_pane_id,
            EnvironmentEntryIntent::AgentView(entry),
        );
    }

    pub(crate) fn queue_forked_conversation(
        &mut self,
        authority: &str,
        entry: ForkEntry,
        materialization_pane_id: PaneId,
    ) {
        self.queue_materialization(
            authority,
            materialization_pane_id,
            EnvironmentEntryIntent::ForkedConversation(entry),
        );
    }

    pub(crate) fn queue_restore(
        &mut self,
        authority: &str,
        restore: SessionRestoreEntry,
        materialization_pane_id: PaneId,
    ) {
        self.queue_materialization(
            authority,
            materialization_pane_id,
            EnvironmentEntryIntent::SessionRestore(restore),
        );
    }

    fn queue_materialization(
        &mut self,
        authority: &str,
        pane_id: PaneId,
        intent: EnvironmentEntryIntent,
    ) {
        self.ensure_entry_for_authority(authority);
        if let Some(entry) = self.entries.get_mut(authority) {
            assert!(
                entry
                    .pending_materializations
                    .iter()
                    .all(|pending| pending.pane_id() != pane_id),
                "environment runtime pane {pane_id:?} already owns a pending materialization for {authority}"
            );
            entry.next_materialization_generation =
                entry.next_materialization_generation.saturating_add(1);
            entry
                .pending_materializations
                .push_back(PendingMaterialization {
                    stage: PendingMaterializationStage::Queued { pane_id },
                    intent,
                    generation: entry.next_materialization_generation,
                });
        }
    }

    #[cfg(test)]
    pub(crate) fn pending_materialization(
        &self,
        authority: &str,
    ) -> Option<&PendingMaterialization> {
        self.entries
            .get(authority)?
            .pending_materializations
            .front()
    }

    pub(crate) fn pending_materialization_for_pane(
        &self,
        authority: &str,
        pane_id: PaneId,
    ) -> Option<&PendingMaterialization> {
        self.entries
            .get(authority)?
            .pending_materializations
            .iter()
            .find(|pending| pending.pane_id() == pane_id)
    }

    pub(crate) fn next_queued_materialization(
        &self,
        authority: &str,
    ) -> Option<&PendingMaterialization> {
        self.entries
            .get(authority)?
            .pending_materializations
            .iter()
            .find(|pending| pending.is_queued())
    }

    pub(crate) fn begin_materialization(
        &mut self,
        authority: &str,
        queued_pane_id: PaneId,
        terminal_pane_id: PaneId,
    ) -> Option<MaterializationTransition> {
        let Some(entry) = self.entries.get_mut(authority) else {
            return None;
        };
        assert!(
            entry
                .pending_materializations
                .iter()
                .all(|pending| pending.pane_id() != terminal_pane_id),
            "environment runtime terminal pane {terminal_pane_id:?} already owns a pending materialization for {authority}"
        );
        let Some(pending) = entry.pending_materializations.iter_mut().find(|pending| {
            pending.stage
                == PendingMaterializationStage::Queued {
                    pane_id: queued_pane_id,
                }
        }) else {
            return None;
        };
        pending.stage = PendingMaterializationStage::Materializing {
            pane_id: terminal_pane_id,
        };
        Some(MaterializationTransition {
            authority: authority.to_owned(),
            pane_id: terminal_pane_id,
            generation: pending.generation,
        })
    }

    pub(crate) fn completion_transition_for_pane(
        &self,
        authority: &str,
        pane_id: PaneId,
    ) -> Option<MaterializationTransition> {
        let pending = self.pending_materialization_for_pane(authority, pane_id)?;
        Some(MaterializationTransition {
            authority: authority.to_owned(),
            pane_id,
            generation: pending.generation,
        })
    }

    pub(crate) fn complete_materialization(
        &mut self,
        transition: MaterializationTransition,
        outcome: MaterializationOutcome,
    ) -> MaterializationCompletion {
        let Some(entry) = self.entries.get_mut(&transition.authority) else {
            return MaterializationCompletion::Stale;
        };
        let Some(index) = entry.pending_materializations.iter().position(|pending| {
            pending.pane_id() == transition.pane_id && pending.generation == transition.generation
        }) else {
            return MaterializationCompletion::Stale;
        };

        match outcome {
            MaterializationOutcome::Success | MaterializationOutcome::Cancelled => {
                MaterializationCompletion::Applied(
                    entry
                        .pending_materializations
                        .remove(index)
                        .expect("exact materialization completion target must remain present"),
                )
            }
            MaterializationOutcome::RetryableFailure { retryable_pane_id } => {
                assert!(
                    entry.pending_materializations.iter().enumerate().all(
                        |(other_index, pending)| {
                            other_index == index || pending.pane_id() != retryable_pane_id
                        }
                    ),
                    "environment runtime placeholder pane {retryable_pane_id:?} already owns a pending materialization for {}",
                    transition.authority
                );
                let pending = entry
                    .pending_materializations
                    .get_mut(index)
                    .expect("exact retryable materialization target must remain present");
                pending.stage = PendingMaterializationStage::Queued {
                    pane_id: retryable_pane_id,
                };
                MaterializationCompletion::Applied(pending.clone())
            }
            MaterializationOutcome::CarrierMissing => {
                MaterializationCompletion::Failed(MaterializationError::CarrierMissing)
            }
        }
    }

    pub(crate) fn drain_pending_materializations(
        &mut self,
        authority: &str,
    ) -> Vec<PendingMaterialization> {
        self.entries
            .get_mut(authority)
            .map(|entry| entry.drain_pending_materializations().collect())
            .unwrap_or_default()
    }

    /// Remove pending materializations whose owning pane no longer belongs to
    /// the same environment authority.
    ///
    /// Pending requests are pane-owned runtime state. PaneGroup can close or
    /// hide-for-close a non-final pane internally and only surface a generic
    /// `AppStateChanged`, so individual close call sites are not a complete
    /// lifecycle boundary. Workspace computes the authoritative live-owner set
    /// before persistence and this method reconciles the queue in FIFO order.
    pub(crate) fn remove_orphaned_pending_materializations(
        &mut self,
        live_pane_ids_by_authority: &HashMap<String, HashSet<PaneId>>,
    ) -> Vec<(String, PendingMaterialization)> {
        let mut removed = Vec::new();
        for (authority, entry) in &mut self.entries {
            let live_pane_ids = live_pane_ids_by_authority.get(authority);
            let mut retained = VecDeque::with_capacity(entry.pending_materializations.len());
            while let Some(pending) = entry.pending_materializations.pop_front() {
                if live_pane_ids.is_some_and(|pane_ids| pane_ids.contains(&pending.pane_id())) {
                    retained.push_back(pending);
                } else {
                    removed.push((authority.clone(), pending));
                }
            }
            entry.pending_materializations = retained;
        }
        removed
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
        self.indexed_cli_agent_sessions_by_navigation_key
            .get(&Self::projection_navigation_key(authority))
            .cloned()
            .unwrap_or_default()
    }

    pub(crate) fn all_indexed_cli_agent_sessions(&self) -> Vec<WorkspaceSessionSnapshot> {
        self.indexed_cli_agent_sessions_by_navigation_key
            .values()
            .flatten()
            .cloned()
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn commit_indexed_cli_agent_sessions<E>(
        &mut self,
        authority: &str,
        scan_result: Result<Vec<WorkspaceSessionSnapshot>, E>,
    ) -> Result<(), E> {
        let sessions = scan_result?;
        self.ensure_projection_partition(authority);
        let navigation_key = Self::projection_navigation_key(authority);
        self.indexed_cli_agent_session_scan_by_navigation_key
            .remove(&navigation_key);
        self.indexed_cli_agent_sessions_by_navigation_key
            .insert(navigation_key, sessions);
        Ok(())
    }

    pub(crate) fn begin_indexed_cli_agent_session_scan(
        &mut self,
        authority: &str,
        session_id: Option<SessionId>,
    ) -> IndexedCliAgentSessionScanToken {
        self.ensure_projection_partition(authority);
        let navigation_key = Self::projection_navigation_key(authority);
        let generation = self
            .indexed_cli_agent_session_scan_by_navigation_key
            .get(&navigation_key)
            .map_or(1, |current| {
                current
                    .generation
                    .checked_add(1)
                    .expect("indexed-session scan generation exhausted")
            });
        let token = IndexedCliAgentSessionScanToken {
            navigation_key: navigation_key.clone(),
            session_id,
            generation,
            observed_agents: self
                .indexed_cli_agent_session_observed_agents_by_navigation_key
                .get(&navigation_key)
                .cloned()
                .unwrap_or_default(),
        };
        self.indexed_cli_agent_session_scan_by_navigation_key
            .insert(navigation_key, token.clone());
        token
    }

    /// 被动 lifecycle reconciliation 只能合并同一逻辑 Environment 正在执行的
    /// scan，不能以新的 token 饿死当前 worker。显式用户刷新仍会创建新 token，
    /// 因而能抢占可能停滞的旧扫描。
    pub(crate) fn has_indexed_cli_agent_session_scan_in_flight(&self, authority: &str) -> bool {
        self.indexed_cli_agent_session_scan_by_navigation_key
            .contains_key(&Self::projection_navigation_key(authority))
    }

    pub(crate) fn commit_indexed_cli_agent_session_discovery<E>(
        &mut self,
        token: IndexedCliAgentSessionScanToken,
        scan_result: Result<IndexedCliAgentSessionScanOutcome, E>,
    ) -> Result<bool, E> {
        let outcome = match scan_result {
            Ok(outcome) => outcome,
            Err(error) => {
                if self
                    .indexed_cli_agent_session_scan_by_navigation_key
                    .get(&token.navigation_key)
                    == Some(&token)
                {
                    self.indexed_cli_agent_session_scan_by_navigation_key
                        .remove(&token.navigation_key);
                }
                return Err(error);
            }
        };
        if self
            .indexed_cli_agent_session_scan_by_navigation_key
            .get(&token.navigation_key)
            != Some(&token)
        {
            return Ok(false);
        }

        match outcome {
            IndexedCliAgentSessionScanOutcome::Complete {
                observed_agents,
                sessions,
            } => {
                self.indexed_cli_agent_sessions_by_navigation_key
                    .insert(token.navigation_key.clone(), sessions);
                self.indexed_cli_agent_session_observed_agents_by_navigation_key
                    .insert(token.navigation_key.clone(), observed_agents);
            }
            IndexedCliAgentSessionScanOutcome::SourceMissing(agent) => {
                log::debug!(
                    "CLI-agent discovery source temporarily missing for {}",
                    agent.command_prefix()
                );
                // 当前 generation 不能观察到完整集合，保留全量 committed projection。
            }
            IndexedCliAgentSessionScanOutcome::Cancelled => {
                // 当前 generation 不能观察到完整集合，保留全量 committed projection。
            }
            IndexedCliAgentSessionScanOutcome::PermanentlyDeleted(agent) => {
                if let Some(sessions) = self
                    .indexed_cli_agent_sessions_by_navigation_key
                    .get_mut(&token.navigation_key)
                {
                    sessions.retain(|session| {
                        session
                            .cli_agent
                            .as_deref()
                            .map(CLIAgent::from_serialized_name)
                            != Some(agent)
                    });
                }
                if let Some(observed_agents) = self
                    .indexed_cli_agent_session_observed_agents_by_navigation_key
                    .get_mut(&token.navigation_key)
                {
                    observed_agents.remove(&agent);
                }
            }
        }
        self.indexed_cli_agent_session_scan_by_navigation_key
            .remove(&token.navigation_key);
        Ok(true)
    }

    #[cfg(test)]
    pub(crate) fn commit_indexed_cli_agent_session_scan<E>(
        &mut self,
        token: IndexedCliAgentSessionScanToken,
        scan_result: Result<Vec<WorkspaceSessionSnapshot>, E>,
    ) -> Result<bool, E> {
        let sessions = scan_result?;
        let observed_agents = sessions
            .iter()
            .filter_map(|session| session.cli_agent.as_deref())
            .map(CLIAgent::from_serialized_name)
            .filter(|agent| !matches!(agent, CLIAgent::Unknown))
            .collect();
        self.commit_indexed_cli_agent_session_discovery(
            token,
            Ok(IndexedCliAgentSessionScanOutcome::Complete {
                observed_agents,
                sessions,
            }),
        )
    }

    pub(crate) fn clear_indexed_cli_agent_sessions(&mut self, authority: &str) {
        let navigation_key = Self::projection_navigation_key(authority);
        self.indexed_cli_agent_session_scan_by_navigation_key
            .remove(&navigation_key);
        self.indexed_cli_agent_session_observed_agents_by_navigation_key
            .remove(&navigation_key);
        if let Some(sessions) = self
            .indexed_cli_agent_sessions_by_navigation_key
            .get_mut(&navigation_key)
        {
            sessions.clear();
        }
    }

    pub(crate) fn retain_indexed_cli_agent_sessions(
        &mut self,
        mut retain: impl FnMut(&WorkspaceSessionSnapshot) -> bool,
    ) {
        self.indexed_cli_agent_session_scan_by_navigation_key
            .clear();
        for sessions in self
            .indexed_cli_agent_sessions_by_navigation_key
            .values_mut()
        {
            sessions.retain(|session| retain(session));
        }
    }

    pub(crate) fn cli_agent_session_user_state(
        &self,
        authority: &str,
    ) -> EnvironmentCliAgentSessionUserState {
        self.cli_agent_session_user_state_by_navigation_key
            .get(&Self::projection_navigation_key(authority))
            .cloned()
            .unwrap_or_default()
    }

    pub(crate) fn set_cli_agent_session_user_state(
        &mut self,
        authority: String,
        state: EnvironmentCliAgentSessionUserState,
    ) {
        self.ensure_projection_partition(&authority);
        let navigation_key = Self::projection_navigation_key(&authority);
        let generation = self
            .cli_agent_session_user_state_generation_by_navigation_key
            .entry(navigation_key.clone())
            .or_default();
        *generation = generation.wrapping_add(1);
        self.cli_agent_session_user_state_by_navigation_key
            .insert(navigation_key, state);
    }

    pub(crate) fn begin_cli_agent_session_user_state_mutation(
        &mut self,
        authority: &str,
        optimistic_state: EnvironmentCliAgentSessionUserState,
    ) -> u64 {
        self.ensure_projection_partition(authority);
        let navigation_key = Self::projection_navigation_key(authority);
        let generation = self
            .cli_agent_session_user_state_generation_by_navigation_key
            .entry(navigation_key.clone())
            .or_default();
        *generation = generation.wrapping_add(1);
        self.cli_agent_session_user_state_by_navigation_key
            .insert(navigation_key, optimistic_state);
        *generation
    }

    pub(crate) fn complete_cli_agent_session_user_state_mutation(
        &mut self,
        authority: &str,
        generation: u64,
        state: EnvironmentCliAgentSessionUserState,
    ) -> bool {
        let navigation_key = Self::projection_navigation_key(authority);
        if self
            .cli_agent_session_user_state_generation_by_navigation_key
            .get(&navigation_key)
            .copied()
            != Some(generation)
        {
            return false;
        }
        self.cli_agent_session_user_state_by_navigation_key
            .insert(navigation_key, state);
        true
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
    use crate::app_state::{EnvironmentLifecycleState, WorkspaceSessionKind};
    use crate::pane_group::TerminalPaneId;

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

    fn indexed_session(id: &str, authority: &str) -> WorkspaceSessionSnapshot {
        WorkspaceSessionSnapshot {
            id: id.to_owned(),
            container_uuid: None,
            kind: WorkspaceSessionKind::AgentTerminal,
            label: Some(id.to_owned()),
            environment_authority_key: Some(authority.to_owned()),
            cwd: None,
            startup_directory: None,
            cli_agent: Some("codex".to_owned()),
            cli_command: Some("codex".to_owned()),
            cli_agent_origin: None,
            conversation_ids: Vec::new(),
            active_conversation_id: None,
            cli_agent_session_id: Some(id.to_owned()),
            is_active: false,
            is_pinned: false,
            updated_at_unix_ms: None,
            is_live_container: false,
        }
    }

    fn indexed_session_for_agent(
        id: &str,
        authority: &str,
        agent: CLIAgent,
    ) -> WorkspaceSessionSnapshot {
        let mut session = indexed_session(id, authority);
        session.cli_agent = Some(agent.to_serialized_name());
        session.cli_command = Some(agent.command_prefix().to_owned());
        session
    }

    #[test]
    fn discovery_owner_preserves_full_order_until_explicit_provider_deletion() {
        let authority = "ssh:test";
        let mut table = EnvironmentTable::default();
        table.upsert(ssh_environment(authority));
        let baseline = vec![
            indexed_session_for_agent("unrelated-claude", authority, CLIAgent::Claude),
            indexed_session_for_agent("target-codex", authority, CLIAgent::Codex),
            indexed_session_for_agent("unrelated-omp", authority, CLIAgent::Omp),
        ];
        let observed_agents = HashSet::from([CLIAgent::Claude, CLIAgent::Codex, CLIAgent::Omp]);
        let token = table.begin_indexed_cli_agent_session_scan(authority, None);
        assert!(table
            .commit_indexed_cli_agent_session_discovery(
                token,
                Ok::<_, &str>(IndexedCliAgentSessionScanOutcome::Complete {
                    observed_agents: observed_agents.clone(),
                    sessions: baseline.clone(),
                }),
            )
            .expect("complete discovery commits"));

        let token = table.begin_indexed_cli_agent_session_scan(authority, None);
        assert!(token.observed_agents().contains(&CLIAgent::Codex));
        assert!(table
            .commit_indexed_cli_agent_session_discovery(
                token,
                Ok::<_, &str>(IndexedCliAgentSessionScanOutcome::SourceMissing(
                    CLIAgent::Codex
                )),
            )
            .expect("source missing preserves committed collection"));
        assert_eq!(
            table.indexed_cli_agent_sessions_for_authority(authority),
            baseline
        );

        let token = table.begin_indexed_cli_agent_session_scan(authority, None);
        assert!(table
            .commit_indexed_cli_agent_session_discovery(
                token,
                Ok::<_, &str>(IndexedCliAgentSessionScanOutcome::Cancelled),
            )
            .expect("cancel preserves committed collection"));
        assert_eq!(
            table.indexed_cli_agent_sessions_for_authority(authority),
            baseline
        );

        let token = table.begin_indexed_cli_agent_session_scan(authority, None);
        assert!(table
            .commit_indexed_cli_agent_session_discovery(
                token,
                Ok::<_, &str>(IndexedCliAgentSessionScanOutcome::PermanentlyDeleted(
                    CLIAgent::Codex,
                )),
            )
            .expect("explicit permanent deletion commits"));
        assert_eq!(
            table
                .indexed_cli_agent_sessions_for_authority(authority)
                .into_iter()
                .map(|session| session.cli_agent_session_id.expect("fixture session id"))
                .collect::<Vec<_>>(),
            ["unrelated-claude", "unrelated-omp"],
            "only the explicitly deleted provider may leave the canonical collection",
        );
    }

    #[test]
    fn expanded_provider_failure_preserves_committed_collection() {
        let authority = "ssh:test";
        let mut table = EnvironmentTable::default();
        table.upsert(ssh_environment(authority));
        let existing = indexed_session("existing-session", authority);
        table
            .commit_indexed_cli_agent_sessions(authority, Ok::<_, &str>(vec![existing.clone()]))
            .expect("complete remote scan commits");

        let error = table
            .commit_indexed_cli_agent_sessions(
                authority,
                Err::<Vec<WorkspaceSessionSnapshot>, _>("traversal failed"),
            )
            .expect_err("failed remote scan must not commit");

        assert_eq!(error, "traversal failed");
        assert_eq!(
            table.indexed_cli_agent_sessions_for_authority(authority),
            vec![existing]
        );
    }

    #[test]
    fn stale_successful_subset_scan_preserves_canonical_session_collection() {
        let authority = "ssh:test";
        let session_id = SessionId::from(7);
        let mut table = EnvironmentTable::default();
        table.upsert(ssh_environment(authority));
        let target = indexed_session("target", authority);
        let unrelated_newer = indexed_session("unrelated-newer", authority);
        let unrelated_older = indexed_session("unrelated-older", authority);
        let baseline = vec![
            unrelated_older.clone(),
            target.clone(),
            unrelated_newer.clone(),
        ];
        table
            .commit_indexed_cli_agent_sessions(authority, Ok::<_, &str>(baseline.clone()))
            .expect("baseline scan commits");

        let stale = table.begin_indexed_cli_agent_session_scan(authority, Some(session_id));
        let current = table.begin_indexed_cli_agent_session_scan(authority, Some(session_id));
        let committed = table
            .commit_indexed_cli_agent_session_scan(
                stale,
                Ok::<_, &str>(vec![target, unrelated_older]),
            )
            .expect("stale success is rejected without becoming an error");

        assert!(!committed, "stale scan generation must not commit");
        assert_eq!(
            table.indexed_cli_agent_sessions_for_authority(authority),
            baseline,
            "an older successful subset must not shrink the canonical collection"
        );

        let committed = table
            .commit_indexed_cli_agent_session_scan(current, Ok::<_, &str>(vec![unrelated_newer]))
            .expect("current complete scan commits");
        assert!(committed);
        assert_eq!(
            table.indexed_cli_agent_sessions_for_authority(authority),
            vec![indexed_session("unrelated-newer", authority)]
        );
    }

    #[test]
    fn indexed_session_scan_in_flight_is_authority_scoped_and_clears_on_completion() {
        let local_authority = "local";
        let remote_authority = "ssh:test";
        let mut table = EnvironmentTable::default();
        table.upsert(ssh_environment(remote_authority));

        assert!(
            !table.has_indexed_cli_agent_session_scan_in_flight(local_authority),
            "an authority without a token must accept its first passive scan"
        );
        assert!(
            !table.has_indexed_cli_agent_session_scan_in_flight(remote_authority),
            "independent authorities must not inherit each other's scan state"
        );

        let local_token = table.begin_indexed_cli_agent_session_scan(local_authority, None);
        assert!(
            table.has_indexed_cli_agent_session_scan_in_flight(local_authority),
            "a pending local worker must coalesce later passive refreshes"
        );
        assert!(
            !table.has_indexed_cli_agent_session_scan_in_flight(remote_authority),
            "a local worker must not block a remote authority"
        );

        assert!(table
            .commit_indexed_cli_agent_session_discovery(
                local_token,
                Ok::<_, &str>(IndexedCliAgentSessionScanOutcome::Complete {
                    observed_agents: HashSet::new(),
                    sessions: Vec::new(),
                }),
            )
            .expect("current local completion commits"));
        assert!(
            !table.has_indexed_cli_agent_session_scan_in_flight(local_authority),
            "completion must release the authority for a later passive refresh"
        );
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

    #[test]
    fn queue_startup_command_creates_entry_when_missing() {
        let mut table = EnvironmentTable::default();
        let pane_id = TerminalPaneId::dummy_terminal_pane_id().into();
        table.queue_startup_command(
            "ssh:ssh-config:remote-fixture-primary",
            "cd /srv && codex".to_owned(),
            pane_id,
        );
        assert_eq!(
            table
                .pending_materialization("ssh:ssh-config:remote-fixture-primary")
                .and_then(|pending| match &pending.intent {
                    EnvironmentEntryIntent::StartupCommand(command) => Some(command.as_str()),
                    EnvironmentEntryIntent::PlainTerminal(_)
                    | EnvironmentEntryIntent::AgentView(_)
                    | EnvironmentEntryIntent::ForkedConversation(_)
                    | EnvironmentEntryIntent::SessionRestore(_) => None,
                }),
            Some("cd /srv && codex")
        );
        assert_eq!(
            table
                .pending_materialization("ssh:ssh-config:remote-fixture-primary")
                .map(PendingMaterialization::pane_id),
            Some(pane_id)
        );
    }

    #[test]
    fn fallback_entry_classification_uses_authority_capability_not_string_prefix() {
        let mut table = EnvironmentTable::default();
        let pane_id = TerminalPaneId::dummy_terminal_pane_id().into();
        table.queue_terminal(
            "locality:remote",
            PlainTerminalEntry::default_tab(false),
            pane_id,
        );

        let entry = table
            .entry_for_authority("locality:remote")
            .expect("queueing must create the fallback entry");
        assert_eq!(entry.snapshot.kind, EnvironmentKind::Ssh);
        assert_eq!(entry.status, EnvironmentRuntimeStatus::Dormant);
    }

    #[test]
    #[should_panic(expected = "already owns a pending materialization")]
    fn duplicate_pending_materialization_for_same_pane_is_model_violation() {
        let mut table = EnvironmentTable::default();
        let pane_id = TerminalPaneId::dummy_terminal_pane_id().into();
        table.queue_terminal(
            "ssh:example",
            PlainTerminalEntry::default_tab(false),
            pane_id,
        );
        table.queue_startup_command("ssh:example", "pwd".to_owned(), pane_id);
    }

    #[test]
    fn materialization_completion_success_consumes_only_exact_generation() {
        let authority = "ssh:example";
        let mut table = EnvironmentTable::default();
        let first_placeholder: PaneId = TerminalPaneId::dummy_terminal_pane_id().into();
        let second_placeholder: PaneId = TerminalPaneId::dummy_terminal_pane_id().into();
        let first_terminal: PaneId = TerminalPaneId::dummy_terminal_pane_id().into();
        let second_terminal: PaneId = TerminalPaneId::dummy_terminal_pane_id().into();
        table.queue_terminal(
            authority,
            PlainTerminalEntry::default_tab(false),
            first_placeholder,
        );
        table.queue_startup_command(authority, "pwd".to_owned(), second_placeholder);

        let first = table
            .begin_materialization(authority, first_placeholder, first_terminal)
            .expect("first entry must enter Delivering");
        let second = table
            .begin_materialization(authority, second_placeholder, second_terminal)
            .expect("second entry must enter Delivering");
        let stale = MaterializationTransition {
            generation: first.generation.saturating_sub(1),
            ..first.clone()
        };

        assert!(matches!(
            table.complete_materialization(stale, MaterializationOutcome::Success),
            MaterializationCompletion::Stale
        ));
        assert!(matches!(
            table.complete_materialization(first, MaterializationOutcome::Success),
            MaterializationCompletion::Applied(_)
        ));
        assert!(table
            .pending_materialization_for_pane(authority, second_terminal)
            .is_some());
        assert!(matches!(
            table.complete_materialization(second, MaterializationOutcome::Success),
            MaterializationCompletion::Applied(_)
        ));
        assert!(!table.has_pending_entry(authority));
    }

    #[test]
    fn materialization_completion_retry_cancel_and_carrier_missing_are_distinct() {
        let authority = "ssh:example";
        let mut table = EnvironmentTable::default();
        let retry_placeholder: PaneId = TerminalPaneId::dummy_terminal_pane_id().into();
        let cancelled_placeholder: PaneId = TerminalPaneId::dummy_terminal_pane_id().into();
        let retry_terminal: PaneId = TerminalPaneId::dummy_terminal_pane_id().into();
        let replacement_placeholder: PaneId = TerminalPaneId::dummy_terminal_pane_id().into();
        table.queue_terminal(
            authority,
            PlainTerminalEntry::default_tab(false),
            retry_placeholder,
        );
        table.queue_startup_command(authority, "pwd".to_owned(), cancelled_placeholder);
        let retry = table
            .begin_materialization(authority, retry_placeholder, retry_terminal)
            .expect("retry entry must enter Delivering");
        let cancelled = table
            .completion_transition_for_pane(authority, cancelled_placeholder)
            .expect("cancelled carrier must remain canonically owned");

        assert!(matches!(
            table.complete_materialization(
                retry,
                MaterializationOutcome::RetryableFailure {
                    retryable_pane_id: replacement_placeholder,
                },
            ),
            MaterializationCompletion::Applied(_)
        ));
        let pending = table
            .pending_materialization_for_pane(authority, replacement_placeholder)
            .expect("retry must preserve the original pane-owned intent");
        assert!(matches!(
            pending.intent,
            EnvironmentEntryIntent::PlainTerminal(_)
        ));

        assert!(matches!(
            table.complete_materialization(cancelled, MaterializationOutcome::Cancelled),
            MaterializationCompletion::Applied(_)
        ));
        assert!(table
            .pending_materialization_for_pane(authority, replacement_placeholder)
            .is_some());

        let carrier_missing = table
            .completion_transition_for_pane(authority, replacement_placeholder)
            .expect("retryable carrier must remain canonically owned");
        assert!(table
            .complete_materialization(carrier_missing, MaterializationOutcome::CarrierMissing)
            .is_carrier_missing());
        assert!(table
            .pending_materialization_for_pane(authority, replacement_placeholder)
            .is_some());
    }

    #[test]
    fn clear_runtime_handle_preserves_pending_intents() {
        let mut table = EnvironmentTable::default();
        let session = SessionId::from(9);
        table.mark_connecting(
            ssh_environment("ssh:example"),
            session,
            PathBuf::from("/tmp/preserve.sock"),
        );
        table.queue_terminal(
            "ssh:example",
            PlainTerminalEntry::default_tab(false),
            TerminalPaneId::dummy_terminal_pane_id().into(),
        );
        table.clear_runtime_handle("ssh:example");

        let entry = table
            .entry_for_authority("ssh:example")
            .expect("row must survive clear_runtime_handle");
        assert!(matches!(
            entry.pending_materializations.front(),
            Some(PendingMaterialization {
                intent: EnvironmentEntryIntent::PlainTerminal(_),
                ..
            })
        ));
        assert_eq!(entry.synthetic_session_id, None);
        assert_eq!(entry.status, EnvironmentRuntimeStatus::Dormant);
        assert_eq!(table.authority_for_session(session), None);
    }

    #[test]
    fn remembers_last_active_tab_by_navigation_key() {
        let mut table = EnvironmentTable::default();
        let first_tab = EntityId::new();
        let second_tab = EntityId::new();

        table.remember_active_tab("local".to_owned(), first_tab);
        assert_eq!(table.last_active_tab("local"), Some(first_tab));

        table.remember_active_tab("local".to_owned(), second_tab);
        assert_eq!(table.last_active_tab("local"), Some(second_tab));

        table.forget_navigation_context("local");
        assert_eq!(table.last_active_tab("local"), None);
    }

    #[test]
    fn local_entry_is_connected_and_excluded_from_runtime_snapshots() {
        let mut table = EnvironmentTable::default();
        let local = EnvironmentSnapshot::local(Some("/tmp/project".to_owned()));
        let authority = local.authority_key.clone();

        table.upsert(local);

        assert_eq!(
            table.lifecycle_for_authority(&authority),
            Some(EnvironmentLifecycleState::Connected)
        );
        assert!(table.runtime_snapshots().is_empty());
    }

    #[test]
    fn restored_runtime_error_snapshot_remains_error_until_explicit_reconnect() {
        let mut table = EnvironmentTable::default();
        let mut remote = ssh_environment("ssh:ssh-config:remote-fixture-relay");
        remote.lifecycle_state = EnvironmentLifecycleState::Error;
        let authority = remote.authority_key.clone();

        table.upsert(remote);

        assert_eq!(
            table.lifecycle_for_authority(&authority),
            Some(EnvironmentLifecycleState::Error),
            "persisted Error is a retry boundary, not a dormant runtime"
        );
        assert_eq!(table.session_for_authority(&authority), None);
    }

    #[test]
    fn test_environment_projection_data_uses_canonical_navigation_key() {
        let local = EnvironmentSnapshot::local(None);
        let mut local_with_root = EnvironmentSnapshot::local(Some("/tmp/project".to_owned()));
        local_with_root.authority_key = "local:/tmp/project".to_owned();
        let mut table = EnvironmentTable::default();
        table.upsert(local.clone());
        table.upsert(local_with_root.clone());

        let indexed = indexed_session("canonical-local-session", &local_with_root.authority_key);
        table
            .commit_indexed_cli_agent_sessions(
                &local_with_root.authority_key,
                Ok::<_, &str>(vec![indexed.clone()]),
            )
            .expect("local authority alias scan should commit");
        assert_eq!(
            table.indexed_cli_agent_sessions_for_authority(&local.authority_key),
            vec![indexed],
            "local authority aliases must share one indexed-session projection"
        );

        let user_state = EnvironmentCliAgentSessionUserState {
            aliases: HashMap::from([(
                "local::source:canonical-local-session".to_owned(),
                "Canonical Alias".to_owned(),
            )]),
            pinned: HashSet::from(["local::source:canonical-local-session".to_owned()]),
        };
        table.set_cli_agent_session_user_state(
            local_with_root.authority_key.clone(),
            user_state.clone(),
        );
        let observed = table.cli_agent_session_user_state(&local.authority_key);
        assert_eq!(
            observed.aliases, user_state.aliases,
            "local authority aliases must share one alias projection"
        );
        assert_eq!(
            observed.pinned, user_state.pinned,
            "local authority aliases must share one pin projection"
        );
    }

    #[test]
    fn stale_session_user_state_mutation_completion_cannot_replace_newer_projection() {
        let mut table = EnvironmentTable::default();
        let authority = "ssh:ssh-config:test";
        let first = EnvironmentCliAgentSessionUserState {
            aliases: HashMap::from([("session".to_owned(), "first".to_owned())]),
            pinned: HashSet::new(),
        };
        let second = EnvironmentCliAgentSessionUserState {
            aliases: HashMap::from([("session".to_owned(), "second".to_owned())]),
            pinned: HashSet::new(),
        };
        let stale_completion = EnvironmentCliAgentSessionUserState {
            aliases: HashMap::from([("session".to_owned(), "stale".to_owned())]),
            pinned: HashSet::new(),
        };

        let first_generation = table.begin_cli_agent_session_user_state_mutation(authority, first);
        let second_generation =
            table.begin_cli_agent_session_user_state_mutation(authority, second.clone());

        assert!(!table.complete_cli_agent_session_user_state_mutation(
            authority,
            first_generation,
            stale_completion,
        ));
        assert_eq!(
            table.cli_agent_session_user_state(authority).aliases,
            second.aliases
        );
        assert!(table.complete_cli_agent_session_user_state_mutation(
            authority,
            second_generation,
            second,
        ));
    }

    #[test]
    fn session_user_state_mutation_generations_are_partitioned_by_navigation_key() {
        let mut table = EnvironmentTable::default();
        let first_authority = "ssh:ssh-config:first";
        let second_authority = "ssh:ssh-config:second";
        let first_state = EnvironmentCliAgentSessionUserState {
            aliases: HashMap::from([("session".to_owned(), "first".to_owned())]),
            pinned: HashSet::new(),
        };
        let second_state = EnvironmentCliAgentSessionUserState {
            aliases: HashMap::from([("session".to_owned(), "second".to_owned())]),
            pinned: HashSet::new(),
        };

        let first_generation =
            table.begin_cli_agent_session_user_state_mutation(first_authority, first_state.clone());
        let second_generation = table
            .begin_cli_agent_session_user_state_mutation(second_authority, second_state.clone());

        assert!(table.complete_cli_agent_session_user_state_mutation(
            first_authority,
            first_generation,
            first_state,
        ));
        assert!(table.complete_cli_agent_session_user_state_mutation(
            second_authority,
            second_generation,
            second_state,
        ));
    }

    #[test]
    fn refreshed_session_user_state_invalidates_pending_mutation_completion() {
        let mut table = EnvironmentTable::default();
        let authority = "ssh:ssh-config:test";
        let pending_state = EnvironmentCliAgentSessionUserState {
            aliases: HashMap::from([("session".to_owned(), "pending".to_owned())]),
            pinned: HashSet::new(),
        };
        let refreshed_state = EnvironmentCliAgentSessionUserState {
            aliases: HashMap::from([("session".to_owned(), "refreshed".to_owned())]),
            pinned: HashSet::new(),
        };
        let generation =
            table.begin_cli_agent_session_user_state_mutation(authority, pending_state.clone());

        table.set_cli_agent_session_user_state(authority.to_owned(), refreshed_state.clone());

        assert!(!table.complete_cli_agent_session_user_state_mutation(
            authority,
            generation,
            pending_state,
        ));
        assert_eq!(
            table.cli_agent_session_user_state(authority).aliases,
            refreshed_state.aliases
        );
    }

    #[test]
    fn test_session_navigator_state_uses_canonical_environment_key() {
        let local = EnvironmentSnapshot::local(None);
        let mut local_with_root = EnvironmentSnapshot::local(Some("/tmp/project".to_owned()));
        local_with_root.authority_key = "local:/tmp/project".to_owned();
        let mut table = EnvironmentTable::default();
        table.upsert(local.clone());
        table.upsert(local_with_root.clone());

        table.set_active_authority(Some(local.authority_key));
        table
            .active_session_navigator_state_mut()
            .expect("canonical local navigator state")
            .selected_row_id = Some("local::agent:codex:a".to_owned());

        table.set_active_authority(Some(local_with_root.authority_key));
        assert_eq!(
            table
                .active_session_navigator_state()
                .expect("local authority alias must reuse navigator state")
                .selected_row_id
                .as_deref(),
            Some("local::agent:codex:a")
        );
    }

    #[test]
    fn test_session_navigator_state_is_partitioned_by_environment() {
        let local = EnvironmentSnapshot::local(None);
        let remote = EnvironmentSnapshot::runtime_transport(
            EnvironmentKind::Ssh,
            "test".to_string(),
            "ssh:test".to_string(),
            Some("test".to_string()),
            None,
            EnvironmentLifecycleState::Dormant,
        );
        let mut table = EnvironmentTable::default();
        table.upsert(local.clone());
        table.upsert(remote.clone());

        table.set_active_authority(Some(local.authority_key.clone()));
        let local_state = table
            .active_session_navigator_state_mut()
            .expect("local navigator state");
        local_state.selected_row_id = Some("local::agent:codex:a".to_string());
        local_state
            .display_order
            .insert("local::agent:codex:a".to_string(), 7);

        table.set_active_authority(Some(remote.authority_key.clone()));
        let remote_state = table
            .active_session_navigator_state_mut()
            .expect("remote navigator state");
        assert!(remote_state.selected_row_id.is_none());
        assert!(remote_state.display_order.is_empty());
        remote_state.selected_row_id = Some("ssh:test::agent:codex:b".to_string());
        remote_state
            .display_order
            .insert("ssh:test::agent:codex:b".to_string(), 3);

        table.set_active_authority(Some(local.authority_key));
        let restored_local = table
            .active_session_navigator_state()
            .expect("restored local navigator state");
        assert_eq!(
            restored_local.selected_row_id.as_deref(),
            Some("local::agent:codex:a")
        );
        assert_eq!(restored_local.display_order["local::agent:codex:a"], 7);
        assert!(!restored_local
            .display_order
            .contains_key("ssh:test::agent:codex:b"));
    }
}
