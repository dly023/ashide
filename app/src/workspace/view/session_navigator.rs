//! Session Navigator —— 会话导航器职责簇。
//!
//! 从 [`super`](crate::workspace::view) 抽出的会话导航器实现：会话集合装配、
//! 稳定显示顺序、刷新生命周期、别名/重命名、归档/删除，以及恢复点的激活。
//!
//! 所有条目都是 [`Workspace`] 的固有方法，作为 `view` 模块的内部拆分而存在。
//! 这些方法均由 `view` 模块（`view.rs` 及 `vertical_tabs` 等同级子模块）回调，
//! 故统一以 `pub(super)` 暴露为「view 模块内部协作 API」；唯一的交互入口
//! `show_workspace_session_context_menu` 保持 `pub`。本模块是等价结构重构的
//! 结果，不引入任何行为变更。

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use warpui::{AppContext, EntityId, SingletonEntity, UpdateView, ViewContext, ViewHandle};

use crate::ai::blocklist::history_model::BlocklistAIHistoryModel;
use crate::environment_authority::{
    session_authority_matches, session_authority_or_terminal_bootstrap, ParsedEnvironmentAuthority,
};
use crate::pane_group::EnvironmentRuntimePlaceholderPane;
use crate::session_management::SessionNavigationData;
use crate::workspace::environment_backend::{
    EnvironmentBackendKind, EnvironmentNavigationActivationIntent, EnvironmentSessionRefreshIntent,
};
use crate::workspace::environment_provider;

use super::session_navigator_reducer::{
    self, DeleteCloseKind, DeleteEffects, PaneGroupInfo, ReduceResult, SessionNavigatorAction,
    SessionNavigatorModel, SessionNavigatorState, SideEffect, TabPaneInfo,
};
const WORKSPACE_SESSIONS_REFRESH_OPERATION_KEY: &str = "workspace-sessions-refresh";

use super::{
    AgentActionSidecarSource, AlertDialogWithCallbacks, Appearance, CLIAgentInputState,
    CLIAgentSession, CLIAgentSessionContext, CLIAgentSessionStatus, CLIAgentSessionsModel,
    CLIAgentSessionsModelEvent, ContextFlag, DismissibleToast, EditorEvent, EditorView,
    EnvironmentCliAgentSessionSourceAction, MenuItem, MenuItemFields, ModalButton,
    OpenDialogSource, PaneId, PaneViewLocator, SessionBridgeActionSource, SingleLineEditorOptions,
    TabContextMenuAnchor, TerminalView, TextOptions, Vector2F, Workspace, WorkspaceAction,
    WorkspaceRegistry, WorkspaceRegistryEvent, WorkspaceSessionActionTarget,
    WorkspaceSessionAliasSubject, WorkspaceSessionKind, WorkspaceSessionSnapshot,
};

