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

use warpui::{AppContext, SingletonEntity, UpdateView, ViewContext, ViewHandle};

use crate::workspace::environment_provider;

use super::{
    AlertDialogWithCallbacks, Appearance, CLIAgentInputState, CLIAgentSession,
    CLIAgentSessionContext, CLIAgentSessionStatus, CLIAgentSessionsModel,
    CLIAgentSessionsModelEvent, ContextFlag, DismissibleToast, EditorEvent, EditorView,
    EnvironmentCliAgentSessionSourceAction, MenuItem, MenuItemFields, ModalButton, PaneViewLocator,
    RestoreConversationLayout, SingleLineEditorOptions, TabContextMenuAnchor, TabSnapshot,
    TerminalView, TextOptions, Vector2F, Workspace, WorkspaceAction, WorkspaceSessionActionTarget,
    WorkspaceSessionKind, WorkspaceSessionSnapshot, WORKSPACE_SESSIONS_REFRESH_TOAST_ID,
};

impl Workspace {
    pub(super) fn session_navigator_sessions(
        &self,
        ctx: &AppContext,
    ) -> Vec<WorkspaceSessionSnapshot> {
        let mut sessions = self.raw_session_navigator_sessions(ctx);
        self.sort_session_navigator_sessions_by_display_order(&mut sessions);
        sessions
    }

    pub(super) fn session_navigator_sessions_for_display_update(
        &mut self,
        ctx: &AppContext,
    ) -> Vec<WorkspaceSessionSnapshot> {
        let mut sessions = self.raw_session_navigator_sessions(ctx);
        self.reconcile_session_navigator_display_order(&sessions);
        self.sort_session_navigator_sessions_by_display_order(&mut sessions);
        sessions
    }