#[derive(Debug)]
pub(super) struct WorkspaceSessionDeletePlan {
    requested_session: WorkspaceSessionSnapshot,
    backing_sessions: Vec<WorkspaceSessionSnapshot>,
    cache_sessions: Vec<WorkspaceSessionSnapshot>,
    identity_keys: Vec<String>,
    alias_keys: Vec<String>,
    pin_keys: Vec<String>,
    user_state_authority: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SessionNavigatorRowIdentity {
    pub(super) row_id: String,
    pub(super) environment_navigation_key: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SessionNavigatorRenameState {
    pub(super) identity: SessionNavigatorRowIdentity,
    subject: WorkspaceSessionAliasSubject,
    authority_key: String,
}

impl SessionNavigatorRowIdentity {
    pub(super) fn for_session(
        session: &WorkspaceSessionSnapshot,
        state: &SessionNavigatorState,
    ) -> Self {
        Self {
            row_id: Workspace::workspace_session_row_id(session, state),
            environment_navigation_key: ParsedEnvironmentAuthority::parse(
                session_authority_or_terminal_bootstrap(
                    session.environment_authority_key.as_deref(),
                ),
            )
            .navigation_key()
            .to_owned(),
        }
    }

    pub(super) fn matches_session(
        &self,
        session: &WorkspaceSessionSnapshot,
        state: &SessionNavigatorState,
    ) -> bool {
        Self::for_session(session, state) == *self
    }
}

#[cfg(test)]
fn commit_complete_cli_agent_session_scan<T, E>(
    cache: &mut Vec<T>,
    scan: Result<Vec<T>, E>,
) -> Result<(), E> {
    let sessions = scan?;
    *cache = sessions;
    Ok(())
}

impl Workspace {
    pub(super) fn session_navigator_model(&self) -> SessionNavigatorModel {
        self.snapshot_session_navigator_model()
    }

    /// Render adapters that need to reconcile geometry may borrow the committed
    /// Environment-owned model directly. They must not rebuild membership or
    /// RowId state from live/provider sources in the UI hot path.
    pub(super) fn committed_session_navigator_model(&self) -> Option<&SessionNavigatorModel> {
        self.environments.active_session_navigator_model()
    }

    pub(super) fn session_navigator_preview_sessions(
        &self,
        navigation_key: &str,
    ) -> Vec<WorkspaceSessionSnapshot> {
        self.environments
            .session_navigator_model_for_navigation_key(navigation_key)
            .map(|model| model.sessions.clone())
            .unwrap_or_default()
    }

    pub(super) fn session_navigator_sessions(&self) -> Vec<WorkspaceSessionSnapshot> {
        self.session_navigator_model().sessions
    }

    pub(super) fn session_navigator_sessions_for_display_update(
        &mut self,
        ctx: &mut ViewContext<Self>,
    ) -> Vec<WorkspaceSessionSnapshot> {
        // 用户动作必须基于“屏幕上刚看到的那次 Refresh”提交状态，不能重新从空/旧
        // position state 开始。否则第一次点击 Resume 就可能把当前视觉顺序丢掉。
        let model = self.session_navigator_action_model(ctx);
        let sessions = model.sessions.clone();
        self.apply_session_navigator_reduction(
            &ReduceResult {
                sessions: model.sessions,
                state: model.state,
                side_effect: SideEffect::None,
            },
            ctx,
        );
        sessions
    }

    /// Action 层显式读取并归约最新 source；render/detail 只能读取已提交快照，禁止
    /// 通过查询路径提前发布尚未 canonical sync 的 membership、RowId 或顺序。
    fn session_navigator_action_model(&self, ctx: &AppContext) -> SessionNavigatorModel {
        let reduced = self.reduce_session_navigator_refresh(ctx);
        SessionNavigatorModel::new(reduced.sessions, reduced.state)
    }

    /// 只通过 reducer 执行 Refresh，并在同一次归约中提交当前 focus projection。
    /// typed `PaneFocused` 只计算展示态 `is_active`，不会修改 Environment selection，
    /// 因此 membership 与 focus 不再存在可被调用方错误拆开的提交分支。
    fn reduce_session_navigator_refresh(&self, ctx: &AppContext) -> ReduceResult {
        let current_authority = self.current_environment_authority_key(ctx);
        let user_state = self.workspace_session_user_state_for_authority(&current_authority);
        let indexed_sessions = self.indexed_cli_agent_sessions_for_authority(&current_authority);
        let live_sessions = self.live_workspace_sessions(ctx);
        let restored_sessions = self.restored_workspace_sessions.clone();
        let represented_conversation_ids = Self::ai_conversation_ids_from_sessions(
            live_sessions
                .iter()
                .chain(indexed_sessions.iter())
                .chain(restored_sessions.iter()),
        );
        let historical_ashide_sessions =
            Self::historical_ashide_conversation_sessions(ctx, &represented_conversation_ids);
        let sources = live_sessions
            .into_iter()
            .chain(indexed_sessions)
            .chain(restored_sessions)
            .chain(historical_ashide_sessions)
            .filter(|session| {
                Self::session_matches_current_environment(session, &current_authority)
            });
        let mut merged = WorkspaceSessionSnapshot::merge_for_session_navigator(sources);
        self.filter_deleting_workspace_sessions(&mut merged);
        self.apply_workspace_session_aliases(&mut merged, &user_state);

        let pane_info = self.snapshot_pane_group_info(ctx);
        let model = self.snapshot_session_navigator_model();
        let mut pinned_identity_keys = user_state.pinned.clone();
        for session in &merged {
            let pin_keys = self.workspace_session_pin_keys(session, ctx);
            if pin_keys.iter().any(|key| user_state.pinned.contains(key)) {
                pinned_identity_keys.extend(pin_keys);
            }
        }
        let refreshed = session_navigator_reducer::reduce(
            model.sessions,
            model.state,
            SessionNavigatorAction::Refresh {
                new_sessions: merged,
                pinned_identity_keys,
            },
            &pane_info,
        );
        let Some(focused_key) = self.logical_key_for_focused_live_pane(ctx) else {
            return refreshed;
        };
        if self.tabs.get(self.active_tab_index).is_none() {
            return refreshed;
        }
        session_navigator_reducer::reduce(
            refreshed.sessions,
            refreshed.state,
            SessionNavigatorAction::PaneFocused {
                session_logical_key: Some(focused_key),
            },
            &pane_info,
        )
    }

    /// Public entry for the command-palette session search: returns the same
    /// environment-filtered, deduplicated, merged session set the Session
    /// Navigator sidebar shows, minus display-only post-processing (active-key
    /// reselection and alias label override) that the search doesn't need.
    ///
    /// This is the fix for "sidebar-visible sessions can't be found via the
    /// title-bar search": the search previously only scanned live terminal
    /// panes (`SessionNavigationData::all_sessions` → `pane_sessions`), so
    /// restored / CLI-agent-indexed / historical Ashide conversation sessions
    /// were invisible to it even though they appear in the navigator list.
    pub fn workspace_session_snapshots_for_search(&self) -> Vec<WorkspaceSessionSnapshot> {
        self.session_navigator_sessions()
    }

    #[cfg(test)]
    pub(super) fn workspace_session_display_order_key(
        session: &WorkspaceSessionSnapshot,
    ) -> String {
        Self::workspace_session_logical_key(session)
    }

    fn workspace_session_durable_display_order_key(
        session: &WorkspaceSessionSnapshot,
    ) -> Option<String> {
        session.durable_identity_key()
    }

    pub(super) fn workspace_session_identity_keys(
        session: &WorkspaceSessionSnapshot,
    ) -> Vec<String> {
        session.observed_identity_keys()
    }

    fn workspace_session_volatile_identity_keys(session: &WorkspaceSessionSnapshot) -> Vec<String> {
        let mut keys = Vec::new();
        if session.id.starts_with("tab:") {
            keys.push(session.id.clone());
        }
        let logical_key = Self::workspace_session_logical_key(session);
        if logical_key.contains("::source:tab:") {
            keys.push(logical_key);
        }
        keys.sort();
        keys.dedup();
        keys
    }

    fn workspace_session_identity_keys_for_sessions(
        sessions: &[WorkspaceSessionSnapshot],
    ) -> Vec<String> {
        let mut keys = sessions
            .iter()
            .flat_map(Self::workspace_session_identity_keys)
            .collect::<Vec<_>>();
        keys.sort();
        keys.dedup();
        keys
    }

    pub(super) fn workspace_session_row_id(
        session: &WorkspaceSessionSnapshot,
        state: &SessionNavigatorState,
    ) -> String {
        Self::workspace_session_identity_keys(session)
            .into_iter()
            .find_map(|identity| state.row_id_by_identity.get(&identity).cloned())
            .unwrap_or_else(|| Self::workspace_session_logical_key(session))
    }

    pub(super) fn session_navigator_row_id_for_identity(
        identity: &str,
        state: &SessionNavigatorState,
    ) -> String {
        state
            .row_id_by_identity
            .get(identity)
            .cloned()
            .unwrap_or_else(|| identity.to_owned())
    }

    pub(super) fn filter_deleting_workspace_sessions(
        &self,
        sessions: &mut Vec<WorkspaceSessionSnapshot>,
    ) {
        let state = self.snapshot_session_navigator_state();
        if state.deleting_row_ids.is_empty() && state.deleted_row_ids.is_empty() {
            return;
        }
        sessions.retain(|session| {
            let row_id = Self::workspace_session_row_id(session, &state);
            !state.deleting_row_ids.contains(&row_id) && !state.deleted_row_ids.contains(&row_id)
        });
    }

    fn tab_is_same_window_split_group(&self, tab_index: usize, ctx: &AppContext) -> bool {
        self.tabs
            .get(tab_index)
            .is_some_and(|tab| tab.pane_group.as_ref(ctx).visible_pane_ids().len() > 1)
    }

    pub(super) fn same_window_split_group_numbers_for_sessions(
        &self,
        sessions: &[WorkspaceSessionSnapshot],
        ctx: &AppContext,
    ) -> HashMap<usize, usize> {
        let mut group_numbers = HashMap::new();
        let mut next_group_number = 1;

        for session in sessions {
            let Some((tab_index, _)) = Self::locator_from_restored_session_id(&session.id) else {
                continue;
            };
            if group_numbers.contains_key(&tab_index) {
                continue;
            }
            if !self.tab_is_same_window_split_group(tab_index, ctx) {
                continue;
            }

            group_numbers.insert(tab_index, next_group_number);
            next_group_number += 1;
        }

        group_numbers
    }

    pub(super) fn same_window_split_group_number_for_tab(
        &self,
        tab_index: usize,
        ctx: &AppContext,
    ) -> Option<usize> {
        if !self.tab_is_same_window_split_group(tab_index, ctx) {
            return None;
        }

        let sessions = self.session_navigator_sessions();
        self.same_window_split_group_numbers_for_sessions(&sessions, ctx)
            .get(&tab_index)
            .copied()
    }

    pub(super) fn workspace_session_same_window_split_group_number(
        &self,
        session: &WorkspaceSessionSnapshot,
        group_numbers: &HashMap<usize, usize>,
    ) -> Option<usize> {
        let Some((tab_index, _)) = Self::locator_from_restored_session_id(&session.id) else {
            return None;
        };
        group_numbers.get(&tab_index).copied()
    }

    // preferred_active_* 双轨已删除：Refresh identity reconcile 独占 §6.4。

    #[cfg(test)]
    pub(super) fn normalize_session_navigator_active_state(
        sessions: &mut [WorkspaceSessionSnapshot],
        preferred_selected_row_id: Option<&str>,
    ) {
        if sessions
            .iter()
            .filter(|session| session.is_active)
            .take(2)
            .count()
            <= 1
        {
            return;
        }

        let preferred_key = preferred_selected_row_id
            .filter(|key| {
                sessions.iter().any(|session| {
                    session.is_active && Self::workspace_session_logical_key(session) == *key
                })
            })
            .map(str::to_owned)
            .or_else(|| {
                sessions
                    .iter()
                    .find(|session| session.is_active)
                    .map(Self::workspace_session_logical_key)
            });

        let Some(preferred_key) = preferred_key else {
            return;
        };

        for session in sessions {
            session.is_active =
                session.is_active && Self::workspace_session_logical_key(session) == preferred_key;
        }
    }

    pub(super) fn is_restoring_workspace_session(
        &self,
        session: &WorkspaceSessionSnapshot,
    ) -> bool {
        let state = self.snapshot_session_navigator_state();
        state
            .restoring_row_ids
            .contains(&Self::workspace_session_row_id(session, &state))
    }

    pub(super) fn workspace_session_logical_key(session: &WorkspaceSessionSnapshot) -> String {
        session.logical_key()
    }

    pub(super) fn is_same_workspace_session(
        left: &WorkspaceSessionSnapshot,
        right: &WorkspaceSessionSnapshot,
    ) -> bool {
        if Self::workspace_session_logical_key(left) == Self::workspace_session_logical_key(right) {
            return true;
        }

        Self::workspace_session_durable_display_order_key(left).is_some_and(|left_key| {
            Self::workspace_session_durable_display_order_key(right)
                .is_some_and(|right_key| left_key == right_key)
        })
    }

    fn workspace_session_binding_source_identity_keys(
        &self,
        session: &WorkspaceSessionSnapshot,
        ctx: &AppContext,
    ) -> Vec<String> {
        let Some(locator) = self.locator_for_workspace_session_snapshot(session, ctx) else {
            return Vec::new();
        };
        self.tabs
            .iter()
            .find(|tab| tab.pane_group.id() == locator.pane_group_id)
            .and_then(|tab| {
                tab.pane_group
                    .as_ref(ctx)
                    .session_binding_for_pane_id(locator.pane_id, ctx)
            })
            .map(|binding| binding.source_identity_keys().to_vec())
            .unwrap_or_default()
    }

    pub(super) fn workspace_session_pin_keys(
        &self,
        session: &WorkspaceSessionSnapshot,
        ctx: &AppContext,
    ) -> Vec<String> {
        let mut keys = session.stable_pin_keys();
        keys.extend(self.workspace_session_binding_source_identity_keys(session, ctx));
        keys.sort();
        keys.dedup();
        keys
    }

    pub(super) fn workspace_session_alias_keys_for_session(
        session: &WorkspaceSessionSnapshot,
    ) -> Vec<String> {
        session
            .alias_subject()
            .user_state_key()
            .map(|key| vec![key.to_owned()])
            .unwrap_or_default()
    }

    pub(super) fn workspace_session_alias(
        &self,
        session: &WorkspaceSessionSnapshot,
    ) -> Option<String> {
        let authority =
            session_authority_or_terminal_bootstrap(session.environment_authority_key.as_deref());
        let user_state = self.workspace_session_user_state_for_authority(authority);
        self.workspace_session_alias_with_state(session, &user_state)
    }

    pub(super) fn workspace_session_alias_with_state(
        &self,
        session: &WorkspaceSessionSnapshot,
        user_state: &crate::workspace::environment_runtime::EnvironmentCliAgentSessionUserState,
    ) -> Option<String> {
        session
            .alias_subject()
            .user_state_key()
            .and_then(|key| user_state.aliases.get(key).cloned())
    }

    pub(super) fn apply_workspace_session_aliases(
        &self,
        sessions: &mut [WorkspaceSessionSnapshot],
        user_state: &crate::workspace::environment_runtime::EnvironmentCliAgentSessionUserState,
    ) {
        for session in sessions {
            if let Some(alias) = self.workspace_session_alias_with_state(session, user_state) {
                session.label = Some(alias);
            }
        }
    }

    pub(super) fn workspace_session_user_state_for_authority(
        &self,
        authority: &str,
    ) -> crate::workspace::environment_runtime::EnvironmentCliAgentSessionUserState {
        crate::workspace::environment_backend::EnvironmentBackendKind::for_authority(authority)
            .backend()
            .session_user_state(self, authority)
    }

    pub(super) fn local_cli_agent_session_aliases() -> HashMap<String, String> {
        crate::terminal::cli_agent_session_index::session_aliases()
    }

    pub(super) fn try_scan_terminal_cli_agent_session_discovery(
        enabled_agents: Vec<crate::terminal::CLIAgent>,
        previously_observed_agents: &std::collections::HashSet<crate::terminal::CLIAgent>,
        scope_paths: Vec<PathBuf>,
    ) -> Result<
        (
            crate::workspace::environment_table::IndexedCliAgentSessionScanOutcome,
            crate::workspace::environment_runtime::EnvironmentCliAgentSessionUserState,
        ),
        String,
    > {
        let outcome = crate::terminal::cli_agent_session_index::try_scan_current_app_cli_agent_session_discovery(
            crate::app_state::WORKSPACE_SESSION_NAVIGATOR_LOGICAL_LIMIT,
            enabled_agents,
            previously_observed_agents,
            scope_paths,
        )
        .map_err(|error| error.to_string())?;
        let user_state =
            crate::workspace::environment_runtime::EnvironmentCliAgentSessionUserState {
                aliases: crate::terminal::cli_agent_session_index::session_aliases(),
                pinned: crate::terminal::cli_agent_session_index::pinned_session_ids(),
            };
        Ok((outcome, user_state))
    }

    pub(super) fn backing_sessions_for_workspace_session(
        &self,
        session: &WorkspaceSessionSnapshot,
    ) -> Vec<WorkspaceSessionSnapshot> {
        let mut seen = HashSet::new();
        let mut sessions = Vec::new();
        let indexed_environment_sessions = self.all_indexed_environment_cli_agent_sessions();
        for candidate in indexed_environment_sessions
            .iter()
            .chain(self.restored_workspace_sessions.iter())
        {
            if Self::is_same_workspace_session(session, candidate)
                && seen.insert(candidate.id.clone())
            {
                sessions.push(candidate.clone());
            }
        }
        if seen.insert(session.id.clone()) {
            sessions.push(session.clone());
        }
        sessions
    }

    pub(super) fn pinned_cli_agent_session_ids() -> HashSet<String> {
        crate::terminal::cli_agent_session_index::pinned_session_ids()
    }

    pub(super) fn cli_agent_history_source_session_for_workspace_session(
        &self,
        session: &WorkspaceSessionSnapshot,
    ) -> Option<WorkspaceSessionSnapshot> {
        self.backing_sessions_for_workspace_session(session)
            .into_iter()
            .find(Self::workspace_session_can_fork_cli_agent_history)
    }

    pub(super) fn workspace_session_can_fork_cli_agent_history_with_backing(
        &self,
        session: &WorkspaceSessionSnapshot,
    ) -> bool {
        self.cli_agent_history_source_session_for_workspace_session(session)
            .is_some()
    }

    pub(super) fn mutate_workspace_session_user_state_for_authority(
        &mut self,
        authority: &str,
        keys: &[String],
        mutation: crate::workspace::environment_runtime::EnvironmentCliAgentSessionUserStateMutation,
        feedback: crate::workspace::environment_backend::SessionUserStateMutationFeedback,
        ctx: &mut ViewContext<Self>,
    ) -> Result<(), String> {
        let keys = keys
            .iter()
            .map(|key| key.trim().to_owned())
            .filter(|key| !key.is_empty())
            .collect::<Vec<_>>();
        if keys.is_empty() {
            return Err("session user-state mutation has no keys".to_owned());
        }
        if let Some(volatile_key) = keys
            .iter()
            .find(|key| WorkspaceSessionSnapshot::is_volatile_layout_identity_key(key))
        {
            return Err(format!(
                "session user-state mutation rejected volatile layout key: {volatile_key}"
            ));
        }

        self.environments.entry_target_snapshot(authority);
        let previous_state = self.environments.cli_agent_session_user_state(authority);
        let mut optimistic_state = previous_state.clone();
        Self::apply_workspace_session_user_state_mutation(&mut optimistic_state, &keys, &mutation);
        let generation = self
            .environments
            .begin_cli_agent_session_user_state_mutation(authority, optimistic_state);

        let delivery =
            crate::workspace::environment_backend::EnvironmentBackendKind::for_authority(authority)
                .backend()
                .mutate_session_user_state(
                    self,
                    authority,
                    generation,
                    feedback,
                    keys,
                    mutation,
                    previous_state.clone(),
                    ctx,
                );

        match delivery {
            Ok(
                crate::workspace::environment_backend::SessionUserStateMutationDelivery::Applied(
                    state,
                ),
            ) => {
                self.environments
                    .complete_cli_agent_session_user_state_mutation(authority, generation, state);
                if let Some(message) = feedback.success_message() {
                    self.show_workspace_session_success_toast(message.to_owned(), ctx);
                }
                Ok(())
            }
            Ok(
                crate::workspace::environment_backend::SessionUserStateMutationDelivery::Pending,
            ) => Ok(()),
            Err(error) => {
                self.environments
                    .complete_cli_agent_session_user_state_mutation(
                        authority,
                        generation,
                        previous_state,
                    );
                Err(error)
            }
        }
    }

    pub(super) fn mutate_local_workspace_session_user_state(
        keys: &[String],
        mutation: crate::workspace::environment_runtime::EnvironmentCliAgentSessionUserStateMutation,
    ) -> Result<(), String> {
        match mutation {
            crate::workspace::environment_runtime::EnvironmentCliAgentSessionUserStateMutation::SetAlias(alias) => {
                crate::terminal::cli_agent_session_index::mutate_session_user_state(
                    keys,
                    Some(Some(&alias)),
                    None,
                )?;
            }
            crate::workspace::environment_runtime::EnvironmentCliAgentSessionUserStateMutation::ClearAlias => {
                crate::terminal::cli_agent_session_index::mutate_session_user_state(
                    keys,
                    Some(None),
                    None,
                )?;
            }
            crate::workspace::environment_runtime::EnvironmentCliAgentSessionUserStateMutation::SetPinned => {
                crate::terminal::cli_agent_session_index::mutate_session_user_state(
                    keys,
                    None,
                    Some(true),
                )?;
            }
            crate::workspace::environment_runtime::EnvironmentCliAgentSessionUserStateMutation::ClearPinned => {
                crate::terminal::cli_agent_session_index::mutate_session_user_state(
                    keys,
                    None,
                    Some(false),
                )?;
            }
        }
        Ok(())
    }

    pub(super) fn apply_workspace_session_user_state_mutation(
        state: &mut crate::workspace::environment_runtime::EnvironmentCliAgentSessionUserState,
        keys: &[String],
        mutation: &crate::workspace::environment_runtime::EnvironmentCliAgentSessionUserStateMutation,
    ) {
        match mutation {
            crate::workspace::environment_runtime::EnvironmentCliAgentSessionUserStateMutation::SetAlias(alias) => {
                let alias = alias.trim();
                if alias.is_empty() {
                    for key in keys {
                        state.aliases.remove(key);
                    }
                } else {
                    for key in keys {
                        state.aliases.insert(key.clone(), alias.to_owned());
                    }
                }
            }
            crate::workspace::environment_runtime::EnvironmentCliAgentSessionUserStateMutation::ClearAlias => {
                for key in keys {
                    state.aliases.remove(key);
                }
            }
            crate::workspace::environment_runtime::EnvironmentCliAgentSessionUserStateMutation::SetPinned => {
                state.pinned.extend(keys.iter().cloned());
            }
            crate::workspace::environment_runtime::EnvironmentCliAgentSessionUserStateMutation::ClearPinned => {
                for key in keys {
                    state.pinned.remove(key);
                }
            }
        }
    }

    // 仅 non-local_fs / wasm 构建经由同步路径调用；local_fs 构建走
    // delete_workspace_session 的异步 `session_source_mutation_for_backing` 路径。
    #[cfg(any(not(feature = "local_fs"), target_family = "wasm"))]
    pub(super) fn delete_terminal_cli_agent_session_source(session_id: &str) -> Result<(), String> {
        crate::terminal::cli_agent_session_index::delete_current_app_cli_agent_session(session_id)
    }

    // ── 会话别名编辑（构造器 / 事件分发 / 重命名业务）─────────────────────

    pub(super) fn workspace_session_title_editor(
        ctx: &mut ViewContext<Self>,
    ) -> ViewHandle<EditorView> {
        let editor = ctx.add_typed_action_view(|ctx| {
            let appearance = Appearance::as_ref(ctx);
            let options = SingleLineEditorOptions {
                text: TextOptions::ui_text(Some(12.), appearance),
                select_all_on_focus: true,
                clear_selections_on_blur: true,
                ..Default::default()
            };
            EditorView::single_line(options, ctx)
        });
        ctx.subscribe_to_view(&editor, move |me, _, event, ctx| {
            me.handle_workspace_session_title_editor_event(event, ctx);
        });
        editor
    }

    pub(super) fn handle_workspace_session_title_editor_event(
        &mut self,
        event: &EditorEvent,
        ctx: &mut ViewContext<Self>,
    ) {
        if self.renaming_workspace_session.is_some() {
            match event {
                EditorEvent::Blurred | EditorEvent::Enter => {
                    self.finish_workspace_session_rename(ctx);
                }
                EditorEvent::Escape => {
                    self.cancel_workspace_session_rename(ctx);
                }
                _ => {}
            }
        }
    }

    pub(super) fn request_rename_workspace_session(
        &mut self,
        target: &WorkspaceSessionActionTarget,
        ctx: &mut ViewContext<Self>,
    ) {
        let Some(session) = self.workspace_session_for_action_target(target, ctx) else {
            log::warn!(
                "request_rename_workspace_session: missing session {} in {:?}",
                target.session_id,
                target.environment_authority_key
            );
            self.show_workspace_session_error_toast("会话不存在，已刷新后请重试".to_owned(), ctx);
            return;
        };

        let initial_alias = self
            .workspace_session_alias(&session)
            .unwrap_or_else(|| Self::workspace_session_label(&session));
        let state = self.snapshot_session_navigator_state();
        self.renaming_workspace_session = Some(SessionNavigatorRenameState {
            identity: SessionNavigatorRowIdentity::for_session(&session, &state),
            subject: session.alias_subject(),
            authority_key: session_authority_or_terminal_bootstrap(
                session.environment_authority_key.as_deref(),
            )
            .to_owned(),
        });
        self.workspace_session_title_editor
            .update(ctx, |editor, ctx| {
                editor.clear_buffer_and_reset_undo_stack(ctx);
                editor.set_buffer_text(&initial_alias, ctx);
                editor.select_all(ctx);
            });
        ctx.focus(&self.workspace_session_title_editor);
        ctx.notify();
    }

    pub(super) fn finish_workspace_session_rename(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(rename) = self.renaming_workspace_session.take() else {
            return;
        };
        let alias = self
            .workspace_session_title_editor
            .as_ref(ctx)
            .buffer_text(ctx)
            .trim()
            .to_owned();
        self.clear_workspace_session_title_editor(ctx);

        let Some(session) = self.workspace_session_for_alias_subject(&rename.subject) else {
            log::warn!(
                "finish_workspace_session_rename: missing subject {} from row {} in {:?}",
                rename.subject.key(),
                rename.identity.row_id,
                rename.identity.environment_navigation_key
            );
            self.sync_session_navigator_sessions(ctx);
            ctx.notify();
            return;
        };
        match &rename.subject {
            WorkspaceSessionAliasSubject::Container(_) => {
                let Some(locator) = self.locator_for_workspace_session_snapshot(&session, ctx)
                else {
                    log::warn!(
                        "finish_workspace_session_rename: container {} has no current pane locator",
                        rename.subject.key()
                    );
                    self.sync_session_navigator_sessions(ctx);
                    ctx.notify();
                    return;
                };
                self.set_custom_pane_name(locator, alias, ctx);
                self.sync_session_navigator_sessions(ctx);
                ctx.notify();
                return;
            }
            WorkspaceSessionAliasSubject::DurableSession(_)
            | WorkspaceSessionAliasSubject::VirtualSource(_) => {}
        }

        let keys = vec![rename.subject.key().to_owned()];
        let mutation = if alias.is_empty() {
            crate::workspace::environment_runtime::EnvironmentCliAgentSessionUserStateMutation::ClearAlias
        } else {
            crate::workspace::environment_runtime::EnvironmentCliAgentSessionUserStateMutation::SetAlias(
                alias.clone(),
            )
        };
        let result = self
            .mutate_workspace_session_user_state_for_authority(
                &rename.authority_key,
                &keys,
                mutation,
                if alias.is_empty() {
                    crate::workspace::environment_backend::SessionUserStateMutationFeedback::AliasCleared
                } else {
                    crate::workspace::environment_backend::SessionUserStateMutationFeedback::AliasSet
                },
                ctx,
            );

        match result {
            Ok(_) => {
                self.sync_session_navigator_sessions(ctx);
            }
            Err(error) => {
                log::warn!("finish_workspace_session_rename: {error}");
                let feedback = if alias.is_empty() {
                    crate::workspace::environment_backend::SessionUserStateMutationFeedback::AliasCleared
                } else {
                    crate::workspace::environment_backend::SessionUserStateMutationFeedback::AliasSet
                };
                self.show_workspace_session_error_toast(feedback.error_message(&error), ctx);
            }
        }
        ctx.notify();
    }

    pub(super) fn cancel_workspace_session_rename(&mut self, ctx: &mut ViewContext<Self>) {
        if self.renaming_workspace_session.take().is_some() {
            self.clear_workspace_session_title_editor(ctx);
            ctx.notify();
        }
    }

    pub(super) fn clear_workspace_session_title_editor(&mut self, ctx: &mut ViewContext<Self>) {
        self.workspace_session_title_editor
            .update(ctx, |editor, ctx| {
                editor.clear_buffer_and_reset_undo_stack(ctx)
            });
    }

    pub(super) fn reset_workspace_session_name(
        &mut self,
        target: &WorkspaceSessionActionTarget,
        ctx: &mut ViewContext<Self>,
    ) {
        let Some(session) = self.workspace_session_for_action_target(target, ctx) else {
            log::warn!(
                "reset_workspace_session_name: missing session {} in {:?}",
                target.session_id,
                target.environment_authority_key
            );
            self.show_workspace_session_error_toast("会话不存在，已刷新后请重试".to_owned(), ctx);
            return;
        };

        let subject = session.alias_subject();
        match &subject {
            WorkspaceSessionAliasSubject::Container(_) => {
                let Some(locator) = self.locator_for_workspace_session_snapshot(&session, ctx)
                else {
                    log::warn!(
                        "reset_workspace_session_name: container has no current pane locator"
                    );
                    return;
                };
                self.clear_pane_name(locator, ctx);
                self.sync_session_navigator_sessions(ctx);
                ctx.notify();
                return;
            }
            WorkspaceSessionAliasSubject::DurableSession(_)
            | WorkspaceSessionAliasSubject::VirtualSource(_) => {}
        }

        let keys = vec![subject.key().to_owned()];
        let authority =
            session_authority_or_terminal_bootstrap(session.environment_authority_key.as_deref())
                .to_owned();
        match self.mutate_workspace_session_user_state_for_authority(
            &authority,
            &keys,
            crate::workspace::environment_runtime::EnvironmentCliAgentSessionUserStateMutation::ClearAlias,
            crate::workspace::environment_backend::SessionUserStateMutationFeedback::AliasCleared,
            ctx,
        ) {
            Ok(_) => {}
            Err(error) => {
                log::warn!("reset_workspace_session_name: {error}");
                self.show_workspace_session_error_toast(format!("清除会话别名失败：{error}"), ctx);
                return;
            }
        }
        let state = self.snapshot_session_navigator_state();
        if self
            .renaming_workspace_session
            .as_ref()
            .is_some_and(|rename| {
                rename.identity.matches_session(&session, &state) || rename.subject == subject
            })
        {
            self.cancel_workspace_session_rename(ctx);
        }
        self.sync_session_navigator_sessions(ctx);
        ctx.notify();
    }

    /// SSTAB-007 discoverability: copies the session's stable identifier to
    /// the clipboard so users can reference it externally. Prefers the CLI
    /// agent session id when present; otherwise falls back to the logical
    /// session id.
    pub(super) fn copy_workspace_session_id(
        &mut self,
        target: &WorkspaceSessionActionTarget,
        ctx: &mut ViewContext<Self>,
    ) {
        let Some(session) = self.workspace_session_for_action_target(target, ctx) else {
            log::warn!(
                "copy_workspace_session_id: missing session {} in {:?}",
                target.session_id,
                target.environment_authority_key
            );
            return;
        };
        let id_to_copy = session
            .cli_agent_session_id
            .clone()
            .unwrap_or_else(|| session.id.clone());
        ctx.clipboard()
            .write(warpui::clipboard::ClipboardContent::plain_text(
                id_to_copy.clone(),
            ));
        self.show_workspace_session_success_toast(
            crate::t!(
                "workspace-session-navigator-menu-copy-id-toast",
                id = id_to_copy.as_str()
            ),
            ctx,
        );
        ctx.notify();
    }

    // ── 模型事件同步 · 活动/恢复中状态 · 操作目标解析 ──────────────────────

    pub(super) fn handle_cli_agent_sessions_event(
        &mut self,
        event: &CLIAgentSessionsModelEvent,
        ctx: &mut ViewContext<Self>,
    ) {
        if matches!(
            event,
            CLIAgentSessionsModelEvent::Started { .. }
                | CLIAgentSessionsModelEvent::StatusChanged { .. }
                | CLIAgentSessionsModelEvent::Ended { .. }
                | CLIAgentSessionsModelEvent::SessionUpdated { .. }
        ) && self.workspace_contains_terminal_view(event.terminal_view_id(), ctx)
        {
            let persists_binding = matches!(
                event,
                CLIAgentSessionsModelEvent::Started { .. }
                    | CLIAgentSessionsModelEvent::SessionUpdated { .. }
            );
            if persists_binding
                && !self.refresh_terminal_pane_session_binding(event.terminal_view_id(), ctx)
            {
                log::error!(
                    "CLI agent event belongs to this workspace but its terminal pane binding owner is missing: {:?}",
                    event.terminal_view_id()
                );
                return;
            }
            self.sync_session_navigator_sessions(ctx);
            if persists_binding {
                // Provider binding 是 terminal pane 的持久身份。只刷新 UI 而不保存
                // 会让下次冷启动再次丢失 session id，别名便只能等 Resume 后出现。
                ctx.dispatch_global_action("workspace:save_app", ());
            }
            ctx.notify();
        }
    }

    pub(super) fn sync_session_navigator_sessions(&mut self, ctx: &mut ViewContext<Self>) {
        // Single committed projection path: materialization + display_order + restoring
        // cleanup + current focus. PaneFocused does not mutate Environment selection, so
        // callers cannot accidentally publish membership while clearing the active row.
        // Restore 的 source 可能先被消费、live identity 后注册；Refresh 前禁止按瞬时
        // source 集合 prune，否则会把合法的 in-flight selection 变成 stale state。
        if let Some(terminal_view) = self
            .active_tab_pane_group()
            .as_ref(ctx)
            .focused_session_view(ctx)
        {
            self.refresh_terminal_pane_session_binding(terminal_view.id(), ctx);
        }
        let reduced = self.reduce_session_navigator_refresh(ctx);
        self.apply_session_navigator_reduction(&reduced, ctx);
        ctx.notify();
    }

    pub(super) fn clear_session_navigator_selection_key_if_present(
        &mut self,
        key: Option<String>,
        ctx: &mut ViewContext<Self>,
    ) {
        let state = self.snapshot_session_navigator_state();
        let should_clear = key.as_deref().is_some_and(|identity| {
            state.selected_row_id.as_deref()
                == Some(Self::session_navigator_row_id_for_identity(identity, &state).as_str())
        });
        if should_clear {
            self.dispatch_session_navigator_state_action(
                SessionNavigatorAction::SelectionChanged {
                    session_logical_key: None,
                },
                ctx,
            );
            self.sync_session_navigator_sessions(ctx);
        }
    }

    pub(super) fn session_navigator_selection_key_for_tab(
        &self,
        tab_index: usize,
        ctx: &AppContext,
    ) -> Option<String> {
        let state = self.snapshot_session_navigator_state();
        let session_navigator_selection_key = state.selected_row_id.as_deref()?;
        self.live_workspace_sessions(ctx)
            .into_iter()
            .filter(|session| {
                Self::locator_from_restored_session_id(&session.id)
                    .is_some_and(|(session_tab_index, _)| session_tab_index == tab_index)
            })
            .filter_map(|session| {
                let logical_key = Self::workspace_session_logical_key(&session);
                (session_navigator_selection_key
                    == Self::workspace_session_row_id(&session, &state))
                .then_some(logical_key)
            })
            .next()
    }

    pub(super) fn session_navigator_selection_key_for_pane(
        &self,
        pane_group_id: warpui::EntityId,
        pane_id: PaneId,
        ctx: &AppContext,
    ) -> Option<String> {
        let state = self.snapshot_session_navigator_state();
        let session_navigator_selection_key = state.selected_row_id.as_deref()?;
        self.live_workspace_sessions(ctx)
            .into_iter()
            .filter(|session| {
                self.locator_for_workspace_session_snapshot(session, ctx)
                    .is_some_and(|locator| {
                        locator.pane_group_id == pane_group_id && locator.pane_id == pane_id
                    })
            })
            .filter_map(|session| {
                let logical_key = Self::workspace_session_logical_key(&session);
                (session_navigator_selection_key
                    == Self::workspace_session_row_id(&session, &state))
                .then_some(logical_key)
            })
            .next()
    }

    pub(super) fn clear_session_navigator_selection_if_matches(
        &mut self,
        session: &WorkspaceSessionSnapshot,
        ctx: &mut ViewContext<Self>,
    ) {
        let state = self.snapshot_session_navigator_state();
        let should_clear = state.selected_row_id.as_deref()
            == Some(Self::workspace_session_row_id(session, &state).as_str());
        if should_clear {
            self.dispatch_session_navigator_state_action(
                SessionNavigatorAction::SelectionChanged {
                    session_logical_key: None,
                },
                ctx,
            );
            self.sync_session_navigator_sessions(ctx);
        }
    }

    pub(super) fn workspace_session_action_target(
        session: &WorkspaceSessionSnapshot,
    ) -> WorkspaceSessionActionTarget {
        WorkspaceSessionActionTarget::new(
            session.id.clone(),
            session.environment_authority_key.clone(),
        )
    }

    pub(super) fn workspace_session_matches_action_target(
        session: &WorkspaceSessionSnapshot,
        target: &WorkspaceSessionActionTarget,
    ) -> bool {
        if session.id != target.session_id {
            return false;
        }

        session_authority_matches(
            session.environment_authority_key.as_deref(),
            &target.environment_authority_key,
        )
    }

    pub(super) fn workspace_session_for_action_target(
        &self,
        target: &WorkspaceSessionActionTarget,
        ctx: &AppContext,
    ) -> Option<WorkspaceSessionSnapshot> {
        self.session_navigator_action_model(ctx)
            .sessions
            .into_iter()
            .find(|session| Self::workspace_session_matches_action_target(session, target))
    }

    fn workspace_session_for_alias_subject(
        &self,
        subject: &WorkspaceSessionAliasSubject,
    ) -> Option<WorkspaceSessionSnapshot> {
        self.session_navigator_model()
            .sessions
            .into_iter()
            .find(|session| match subject {
                WorkspaceSessionAliasSubject::DurableSession(key) => {
                    session.durable_identity_key().as_deref() == Some(key)
                }
                WorkspaceSessionAliasSubject::Container(key)
                | WorkspaceSessionAliasSubject::VirtualSource(key) => session.logical_key() == *key,
            })
    }

    // ── 实时会话快照（来自当前窗口各 tab 的 pane group）─────────────────────

    /// Session Navigator 的 local membership 只接受带有原生 agent 或 Ashide
    /// conversation 语义的 live pane。普通 local shell 仍是正常的工作区 tab，
    /// 但不是可恢复历史；runtime Environment 的 terminal 则保留原有投影语义。
    fn session_navigator_live_session_is_member(
        environment: &crate::app_state::EnvironmentSnapshot,
        session: &WorkspaceSessionSnapshot,
    ) -> bool {
        if !ParsedEnvironmentAuthority::parse(&environment.authority_key).uses_terminal_bootstrap()
        {
            return true;
        }

        match session.kind {
            WorkspaceSessionKind::AgentTerminal => true,
            WorkspaceSessionKind::Terminal
            | WorkspaceSessionKind::Welcome
            | WorkspaceSessionKind::Other => false,
        }
    }

    /// Navigator live projection. Reads PaneConfiguration / tab Environment only.
    /// Must not call `PaneGroup::snapshot` or walk TerminalPane LeafContents — those
    /// lock every TerminalModel and belong to persistence, not Session Navigator.
    pub(super) fn live_workspace_sessions(
        &self,
        ctx: &AppContext,
    ) -> Vec<WorkspaceSessionSnapshot> {
        let mut sessions = Vec::new();
        for (tab_index, tab) in self.tabs.iter().enumerate() {
            let pane_group = tab.pane_group.as_ref(ctx);
            let tab_environment = tab.environment.clone().unwrap_or_else(|| {
                crate::workspace::environment_runtime::terminal_bootstrap_environment(None)
            });
            let tab_requires_runtime_sessions =
                ParsedEnvironmentAuthority::parse(&tab_environment.authority_key)
                    .uses_runtime_environment();
            let focused_pane_index = if tab_index == self.active_tab_index {
                let focused_pane_id = pane_group.focused_pane_id(ctx);
                pane_group
                    .visible_pane_ids()
                    .iter()
                    .position(|pane_id| *pane_id == focused_pane_id)
            } else {
                None
            };

            for (pane_index, pane_id) in pane_group.visible_pane_ids().into_iter().enumerate() {
                let container_uuid = pane_group
                    .container_uuid_for_pane_id(pane_id, ctx)
                    .expect("Navigator 可见 live pane 必须拥有稳定 container UUID");
                let leaf_title = pane_group.pane_by_id(pane_id).and_then(|pane| {
                    pane.pane_configuration()
                        .as_ref(ctx)
                        .custom_vertical_tabs_title()
                        .map(str::to_owned)
                });

                if pane_group
                    .downcast_pane_by_id::<EnvironmentRuntimePlaceholderPane>(pane_id)
                    .is_some()
                {
                    let pane_session_binding = pane_group.session_binding_for_pane_id(pane_id, ctx);
                    let mut session = WorkspaceSessionSnapshot {
                        id: format!("tab:{tab_index}:leaf:{pane_index}"),
                        container_uuid: Some(container_uuid),
                        kind: WorkspaceSessionKind::Terminal,
                        label: leaf_title,
                        environment_authority_key: Some(tab_environment.authority_key.clone()),
                        cwd: tab_environment.active_workspace_root.clone(),
                        startup_directory: None,
                        cli_agent: None,
                        cli_command: None,
                        cli_agent_origin: None,
                        conversation_ids: Vec::new(),
                        active_conversation_id: None,
                        cli_agent_session_id: None,
                        is_active: focused_pane_index == Some(pane_index),
                        is_pinned: false,
                        updated_at_unix_ms: None,
                        is_live_container: true,
                    };
                    if let Some(binding) = pane_session_binding.as_ref() {
                        binding.apply_to_workspace_session(&mut session);
                    }
                    if Self::session_navigator_live_session_is_member(&tab_environment, &session) {
                        sessions.push(session);
                    }
                    continue;
                }

                if let Some(terminal_view) = pane_group.terminal_view_from_pane_id(pane_id, ctx) {
                    if tab_requires_runtime_sessions
                        && !terminal_view.as_ref(ctx).is_environment_runtime_transport()
                    {
                        // Liveness must use the terminal's stable transport identity,
                        // not the active block's session. A CLI agent running inside a
                        // subshell moves the active block onto a non-runtime session,
                        // which must not demote the live remote row to a virtual history row.
                        continue;
                    }

                    let pane_session_binding = pane_group.session_binding_for_pane_id(pane_id, ctx);
                    let conversation_ids: Vec<String> = BlocklistAIHistoryModel::as_ref(ctx)
                        .all_live_conversations_for_terminal_view(terminal_view.id())
                        .map(|conversation| conversation.id().to_string())
                        .collect();
                    let has_conversation = !conversation_ids.is_empty()
                        || pane_session_binding
                            .as_ref()
                            .is_some_and(|binding| binding.has_semantic_identity());
                    let mut session = WorkspaceSessionSnapshot {
                        id: format!("tab:{tab_index}:leaf:{pane_index}"),
                        container_uuid: Some(container_uuid),
                        kind: if has_conversation {
                            WorkspaceSessionKind::AgentTerminal
                        } else {
                            WorkspaceSessionKind::Terminal
                        },
                        label: leaf_title,
                        environment_authority_key: Some(tab_environment.authority_key.clone()),
                        cwd: pane_session_binding
                            .as_ref()
                            .and_then(|binding| binding.cwd.clone())
                            .or_else(|| tab_environment.active_workspace_root.clone()),
                        startup_directory: None,
                        cli_agent: None,
                        cli_command: None,
                        cli_agent_origin: None,
                        conversation_ids,
                        active_conversation_id: None,
                        cli_agent_session_id: None,
                        is_active: focused_pane_index == Some(pane_index),
                        is_pinned: false,
                        updated_at_unix_ms: None,
                        is_live_container: true,
                    };
                    if let Some(binding) = pane_session_binding.as_ref() {
                        binding.apply_to_workspace_session(&mut session);
                    }
                    if Self::session_navigator_live_session_is_member(&tab_environment, &session) {
                        sessions.push(session);
                    }
                    continue;
                }

                // Non-terminal / non-placeholder only. TerminalPane::snapshot locks
                // TerminalModel and is forbidden on the Navigator hot path.
                if let Some(startup_directory) =
                    pane_group
                        .pane_by_id(pane_id)
                        .and_then(|pane| match pane.snapshot(ctx) {
                            crate::app_state::LeafContents::Welcome { startup_directory } => {
                                Some(startup_directory)
                            }
                            _ => None,
                        })
                {
                    sessions.push(WorkspaceSessionSnapshot {
                        id: format!("tab:{tab_index}:leaf:{pane_index}"),
                        container_uuid: Some(container_uuid),
                        kind: WorkspaceSessionKind::Welcome,
                        label: leaf_title,
                        environment_authority_key: Some(tab_environment.authority_key.clone()),
                        cwd: None,
                        startup_directory: startup_directory
                            .as_ref()
                            .map(|path| path.to_string_lossy().into_owned()),
                        cli_agent: None,
                        cli_command: None,
                        cli_agent_origin: None,
                        conversation_ids: Vec::new(),
                        active_conversation_id: None,
                        cli_agent_session_id: None,
                        is_active: focused_pane_index == Some(pane_index),
                        is_pinned: false,
                        updated_at_unix_ms: None,
                        is_live_container: true,
                    });
                }
            }
        }
        sessions
    }

    /// Resolves a durable CLI/AI session identity to a semantic pane container
    /// owned by this Workspace. Placeholder and terminal carriers expose the
    /// same PaneConfiguration binding, so ownership never reads the runtime
    /// pending queue, waits for materialization, or snapshots every terminal.
    pub(crate) fn live_or_pending_workspace_session_locator(
        &self,
        durable_identity_key: &str,
        ctx: &AppContext,
    ) -> Option<PaneViewLocator> {
        for tab in &self.tabs {
            let pane_group = tab.pane_group.as_ref(ctx);
            let environment = tab.environment.clone().unwrap_or_else(|| {
                crate::workspace::environment_runtime::terminal_bootstrap_environment(None)
            });
            for pane_id in pane_group.pane_ids() {
                let Some(binding) = pane_group.session_binding_for_pane_id(pane_id, ctx) else {
                    continue;
                };
                if binding
                    .source_identity_keys
                    .iter()
                    .any(|key| key == durable_identity_key)
                {
                    return Some(PaneViewLocator {
                        pane_group_id: tab.pane_group.id(),
                        pane_id,
                    });
                }
                let binding_identity = WorkspaceSessionSnapshot::durable_cli_agent_identity_key(
                    Some(&environment.authority_key),
                    binding.agent.as_deref(),
                    binding.command.as_deref(),
                    binding.session_id.as_deref(),
                );
                if binding_identity.as_deref() == Some(durable_identity_key) {
                    return Some(PaneViewLocator {
                        pane_group_id: tab.pane_group.id(),
                        pane_id,
                    });
                }
            }
        }

        None
    }

    /// Returns the durable terminal owners that must cross the UI → process
    /// teardown boundary when this Workspace closes.
    ///
    /// The durable key always comes from `WorkspaceSessionSnapshot`; this
    /// helper must not reconstruct provider/session identity from a command,
    /// terminal UUID, or tab/leaf locator.
    pub(crate) fn live_durable_terminal_session_owners(
        &self,
        ctx: &AppContext,
    ) -> Vec<(String, ViewHandle<TerminalView>)> {
        self.live_durable_terminal_session_owners_matching(None, None, ctx)
    }

    pub(crate) fn live_durable_terminal_session_owners_for_pane_group(
        &self,
        pane_group_id: EntityId,
        ctx: &AppContext,
    ) -> Vec<(String, ViewHandle<TerminalView>)> {
        self.live_durable_terminal_session_owners_matching(Some(pane_group_id), None, ctx)
    }

    pub(crate) fn live_durable_terminal_session_owners_for_pane(
        &self,
        pane_group_id: EntityId,
        pane_id: PaneId,
        ctx: &AppContext,
    ) -> Vec<(String, ViewHandle<TerminalView>)> {
        self.live_durable_terminal_session_owners_matching(Some(pane_group_id), Some(pane_id), ctx)
    }

    fn live_durable_terminal_session_owners_matching(
        &self,
        pane_group_id: Option<EntityId>,
        pane_id: Option<PaneId>,
        ctx: &AppContext,
    ) -> Vec<(String, ViewHandle<TerminalView>)> {
        // Do not deduplicate by durable key: duplicate live processes are an invariant
        // violation, but every process must remain leased until its own terminal exits.
        self.live_workspace_sessions(ctx)
            .into_iter()
            .filter_map(|session| {
                let durable_identity_key = session.durable_identity_key()?;
                let locator = self.locator_for_workspace_session_snapshot(&session, ctx)?;
                if pane_group_id.is_some_and(|id| id != locator.pane_group_id)
                    || pane_id.is_some_and(|id| id != locator.pane_id)
                {
                    return None;
                }
                let pane_group = self
                    .tabs
                    .iter()
                    .find(|tab| tab.pane_group.id() == locator.pane_group_id)?
                    .pane_group
                    .as_ref(ctx);
                let terminal_view = pane_group.terminal_view_from_pane_id(locator.pane_id, ctx)?;
                Some((durable_identity_key, terminal_view))
            })
            .collect()
    }

    // ── 已索引的 CLI-agent 会话（本地 current-app / 各环境）──────────────────

    pub(super) fn indexed_cli_agent_sessions_for_authority(
        &self,
        authority: &str,
    ) -> Vec<WorkspaceSessionSnapshot> {
        self.environments
            .indexed_cli_agent_sessions_for_authority(authority)
    }

    pub(super) fn all_indexed_environment_cli_agent_sessions(
        &self,
    ) -> Vec<WorkspaceSessionSnapshot> {
        self.environments.all_indexed_cli_agent_sessions()
    }

    #[cfg(test)]
    pub(super) fn commit_indexed_environment_cli_agent_sessions(
        &mut self,
        authority: &str,
        scan_result: Result<Vec<WorkspaceSessionSnapshot>, String>,
    ) -> Result<(), String> {
        self.environments
            .commit_indexed_cli_agent_sessions(authority, scan_result)
    }

    pub(super) fn begin_indexed_environment_cli_agent_session_scan(
        &mut self,
        authority: &str,
        session_id: Option<warp_core::SessionId>,
    ) -> crate::workspace::environment_table::IndexedCliAgentSessionScanToken {
        self.environments
            .begin_indexed_cli_agent_session_scan(authority, session_id)
    }

    pub(super) fn commit_indexed_environment_cli_agent_session_discovery(
        &mut self,
        token: crate::workspace::environment_table::IndexedCliAgentSessionScanToken,
        scan_result: Result<
            crate::workspace::environment_table::IndexedCliAgentSessionScanOutcome,
            String,
        >,
    ) -> Result<bool, String> {
        self.environments
            .commit_indexed_cli_agent_session_discovery(token, scan_result)
    }

    pub(super) fn remember_indexed_environment_cli_agent_session_user_state(
        &mut self,
        authority: String,
        state: crate::workspace::environment_runtime::EnvironmentCliAgentSessionUserState,
    ) {
        self.environments
            .set_cli_agent_session_user_state(authority, state);
    }

    pub(super) fn clear_indexed_environment_cli_agent_sessions_for_authority(
        &mut self,
        authority: &str,
    ) {
        self.environments
            .clear_indexed_cli_agent_sessions(authority);
        self.environments
            .set_cli_agent_session_user_state(authority.to_owned(), Default::default());
    }

    // ── 刷新生命周期 · 目标 Environment header 局部反馈 ──────────────────

    pub(super) fn refresh_environment_sessions(
        &mut self,
        authority: &str,
        ctx: &mut ViewContext<Self>,
    ) {
        match self.environment_session_refresh_availability(authority, ctx) {
            crate::workspace::environment_backend::EnvironmentSessionRefreshAvailability::Ready => {}
            crate::workspace::environment_backend::EnvironmentSessionRefreshAvailability::Unavailable => {
                log::info!(
                    "Ignoring unavailable Environment session refresh for {authority}; helper client is not connected"
                );
                return;
            }
        }
        let refresh_generation = self.begin_environment_sessions_refresh(authority, ctx);
        match self.refresh_indexed_sessions_for_authority(
            authority,
            EnvironmentSessionRefreshIntent::UserInitiated {
                generation: refresh_generation,
            },
            ctx,
        ) {
            Ok(true) => return,
            Ok(false) => {}
            Err(error) => {
                self.fail_environment_sessions_refresh_if_current(
                    authority,
                    refresh_generation,
                    format!("刷新会话列表失败，已保留上次结果：{error}"),
                    ctx,
                );
                return;
            }
        }

        self.prune_restored_workspace_sessions_with_missing_cli_sources();
        self.open_vertical_tabs_panel_for_recoverable_sessions(ctx);
        self.sync_session_navigator_sessions(ctx);
        self.finish_environment_sessions_refresh_if_current(authority, refresh_generation, ctx);
    }

    pub(super) fn refresh_workspace_sessions_passively(&mut self, ctx: &mut ViewContext<Self>) {
        let current_authority = self.current_environment_authority_key(ctx);
        match self.refresh_indexed_sessions_for_authority(
            &current_authority,
            EnvironmentSessionRefreshIntent::PassiveProjection,
            ctx,
        ) {
            Ok(true) => return,
            Ok(false) => {}
            Err(error) => {
                log::warn!(
                    "Session Navigator passive refresh failed for {current_authority}; preserving committed rows: {error}"
                );
                return;
            }
        }

        self.prune_restored_workspace_sessions_with_missing_cli_sources();
        self.sync_session_navigator_sessions(ctx);
    }

    pub(super) fn refresh_indexed_sessions_for_authority(
        &mut self,
        authority: &str,
        intent: EnvironmentSessionRefreshIntent,
        ctx: &mut ViewContext<Self>,
    ) -> Result<bool, String> {
        self.environments.entry_target_snapshot(authority);
        if matches!(intent, EnvironmentSessionRefreshIntent::PassiveProjection)
            && self
                .environments
                .has_indexed_cli_agent_session_scan_in_flight(authority)
        {
            // Terminal bootstrap、source mutation 与 runtime reconciliation 都可能
            // 连续触发同一个 authority 的被动刷新。它们不能不断替换 token，
            // 否则每个后台结果都会被判 stale，最终没有任何结果能提交。
            // 显式用户刷新不走该分支，仍可抢占一个停滞的旧 worker。
            return Ok(true);
        }
        crate::workspace::environment_backend::EnvironmentBackendKind::for_authority(authority)
            .backend()
            .refresh_indexed_sessions(self, authority, intent, ctx)
    }

    fn remove_workspace_session_from_cached_sources(
        &mut self,
        deleted_sessions: &[WorkspaceSessionSnapshot],
    ) {
        self.environments
            .retain_indexed_cli_agent_sessions(|candidate| {
                !deleted_sessions
                    .iter()
                    .any(|deleted| Self::is_same_workspace_session(deleted, candidate))
            });
        self.restored_workspace_sessions.retain(|candidate| {
            !deleted_sessions
                .iter()
                .any(|deleted| Self::is_same_workspace_session(deleted, candidate))
        });
    }

    pub(super) fn workspace_session_delete_plan(
        &self,
        session: WorkspaceSessionSnapshot,
        ctx: &AppContext,
    ) -> WorkspaceSessionDeletePlan {
        let backing_sessions = self.backing_sessions_for_workspace_session(&session);
        let mut cache_sessions = backing_sessions.clone();
        cache_sessions.push(session.clone());

        let identity_keys = Self::workspace_session_identity_keys_for_sessions(&cache_sessions);

        let mut alias_keys = cache_sessions
            .iter()
            .flat_map(Self::workspace_session_alias_keys_for_session)
            .collect::<Vec<_>>();
        alias_keys.sort();
        alias_keys.dedup();

        let pin_keys = cache_sessions
            .iter()
            .flat_map(|candidate| self.workspace_session_pin_keys(candidate, ctx))
            .collect::<Vec<_>>();

        let user_state_authority =
            session_authority_or_terminal_bootstrap(session.environment_authority_key.as_deref())
                .to_owned();

        WorkspaceSessionDeletePlan {
            requested_session: session,
            backing_sessions,
            cache_sessions,
            identity_keys,
            alias_keys,
            pin_keys,
            user_state_authority,
        }
    }

    #[cfg(test)]
    pub(super) fn begin_workspace_session_delete_plan(
        &mut self,
        plan: &WorkspaceSessionDeletePlan,
        ctx: &mut ViewContext<Self>,
    ) {
        // 测试也必须走和生产一致的原子 Delete，而不是先写一个无法独立成立的
        // 删除 lifecycle 必须由完整 Delete 原子建立。side effect 在此夹具中有意不执行。
        self.dispatch_session_navigator_state_action(
            SessionNavigatorAction::Delete {
                session_logical_key: Self::workspace_session_logical_key(&plan.requested_session),
                session_id: plan.requested_session.id.clone(),
                environment_authority_key: plan.requested_session.environment_authority_key.clone(),
                session_identity_keys: plan.identity_keys.clone(),
            },
            ctx,
        );
        self.sync_session_navigator_sessions(ctx);
    }

    pub(super) fn rollback_workspace_session_delete_plan(
        &mut self,
        plan: &WorkspaceSessionDeletePlan,
        ctx: &mut ViewContext<Self>,
    ) {
        self.dispatch_session_navigator_state_action(
            SessionNavigatorAction::DeleteRolledBack {
                session_keys: plan.identity_keys.clone(),
            },
            ctx,
        );
        self.sync_session_navigator_sessions(ctx);
    }

    pub(super) fn finish_workspace_session_delete_plan(
        &mut self,
        plan: &WorkspaceSessionDeletePlan,
        ctx: &mut ViewContext<Self>,
    ) {
        self.remove_workspace_session_from_cached_sources(&plan.cache_sessions);
        self.clear_workspace_session_delete_plan_user_state(plan, ctx);
        let authority = self.current_environment_authority_key(ctx);
        if let Err(error) = self.refresh_indexed_sessions_for_authority(
            &authority,
            EnvironmentSessionRefreshIntent::PassiveProjection,
            ctx,
        ) {
            log::warn!("delete_workspace_session: failed to refresh session index: {error}");
        }
        let volatile_keys = Self::workspace_session_volatile_identity_keys(&plan.requested_session);
        self.dispatch_session_navigator_state_action(
            SessionNavigatorAction::DeleteCommitted {
                session_keys: plan.identity_keys.clone(),
                volatile_identity_keys: volatile_keys,
            },
            ctx,
        );
        // v3 §8: finish only clears user state; focus/close already applied via reducer.
        self.sync_session_navigator_sessions(ctx);
    }

    fn clear_workspace_session_delete_plan_user_state(
        &mut self,
        plan: &WorkspaceSessionDeletePlan,
        ctx: &mut ViewContext<Self>,
    ) {
        if !plan.pin_keys.is_empty() {
            if let Err(error) = self.mutate_workspace_session_user_state_for_authority(
                &plan.user_state_authority,
                &plan.pin_keys,
                crate::workspace::environment_runtime::EnvironmentCliAgentSessionUserStateMutation::ClearPinned,
                crate::workspace::environment_backend::SessionUserStateMutationFeedback::CleanupPinned,
                ctx,
            ) {
                log::warn!("delete_workspace_session: failed to clear pin keys: {error}");
            }
        }
        if let Err(error) = self.mutate_workspace_session_user_state_for_authority(
            &plan.user_state_authority,
            &plan.alias_keys,
            crate::workspace::environment_runtime::EnvironmentCliAgentSessionUserStateMutation::ClearAlias,
            crate::workspace::environment_backend::SessionUserStateMutationFeedback::CleanupAlias,
            ctx,
        ) {
            log::warn!("delete_workspace_session: failed to clear alias keys: {error}");
        }
    }

    pub(super) fn prune_restored_workspace_sessions_with_missing_cli_sources(&mut self) {
        self.restored_workspace_sessions.retain(|session| {
            if !matches!(
                session.cli_agent_origin,
                Some(crate::app_state::CliAgentSessionOrigin::PluginObserved)
            ) {
                return true;
            }
            crate::terminal::cli_agent_session_index::external_jsonl_session_source_exists(
                &session.id,
            )
        });
    }

    fn environment_sessions_refresh_navigation_key(authority: &str) -> String {
        ParsedEnvironmentAuthority::parse(authority)
            .navigation_key()
            .to_owned()
    }

    pub(super) fn is_environment_sessions_refreshing(&self, authority: &str) -> bool {
        let navigation_key = Self::environment_sessions_refresh_navigation_key(authority);
        self.environment_sessions_refresh_state
            .refresh_generation_by_navigation_key
            .contains_key(&navigation_key)
    }

    pub(super) fn environment_sessions_refresh_tooltip(&self, authority: &str) -> String {
        if self.is_environment_sessions_refreshing(authority) {
            "正在刷新会话列表…".to_owned()
        } else if let Some(issue) = self.environment_rail_issue(authority) {
            match issue.kind {
                super::environment_rail::EnvironmentRailIssueKind::DiscoveryFailed
                | super::environment_rail::EnvironmentRailIssueKind::DiscoverySourceMissing => {
                    format!("{}\n点击重试发现", issue.message)
                }
                super::environment_rail::EnvironmentRailIssueKind::HelperUpdateRequired
                | super::environment_rail::EnvironmentRailIssueKind::ConnectionFailed => {
                    "刷新会话列表".to_owned()
                }
            }
        } else {
            "刷新会话列表".to_owned()
        }
    }

    pub(super) fn environment_connect_tooltip(&self, authority: &str) -> String {
        match self.environment_rail_issue(authority) {
            Some(issue)
                if matches!(
                    issue.kind,
                    super::environment_rail::EnvironmentRailIssueKind::HelperUpdateRequired
                ) =>
            {
                format!("更新远端 Helper 后连接\n{}", issue.message)
            }
            Some(issue)
                if matches!(
                    issue.kind,
                    super::environment_rail::EnvironmentRailIssueKind::ConnectionFailed
                ) =>
            {
                format!("连接环境\n{}", issue.message)
            }
            Some(_) | None => "连接环境".to_owned(),
        }
    }

    pub(super) fn environment_rail_issue(
        &self,
        authority: &str,
    ) -> Option<super::environment_rail::EnvironmentRailIssue> {
        if let Some(failure) = self.environments.runtime_failure_for_authority(authority) {
            let (kind, label) = match failure.recovery {
                crate::workspace::environment_table::EnvironmentRuntimeFailureRecovery::Reconnect => (
                    super::environment_rail::EnvironmentRailIssueKind::ConnectionFailed,
                    "连接失败",
                ),
                crate::workspace::environment_table::EnvironmentRuntimeFailureRecovery::UpdateHelper => (
                    super::environment_rail::EnvironmentRailIssueKind::HelperUpdateRequired,
                    "Helper 需更新",
                ),
            };
            return Some(super::environment_rail::EnvironmentRailIssue {
                kind,
                label: label.to_owned(),
                message: failure.message.clone(),
            });
        }

        self.environments
            .indexed_cli_agent_session_discovery_issue(authority)
            .map(|issue| match issue {
                crate::workspace::environment_table::EnvironmentSessionDiscoveryIssue::Failed(
                    message,
                ) => super::environment_rail::EnvironmentRailIssue {
                    kind: super::environment_rail::EnvironmentRailIssueKind::DiscoveryFailed,
                    label: "发现失败".to_owned(),
                    message: message.clone(),
                },
                crate::workspace::environment_table::EnvironmentSessionDiscoveryIssue::SourceMissing(
                    agent,
                ) => {
                    let scope = if ParsedEnvironmentAuthority::parse(authority)
                        .uses_terminal_bootstrap()
                    {
                        "本机"
                    } else {
                        "远端"
                    };
                    super::environment_rail::EnvironmentRailIssue {
                        kind:
                            super::environment_rail::EnvironmentRailIssueKind::DiscoverySourceMissing,
                        label: "来源不可用".to_owned(),
                        message: format!(
                            "{scope} {} 会话来源本轮不可用；已保留上次成功发现的会话。",
                            agent.display_name()
                        ),
                    }
                }
            })
    }

    pub(super) fn begin_environment_sessions_refresh(
        &mut self,
        authority: &str,
        ctx: &mut ViewContext<Self>,
    ) -> u64 {
        let navigation_key = Self::environment_sessions_refresh_navigation_key(authority);
        self.environment_sessions_refresh_state.next_generation = self
            .environment_sessions_refresh_state
            .next_generation
            .saturating_add(1);
        let generation = self.environment_sessions_refresh_state.next_generation;
        self.environment_sessions_refresh_state
            .refresh_generation_by_navigation_key
            .insert(navigation_key, generation);
        ctx.notify();
        generation
    }

    pub(super) fn finish_environment_sessions_refresh_if_current(
        &mut self,
        authority: &str,
        generation: u64,
        ctx: &mut ViewContext<Self>,
    ) {
        let navigation_key = Self::environment_sessions_refresh_navigation_key(authority);
        if self
            .environment_sessions_refresh_state
            .refresh_generation_by_navigation_key
            .get(&navigation_key)
            .copied()
            != Some(generation)
        {
            return;
        }
        self.environment_sessions_refresh_state
            .refresh_generation_by_navigation_key
            .remove(&navigation_key);
        ctx.notify();
    }

    pub(super) fn fail_environment_sessions_refresh_if_current(
        &mut self,
        authority: &str,
        generation: u64,
        message: String,
        ctx: &mut ViewContext<Self>,
    ) {
        let navigation_key = Self::environment_sessions_refresh_navigation_key(authority);
        if self
            .environment_sessions_refresh_state
            .refresh_generation_by_navigation_key
            .get(&navigation_key)
            .copied()
            != Some(generation)
        {
            return;
        }
        self.environment_sessions_refresh_state
            .refresh_generation_by_navigation_key
            .remove(&navigation_key);
        let operation_key = format!("{WORKSPACE_SESSIONS_REFRESH_OPERATION_KEY}:{navigation_key}");
        self.toast_stack.update(ctx, |toast_stack, ctx| {
            toast_stack.add_operation_toast(
                DismissibleToast::error(message),
                "session-navigator",
                &operation_key,
                ctx,
            );
        });
        ctx.notify();
    }

    pub(super) fn cancel_environment_sessions_refresh(
        &mut self,
        authority: &str,
        ctx: &mut ViewContext<Self>,
    ) {
        let navigation_key = Self::environment_sessions_refresh_navigation_key(authority);
        if self
            .environment_sessions_refresh_state
            .refresh_generation_by_navigation_key
            .remove(&navigation_key)
            .is_some()
        {
            ctx.notify();
        }
    }

    pub(super) fn show_workspace_session_success_toast(
        &self,
        message: String,
        ctx: &mut ViewContext<Self>,
    ) {
        self.toast_stack.update(ctx, |toast_stack, ctx| {
            toast_stack.add_transient_toast(DismissibleToast::success(message), ctx);
        });
    }

    pub(super) fn show_workspace_session_error_toast(
        &self,
        message: String,
        ctx: &mut ViewContext<Self>,
    ) {
        self.toast_stack.update(ctx, |toast_stack, ctx| {
            toast_stack.add_transient_toast(DismissibleToast::error(message), ctx);
        });
    }

    // ── 置顶切换 ───────────────────────────────────────────────────────────

    pub(super) fn toggle_workspace_session_pinned(
        &mut self,
        target: &WorkspaceSessionActionTarget,
        pinned: bool,
        ctx: &mut ViewContext<Self>,
    ) {
        let Some(session) = self.workspace_session_for_action_target(target, ctx) else {
            log::warn!(
                "toggle_workspace_session_pinned: missing session {} in {:?}",
                target.session_id,
                target.environment_authority_key
            );
            self.show_workspace_session_error_toast("会话不存在，已刷新后请重试".to_owned(), ctx);
            return;
        };

        let mut keys = self.workspace_session_pin_keys(&session, ctx);
        for backing_session in self.backing_sessions_for_workspace_session(&session) {
            keys.extend(self.workspace_session_pin_keys(&backing_session, ctx));
        }
        keys.sort();
        keys.dedup();

        if keys.is_empty() {
            log::warn!(
                "toggle_workspace_session_pinned: refusing volatile session {} in {:?}",
                target.session_id,
                target.environment_authority_key
            );
            self.show_workspace_session_error_toast(
                "当前会话没有稳定身份，不能置顶；请先进入可恢复的 Agent 会话".to_owned(),
                ctx,
            );
            return;
        }

        log::info!(
            "toggle_workspace_session_pinned: session_id={} pinned={pinned} keys={keys:?}",
            target.session_id
        );
        let authority =
            session_authority_or_terminal_bootstrap(session.environment_authority_key.as_deref())
                .to_owned();
        let mutation = if pinned {
            crate::workspace::environment_runtime::EnvironmentCliAgentSessionUserStateMutation::SetPinned
        } else {
            crate::workspace::environment_runtime::EnvironmentCliAgentSessionUserStateMutation::ClearPinned
        };
        match self.mutate_workspace_session_user_state_for_authority(
            &authority,
            &keys,
            mutation,
            if pinned {
                crate::workspace::environment_backend::SessionUserStateMutationFeedback::Pinned
            } else {
                crate::workspace::environment_backend::SessionUserStateMutationFeedback::Unpinned
            },
            ctx,
        ) {
            Ok(_) => {}
            Err(error) => {
                log::warn!("toggle_workspace_session_pinned: {error}");
                self.show_workspace_session_error_toast(format!("置顶状态更新失败：{error}"), ctx);
                return;
            }
        }
        // v3: Pin action must not change focus (reducer yields WriteUserState only).
        let pane_info = self.snapshot_pane_group_info(ctx);
        let SessionNavigatorModel {
            sessions, state, ..
        } = self.session_navigator_action_model(ctx);
        let reduced = session_navigator_reducer::reduce(
            sessions,
            state,
            SessionNavigatorAction::Pin {
                session_logical_key: Self::workspace_session_logical_key(&session),
                pinned,
            },
            &pane_info,
        );
        debug_assert!(matches!(reduced.side_effect, SideEffect::WriteUserState));
        self.apply_session_navigator_reduction(&reduced, ctx);
    }

    /// EC-08: reorder navigator rows by logical_key while keeping focus/active.
    pub(super) fn reorder_session_navigator_sessions(
        &mut self,
        ordered_logical_keys: Vec<String>,
        ctx: &mut ViewContext<Self>,
    ) {
        let pane_info = self.snapshot_pane_group_info(ctx);
        let SessionNavigatorModel {
            sessions, state, ..
        } = self.session_navigator_action_model(ctx);
        let action = SessionNavigatorAction::Reorder {
            ordered_logical_keys: ordered_logical_keys.clone(),
        };
        let before = ReduceResult {
            sessions: sessions.clone(),
            state: state.clone(),
            side_effect: SideEffect::None,
        };
        let reduced =
            session_navigator_reducer::reduce(sessions, state, action.clone(), &pane_info);
        if let Err(error) =
            session_navigator_reducer::validate_transition(&before, &reduced, &action, &pane_info)
        {
            log::warn!("session_navigator reorder validate_transition: {error}");
            return;
        }
        self.apply_session_navigator_reduction(&reduced, ctx);
        self.sync_session_navigator_sessions(ctx);
        ctx.notify();
    }

    /// EC-17: move a drag unit (split group or single row) then flatten into Reorder.
    pub(super) fn reorder_session_navigator_unit(
        &mut self,
        dragged_unit_id: &str,
        target_index: usize,
        ctx: &mut ViewContext<Self>,
    ) {
        let sessions = self.session_navigator_sessions();
        let units = session_navigator_reducer::build_reorder_units(&sessions);
        let Some(from_index) = units.iter().position(|unit| unit.id() == dragged_unit_id) else {
            log::warn!("reorder_session_navigator_unit: unknown unit id {dragged_unit_id}");
            return;
        };
        if from_index == target_index || from_index + 1 == target_index {
            // Dropped on own boundaries — no-op.
            return;
        }
        let ordered = session_navigator_reducer::move_reorder_unit(units, from_index, target_index);
        self.reorder_session_navigator_sessions(ordered, ctx);
    }

    // ── 恢复点激活 ─────────────────────────────────────────────────────────

    pub(super) fn activate_restored_workspace_session(
        &mut self,
        target: &WorkspaceSessionActionTarget,
        ctx: &mut ViewContext<Self>,
    ) {
        let navigator_model = self.session_navigator_model();
        let Some(session) = navigator_model
            .sessions
            .iter()
            .find(|session| Self::workspace_session_matches_action_target(session, target))
            .cloned()
        else {
            log::warn!(
                "activate_restored_workspace_session: missing session {} in {:?}",
                target.session_id,
                target.environment_authority_key
            );
            return;
        };

        let current_authority = self.current_environment_authority_key(ctx);
        if !Self::session_matches_current_environment(&session, &current_authority) {
            let session_authority = session_authority_or_terminal_bootstrap(
                session.environment_authority_key.as_deref(),
            );
            let session_environment_label = ParsedEnvironmentAuthority::parse(session_authority)
                .display_label()
                .unwrap_or(session_authority)
                .to_owned();
            log::warn!(
                "activate_restored_workspace_session: rejecting session {} for current environment {}",
                Self::workspace_session_logical_key(&session),
                current_authority
            );
            self.show_workspace_session_error_toast(
                crate::t!(
                    "workspace-session-activate-wrong-environment",
                    environment = session_environment_label
                ),
                ctx,
            );
            self.sync_session_navigator_sessions(ctx);
            ctx.notify();
            return;
        }

        let logical_key = Self::workspace_session_logical_key(&session);
        if navigator_model
            .state
            .restoring_row_ids
            .contains(&Self::workspace_session_row_id(
                &session,
                &navigator_model.state,
            ))
        {
            log::info!(
                "activate_restored_workspace_session: session {} is already restoring",
                logical_key
            );
            return;
        }

        let locator = self.locator_for_workspace_session_snapshot(&session, ctx);

        if let Some(_locator) = locator {
            // v3: Activate live via reducer (FocusPane side effect).
            let pane_info = self.snapshot_pane_group_info(ctx);
            let SessionNavigatorModel {
                sessions, state, ..
            } = navigator_model;
            let reduced = session_navigator_reducer::reduce(
                sessions,
                state,
                SessionNavigatorAction::Activate {
                    session_logical_key: logical_key.clone(),
                    session_id: session.id.clone(),
                    is_live: true,
                },
                &pane_info,
            );
            self.apply_session_navigator_reduction(&reduced, ctx);
            let _ = self.apply_session_navigator_side_effect(&reduced.side_effect, None, ctx);
            self.clear_workspace_session_restoring(&session, ctx);
            ctx.notify();
            return;
        }

        if let Some(durable_identity_key) = session.durable_identity_key() {
            if let Some(locator) =
                self.live_or_pending_workspace_session_locator(&durable_identity_key, ctx)
            {
                self.focus_pane(locator, ctx);
                return;
            }

            if let Some(owner) = crate::workspace::WorkspaceRegistry::as_ref(ctx)
                .other_workspace_session_owner(ctx.window_id(), &durable_identity_key, ctx)
            {
                ctx.windows().show_window_and_focus_app(owner.window_id);
                if let Some(root_view_id) = ctx.root_view_id(owner.window_id) {
                    ctx.dispatch_action_for_view(
                        owner.window_id,
                        root_view_id,
                        "root_view:handle_pane_navigation_event",
                        &owner.locator,
                    );
                }
                return;
            }

            let is_retiring = crate::workspace::WorkspaceRegistry::handle(ctx)
                .update(ctx, |registry, _| {
                    registry.is_session_owner_retiring(&durable_identity_key)
                });
            if is_retiring {
                log::info!(
                    "activate_restored_workspace_session: durable session {durable_identity_key} is waiting for its previous terminal process to exit"
                );
                self.show_workspace_session_error_toast("会话正在关闭，请稍后重试".to_owned(), ctx);
                return;
            }
        }

        // virtual Activate 由 reducer 原子设置 restoring、selection 和 SpawnTerminal。
        let pane_info = self.snapshot_pane_group_info(ctx);
        let SessionNavigatorModel {
            sessions, state, ..
        } = navigator_model;
        let reduced = session_navigator_reducer::reduce(
            sessions,
            state,
            SessionNavigatorAction::Activate {
                session_logical_key: logical_key.clone(),
                session_id: session.id.clone(),
                is_live: false,
            },
            &pane_info,
        );
        self.apply_session_navigator_reduction(&reduced, ctx);
        let _ = self.apply_session_navigator_side_effect(&reduced.side_effect, Some(&session), ctx);
    }

    /// Spawn / restore a virtual session after reducer emitted `SpawnTerminal`.
    pub(crate) fn spawn_virtual_workspace_session_from_activate(
        &mut self,
        session: &WorkspaceSessionSnapshot,
        ctx: &mut ViewContext<Self>,
    ) {
        if let Some(conversation_id) =
            Self::conversation_id_from_ashide_conversation_session_id(&session.id)
        {
            self.restore_or_navigate_to_conversation(
                conversation_id,
                None,
                None,
                None,
                None,
                None,
                Some(session),
                ctx,
            );
            self.sync_session_navigator_sessions(ctx);
            ctx.notify();
            return;
        }

        let pending_command = if matches!(session.kind, WorkspaceSessionKind::AgentTerminal)
            || session.cli_agent.is_some()
            || session.cli_command.is_some()
        {
            Self::cli_agent_from_session(session).and_then(|agent| {
                agent.explicit_resume_command(
                    session.cli_agent_session_id.as_deref(),
                    session.cwd.as_deref(),
                )
            })
        } else {
            None
        };
        self.deliver_workspace_session_restore(session, pending_command, ctx);
    }

    // ── 删除 / 删除后重选 ────────────────────────────────────────────

    pub(super) fn request_close_workspace_session(
        &mut self,
        target: &WorkspaceSessionActionTarget,
        ctx: &mut ViewContext<Self>,
    ) {
        let Some(session) = self.workspace_session_for_action_target(target, ctx) else {
            log::warn!(
                "request_close_workspace_session: missing session {} in {:?}",
                target.session_id,
                target.environment_authority_key
            );
            self.show_workspace_session_error_toast("会话不存在，已刷新后请重试".to_owned(), ctx);
            return;
        };

        if self.is_restoring_workspace_session(&session) {
            return;
        }

        let Some(locator) = self.locator_for_workspace_session_snapshot(&session, ctx) else {
            log::warn!(
                "request_close_workspace_session: missing live locator for session {}",
                session.id
            );
            self.show_workspace_session_error_toast("当前窗口中找不到该会话的面板".to_owned(), ctx);
            return;
        };

        let source = OpenDialogSource::ClosePane {
            pane_group_id: locator.pane_group_id,
            pane_id: locator.pane_id,
        };
        if self.should_confirm_close_session(ctx) {
            self.show_close_session_confirmation_dialog(source, ctx);
        } else {
            self.close_pane(locator.pane_group_id, locator.pane_id, ctx);
        }
    }

    pub(super) fn request_delete_workspace_session(
        &mut self,
        target: &WorkspaceSessionActionTarget,
        ctx: &mut ViewContext<Self>,
    ) {
        let Some(session) = self.workspace_session_for_action_target(target, ctx) else {
            log::warn!(
                "request_delete_workspace_session: missing session {} in {:?}",
                target.session_id,
                target.environment_authority_key
            );
            self.show_workspace_session_error_toast("会话不存在，已刷新后请重试".to_owned(), ctx);
            return;
        };

        let title = Self::workspace_session_label(&session);
        let is_restoring = self.is_restoring_workspace_session(&session);
        let is_live =
            session.is_active || Self::locator_from_restored_session_id(&session.id).is_some();
        let (dialog_title, dialog_message, confirm_label) = if is_restoring {
            (
                crate::t!(
                    "workspace-delete-session-dialog-title-restoring",
                    title = title
                ),
                crate::t!("workspace-delete-session-dialog-message-restoring"),
                crate::t!("workspace-delete-session-dialog-confirm-restoring"),
            )
        } else if is_live {
            (
                crate::t!("workspace-delete-session-dialog-title-live", title = title),
                crate::t!("workspace-delete-session-dialog-message-live"),
                crate::t!("workspace-delete-session-dialog-confirm-live"),
            )
        } else {
            (
                crate::t!("workspace-delete-session-dialog-title", title = title),
                crate::t!("workspace-delete-session-dialog-message"),
                crate::t!("workspace-delete-session-dialog-confirm"),
            )
        };
        let confirm_session = session.clone();
        let dialog = AlertDialogWithCallbacks::for_view(
            dialog_title,
            dialog_message,
            vec![
                ModalButton::for_view(confirm_label, move |workspace: &mut Workspace, ctx| {
                    workspace.delete_workspace_session_for_session(&confirm_session, ctx);
                }),
                ModalButton::for_view(crate::t!("common-cancel"), |_: &mut Workspace, _| {}),
            ],
            |_, _| {},
        );
        ctx.show_native_platform_modal(dialog);
    }

    /// Session Navigator 持久状态的唯一接入入口。
    ///
    /// 组件只能发送 typed action；Reducer 先计算并校验 state-slice 写权限，校验失败
    /// 时拒绝提交。这样 selection/lifecycle/position 不会再从事件适配层被随手改写。
    pub(super) fn dispatch_session_navigator_state_action(
        &mut self,
        action: SessionNavigatorAction,
        ctx: &mut ViewContext<Self>,
    ) -> bool {
        // Lifecycle / selection / pin mutations consume the committed render model.
        // Source membership changes must go through sync_session_navigator_sessions
        // or commit_session_navigator_restore_started_after_delivery — never through
        // a speculative action-time Refresh smuggled into this helper.
        let pane_info = self.snapshot_pane_group_info(ctx);
        let SessionNavigatorModel {
            sessions, state, ..
        } = self.session_navigator_model();
        let before = ReduceResult {
            sessions: sessions.clone(),
            state: state.clone(),
            side_effect: SideEffect::None,
        };
        let reduced =
            session_navigator_reducer::reduce(sessions, state, action.clone(), &pane_info);
        if let Err(error) =
            session_navigator_reducer::validate_transition(&before, &reduced, &action, &pane_info)
        {
            log::error!("session_navigator state action rejected: {error}");
            debug_assert!(false, "session_navigator state action rejected: {error}");
            return false;
        }
        self.apply_session_navigator_reduction(&reduced, ctx);
        true
    }

    /// Single post-delivery Navigator commit: one binding-based Refresh, then
    /// RestoreStarted on that result. Local and runtime share this boundary;
    /// only carrier arrival timing differs upstream.
    pub(super) fn commit_session_navigator_restore_started_after_delivery(
        &mut self,
        session_keys: Vec<String>,
        selected_logical_key: Option<String>,
        ctx: &mut ViewContext<Self>,
    ) {
        let pane_info = self.snapshot_pane_group_info(ctx);
        let refreshed = self.reduce_session_navigator_refresh(ctx);
        let action = SessionNavigatorAction::RestoreStarted {
            session_keys,
            selected_logical_key,
        };
        let before = ReduceResult {
            sessions: refreshed.sessions.clone(),
            state: refreshed.state.clone(),
            side_effect: SideEffect::None,
        };
        let reduced = session_navigator_reducer::reduce(
            refreshed.sessions,
            refreshed.state,
            action.clone(),
            &pane_info,
        );
        if let Err(error) =
            session_navigator_reducer::validate_transition(&before, &reduced, &action, &pane_info)
        {
            log::error!("session_navigator restore-started commit rejected: {error}");
            debug_assert!(
                false,
                "session_navigator restore-started commit rejected: {error}"
            );
            return;
        }
        self.apply_session_navigator_reduction(&reduced, ctx);
    }

    pub(super) fn snapshot_session_navigator_model(&self) -> SessionNavigatorModel {
        self.environments
            .active_session_navigator_model()
            .cloned()
            .unwrap_or_default()
    }

    pub(super) fn snapshot_session_navigator_state(&self) -> SessionNavigatorState {
        self.snapshot_session_navigator_model().state
    }

    pub(super) fn logical_key_for_focused_live_pane(&self, ctx: &AppContext) -> Option<String> {
        let pane_group = self.tabs.get(self.active_tab_index)?.pane_group.as_ref(ctx);
        let focused_id = pane_group.focused_pane_id(ctx);
        self.live_workspace_sessions(ctx)
            .into_iter()
            .find_map(|session| {
                self.locator_for_workspace_session_snapshot(&session, ctx)
                    .filter(|locator| locator.pane_id == focused_id)
                    // 已恢复的 runtime placeholder 已在
                    // PaneConfiguration::session_binding 中拥有语义身份。live
                    // container UUID 仍是行的 logical identity；但 focus 必须使用
                    // durable restore identity，才能让 virtual row 在 materialize
                    // 全程保持 active。
                    .map(|_| {
                        session
                            .durable_identity_key()
                            .unwrap_or_else(|| Self::workspace_session_logical_key(&session))
                    })
            })
    }

    /// 显式创建会话完成后，由创建行为接管当前 Environment 的 Navigator selection。
    ///
    /// 普通 tab/pane focus 只能改变 active projection，不能写 selection；但用户点击
    /// “新建会话”表达了新的持久选择意图。远程 placeholder 尚未 materialize 时这里会
    /// 先清掉旧 selection，真正 terminal 创建完成后再以 live identity 绑定新 RowId。
    pub(super) fn select_explicitly_created_session_in_navigator(
        &mut self,
        ctx: &mut ViewContext<Self>,
    ) {
        let session_logical_key = self.logical_key_for_focused_live_pane(ctx);
        self.dispatch_session_navigator_state_action(
            SessionNavigatorAction::SelectionChanged {
                session_logical_key,
            },
            ctx,
        );
    }

    /// Tab/pane focus 变化后清理已经失效的恢复 selection，并刷新 projection。
    /// `PaneFocused` 只能决定本帧 `is_active`，不能反向写入 Environment 独立保存的
    /// selection；这是 focus 与 Navigator 状态之间的单向边界。
    pub(super) fn notify_session_navigator_focus_changed(&mut self, ctx: &mut ViewContext<Self>) {
        let focused_key = self.logical_key_for_focused_live_pane(ctx);

        // Drive reducer TabActivated / PaneFocused for session-list is_active bookkeeping.
        // Focus 只能改变 projection，不能改 Environment-owned selection。
        let pane_info = self.snapshot_pane_group_info(ctx);
        let model = self.session_navigator_action_model(ctx);
        let after_tab = session_navigator_reducer::reduce(
            model.sessions,
            model.state,
            SessionNavigatorAction::TabActivated,
            &pane_info,
        );
        let reduced = if self.tabs.get(self.active_tab_index).is_some() {
            session_navigator_reducer::reduce(
                after_tab.sessions,
                after_tab.state,
                SessionNavigatorAction::PaneFocused {
                    session_logical_key: focused_key,
                },
                &pane_info,
            )
        } else {
            after_tab
        };
        self.apply_session_navigator_reduction(&reduced, ctx);
    }

    fn apply_session_navigator_reduction(
        &mut self,
        reduced: &ReduceResult,
        ctx: &mut ViewContext<Self>,
    ) {
        if let Some(environment_model) = self.environments.active_session_navigator_model_mut() {
            *environment_model = SessionNavigatorModel {
                revision: environment_model.revision.wrapping_add(1),
                sessions: reduced.sessions.clone(),
                state: reduced.state.clone(),
            };
        }
        self.publish_session_navigator_search_documents(ctx);
    }

    /// Publishes only the already-committed Navigator membership to the global
    /// search projection. Live terminal data may enrich the matching row's
    /// prompt, but may not add/remove a row or choose its identity.
    fn publish_session_navigator_search_documents(&self, ctx: &mut ViewContext<Self>) {
        let window_id = ctx.window_id();
        let mut live_by_pane_id = self
            .workspace_sessions(window_id, ctx)
            .map(|session| (session.pane_view_locator().pane_id, session))
            .collect::<HashMap<_, _>>();
        let documents = self
            .committed_session_navigator_model()
            .into_iter()
            .flat_map(|model| model.sessions.iter())
            .map(|session| {
                if session.is_live_container() {
                    self.locator_for_workspace_session_snapshot(session, ctx)
                        .and_then(|locator| live_by_pane_id.remove(&locator.pane_id))
                        .unwrap_or_else(|| {
                            SessionNavigationData::from_workspace_session_snapshot(
                                session, window_id,
                            )
                        })
                } else {
                    SessionNavigationData::from_workspace_session_snapshot(session, window_id)
                }
            })
            .collect();
        WorkspaceRegistry::handle(ctx).update(ctx, |registry, registry_ctx| {
            let generation = registry.replace_session_search_documents(window_id, documents);
            registry_ctx
                .emit(WorkspaceRegistryEvent::SessionSearchProjectionChanged { generation });
        });
    }

    pub(super) fn snapshot_pane_group_info(&self, ctx: &AppContext) -> PaneGroupInfo {
        let mut info = PaneGroupInfo::new();
        for (tab_index, tab) in self.tabs.iter().enumerate() {
            let pane_group = tab.pane_group.as_ref(ctx);
            let visible_ids = pane_group.visible_pane_ids();
            let focused_id = pane_group.focused_pane_id(ctx);
            let mut all_pane_locators = Vec::with_capacity(visible_ids.len());
            for (leaf, pane_id) in visible_ids.iter().enumerate() {
                if let Some(locator) = self.locator_for_tab_pane_index(tab_index, leaf, ctx) {
                    // Prefer index-based locator; fall back to constructing from visible id.
                    all_pane_locators.push(if locator.pane_id == *pane_id {
                        locator
                    } else {
                        PaneViewLocator {
                            pane_group_id: tab.pane_group.id(),
                            pane_id: *pane_id,
                        }
                    });
                } else {
                    all_pane_locators.push(PaneViewLocator {
                        pane_group_id: tab.pane_group.id(),
                        pane_id: *pane_id,
                    });
                }
            }
            let focused_locator = all_pane_locators
                .iter()
                .find(|locator| locator.pane_id == focused_id)
                .cloned()
                .or_else(|| {
                    Some(PaneViewLocator {
                        pane_group_id: tab.pane_group.id(),
                        pane_id: focused_id,
                    })
                });
            let prev_pane_locator =
                pane_group
                    .prev_pane_id(focused_id)
                    .map(|pane_id| PaneViewLocator {
                        pane_group_id: tab.pane_group.id(),
                        pane_id,
                    });
            info.tabs.insert(
                tab_index,
                TabPaneInfo {
                    visible_pane_count: visible_ids.len(),
                    focused_locator,
                    prev_pane_locator,
                    all_pane_locators,
                },
            );
        }
        info
    }

    /// Apply reducer side effects. Returns false if close was refused (sole pane / no CloseWindow).
    /// `context_session` is the deleted session for DeleteEffects, or the virtual session for SpawnTerminal.
    pub(super) fn apply_session_navigator_side_effect(
        &mut self,
        side_effect: &SideEffect,
        context_session: Option<&WorkspaceSessionSnapshot>,
        ctx: &mut ViewContext<Self>,
    ) -> bool {
        match side_effect {
            SideEffect::FocusPane(locator) => {
                self.focus_pane(*locator, ctx);
                true
            }
            SideEffect::SpawnTerminal { session_id, .. } => {
                if let Some(session) = context_session {
                    if session.id != *session_id {
                        log::warn!(
                            "SpawnTerminal session id mismatch: effect={session_id} context={}",
                            session.id
                        );
                    }
                    self.spawn_virtual_workspace_session_from_activate(session, ctx);
                } else {
                    log::warn!(
                        "SpawnTerminal missing context session for id={session_id}; spawn skipped"
                    );
                }
                true
            }
            SideEffect::WriteUserState => true,
            SideEffect::None => {
                // Live delete with no reducer hit: still close via derived DeleteEffects path.
                if let Some(session) = context_session.filter(|s| s.is_live_container()) {
                    return self.apply_delete_effects(
                        &DeleteEffects {
                            focus: None,
                            close: DeleteCloseKind::None,
                        },
                        Some(session),
                        ctx,
                    );
                }
                true
            }
            SideEffect::DeleteEffects(effects) => {
                self.apply_delete_effects(effects, context_session, ctx)
            }
        }
    }

    fn apply_delete_effects(
        &mut self,
        effects: &DeleteEffects,
        deleted_session: Option<&WorkspaceSessionSnapshot>,
        ctx: &mut ViewContext<Self>,
    ) -> bool {
        // Navigator selection/lifecycle 已由 Delete typed action 原子计算；side-effect
        // adapter 只负责物理 focus/close，禁止反向写 SessionNavigatorState。
        // Adapter: refuse closing the window's last visible pane.
        if matches!(effects.close, DeleteCloseKind::CloseTab(_))
            && self.tabs.len() == 1
            && !ContextFlag::CloseWindow.is_enabled()
        {
            if let Some(tab) = self.tabs.first() {
                if tab.pane_group.as_ref(ctx).visible_pane_ids().len() <= 1 {
                    return false;
                }
            }
        }
        if let DeleteCloseKind::ClosePane(locator) = &effects.close {
            if self.tabs.len() == 1 && !ContextFlag::CloseWindow.is_enabled() {
                if let Some(tab) = self.tabs.first() {
                    let pane_group = tab.pane_group.as_ref(ctx);
                    if pane_group.visible_pane_ids().len() <= 1
                        && pane_group.visible_pane_ids().contains(&locator.pane_id)
                    {
                        return false;
                    }
                }
            }
        }

        // Focus first (SPEC §8: Focus then Close), except multi-pane ClosePane where
        // close_pane_permanently already focuses the sibling via focus_next.
        let skip_explicit_focus =
            matches!(effects.close, DeleteCloseKind::ClosePane(_)) && effects.focus.is_some();
        if !skip_explicit_focus {
            if let Some(locator) = &effects.focus {
                self.focus_pane(*locator, ctx);
            }
        }

        match &effects.close {
            DeleteCloseKind::None => {
                // Live twin without reducer close (unparseable id): derive close here
                // so production never needs a second close_live path.
                if let Some(session) = deleted_session.filter(|s| s.is_live_container()) {
                    if let Some((tab_index, pane_index)) =
                        Self::locator_from_restored_session_id(&session.id)
                    {
                        if let Some(locator) =
                            self.locator_for_tab_pane_index(tab_index, pane_index, ctx)
                        {
                            let visible = self
                                .tabs
                                .get(tab_index)
                                .map(|tab| tab.pane_group.as_ref(ctx).visible_pane_ids().len())
                                .unwrap_or(0);
                            if visible <= 1 {
                                if self.tabs.len() == 1 && !ContextFlag::CloseWindow.is_enabled() {
                                    return false;
                                }
                                self.close_tab(tab_index, true, false, ctx);
                            } else {
                                if let Some(authority) = self.tabs[tab_index]
                                    .environment
                                    .as_ref()
                                    .map(|environment| environment.authority_key.clone())
                                {
                                    self.cancel_pending_environment_runtime_materialization_for_pane(
                                        &authority,
                                        locator.pane_id,
                                        ctx,
                                    );
                                }
                                if let Some(pane_group) =
                                    self.tabs.get(tab_index).map(|tab| tab.pane_group.clone())
                                {
                                    pane_group.update(ctx, |pane_group, ctx| {
                                        pane_group.close_pane_permanently(locator.pane_id, ctx);
                                    });
                                }
                            }
                        }
                    }
                }
            }
            DeleteCloseKind::ClosePane(locator) => {
                let Some((pane_group, authority)) = self
                    .tabs
                    .iter()
                    .find(|tab| tab.pane_group.id() == locator.pane_group_id)
                    .map(|tab| {
                        (
                            tab.pane_group.clone(),
                            tab.environment
                                .as_ref()
                                .map(|environment| environment.authority_key.clone()),
                        )
                    })
                else {
                    return true;
                };
                if let Some(authority) = authority {
                    self.cancel_pending_environment_runtime_materialization_for_pane(
                        &authority,
                        locator.pane_id,
                        ctx,
                    );
                }
                pane_group.update(ctx, |pane_group, ctx| {
                    pane_group.close_pane_permanently(locator.pane_id, ctx);
                });
            }
            DeleteCloseKind::CloseTab(tab_index) => {
                if *tab_index < self.tabs.len() {
                    self.close_tab(*tab_index, true, false, ctx);
                }
            }
        }

        // Adapter hooks: env recreate / terminal-bootstrap after close.
        if let Some(session) = deleted_session {
            self.apply_delete_adapter_hooks(session, effects, ctx);
        }
        true
    }

    fn apply_delete_adapter_hooks(
        &mut self,
        deleted_session: &WorkspaceSessionSnapshot,
        effects: &DeleteEffects,
        ctx: &mut ViewContext<Self>,
    ) {
        // Multi-pane ClosePane: sibling already focused; nothing else.
        if matches!(effects.close, DeleteCloseKind::ClosePane(_)) {
            self.sync_session_navigator_sessions(ctx);
            return;
        }

        // If focus already landed on a same-environment live pane, commit the shared
        // membership + focus projection through the canonical sync boundary.
        if effects.focus.is_some() {
            self.sync_session_navigator_sessions(ctx);
            return;
        }

        let authority = session_authority_or_terminal_bootstrap(
            deleted_session.environment_authority_key.as_deref(),
        );
        let environment = self.environments.entry_target_snapshot(authority);
        if let Err(error) = EnvironmentBackendKind::for_environment(&environment)
            .backend()
            .activate_navigation_container(
                self,
                &environment,
                EnvironmentNavigationActivationIntent::AfterContainerClosed,
                ctx,
            )
        {
            log::warn!("apply_delete_adapter_hooks: {error}");
        }
        self.sync_session_navigator_sessions(ctx);
    }

    pub(super) fn delete_workspace_session_for_session(
        &mut self,
        session: &WorkspaceSessionSnapshot,
        ctx: &mut ViewContext<Self>,
    ) {
        self.delete_workspace_session_for_session_with_forced_side_effect_result(
            session, None, ctx,
        );
    }

    /// 测试故障注入点：无需依赖窗口 ContextFlag 或平台 close 行为，就能稳定验证
    /// “reducer 已提交、物理副作用失败”这一事务边界。生产调用始终传 None。
    #[cfg(test)]
    pub(super) fn delete_workspace_session_for_session_with_refused_side_effect(
        &mut self,
        session: &WorkspaceSessionSnapshot,
        ctx: &mut ViewContext<Self>,
    ) {
        self.delete_workspace_session_for_session_with_forced_side_effect_result(
            session,
            Some(false),
            ctx,
        );
    }

    fn delete_workspace_session_for_session_with_forced_side_effect_result(
        &mut self,
        session: &WorkspaceSessionSnapshot,
        forced_side_effect_result: Option<bool>,
        ctx: &mut ViewContext<Self>,
    ) {
        let plan = self.workspace_session_delete_plan(session.clone(), ctx);

        // v3 unified delete: one reducer.Delete decides focus + close.
        let pane_info = self.snapshot_pane_group_info(ctx);
        let SessionNavigatorModel {
            sessions: sessions_before,
            state: navigator_state,
            ..
        } = self.session_navigator_action_model(ctx);
        let action = SessionNavigatorAction::Delete {
            session_logical_key: Self::workspace_session_logical_key(&plan.requested_session),
            session_id: plan.requested_session.id.clone(),
            environment_authority_key: plan.requested_session.environment_authority_key.clone(),
            session_identity_keys: plan.identity_keys.clone(),
        };
        let before = ReduceResult {
            sessions: sessions_before.clone(),
            state: navigator_state.clone(),
            side_effect: SideEffect::None,
        };
        let reduced = session_navigator_reducer::reduce(
            sessions_before,
            navigator_state,
            action.clone(),
            &pane_info,
        );
        if let Err(error) =
            session_navigator_reducer::validate_transition(&before, &reduced, &action, &pane_info)
        {
            log::warn!("session_navigator delete validate_transition: {error}");
        }
        self.apply_session_navigator_reduction(&reduced, ctx);

        let applied = forced_side_effect_result.unwrap_or_else(|| {
            self.apply_session_navigator_side_effect(
                &reduced.side_effect,
                Some(&plan.requested_session),
                ctx,
            )
        });
        if !applied {
            // Delete reducer 已经预计算 selection/lifecycle，但物理关闭被拒绝时必须
            // 原子回滚，不能留下“行消失 / selection 丢失”的半事务状态。
            self.apply_session_navigator_reduction(&before, ctx);
            self.sync_session_navigator_sessions(ctx);
            self.show_workspace_session_error_toast(
                "无法关闭当前唯一会话窗口，删除已取消".to_owned(),
                ctx,
            );
            ctx.notify();
            return;
        }

        #[cfg(all(feature = "local_fs", not(target_family = "wasm")))]
        {
            let mut seen = HashSet::new();
            let mut mutations = Vec::new();
            for backing in &plan.backing_sessions {
                if !seen.insert(backing.id.clone()) {
                    continue;
                }
                match self.session_source_mutation_for_backing(backing, ctx) {
                    Ok(Some((env, source_target))) => {
                        let source_id = backing.id.clone();
                        mutations.push(async move {
                            let env = env
                                .resolve_cli_agent_store_roots()
                                .await
                                .map_err(|error| format!("{error:#}"))?;
                            env.mutate_session_source(
                                source_id,
                                source_target,
                                EnvironmentCliAgentSessionSourceAction::Delete,
                            )
                            .await
                        });
                    }
                    Ok(None) => {}
                    Err(error) => {
                        self.rollback_workspace_session_delete_plan(&plan, ctx);
                        self.show_workspace_session_error_toast(
                            format!("删除会话来源失败，未改动本地状态：{error}"),
                            ctx,
                        );
                        ctx.notify();
                        return;
                    }
                }
            }

            let future = async move { futures::future::join_all(mutations).await };
            ctx.spawn(future, move |workspace, results, ctx| {
                if let Some(error) = results.into_iter().filter_map(Result::err).next() {
                    log::warn!("delete_workspace_session: {error}");
                    workspace.rollback_workspace_session_delete_plan(&plan, ctx);
                    workspace.show_workspace_session_error_toast(
                        format!("删除会话来源失败，未改动本地状态：{error}"),
                        ctx,
                    );
                    ctx.notify();
                    return;
                }
                workspace.finish_workspace_session_delete_plan(&plan, ctx);
                workspace.show_workspace_session_success_toast("已永久删除会话".to_owned(), ctx);
                ctx.notify();
            });
        }

        #[cfg(any(not(feature = "local_fs"), target_family = "wasm"))]
        {
            let mut session_source_delete_errors = Vec::new();
            let mut seen_source_ids = HashSet::new();
            for backing_session in &plan.backing_sessions {
                if !seen_source_ids.insert(backing_session.id.clone()) {
                    continue;
                }
                if Self::is_environment_cli_agent_session_source_id(&backing_session.id) {
                    if !self.schedule_environment_cli_agent_session_source_action(
                        backing_session,
                        EnvironmentCliAgentSessionSourceAction::Delete,
                        ctx,
                    ) {
                        session_source_delete_errors.push(format!(
                            "environment session source delete is unavailable: {}",
                            backing_session.id
                        ));
                    }
                } else if Self::is_terminal_cli_agent_session_source_id(&backing_session.id) {
                    if let Err(error) =
                        Self::delete_terminal_cli_agent_session_source(&backing_session.id)
                    {
                        log::warn!("delete_workspace_session: {error}");
                        session_source_delete_errors.push(error);
                    }
                }
            }
            if let Some(error) = session_source_delete_errors.first() {
                self.rollback_workspace_session_delete_plan(&plan, ctx);
                self.show_workspace_session_error_toast(
                    format!("删除会话来源失败，未改动会话状态：{error}"),
                    ctx,
                );
                ctx.notify();
                return;
            }

            self.finish_workspace_session_delete_plan(&plan, ctx);
            self.show_workspace_session_success_toast("已永久删除会话".to_owned(), ctx);
            ctx.notify();
        }
    }

    pub(super) fn delete_workspace_session(
        &mut self,
        target: &WorkspaceSessionActionTarget,
        ctx: &mut ViewContext<Self>,
    ) {
        let Some(session) = self.workspace_session_for_action_target(target, ctx) else {
            log::warn!(
                "delete_workspace_session: missing session {} in {:?}",
                target.session_id,
                target.environment_authority_key
            );
            self.show_workspace_session_error_toast("会话不存在，已刷新后请重试".to_owned(), ctx);
            return;
        };
        self.delete_workspace_session_for_session(&session, ctx);
    }

    #[cfg(test)]
    pub(super) fn workspace_session_is_active_selection(
        &self,
        session: &WorkspaceSessionSnapshot,
        ctx: &AppContext,
    ) -> bool {
        session.is_active
            || self
                .snapshot_session_navigator_state()
                .selected_row_id
                .as_deref()
                .is_some_and(|row_id| {
                    row_id
                        == Self::workspace_session_row_id(
                            session,
                            &self.snapshot_session_navigator_state(),
                        )
                })
            || self
                .locator_for_workspace_session_snapshot(session, ctx)
                .is_some_and(|locator| {
                    self.tabs
                        .get(self.active_tab_index)
                        .is_some_and(|active_tab| {
                            active_tab.pane_group.id() == locator.pane_group_id
                                && active_tab.pane_group.as_ref(ctx).focused_pane_id(ctx)
                                    == locator.pane_id
                        })
                })
    }

    // ── 恢复中状态清理 ─────────────────────────────────────────────────────

    pub(super) fn clear_workspace_session_restoring(
        &mut self,
        session: &WorkspaceSessionSnapshot,
        ctx: &mut ViewContext<Self>,
    ) {
        self.dispatch_session_navigator_state_action(
            SessionNavigatorAction::RestoreFinished {
                session_keys: vec![
                    session.id.clone(),
                    Self::workspace_session_logical_key(session),
                ],
            },
            ctx,
        );
    }

    // ── 恢复会话注册 · 环境主机键 · pane 定位 ───────────────────────────────

    pub(super) fn register_restored_cli_agent_session(
        &self,
        terminal_view: &ViewHandle<TerminalView>,
        session: &WorkspaceSessionSnapshot,
        ctx: &mut ViewContext<Self>,
    ) {
        let Some(agent) = Self::cli_agent_from_session(session) else {
            return;
        };
        let terminal_view_id = terminal_view.id();
        let catalog = crate::ssh_manager::SshTargetCatalog::handle(ctx);
        let environment_host_key =
            Self::workspace_session_environment_host_key(session, catalog.as_ref(ctx));
        let fallback_title = session.title_fallback_label(self.workspace_session_alias(session));
        CLIAgentSessionsModel::handle(ctx).update(ctx, |sessions, ctx| {
            sessions.set_session(
                terminal_view_id,
                CLIAgentSession {
                    agent,
                    status: CLIAgentSessionStatus::InProgress,
                    session_context: CLIAgentSessionContext {
                        cwd: session.cwd.clone(),
                        session_id: session.cli_agent_session_id.clone(),
                        fallback_title,
                        ..Default::default()
                    },
                    input_state: CLIAgentInputState::Closed,
                    should_auto_toggle_input: false,
                    listener: None,
                    plugin_version: None,
                    environment_host_key,
                    draft_text: None,
                    custom_command_prefix: session.cli_command.clone(),
                },
                ctx,
            );
        });
    }

    pub(super) fn workspace_session_environment_host_key(
        session: &WorkspaceSessionSnapshot,
        catalog: &crate::ssh_manager::SshTargetCatalog,
    ) -> Option<String> {
        let authority = session.environment_authority_key.as_deref()?;
        let parsed_authority = ParsedEnvironmentAuthority::parse(authority);
        if parsed_authority.uses_terminal_bootstrap() {
            return None;
        }
        Some(
            parsed_authority
                .runtime_connection_ref()
                .and_then(|connection_ref| {
                    environment_provider::runtime_transport_descriptor_for_connection_ref(
                        connection_ref,
                        catalog,
                    )
                })
                .map(|descriptor| descriptor.target())
                .unwrap_or_else(|| authority.to_owned()),
        )
    }

    pub(super) fn locator_for_workspace_session_snapshot(
        &self,
        session: &WorkspaceSessionSnapshot,
        ctx: &AppContext,
    ) -> Option<PaneViewLocator> {
        let (tab_index, pane_index) = Self::locator_from_restored_session_id(&session.id)?;
        let session_authority =
            session_authority_or_terminal_bootstrap(session.environment_authority_key.as_deref());
        let tab_authority = self.tab_environment_authority_for_index(tab_index, ctx)?;
        if !session_authority_matches(Some(tab_authority.as_str()), session_authority) {
            log::warn!(
                "locator_for_workspace_session_snapshot: refusing cross-environment locator {} for session authority {} because tab {tab_index} belongs to {}",
                session.id,
                session_authority,
                tab_authority
            );
            return None;
        }
        self.locator_for_tab_pane_index(tab_index, pane_index, ctx)
    }

    // ── 展示标签 · 右键上下文菜单 ──────────────────────────────────────────

    pub(super) fn workspace_session_label(session: &WorkspaceSessionSnapshot) -> String {
        if let Some(label) = session.label.as_deref().filter(|label| !label.is_empty()) {
            return label.to_string();
        }

        if let Some(agent) = Self::cli_agent_from_session(session) {
            return agent.display_name().to_string();
        }

        let Some(command) = session.cli_command.as_deref() else {
            return match session.kind {
                WorkspaceSessionKind::Terminal => {
                    crate::t!("workspace-restored-sessions-terminal-fallback")
                }
                WorkspaceSessionKind::Welcome => {
                    crate::t!("workspace-restored-sessions-welcome-fallback")
                }
                WorkspaceSessionKind::AgentTerminal => {
                    crate::t!("workspace-restored-sessions-agent-fallback")
                }
                WorkspaceSessionKind::Other => crate::t!("workspace-restored-sessions-fallback"),
            };
        };

        let lower_command = command.to_lowercase();
        if lower_command.contains("codex") {
            "Codex".to_string()
        } else if lower_command.contains("claude") {
            "Claude".to_string()
        } else if lower_command.contains("agy") {
            "agy".to_string()
        } else if lower_command.contains("opencode") {
            "OpenCode".to_string()
        } else {
            command
                .split_whitespace()
                .next()
                .unwrap_or(command)
                .to_string()
        }
    }

    pub fn show_workspace_session_context_menu(
        &mut self,
        target: &WorkspaceSessionActionTarget,
        position: Vector2F,
        ctx: &mut ViewContext<Self>,
    ) {
        let Some(session) = self.workspace_session_for_action_target(target, ctx) else {
            log::warn!(
                "show_workspace_session_context_menu: missing session {} in {:?}",
                target.session_id,
                target.environment_authority_key
            );
            return;
        };

        let is_restoring = self.is_restoring_workspace_session(&session);
        self.agent_action_sidecar_source = None;
        let session_target = Self::workspace_session_action_target(&session);
        let is_live_in_current_environment = session.is_active
            || self
                .locator_for_workspace_session_snapshot(&session, ctx)
                .is_some();
        let open_label = if is_restoring {
            crate::t!("workspace-session-navigator-menu-restoring")
        } else if is_live_in_current_environment {
            crate::t!("workspace-session-navigator-menu-focus")
        } else {
            crate::t!("workspace-session-navigator-menu-restore")
        };

        let mut menu_items = vec![
            MenuItemFields::new(open_label)
                .with_on_select_action(WorkspaceAction::ActivateRestoredWorkspaceSession {
                    target: session_target.clone(),
                })
                .with_disabled(is_restoring)
                .into_item(),
            MenuItem::Separator,
        ];

        if let Some(session_bridge_items) =
            Self::session_bridge_menu_items_for_workspace_session_in_context(&session, ctx)
        {
            let conversation_id =
                Self::session_bridge_conversation_id_for_workspace_session_in_context(
                    &session, ctx,
                )
                .expect("SessionBridge menu requires a conversation source");
            self.agent_action_sidecar_source = Some(AgentActionSidecarSource::SessionBridge(
                SessionBridgeActionSource::Conversation {
                    conversation_id,
                    source_environment_authority_key: session.environment_authority_key.clone(),
                },
            ));
            menu_items.extend(session_bridge_items);
            menu_items.push(MenuItem::Separator);
        } else if let Some(fork_items) =
            self.cli_agent_session_bridge_menu_items_for_workspace_session(&session)
        {
            self.agent_action_sidecar_source = Some(AgentActionSidecarSource::SessionBridge(
                SessionBridgeActionSource::WorkspaceTarget {
                    target: session_target.clone(),
                },
            ));
            menu_items.extend(fork_items);
            menu_items.push(MenuItem::Separator);
        } else if Self::workspace_session_should_show_session_bridge_unavailable(&session) {
            menu_items.push(
                MenuItemFields::new(crate::t!("workspace-session-bridge-fork-unavailable"))
                    .with_disabled(true)
                    .into_item(),
            );
            menu_items.push(MenuItem::Separator);
        }

        if !self.workspace_session_pin_keys(&session, ctx).is_empty() {
            menu_items.push(
                MenuItemFields::new(if session.is_pinned {
                    crate::t!("workspace-session-navigator-menu-unpin")
                } else {
                    crate::t!("workspace-session-navigator-menu-pin")
                })
                .with_on_select_action(WorkspaceAction::ToggleWorkspaceSessionPinned {
                    target: session_target.clone(),
                    pinned: !session.is_pinned,
                })
                .into_item(),
            );
        }

        let alias_subject = session.alias_subject();
        let uses_container_override = matches!(
            alias_subject,
            crate::app_state::WorkspaceSessionAliasSubject::Container(_)
        );
        menu_items.extend([MenuItemFields::new(if uses_container_override {
            crate::t!("workspace-session-navigator-menu-rename-pane")
        } else {
            crate::t!("workspace-session-navigator-menu-rename-alias")
        })
        .with_on_select_action(WorkspaceAction::RequestRenameWorkspaceSession {
            target: session_target.clone(),
        })
        .into_item()]);

        let has_clearable_title = if uses_container_override {
            self.locator_for_workspace_session_snapshot(&session, ctx)
                .and_then(|locator| {
                    self.get_pane_group_view_with_id(locator.pane_group_id)
                        .and_then(|pane_group| pane_group.as_ref(ctx).pane_by_id(locator.pane_id))
                        .map(|pane| {
                            pane.pane_configuration()
                                .as_ref(ctx)
                                .custom_vertical_tabs_title()
                                .is_some()
                        })
                })
                .unwrap_or(false)
        } else {
            self.workspace_session_alias(&session).is_some()
        };
        if has_clearable_title {
            menu_items.push(
                MenuItemFields::new(if uses_container_override {
                    crate::t!("workspace-session-navigator-menu-reset-pane-name")
                } else {
                    crate::t!("workspace-session-navigator-menu-clear-alias")
                })
                .with_on_select_action(WorkspaceAction::ResetWorkspaceSessionName {
                    target: session_target.clone(),
                })
                .into_item(),
            );
        }

        menu_items.push(MenuItem::Separator);
        menu_items.push(
            MenuItemFields::new(crate::t!("workspace-session-navigator-menu-copy-id"))
                .with_on_select_action(WorkspaceAction::CopyWorkspaceSessionId {
                    target: session_target.clone(),
                })
                .into_item(),
        );
        if cfg!(debug_assertions) {
            menu_items.push(
                MenuItemFields::new("复制关系 X-Ray JSON")
                    .with_on_select_action(WorkspaceAction::CopyWorkspaceSessionXRay {
                        target: session_target.clone(),
                    })
                    .into_item(),
            );
        }

        if is_live_in_current_environment && !is_restoring {
            menu_items.push(
                MenuItemFields::new(crate::t!(
                    "workspace-session-navigator-menu-close-conversation"
                ))
                .with_on_select_action(WorkspaceAction::RequestCloseWorkspaceSession {
                    target: session_target.clone(),
                })
                .into_item(),
            );
        }

        if !is_restoring {
            menu_items.push(
                MenuItemFields::new(crate::t!("workspace-session-navigator-menu-delete"))
                    .with_on_select_action(WorkspaceAction::RequestDeleteWorkspaceSession {
                        target: session_target,
                    })
                    .into_item(),
            );
        } else {
            menu_items.push(
                MenuItemFields::new(crate::t!("workspace-session-navigator-menu-delete"))
                    .with_disabled(true)
                    .into_item(),
            );
        }

        ctx.update_view(&self.tab_right_click_menu, |context_menu, view_ctx| {
            context_menu.set_items(menu_items, view_ctx);
        });
        self.show_tab_right_click_menu = Some((
            self.active_tab_index,
            TabContextMenuAnchor::Pointer(position),
        ));
        ctx.focus(&self.tab_right_click_menu);
        ctx.notify();
    }
}

#[cfg(test)]
#[path = "session_navigator_scan_tests.rs"]
mod scan_tests;