    pub(super) fn raw_session_navigator_sessions(
        &self,
        ctx: &AppContext,
    ) -> Vec<WorkspaceSessionSnapshot> {
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
            .filter(|session| Self::session_matches_current_environment(session, &current_authority));
        let mut sessions =
            WorkspaceSessionSnapshot::merge_for_session_navigator(sources, &user_state.pinned);
        let active_restored_key = self.active_restored_workspace_session_key.as_deref();
        if let Some(active_key) = active_restored_key {
            for session in &mut sessions {
                if Self::workspace_session_logical_key(session) == active_key {
                    session.is_active = true;
                }
            }
        }
        Self::normalize_session_navigator_active_state(&mut sessions, active_restored_key);
        for session in &mut sessions {
            if let Some(alias) = self.workspace_session_alias_with_state(session, &user_state) {
                session.label = Some(alias);
            }
        }
        sessions
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
    pub fn workspace_session_snapshots_for_search(
        &self,
        ctx: &AppContext,
    ) -> Vec<WorkspaceSessionSnapshot> {
        self.raw_session_navigator_sessions(ctx)
    }

    pub(super) fn workspace_session_display_order_key(
        session: &WorkspaceSessionSnapshot,
    ) -> String {
        let environment_key =
            crate::workspace::environment_runtime::session_authority_or_terminal_bootstrap(
                session
                    .environment_authority_key
                    .as_deref()
                    .filter(|key| !key.trim().is_empty()),
            );

        if let Some(cli_agent_session_id) = session
            .cli_agent_session_id
            .as_deref()
            .filter(|id| !id.trim().is_empty())
        {
            return format!(
                "{environment_key}::agent:{}:{}",
                session
                    .cli_agent
                    .as_deref()
                    .or(session.cli_command.as_deref())
                    .unwrap_or_default(),
                cli_agent_session_id
            );
        }

        if let Some(conversation_id) = session
            .active_conversation_id
            .iter()
            .chain(session.conversation_ids.iter())
            .find(|id| !id.trim().is_empty())
        {
            return format!("{environment_key}::conversation:{conversation_id}");
        }

        Self::workspace_session_logical_key(session)
    }

    pub(super) fn reconcile_session_navigator_display_order(
        &mut self,
        sessions: &[WorkspaceSessionSnapshot],
    ) {
        let current_keys = sessions
            .iter()
            .map(Self::workspace_session_display_order_key)
            .collect::<HashSet<_>>();
        self.session_navigator_display_order
            .retain(|key, _| current_keys.contains(key));

        for session in sessions {
            let order_key = Self::workspace_session_display_order_key(session);
            if self
                .session_navigator_display_order
                .contains_key(&order_key)
            {
                continue;
            }
            let display_order = self.next_session_navigator_display_order;
            self.next_session_navigator_display_order += 1;
            self.session_navigator_display_order
                .insert(order_key, display_order);
        }
    }

    pub(super) fn sort_session_navigator_sessions_by_display_order(
        &self,
        sessions: &mut [WorkspaceSessionSnapshot],
    ) {
        let original_positions = sessions
            .iter()
            .enumerate()
            .map(|(index, session)| (Self::workspace_session_display_order_key(session), index))
            .collect::<HashMap<_, _>>();

        sessions.sort_by(|left, right| {
            let left_key = Self::workspace_session_display_order_key(left);
            let right_key = Self::workspace_session_display_order_key(right);
            let left_order = self
                .session_navigator_display_order
                .get(&left_key)
                .copied()
                .unwrap_or(u64::MAX);
            let right_order = self
                .session_navigator_display_order
                .get(&right_key)
                .copied()
                .unwrap_or(u64::MAX);
            let left_original_position = original_positions
                .get(&left_key)
                .copied()
                .unwrap_or(usize::MAX);
            let right_original_position = original_positions
                .get(&right_key)
                .copied()
                .unwrap_or(usize::MAX);

            right
                .is_pinned
                .cmp(&left.is_pinned)
                .then_with(|| left_order.cmp(&right_order))
                .then_with(|| left_original_position.cmp(&right_original_position))
                .then_with(|| {
                    left.label
                        .as_deref()
                        .unwrap_or_default()
                        .cmp(right.label.as_deref().unwrap_or_default())
                })
                .then_with(|| left.id.cmp(&right.id))
        });
    }

    pub(super) fn normalize_session_navigator_active_state(
        sessions: &mut [WorkspaceSessionSnapshot],
        preferred_active_key: Option<&str>,
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

        let preferred_key = preferred_active_key
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
        self.restoring_workspace_session_keys.contains(&session.id)
            || self
                .restoring_workspace_session_keys
                .contains(&Self::workspace_session_logical_key(session))
    }

    pub(super) fn workspace_session_logical_key(session: &WorkspaceSessionSnapshot) -> String {
        session.logical_key()
    }

    pub(super) fn is_same_workspace_session(
        left: &WorkspaceSessionSnapshot,
        right: &WorkspaceSessionSnapshot,
    ) -> bool {
        Self::workspace_session_logical_key(left) == Self::workspace_session_logical_key(right)
    }

    pub(super) fn workspace_session_pin_keys(session: &WorkspaceSessionSnapshot) -> Vec<String> {
        session.stable_pin_keys()
    }

    pub(super) fn workspace_session_alias_keys_for_session(
        session: &WorkspaceSessionSnapshot,
    ) -> Vec<String> {
        let mut keys = vec![
            session.id.clone(),
            Self::workspace_session_logical_key(session),
        ];
        keys.sort();
        keys.dedup();
        keys
    }

    pub(super) fn workspace_session_alias_keys(
        &self,
        session: &WorkspaceSessionSnapshot,
    ) -> Vec<String> {
        let mut keys = Self::workspace_session_alias_keys_for_session(session);
        for backing_session in self.backing_sessions_for_workspace_session(session) {
            keys.extend(Self::workspace_session_alias_keys_for_session(
                &backing_session,
            ));
        }
        keys.sort();
        keys.dedup();
        keys
    }

    pub(super) fn workspace_session_alias(
        &self,
        session: &WorkspaceSessionSnapshot,
    ) -> Option<String> {
        let authority =
            crate::workspace::environment_runtime::session_authority_or_terminal_bootstrap(
                session.environment_authority_key.as_deref(),
            );
        let user_state = self.workspace_session_user_state_for_authority(authority);
        self.workspace_session_alias_with_state(session, &user_state)
    }

    pub(super) fn workspace_session_alias_with_state(
        &self,
        session: &WorkspaceSessionSnapshot,
        user_state: &crate::workspace::environment_runtime::EnvironmentCliAgentSessionUserState,
    ) -> Option<String> {
        self.workspace_session_alias_keys(session)
            .into_iter()
            .find_map(|key| user_state.aliases.get(&key).cloned())
    }

    pub(super) fn workspace_session_user_state_for_authority(
        &self,
        authority: &str,
    ) -> crate::workspace::environment_runtime::EnvironmentCliAgentSessionUserState {
        if crate::workspace::environment_runtime::authority_uses_terminal_bootstrap(authority) {
            return crate::workspace::environment_runtime::EnvironmentCliAgentSessionUserState {
                aliases: Self::local_cli_agent_session_aliases(),
                pinned: Self::pinned_cli_agent_session_ids(),
            };
        }
        self.indexed_environment_cli_agent_session_user_states
            .get(authority)
            .cloned()
            .unwrap_or_default()
    }

    pub(super) fn local_cli_agent_session_aliases() -> HashMap<String, String> {
        crate::terminal::cli_agent_session_index::session_aliases()
    }

    pub(super) fn scan_terminal_cli_agent_sessions(limit: usize) -> Vec<WorkspaceSessionSnapshot> {
        crate::terminal::cli_agent_session_index::scan_current_app_cli_agent_sessions(limit)
    }

    pub(super) fn backing_sessions_for_workspace_session(
        &self,
        session: &WorkspaceSessionSnapshot,
    ) -> Vec<WorkspaceSessionSnapshot> {
        let mut seen = HashSet::new();
        let mut sessions = Vec::new();
        let indexed_environment_sessions = self.all_indexed_environment_cli_agent_sessions();
        for candidate in self
            .indexed_cli_agent_sessions
            .iter()
            .chain(indexed_environment_sessions.iter())
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

    pub(super) fn pinned_cli_agent_session_ids() -> HashSet<String> {
        crate::terminal::cli_agent_session_index::pinned_session_ids()
    }

    pub(super) fn set_cli_agent_session_pinned(
        session_id: &str,
        pinned: bool,
    ) -> Result<(), String> {
        crate::terminal::cli_agent_session_index::set_session_pinned(session_id, pinned)
    }

    pub(super) fn set_cli_agent_session_alias(
        key: &str,
        alias: Option<&str>,
    ) -> Result<(), String> {
        crate::terminal::cli_agent_session_index::set_session_alias(key, alias)
    }

    pub(super) fn mutate_workspace_session_user_state_for_authority(
        &mut self,
        authority: &str,
        keys: &[String],
        mutation: crate::workspace::environment_runtime::EnvironmentCliAgentSessionUserStateMutation,
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

        if crate::workspace::environment_runtime::authority_uses_terminal_bootstrap(authority) {
            return Self::mutate_local_workspace_session_user_state(&keys, mutation);
        }

        #[cfg(target_family = "wasm")]
        {
            return Err("remote session user-state is unavailable in wasm".to_owned());
        }

        #[cfg(not(target_family = "wasm"))]
        {
            let target = self
                .environment_runtime_target_for_authority(authority)
                .ok_or_else(|| format!("environment runtime is not connected: {authority}"))?;
            let client =
                crate::workspace::environment_runtime::client_for_session(target.session_id, ctx)
                    .ok_or_else(|| {
                        format!("environment runtime client is not connected: {authority}")
                    })?;

            let previous_state = self
                .indexed_environment_cli_agent_session_user_states
                .get(authority)
                .cloned();
            let mut optimistic_state = previous_state.clone().unwrap_or_default();
            Self::apply_workspace_session_user_state_mutation(
                &mut optimistic_state,
                &keys,
                &mutation,
            );
            self.indexed_environment_cli_agent_session_user_states
                .insert(authority.to_owned(), optimistic_state);

            let authority = authority.to_owned();
            let future = async move {
                crate::workspace::environment_runtime::mutate_environment_cli_agent_session_user_state(
                    client, keys, mutation,
                )
                .await
            };
            ctx.spawn(future, move |workspace, result, ctx| {
                match result {
                    Ok(state) => {
                        workspace.remember_indexed_environment_cli_agent_session_user_state(
                            authority.clone(),
                            state,
                        );
                    }
                    Err(error) => {
                        log::warn!("remote session user-state mutation failed: {error}");
                        if let Some(previous_state) = previous_state {
                            workspace.remember_indexed_environment_cli_agent_session_user_state(
                                authority.clone(),
                                previous_state,
                            );
                        } else {
                            workspace
                                .indexed_environment_cli_agent_session_user_states
                                .remove(&authority);
                        }
                        workspace.show_workspace_session_error_toast(
                            format!("同步远程会话状态失败：{error}"),
                            ctx,
                        );
                    }
                }
                workspace.sync_session_navigator_sessions(ctx);
                ctx.notify();
            });
            Ok(())
        }
    }

    fn mutate_local_workspace_session_user_state(
        keys: &[String],
        mutation: crate::workspace::environment_runtime::EnvironmentCliAgentSessionUserStateMutation,
    ) -> Result<(), String> {
        match mutation {
            crate::workspace::environment_runtime::EnvironmentCliAgentSessionUserStateMutation::SetAlias(alias) => {
                for key in keys {
                    Self::set_cli_agent_session_alias(key, Some(&alias))?;
                }
            }
            crate::workspace::environment_runtime::EnvironmentCliAgentSessionUserStateMutation::ClearAlias => {
                for key in keys {
                    Self::set_cli_agent_session_alias(key, None)?;
                }
            }
            crate::workspace::environment_runtime::EnvironmentCliAgentSessionUserStateMutation::SetPinned => {
                for key in keys {
                    Self::set_cli_agent_session_pinned(key, true)?;
                }
            }
            crate::workspace::environment_runtime::EnvironmentCliAgentSessionUserStateMutation::ClearPinned => {
                for key in keys {
                    Self::set_cli_agent_session_pinned(key, false)?;
                }
            }
        }
        Ok(())
    }

    fn apply_workspace_session_user_state_mutation(
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

    pub(super) fn workspace_session_alias_editor(
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
            me.handle_workspace_session_alias_editor_event(event, ctx);
        });
        editor
    }

    pub(super) fn handle_workspace_session_alias_editor_event(
        &mut self,
        event: &EditorEvent,
        ctx: &mut ViewContext<Self>,
    ) {
        if self.renaming_workspace_session_target.is_some() {
            match event {
                EditorEvent::Blurred | EditorEvent::Enter => {
                    self.finish_workspace_session_alias_rename(ctx);
                }
                EditorEvent::Escape => {
                    self.cancel_workspace_session_alias_rename(ctx);
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
        self.renaming_workspace_session_target =
            Some(Self::workspace_session_action_target(&session));
        self.workspace_session_alias_editor
            .update(ctx, |editor, ctx| {
                editor.clear_buffer_and_reset_undo_stack(ctx);
                editor.set_buffer_text(&initial_alias, ctx);
                editor.select_all(ctx);
            });
        ctx.focus(&self.workspace_session_alias_editor);
        ctx.notify();
    }

    pub(super) fn finish_workspace_session_alias_rename(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(target) = self.renaming_workspace_session_target.take() else {
            return;
        };
        let alias = self
            .workspace_session_alias_editor
            .as_ref(ctx)
            .buffer_text(ctx)
            .trim()
            .to_owned();
        self.clear_workspace_session_alias_editor(ctx);

        let Some(session) = self.workspace_session_for_action_target(&target, ctx) else {
            log::warn!(
                "finish_workspace_session_alias_rename: missing session {} in {:?}",
                target.session_id,
                target.environment_authority_key
            );
            self.sync_session_navigator_sessions(ctx);
            ctx.notify();
            return;
        };
        let keys = self.workspace_session_alias_keys(&session);
        let authority =
            crate::workspace::environment_runtime::session_authority_or_terminal_bootstrap(
                session.environment_authority_key.as_deref(),
            )
            .to_owned();
        let mutation = if alias.is_empty() {
            crate::workspace::environment_runtime::EnvironmentCliAgentSessionUserStateMutation::ClearAlias
        } else {
            crate::workspace::environment_runtime::EnvironmentCliAgentSessionUserStateMutation::SetAlias(
                alias.clone(),
            )
        };
        let result = self
            .mutate_workspace_session_user_state_for_authority(&authority, &keys, mutation, ctx);

        match result {
            Ok(()) => {
                self.sync_session_navigator_sessions(ctx);
                if alias.is_empty() {
                    self.show_workspace_session_success_toast("已清除会话别名".to_owned(), ctx);
                } else {
                    self.show_workspace_session_success_toast("已更新会话别名".to_owned(), ctx);
                }
            }
            Err(error) => {
                log::warn!("finish_workspace_session_alias_rename: {error}");
                self.show_workspace_session_error_toast(format!("更新会话别名失败：{error}"), ctx);
            }
        }
        ctx.notify();
    }

    pub(super) fn cancel_workspace_session_alias_rename(&mut self, ctx: &mut ViewContext<Self>) {
        if self.renaming_workspace_session_target.take().is_some() {
            self.clear_workspace_session_alias_editor(ctx);
            ctx.notify();
        }
    }

    pub(super) fn clear_workspace_session_alias_editor(&mut self, ctx: &mut ViewContext<Self>) {
        self.workspace_session_alias_editor
            .update(ctx, |editor, ctx| {
                editor.clear_buffer_and_reset_undo_stack(ctx)
            });
    }

    pub(super) fn clear_workspace_session_alias(
        &mut self,
        target: &WorkspaceSessionActionTarget,
        ctx: &mut ViewContext<Self>,
    ) {
        let Some(session) = self.workspace_session_for_action_target(target, ctx) else {
            log::warn!(
                "clear_workspace_session_alias: missing session {} in {:?}",
                target.session_id,
                target.environment_authority_key
            );
            self.show_workspace_session_error_toast("会话不存在，已刷新后请重试".to_owned(), ctx);
            return;
        };

        let keys = self.workspace_session_alias_keys(&session);
        let authority =
            crate::workspace::environment_runtime::session_authority_or_terminal_bootstrap(
                session.environment_authority_key.as_deref(),
            )
            .to_owned();
        if let Err(error) = self.mutate_workspace_session_user_state_for_authority(
            &authority,
            &keys,
            crate::workspace::environment_runtime::EnvironmentCliAgentSessionUserStateMutation::ClearAlias,
            ctx,
        ) {
            log::warn!("clear_workspace_session_alias: {error}");
            self.show_workspace_session_error_toast(format!("清除会话别名失败：{error}"), ctx);
            return;
        }
        if self
            .renaming_workspace_session_target
            .as_ref()
            .is_some_and(|renaming| {
                renaming.session_id == target.session_id
                    && renaming.environment_authority_key == target.environment_authority_key
            })
        {
            self.cancel_workspace_session_alias_rename(ctx);
        }
        self.sync_session_navigator_sessions(ctx);
        self.show_workspace_session_success_toast("已清除会话别名".to_owned(), ctx);
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
            self.sync_session_navigator_sessions(ctx);
            ctx.notify();
        }
    }

    pub(super) fn sync_session_navigator_sessions(&mut self, ctx: &mut ViewContext<Self>) {
        let sessions = self.session_navigator_sessions_for_display_update(ctx);
        for active_session in sessions.iter().filter(|session| session.is_active) {
            self.restoring_workspace_session_keys
                .remove(&active_session.id);
            self.restoring_workspace_session_keys
                .remove(&Self::workspace_session_logical_key(active_session));
        }
        ctx.notify();
    }

    pub(super) fn clear_active_restored_workspace_session_key(
        &mut self,
        ctx: &mut ViewContext<Self>,
    ) {
        if self.active_restored_workspace_session_key.take().is_some() {
            self.sync_session_navigator_sessions(ctx);
        }
    }

    pub(super) fn workspace_session_key_belongs_to_authority(key: &str, authority: &str) -> bool {
        key.strip_prefix(authority)
            .is_some_and(|suffix| suffix.starts_with("::"))
    }

    pub(super) fn clear_active_restored_workspace_session_for_authority(
        &mut self,
        authority: &str,
        ctx: &mut ViewContext<Self>,
    ) {
        if self
            .active_restored_workspace_session_key
            .as_deref()
            .is_some_and(|key| Self::workspace_session_key_belongs_to_authority(key, authority))
        {
            self.clear_active_restored_workspace_session_key(ctx);
        }
    }

    pub(super) fn set_active_restored_workspace_session_key_for_session(
        &mut self,
        session: &WorkspaceSessionSnapshot,
        ctx: &mut ViewContext<Self>,
    ) {
        self.active_restored_workspace_session_key =
            Some(Self::workspace_session_logical_key(session));
        self.sync_session_navigator_sessions(ctx);
    }

    pub(super) fn clear_active_restored_workspace_session_if_matches(
        &mut self,
        session: &WorkspaceSessionSnapshot,
        ctx: &mut ViewContext<Self>,
    ) {
        let logical_key = Self::workspace_session_logical_key(session);
        if self.active_restored_workspace_session_key.as_deref() == Some(logical_key.as_str()) {
            self.clear_active_restored_workspace_session_key(ctx);
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

        match target.environment_authority_key.as_deref() {
            Some(authority) => crate::workspace::environment_runtime::session_authority_matches(
                session.environment_authority_key.as_deref(),
                authority,
            ),
            None => {
                // Legacy actions that did not carry an authority are current-app
                // targets, not wildcards. Treating `None` as "any environment"
                // lets a stale/miswired UI event focus or mutate a remote row
                // with the same volatile tab:* id from the local/current-app
                // side, which is exactly the kind of environment bleed the
                // Environment facade is meant to prevent.
                session.environment_authority_key.as_deref().is_none_or(
                    crate::workspace::environment_runtime::authority_uses_terminal_bootstrap,
                )
            }
        }
    }

    pub(super) fn workspace_session_for_action_target(
        &self,
        target: &WorkspaceSessionActionTarget,
        ctx: &AppContext,
    ) -> Option<WorkspaceSessionSnapshot> {
        self.session_navigator_sessions(ctx)
            .into_iter()
            .find(|session| Self::workspace_session_matches_action_target(session, target))
    }

    // ── 实时会话快照（来自当前窗口各 tab 的 pane group）─────────────────────

    pub(super) fn live_workspace_sessions(
        &self,
        ctx: &AppContext,
    ) -> Vec<WorkspaceSessionSnapshot> {
        let mut sessions = Vec::new();
        for (tab_index, tab) in self.tabs.iter().enumerate() {
            let pane_group = tab.pane_group.as_ref(ctx);
            let root = pane_group.snapshot(ctx);
            let tab_environment =
                Self::tab_environment_or_terminal_bootstrap_environment(Some(tab), &root);
            let tab_requires_runtime_sessions =
                !crate::workspace::environment_runtime::authority_uses_terminal_bootstrap(
                    &tab_environment.authority_key,
                );
            let placeholder_leaf_index = if tab_requires_runtime_sessions {
                Self::environment_runtime_placeholder_leaf_index(&root)
            } else {
                None
            };
            let focused_pane_index = if tab_index == self.active_tab_index {
                let focused_pane_id = pane_group.focused_pane_id(ctx);
                pane_group
                    .visible_pane_ids()
                    .iter()
                    .position(|pane_id| *pane_id == focused_pane_id)
            } else {
                None
            };
            let tab_snapshot = TabSnapshot {
                environment: Some(tab_environment.clone()),
                custom_title: pane_group.custom_title(ctx),
                root,
                default_directory_color: tab.default_directory_color,
                selected_color: tab.selected_color,
                left_panel: None,
                right_panel: None,
            };
            let tab_sessions = WorkspaceSessionSnapshot::from_tabs(&[tab_snapshot], None);
            for mut session in tab_sessions {
                if let Some((_, pane_index)) = Self::locator_from_restored_session_id(&session.id) {
                    if tab_requires_runtime_sessions {
                        let pane_uses_runtime = pane_group
                            .pane_id_from_index(pane_index)
                            .and_then(|pane_id| pane_group.terminal_view_from_pane_id(pane_id, ctx))
                            .and_then(|terminal_view| {
                                terminal_view
                                    .as_ref(ctx)
                                    .active_session_uses_environment_runtime(ctx)
                            })
                            .unwrap_or(false);
                        if !pane_uses_runtime {
                            continue;
                        }
                    }
                    session.id = format!("tab:{tab_index}:leaf:{pane_index}");
                    session.is_active = focused_pane_index == Some(pane_index);
                } else {
                    session.is_active = false;
                }
                sessions.push(session);
            }

            if let Some(placeholder_leaf_index) = placeholder_leaf_index {
                sessions.push(WorkspaceSessionSnapshot {
                    id: format!("tab:{tab_index}:leaf:{placeholder_leaf_index}"),
                    kind: WorkspaceSessionKind::Terminal,
                    label: pane_group
                        .custom_title(ctx)
                        .or_else(|| Some(tab_environment.label.clone())),
                    environment_authority_key: Some(tab_environment.authority_key.clone()),
                    cwd: tab_environment.active_workspace_root.clone(),
                    startup_directory: None,
                    cli_agent: None,
                    cli_command: None,
                    cli_agent_origin: None,
                    conversation_ids: Vec::new(),
                    active_conversation_id: None,
                    cli_agent_session_id: None,
                    is_active: focused_pane_index == Some(placeholder_leaf_index),
                    is_pinned: false,
                    updated_at_unix_ms: None,
                });
            }
        }
        sessions
    }

    // ── 已索引的 CLI-agent 会话（本地 current-app / 各环境）──────────────────

    pub(super) fn indexed_cli_agent_sessions_for_authority(
        &self,
        authority: &str,
    ) -> Vec<WorkspaceSessionSnapshot> {
        if crate::workspace::environment_runtime::authority_uses_terminal_bootstrap(authority) {
            return self.indexed_cli_agent_sessions.clone();
        }
        self.indexed_environment_cli_agent_sessions
            .get(authority)
            .cloned()
            .unwrap_or_default()
    }

    pub(super) fn all_indexed_environment_cli_agent_sessions(
        &self,
    ) -> Vec<WorkspaceSessionSnapshot> {
        self.indexed_environment_cli_agent_sessions
            .values()
            .flat_map(|sessions| sessions.iter().cloned())
            .collect()
    }

    pub(super) fn remember_indexed_environment_cli_agent_sessions(
        &mut self,
        authority: String,
        sessions: Vec<WorkspaceSessionSnapshot>,
    ) {
        self.indexed_environment_cli_agent_sessions
            .insert(authority, sessions);
    }

    pub(super) fn remember_indexed_environment_cli_agent_session_user_state(
        &mut self,
        authority: String,
        state: crate::workspace::environment_runtime::EnvironmentCliAgentSessionUserState,
    ) {
        self.indexed_environment_cli_agent_session_user_states
            .insert(authority, state);
    }

    pub(super) fn clear_indexed_environment_cli_agent_sessions_for_authority(
        &mut self,
        authority: &str,
    ) {
        self.indexed_environment_cli_agent_sessions
            .remove(authority);
        self.indexed_environment_cli_agent_session_user_states
            .remove(authority);
    }

    // ── 刷新生命周期 · 刷新/反馈 toast ─────────────────────────────────────

    pub(super) fn refresh_workspace_sessions(&mut self, ctx: &mut ViewContext<Self>) {
        let refresh_generation = self.begin_workspace_sessions_refresh(ctx);
        self.refresh_indexed_cli_agent_sessions();
        self.prune_restored_workspace_sessions_with_missing_cli_sources();
        self.prune_stale_restoring_workspace_session_keys(ctx);
        self.open_vertical_tabs_panel_for_recoverable_sessions(ctx);
        self.sync_session_navigator_sessions(ctx);
        let current_authority = self.current_environment_authority_key(ctx);
        if !crate::workspace::environment_runtime::authority_uses_terminal_bootstrap(
            &current_authority,
        ) && self.refresh_environment_cli_agent_sessions_for_authority_with_generation(
            &current_authority,
            Some(refresh_generation),
            ctx,
        ) {
            return;
        }

        let session_count = self.session_navigator_sessions(ctx).len();
        self.finish_workspace_sessions_refresh_if_current(
            refresh_generation,
            format!("已刷新会话列表：{session_count} 个会话"),
            ctx,
        );
    }

    pub(super) fn prune_stale_restoring_workspace_session_keys(&mut self, ctx: &AppContext) {
        if self.restoring_workspace_session_keys.is_empty() {
            return;
        }
        let live_keys = self
            .live_workspace_sessions(ctx)
            .into_iter()
            .flat_map(|session| {
                vec![
                    session.id.clone(),
                    Self::workspace_session_logical_key(&session),
                ]
            })
            .collect::<HashSet<_>>();
        self.restoring_workspace_session_keys
            .retain(|key| live_keys.contains(key));
    }

    pub(super) fn refresh_indexed_cli_agent_sessions(&mut self) {
        self.indexed_cli_agent_sessions = Self::scan_terminal_cli_agent_sessions(80);
    }

    fn prune_restored_workspace_sessions_with_missing_cli_sources(&mut self) {
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

    pub(super) fn is_workspace_sessions_refreshing(&self) -> bool {
        self.workspace_sessions_refresh_state.is_refreshing
    }

    pub(super) fn workspace_sessions_refresh_tooltip(&self) -> String {
        self.workspace_sessions_refresh_state
            .message
            .clone()
            .unwrap_or_else(|| "刷新会话列表".to_owned())
    }

    pub(super) fn begin_workspace_sessions_refresh(&mut self, ctx: &mut ViewContext<Self>) -> u64 {
        self.workspace_sessions_refresh_state.generation = self
            .workspace_sessions_refresh_state
            .generation
            .saturating_add(1);
        self.workspace_sessions_refresh_state.is_refreshing = true;
        self.workspace_sessions_refresh_state.message = Some("正在刷新会话列表…".to_owned());
        self.show_workspace_sessions_refresh_toast(
            DismissibleToast::default("正在刷新会话列表…".to_owned()),
            false,
            ctx,
        );
        ctx.notify();
        self.workspace_sessions_refresh_state.generation
    }

    pub(super) fn finish_workspace_sessions_refresh_if_current(
        &mut self,
        generation: u64,
        message: String,
        ctx: &mut ViewContext<Self>,
    ) {
        if self.workspace_sessions_refresh_state.generation != generation {
            return;
        }
        self.workspace_sessions_refresh_state.is_refreshing = false;
        self.workspace_sessions_refresh_state.message = Some(message.clone());
        self.show_workspace_sessions_refresh_toast(DismissibleToast::success(message), false, ctx);
        ctx.notify();
    }

    pub(super) fn fail_workspace_sessions_refresh_if_current(
        &mut self,
        generation: u64,
        message: String,
        ctx: &mut ViewContext<Self>,
    ) {
        if self.workspace_sessions_refresh_state.generation != generation {
            return;
        }
        self.workspace_sessions_refresh_state.is_refreshing = false;
        self.workspace_sessions_refresh_state.message = Some(message.clone());
        self.show_workspace_sessions_refresh_toast(DismissibleToast::error(message), true, ctx);
        ctx.notify();
    }

    pub(super) fn show_workspace_sessions_refresh_toast(
        &self,
        toast: DismissibleToast<WorkspaceAction>,
        persistent: bool,
        ctx: &mut ViewContext<Self>,
    ) {
        let toast = toast.with_object_id(WORKSPACE_SESSIONS_REFRESH_TOAST_ID.to_owned());
        self.toast_stack.update(ctx, |toast_stack, ctx| {
            if persistent {
                toast_stack.add_persistent_toast(toast, ctx);
            } else {
                toast_stack.add_ephemeral_toast(toast, ctx);
            }
        });
    }

    pub(super) fn show_workspace_session_success_toast(
        &self,
        message: String,
        ctx: &mut ViewContext<Self>,
    ) {
        self.toast_stack.update(ctx, |toast_stack, ctx| {
            toast_stack.add_ephemeral_toast(DismissibleToast::success(message), ctx);
        });
    }

    pub(super) fn show_workspace_session_error_toast(
        &self,
        message: String,
        ctx: &mut ViewContext<Self>,
    ) {
        self.toast_stack.update(ctx, |toast_stack, ctx| {
            toast_stack.add_persistent_toast(DismissibleToast::error(message), ctx);
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

        let mut keys = Self::workspace_session_pin_keys(&session);
        for backing_session in self.backing_sessions_for_workspace_session(&session) {
            keys.extend(Self::workspace_session_pin_keys(&backing_session));
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
            crate::workspace::environment_runtime::session_authority_or_terminal_bootstrap(
                session.environment_authority_key.as_deref(),
            )
            .to_owned();
        let mutation = if pinned {
            crate::workspace::environment_runtime::EnvironmentCliAgentSessionUserStateMutation::SetPinned
        } else {
            crate::workspace::environment_runtime::EnvironmentCliAgentSessionUserStateMutation::ClearPinned
        };
        if let Err(error) =
            self.mutate_workspace_session_user_state_for_authority(&authority, &keys, mutation, ctx)
        {
            log::warn!("toggle_workspace_session_pinned: {error}");
            self.show_workspace_session_error_toast(format!("置顶状态更新失败：{error}"), ctx);
            return;
        }
        self.refresh_workspace_sessions(ctx);
        let message = if pinned {
            "已置顶会话"
        } else {
            "已取消置顶"
        };
        self.show_workspace_session_success_toast(message.to_owned(), ctx);
    }

    // ── 恢复点激活 ─────────────────────────────────────────────────────────

    pub(super) fn activate_restored_workspace_session(
        &mut self,
        target: &WorkspaceSessionActionTarget,
        ctx: &mut ViewContext<Self>,
    ) {
        let Some(session) = self.workspace_session_for_action_target(target, ctx) else {
            log::warn!(
                "activate_restored_workspace_session: missing session {} in {:?}",
                target.session_id,
                target.environment_authority_key
            );
            return;
        };

        let current_authority = self.current_environment_authority_key(ctx);
        if !Self::session_matches_current_environment(&session, &current_authority) {
            let session_authority = crate::workspace::environment_runtime::session_authority_or_terminal_bootstrap(
                session.environment_authority_key.as_deref(),
            );
            let session_environment_label = crate::workspace::environment_runtime::session_environment_display_label(
                session_authority,
            )
            .unwrap_or_else(|| session_authority.to_string());
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
        if self.restoring_workspace_session_keys.contains(&session.id)
            || self.restoring_workspace_session_keys.contains(&logical_key)
        {
            log::info!(
                "activate_restored_workspace_session: session {} is already restoring",
                logical_key
            );
            return;
        }

        let locator = self.locator_for_workspace_session_snapshot(&session, ctx);

        if let Some(locator) = locator {
            self.focus_pane(locator, ctx);
            self.set_active_restored_workspace_session_key_for_session(&session, ctx);
            self.clear_workspace_session_restoring(&session);
            ctx.notify();
            return;
        }

        if let Some(conversation_id) =
            Self::conversation_id_from_ashide_conversation_session_id(&session.id)
        {
            self.restore_or_navigate_to_conversation(
                conversation_id,
                None,
                None,
                None,
                Some(RestoreConversationLayout::NewTab),
                Some(&session),
                ctx,
            );
            self.set_active_restored_workspace_session_key_for_session(&session, ctx);
            self.clear_workspace_session_restoring(&session);
            ctx.notify();
            return;
        }

        let initial_directory = session
            .cwd
            .as_deref()
            .or(session.startup_directory.as_deref())
            .map(PathBuf::from);
        let pending_command = if matches!(session.kind, WorkspaceSessionKind::AgentTerminal)
            || session.cli_agent.is_some()
            || session.cli_command.is_some()
        {
            Self::cli_agent_from_session(&session).and_then(|agent| {
                agent.explicit_resume_command(
                    session.cli_agent_session_id.as_deref(),
                    session.cwd.as_deref(),
                )
            })
        } else {
            None
        };

        self.restoring_workspace_session_keys
            .insert(session.id.clone());
        self.restoring_workspace_session_keys.insert(logical_key);
        ctx.notify();

        if session
            .environment_authority_key
            .as_deref()
            .and_then(environment_provider::runtime_connection_ref_from_authority)
            .is_some()
        {
            self.open_restored_environment_runtime_session(&session, pending_command, ctx);
            return;
        }

        let terminal_bootstrap_restore_command =
            Self::restored_terminal_bootstrap_startup_command(&session, pending_command);
        self.open_terminal_bootstrap_restored_session_terminal(
            initial_directory,
            &session,
            terminal_bootstrap_restore_command,
            ctx,
        );
        self.set_active_restored_workspace_session_key_for_session(&session, ctx);
        self.sync_session_navigator_sessions(ctx);
    }

    // ── 删除 / 删除后重选 ────────────────────────────────────────────

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
        let confirm_target = Self::workspace_session_action_target(&session);
        let dialog = AlertDialogWithCallbacks::for_view(
            dialog_title,
            dialog_message,
            vec![
                ModalButton::for_view(confirm_label, move |workspace: &mut Workspace, ctx| {
                    workspace.delete_workspace_session(&confirm_target, ctx);
                }),
                ModalButton::for_view(crate::t!("common-cancel"), |_: &mut Workspace, _| {}),
            ],
            |_, _| {},
        );
        ctx.show_native_platform_modal(dialog);
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

        self.clear_workspace_session_restoring(&session);
        let deleted_session_was_active = self.workspace_session_is_active_selection(&session, ctx);
        let backing_sessions = self.backing_sessions_for_workspace_session(&session);
        let mut alias_keys = self.workspace_session_alias_keys(&session);
        if !self.close_live_workspace_session_for_delete(&session, ctx) {
            self.show_workspace_session_error_toast(
                "无法关闭当前唯一会话窗口，删除已取消".to_owned(),
                ctx,
            );
            return;
        }

        for backing_session in &backing_sessions {
            alias_keys.extend(Self::workspace_session_alias_keys_for_session(
                backing_session,
            ));
        }
        alias_keys.sort();
        alias_keys.dedup();

        // Delete each CLI source in its environment FIRST; only clear Ashide
        // side-state once the provider delete confirms. Scan results remain
        // the source of truth, so failed deletes must not hide rows via a UI
        // side-state filter.
        #[cfg(all(feature = "local_fs", not(target_family = "wasm")))]
        {
            let mut seen = HashSet::new();
            let mut mutations = Vec::new();
            for backing in &backing_sessions {
                if !seen.insert(backing.id.clone()) {
                    continue;
                }
                if let Some((env, source_target)) =
                    self.session_source_mutation_for_backing(backing, ctx)
                {
                    let source_id = backing.id.clone();
                    mutations.push(async move {
                        env.mutate_session_source(
                            source_id,
                            source_target,
                            EnvironmentCliAgentSessionSourceAction::Delete,
                        )
                        .await
                    });
                }
            }

            let remove_ids: Vec<String> = backing_sessions.iter().map(|s| s.id.clone()).collect();
            let pin_keys: Vec<String> = backing_sessions
                .iter()
                .flat_map(Self::workspace_session_pin_keys)
                .collect();
            let session_for_reselect = session.clone();
            let user_state_authority =
                crate::workspace::environment_runtime::session_authority_or_terminal_bootstrap(
                    session.environment_authority_key.as_deref(),
                )
                .to_owned();

            let future = async move { futures::future::join_all(mutations).await };
            ctx.spawn(future, move |workspace, results, ctx| {
                if let Some(error) = results.into_iter().filter_map(Result::err).next() {
                    log::warn!("delete_workspace_session: {error}");
                    workspace.show_workspace_session_error_toast(
                        format!("删除会话来源失败，未改动本地状态：{error}"),
                        ctx,
                    );
                    return;
                }
                let remove_set: HashSet<&str> = remove_ids.iter().map(String::as_str).collect();
                workspace
                    .restored_workspace_sessions
                    .retain(|restored| !remove_set.contains(restored.id.as_str()));
                if !pin_keys.is_empty() {
                    if let Err(error) = workspace.mutate_workspace_session_user_state_for_authority(
                        &user_state_authority,
                        &pin_keys,
                        crate::workspace::environment_runtime::EnvironmentCliAgentSessionUserStateMutation::ClearPinned,
                        ctx,
                    ) {
                        log::warn!("delete_workspace_session: failed to clear pin keys: {error}");
                    }
                }
                if let Err(error) = workspace.mutate_workspace_session_user_state_for_authority(
                    &user_state_authority,
                    &alias_keys,
                    crate::workspace::environment_runtime::EnvironmentCliAgentSessionUserStateMutation::ClearAlias,
                    ctx,
                ) {
                    log::warn!("delete_workspace_session: failed to clear alias keys: {error}");
                }
                workspace.refresh_indexed_cli_agent_sessions();
                if deleted_session_was_active {
                    workspace.reselect_workspace_session_after_delete(&session_for_reselect, ctx);
                } else {
                    workspace.sync_session_navigator_sessions(ctx);
                }
                workspace.show_workspace_session_success_toast("已永久删除会话".to_owned(), ctx);
                ctx.notify();
            });
        }

        #[cfg(any(not(feature = "local_fs"), target_family = "wasm"))]
        {
            let mut session_source_delete_errors = Vec::new();
            let mut seen_source_ids = HashSet::new();
            for backing_session in &backing_sessions {
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
                self.show_workspace_session_error_toast(
                    format!("删除会话来源失败，未改动会话状态：{error}"),
                    ctx,
                );
                ctx.notify();
                return;
            }

            let user_state_authority =
                crate::workspace::environment_runtime::session_authority_or_terminal_bootstrap(
                    session.environment_authority_key.as_deref(),
                )
                .to_owned();
            let remove_ids = backing_sessions
                .iter()
                .map(|session| session.id.as_str())
                .collect::<HashSet<_>>();
            self.restored_workspace_sessions
                .retain(|restored_session| !remove_ids.contains(restored_session.id.as_str()));
            let pin_keys = backing_sessions
                .iter()
                .flat_map(Self::workspace_session_pin_keys)
                .collect::<Vec<_>>();
            if !pin_keys.is_empty() {
                if let Err(error) = self.mutate_workspace_session_user_state_for_authority(
                    &user_state_authority,
                    &pin_keys,
                    crate::workspace::environment_runtime::EnvironmentCliAgentSessionUserStateMutation::ClearPinned,
                    ctx,
                ) {
                    log::warn!("delete_workspace_session: failed to clear pin keys: {error}");
                }
            }
            if let Err(error) = self.mutate_workspace_session_user_state_for_authority(
                &user_state_authority,
                &alias_keys,
                crate::workspace::environment_runtime::EnvironmentCliAgentSessionUserStateMutation::ClearAlias,
                ctx,
            ) {
                log::warn!("delete_workspace_session: failed to clear alias keys: {error}");
            }

            self.refresh_indexed_cli_agent_sessions();
            if deleted_session_was_active {
                self.reselect_workspace_session_after_delete(&session, ctx);
            } else {
                self.sync_session_navigator_sessions(ctx);
            }
            self.show_workspace_session_success_toast("已永久删除会话".to_owned(), ctx);
            ctx.notify();
        }
    }

    pub(super) fn workspace_session_is_active_selection(
        &self,
        session: &WorkspaceSessionSnapshot,
        ctx: &AppContext,
    ) -> bool {
        session.is_active
            || self
                .active_restored_workspace_session_key
                .as_deref()
                .is_some_and(|active_key| {
                    active_key == Self::workspace_session_logical_key(session)
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

    pub(super) fn reselect_workspace_session_after_delete(
        &mut self,
        deleted_session: &WorkspaceSessionSnapshot,
        ctx: &mut ViewContext<Self>,
    ) {
        self.clear_active_restored_workspace_session_if_matches(deleted_session, ctx);
        let target_authority =
            crate::workspace::environment_runtime::session_authority_or_terminal_bootstrap(
                deleted_session.environment_authority_key.as_deref(),
            );
        let deleted_key = Self::workspace_session_logical_key(deleted_session);

        let replacement_locator = self
            .live_workspace_sessions(ctx)
            .into_iter()
            .filter(|candidate| {
                Self::session_matches_current_environment(candidate, target_authority)
                    && Self::workspace_session_logical_key(candidate) != deleted_key
            })
            .filter_map(|candidate| self.locator_for_workspace_session_snapshot(&candidate, ctx))
            .next();

        if let Some(locator) = replacement_locator {
            self.focus_pane(locator, ctx);
            self.sync_session_navigator_sessions(ctx);
            return;
        }

        if let Some(tab_index) = self.tab_index_for_environment_authority(target_authority) {
            self.activate_tab(tab_index, ctx);
        } else if crate::workspace::environment_runtime::authority_uses_terminal_bootstrap(
            target_authority,
        ) {
            // 删除的是本地 / terminal-bootstrap 会话:它所在的 tab `environment == None`,
            // `tab_index_for_environment_authority` 永远匹配不到。`close_live_workspace_session_for_delete`
            // 关掉该 tab 后,标准 close_tab 行为可能已把焦点自动切到相邻的**远程环境** tab。
            // 若当前激活 tab 已不是本地,把焦点拉回一个本地(environment == None)tab——
            // 避免「删本地 tab 的会话却跳到其它环境」的回归。
            // SSTAB-008:若已无任何本地 tab 可回(被删会话独占唯一本地 tab),新开一个本地
            // terminal-bootstrap tab,绝不把用户留在远程环境 tab 上。
            let active_is_local = self
                .tabs
                .get(self.active_tab_index)
                .is_some_and(|tab| tab.environment.is_none());
            if !active_is_local {
                if let Some(local_tab_index) =
                    self.tabs.iter().position(|tab| tab.environment.is_none())
                {
                    self.activate_tab(local_tab_index, ctx);
                } else {
                    // 无本地 tab 可回 —— 开一个新的本地 terminal-bootstrap tab。
                    self.add_explicit_terminal_bootstrap_default_tab(None, ctx);
                }
            }
        } else {
            if self.ensure_environment_tab_for_authority(target_authority, ctx) {
                self.prepare_active_environment_after_visible_tab_activation(ctx);
            }
        }
        self.sync_session_navigator_sessions(ctx);
    }

    pub(super) fn close_live_workspace_session_for_delete(
        &mut self,
        session: &WorkspaceSessionSnapshot,
        ctx: &mut ViewContext<Self>,
    ) -> bool {
        let live_session = self
            .live_workspace_sessions(ctx)
            .into_iter()
            .find(|candidate| Self::is_same_workspace_session(session, candidate));
        let Some(live_session) = live_session else {
            return true;
        };
        let Some((tab_index, pane_index)) =
            Self::locator_from_restored_session_id(&live_session.id)
        else {
            return true;
        };
        if self
            .locator_for_workspace_session_snapshot(&live_session, ctx)
            .is_none()
        {
            log::warn!(
                "close_live_workspace_session_for_delete: refusing cross-environment live close for {}",
                live_session.id
            );
            return true;
        }

        if self.tabs.len() == 1 && !ContextFlag::CloseWindow.is_enabled() {
            let Some(pane_group) = self
                .tabs
                .get(tab_index)
                .map(|tab| tab.pane_group.as_ref(ctx))
            else {
                return true;
            };
            if pane_group.visible_pane_ids().len() == 1 {
                return false;
            }
        }

        let Some(locator) = self.locator_for_tab_pane_index(tab_index, pane_index, ctx) else {
            return true;
        };
        let Some(pane_group) = self.tabs.get(tab_index).map(|tab| tab.pane_group.clone()) else {
            return true;
        };
        let visible_pane_count = pane_group.as_ref(ctx).visible_pane_ids().len();
        if visible_pane_count <= 1 {
            self.close_tab(tab_index, true, false, ctx);
            return true;
        }

        pane_group.update(ctx, |pane_group, ctx| {
            pane_group.close_pane_permanently(locator.pane_id, ctx);
        });
        true
    }

    // ── 恢复中状态清理 ─────────────────────────────────────────────────────

    pub(super) fn clear_workspace_session_restoring(&mut self, session: &WorkspaceSessionSnapshot) {
        self.restoring_workspace_session_keys.remove(&session.id);
        self.restoring_workspace_session_keys
            .remove(&Self::workspace_session_logical_key(session));
    }

    pub(super) fn clear_workspace_session_restoring_for_authority(
        &mut self,
        authority: &str,
        ctx: &AppContext,
    ) {
        let mut keys = self
            .live_workspace_sessions(ctx)
            .into_iter()
            .chain(self.indexed_cli_agent_sessions_for_authority(authority))
            .chain(self.restored_workspace_sessions.clone())
            .filter(|session| {
                crate::workspace::environment_runtime::session_authority_matches(
                    session.environment_authority_key.as_deref(),
                    authority,
                )
            })
            .flat_map(|session| {
                let logical_key = Self::workspace_session_logical_key(&session);
                [session.id, logical_key]
            })
            .collect::<HashSet<_>>();

        let authority_prefix = format!("{authority}::");
        self.restoring_workspace_session_keys
            .retain(|key| !keys.remove(key) && !key.starts_with(&authority_prefix));
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
        let environment_host_key = Self::workspace_session_environment_host_key(session);
        CLIAgentSessionsModel::handle(ctx).update(ctx, |sessions, ctx| {
            sessions.set_session(
                terminal_view_id,
                CLIAgentSession {
                    agent,
                    status: CLIAgentSessionStatus::InProgress,
                    session_context: CLIAgentSessionContext {
                        cwd: session.cwd.clone(),
                        session_id: session.cli_agent_session_id.clone(),
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
    ) -> Option<String> {
        let authority = session.environment_authority_key.as_deref()?;
        let connection_ref =
            environment_provider::runtime_connection_ref_from_authority(authority)?;
        environment_provider::runtime_transport_descriptor_for_connection_ref(&connection_ref)
            .map(|descriptor| descriptor.target())
            .or_else(|| Some(authority.to_owned()))
    }

    pub(super) fn locator_for_workspace_session_snapshot(
        &self,
        session: &WorkspaceSessionSnapshot,
        ctx: &AppContext,
    ) -> Option<PaneViewLocator> {
        let (tab_index, pane_index) = Self::locator_from_restored_session_id(&session.id)?;
        let session_authority =
            crate::workspace::environment_runtime::session_authority_or_terminal_bootstrap(
                session.environment_authority_key.as_deref(),
            );
        let tab_authority = self.tab_environment_authority_for_index(tab_index, ctx)?;
        if !crate::workspace::environment_runtime::session_authority_matches(
            Some(tab_authority.as_str()),
            session_authority,
        ) {
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
        } else if lower_command.contains("gemini") {
            "Gemini".to_string()
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
            menu_items.extend(session_bridge_items);
            menu_items.push(MenuItem::Separator);
        } else if let Some(fork_items) =
            self.cli_agent_session_bridge_menu_items_for_workspace_session(&session)
        {
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

        if !Self::workspace_session_pin_keys(&session).is_empty() {
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

        menu_items.extend([MenuItemFields::new(crate::t!(
            "workspace-session-navigator-menu-rename-alias"
        ))
        .with_on_select_action(WorkspaceAction::RequestRenameWorkspaceSession {
            target: session_target.clone(),
        })
        .into_item()]);

        if self.workspace_session_alias(&session).is_some() {
            menu_items.push(
                MenuItemFields::new(crate::t!("workspace-session-navigator-menu-clear-alias"))
                    .with_on_select_action(WorkspaceAction::ClearWorkspaceSessionAlias {
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

        if !is_restoring {
            menu_items.push(
                MenuItemFields::new(if is_live_in_current_environment {
                    "退出并删除…"
                } else {
                    "永久删除…"
                })
                .with_on_select_action(WorkspaceAction::RequestDeleteWorkspaceSession {
                    target: session_target,
                })
                .into_item(),
            );
        } else {
            menu_items.push(MenuItem::Separator);
            menu_items.push(
                MenuItemFields::new("永久删除…")
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
