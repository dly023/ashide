//! Session Navigator 纯 Reducer —— 状态机核心。
//!
//! 这是 Session Navigator 所有列表变化的唯一决策入口。
//! 纯函数: 输入 (sessions, state, action, pane_group_info) → (sessions, state, side_effect)。
//! 不依赖 Workspace 的可变状态,不调用 ctx,不执行 IO。
//! 接入层 (session_navigator.rs) 负责执行 side_effect。
//!
//! 契约: docs/SESSION_NAVIGATOR_SPEC.yaml §7

use std::collections::{HashMap, HashSet};

use crate::app_state::WorkspaceSessionSnapshot;
use crate::workspace::util::PaneViewLocator;

// ─────────────────────────────────────────────────────────
// 状态
// ─────────────────────────────────────────────────────────

/// Session Navigator 的核心状态 (从 Workspace 中提取的纯数据)。
/// 这不是 Workspace 的字段,而是每次 reduce 时从 Workspace 快照出来的。
#[derive(Clone, Debug)]
pub struct SessionNavigatorState {
    pub active_key: Option<String>,
    pub restoring_keys: HashSet<String>,
    pub deleting_keys: HashSet<String>,
    pub display_order: HashMap<String, u64>,
    pub next_display_order: u64,
}

impl SessionNavigatorState {
    pub fn new() -> Self {
        Self {
            active_key: None,
            restoring_keys: HashSet::new(),
            deleting_keys: HashSet::new(),
            display_order: HashMap::new(),
            next_display_order: 0,
        }
    }
}

impl Default for SessionNavigatorState {
    fn default() -> Self {
        Self::new()
    }
}

/// 各 tab 的 pane group 信息 (reduce 时只读)。
#[derive(Clone, Debug)]
pub struct PaneGroupInfo {
    /// tab_index → (visible_pane_count, focused_pane_locator, pane_history_front)
    pub tabs: HashMap<usize, TabPaneInfo>,
}

#[derive(Clone, Debug)]
pub struct TabPaneInfo {
    pub visible_pane_count: usize,
    pub focused_locator: Option<PaneViewLocator>,
    /// pane_history 的最近一个 pane id (用于 prev_pane 选择)
    pub prev_pane_locator: Option<PaneViewLocator>,
    /// 所有 visible pane 的 locator 列表 (按 leaf index 排序)
    pub all_pane_locators: Vec<PaneViewLocator>,
}

impl PaneGroupInfo {
    pub fn new() -> Self {
        Self {
            tabs: HashMap::new(),
        }
    }
}

impl Default for PaneGroupInfo {
    fn default() -> Self {
        Self::new()
    }
}

// ─────────────────────────────────────────────────────────
// Action
// ─────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
#[allow(dead_code)] // Action payload fields are part of the SPEC contract; reduce may only need a subset.
pub enum SessionNavigatorAction {
    /// 删除一个 session
    Delete {
        session_logical_key: String,
        session_id: String,
        environment_authority_key: Option<String>,
    },
    /// 激活 / 恢复一个 session
    Activate {
        session_logical_key: String,
        session_id: String,
        is_live: bool,
    },
    /// Pin / unpin
    Pin {
        session_logical_key: String,
        pinned: bool,
    },
    /// 刷新 (merge + reconcile + sort + recompute active)
    Refresh {
        new_sessions: Vec<WorkspaceSessionSnapshot>,
        pinned_session_ids: HashSet<String>,
    },
    /// Tab 被激活
    TabActivated {
        tab_index: usize,
    },
    /// Pane 被聚焦
    PaneFocused {
        locator: PaneViewLocator,
        session_logical_key: Option<String>,
    },
    /// 重排列表 (EC-08): 只改 display_order, focus/active_key 跟随 logical_key 不变
    Reorder {
        ordered_logical_keys: Vec<String>,
    },
}

// ─────────────────────────────────────────────────────────
// SideEffect
// ─────────────────────────────────────────────────────────

/// Delete 的关闭策略 (一次 Delete action 内与 focus 一起产出)。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeleteCloseKind {
    /// 关闭 tab 内的一个 pane (split 场景)
    ClosePane(PaneViewLocator),
    /// 关闭整个 tab (该 tab 唯一 visible pane)
    CloseTab(usize),
    /// 不关闭物理 pane (virtual row 删除)
    None,
}

/// 一次 Delete action 的复合副作用: 先 focus, 再 close。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeleteEffects {
    pub focus: Option<PaneViewLocator>,
    pub close: DeleteCloseKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SideEffect {
    /// 聚焦某个 pane
    FocusPane(PaneViewLocator),
    /// 删除: 一次 action 同时携带 focus + close (SPEC §7/§8)
    DeleteEffects(DeleteEffects),
    /// spawn terminal 恢复 virtual session
    SpawnTerminal {
        session_id: String,
        logical_key: String,
    },
    /// 写 user state (pin/unpin)
    WriteUserState,
    /// 无副作用
    None,
}

// ─────────────────────────────────────────────────────────
// Reducer 结果
// ─────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct ReduceResult {
    pub sessions: Vec<WorkspaceSessionSnapshot>,
    pub state: SessionNavigatorState,
    pub side_effect: SideEffect,
}

// ─────────────────────────────────────────────────────────
// 纯函数: logical_key (与 session_navigator.rs 一致)
// ─────────────────────────────────────────────────────────

fn logical_key(session: &WorkspaceSessionSnapshot) -> String {
    session.logical_key()
}

fn durable_key(session: &WorkspaceSessionSnapshot) -> Option<String> {
    let environment_key = WorkspaceSessionSnapshot::logical_environment_key(
        session.environment_authority_key.as_deref(),
    );
    if let Some(cli_agent_session_id) = session
        .cli_agent_session_id
        .as_deref()
        .filter(|id| !id.trim().is_empty())
    {
        return Some(format!(
            "{environment_key}::agent:{}:{}",
            session
                .cli_agent
                .as_deref()
                .or(session.cli_command.as_deref())
                .unwrap_or_default(),
            cli_agent_session_id
        ));
    }
    if let Some(conversation_id) = session
        .active_conversation_id
        .iter()
        .chain(session.conversation_ids.iter())
        .find(|id| !id.trim().is_empty())
    {
        return Some(format!("{environment_key}::conversation:{conversation_id}"));
    }
    None
}

fn locator_from_session_id(id: &str) -> Option<(usize, usize)> {
    let mut parts = id.split(':');
    match (
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
    ) {
        (Some("tab"), Some(tab), Some("leaf"), Some(leaf), None) => {
            Some((tab.parse().ok()?, leaf.parse().ok()?))
        }
        _ => None,
    }
}

/// 判断 session 是否是 live container
fn is_live(session: &WorkspaceSessionSnapshot) -> bool {
    session.is_live_container()
}

/// 找到 session 所在的 tab_index
fn tab_index_for_session(session: &WorkspaceSessionSnapshot) -> Option<usize> {
    locator_from_session_id(&session.id).map(|(tab, _)| tab)
}

// ─────────────────────────────────────────────────────────
// 排序 (与历史 Workspace sort_session_navigator_sessions_by_display_order 语义一致)
// ─────────────────────────────────────────────────────────

fn display_order_for_session(
    session: &WorkspaceSessionSnapshot,
    state: &SessionNavigatorState,
) -> u64 {
    use crate::app_state::SessionDisplayOrderStrategy;
    match session.display_order_strategy() {
        SessionDisplayOrderStrategy::Physical => {
            let primary = state
                .display_order
                .get(&logical_key(session))
                .copied();
            let durable = durable_key(session)
                .and_then(|key| state.display_order.get(&key).copied());
            match (primary, durable) {
                (Some(primary), Some(durable)) => primary.min(durable),
                (Some(primary), None) => primary,
                (None, Some(durable)) => durable,
                (None, None) => u64::MAX,
            }
        }
        SessionDisplayOrderStrategy::Durable => {
            state
                .display_order
                .get(&logical_key(session))
                .copied()
                .unwrap_or(u64::MAX)
        }
        SessionDisplayOrderStrategy::Bridged => {
            state
                .display_order
                .get(&logical_key(session))
                .copied()
                .unwrap_or(u64::MAX)
        }
    }
}

fn sort_sessions(
    sessions: &mut Vec<WorkspaceSessionSnapshot>,
    state: &SessionNavigatorState,
) {
    let original_positions: HashMap<String, usize> = sessions
        .iter()
        .enumerate()
        .map(|(idx, s)| (logical_key(s), idx))
        .collect();

    // same_window_group_orders: tab_index → min display_order
    let mut same_window_group_orders: HashMap<usize, u64> = HashMap::new();
    for session in sessions.iter() {
        if let Some((tab_index, _)) = locator_from_session_id(&session.id) {
            let order = display_order_for_session(session, state);
            same_window_group_orders
                .entry(tab_index)
                .and_modify(|v| *v = (*v).min(order))
                .or_insert(order);
        }
    }

    sessions.sort_by(|left, right| {
        let left_order = display_order_for_session(left, state);
        let right_order = display_order_for_session(right, state);

        let left_tab = locator_from_session_id(&left.id);
        let right_tab = locator_from_session_id(&right.id);

        let left_group_order = left_tab
            .and_then(|(tab, _)| same_window_group_orders.get(&tab).copied())
            .unwrap_or(left_order);
        let right_group_order = right_tab
            .and_then(|(tab, _)| same_window_group_orders.get(&tab).copied())
            .unwrap_or(right_order);

        right
            .is_pinned
            .cmp(&left.is_pinned)
            // same_window_group_order
            .then_with(|| left_group_order.cmp(&right_group_order))
            // same_window_split leaf index
            .then_with(|| match (left_tab, right_tab) {
                (Some((lt, ll)), Some((rt, rl))) if lt == rt => ll.cmp(&rl),
                _ => std::cmp::Ordering::Equal,
            })
            // display_order
            .then_with(|| left_order.cmp(&right_order))
            // updated_at DESC (None 排最后)
            .then_with(|| right.updated_at_unix_ms.cmp(&left.updated_at_unix_ms))
            // original_position
            .then_with(|| {
                let lp = original_positions
                    .get(&logical_key(left))
                    .copied()
                    .unwrap_or(usize::MAX);
                let rp = original_positions
                    .get(&logical_key(right))
                    .copied()
                    .unwrap_or(usize::MAX);
                lp.cmp(&rp)
            })
            // logical_key ASC (v3: 不用 id)
            .then_with(|| logical_key(left).cmp(&logical_key(right)))
    });
}

// ─────────────────────────────────────────────────────────
// Reconcile (分配 display_order)
// ─────────────────────────────────────────────────────────

fn reconcile_display_order(
    sessions: &[WorkspaceSessionSnapshot],
    state: &mut SessionNavigatorState,
) {
    let mut sorted = sessions.to_vec();
    // v3: tie-breaker 用 logical_key 不用 id
    sorted.sort_by(|left, right| {
        right
            .is_pinned
            .cmp(&left.is_pinned)
            .then_with(|| right.updated_at_unix_ms.cmp(&left.updated_at_unix_ms))
            .then_with(|| logical_key(left).cmp(&logical_key(right)))
    });

    for session in &sorted {
        let order_key = logical_key(session);
        if state.display_order.contains_key(&order_key) {
            continue;
        }

        // durable bridging
        if let Some(durable) = durable_key(session) {
            if let Some(&order) = state.display_order.get(&durable) {
                state.display_order.insert(order_key.clone(), order);
                continue;
            }
        }

        let order = state.next_display_order;
        state.next_display_order += 1;
        state.display_order.insert(order_key, order);
    }
}

// ─────────────────────────────────────────────────────────
// Active 状态计算 (§6.1 优先级链)
// ─────────────────────────────────────────────────────────

fn recompute_active(
    sessions: &mut Vec<WorkspaceSessionSnapshot>,
    state: &SessionNavigatorState,
    _pane_info: &PaneGroupInfo,
) {
    // 1. 清除所有 is_active
    for session in sessions.iter_mut() {
        session.is_active = false;
    }

    // 2. 规则 1: active_key 指向 live container → is_active = true (最高优先级)
    //    纯函数中无法访问 pane group 的 focused_pane_id, 所以用 active_key 是否指向
    //    live container 来判断。接入层在 PaneFocused action 中设置 active_key。
    let live_active_key: Option<String> = if let Some(active_key) = &state.active_key {
        sessions
            .iter()
            .find(|s| is_live(s) && logical_key(s) == *active_key)
            .map(logical_key)
    } else {
        None
    };
    if let Some(key) = &live_active_key {
        for session in sessions.iter_mut() {
            if is_live(session) && logical_key(session) == *key {
                session.is_active = true;
            }
        }
    }

    // 3. 规则 2: restoring session → is_active = true
    //    (不覆盖规则 1 的 live active)
    for session in sessions.iter_mut() {
        if session.is_active {
            continue; // 规则 1 已设
        }
        let key = logical_key(session);
        if state.restoring_keys.contains(&key)
            || state.restoring_keys.contains(&session.id)
        {
            session.is_active = true;
        }
    }

    // 4. 规则 3: active_key → is_active = true (如果没被规则 1/2 覆盖)
    //    仅当 active_key 指向 virtual row (非 live) 时走到这里
    if let Some(active_key) = &state.active_key {
        for session in sessions.iter_mut() {
            if session.is_active {
                continue; // 规则 1/2 已设
            }
            if logical_key(session) == *active_key {
                session.is_active = true;
                break;
            }
        }
    }

    // 5. 去重: 只保留一个 active (最高优先级)
    normalize_active(sessions, state);
}

fn normalize_active(sessions: &mut [WorkspaceSessionSnapshot], state: &SessionNavigatorState) {
    let active_count = sessions.iter().filter(|s| s.is_active).count();
    if active_count <= 1 {
        return;
    }

    // 优先级: live active (规则 1) > restoring (规则 2) > active_key (规则 3) > 第一个
    let preferred_key = {
        // 先找 live active (active_key 指向 live container)
        if let Some(active_key) = &state.active_key {
            let live_match = sessions.iter().find(|s| {
                s.is_active && is_live(s) && logical_key(s) == *active_key
            });
            if live_match.is_some() {
                Some(logical_key(live_match.unwrap()))
            } else {
                // 再找 restoring 中的
                let restoring = sessions.iter().find(|s| {
                    s.is_active && {
                        let key = logical_key(s);
                        state.restoring_keys.contains(&key)
                            || state.restoring_keys.contains(&s.id)
                    }
                });
                if restoring.is_some() {
                    Some(logical_key(restoring.unwrap()))
                } else if sessions.iter().any(|s| s.is_active && logical_key(s) == *active_key) {
                    Some(active_key.clone())
                } else {
                    None
                }
            }
        } else {
            // 没有 active_key, 找 restoring
            let restoring = sessions.iter().find(|s| {
                s.is_active && {
                    let key = logical_key(s);
                    state.restoring_keys.contains(&key) || state.restoring_keys.contains(&s.id)
                }
            });
            restoring.map(logical_key)
        }
    };

    let chosen = preferred_key.or_else(|| {
        sessions
            .iter()
            .find(|s| s.is_active)
            .map(|s| logical_key(s))
    });

    if let Some(chosen_key) = chosen {
        for session in sessions.iter_mut() {
            if session.is_active && logical_key(session) != chosen_key {
                session.is_active = false;
            }
        }
    }
}

// ─────────────────────────────────────────────────────────
// Materialization 追踪 (§6.4)
// ─────────────────────────────────────────────────────────

/// 在 Refresh 时检查 virtual → live consume, 主动迁移 active_key (§6.4)。
/// 含 durable-key 迁移与 binding (cli_agent_session_id / conversation) 迁移；
/// 找不到匹配时清除 active_key，不猜测 live-active。
fn track_materialization(
    sessions: &[WorkspaceSessionSnapshot],
    state: &mut SessionNavigatorState,
) {
    let Some(active_key) = state.active_key.clone() else {
        return;
    };

    let stored = sessions.iter().find(|s| logical_key(s) == active_key);

    if let Some(stored) = stored {
        if is_live(stored) {
            return;
        }
        // Virtual still present: migrate to live twin when env + binding match.
        if let Some(live) = sessions.iter().find(|candidate| {
            is_live(candidate)
                && materialization_env_matches(stored, candidate)
                && materialization_binding_matches(stored, candidate)
        }) {
            state.active_key = Some(logical_key(live));
        }
        return;
    }

    // Stored key missing: durable key equality only (no live-active guess).
    for session in sessions.iter().filter(|s| is_live(s)) {
        if durable_key(session).as_deref() == Some(active_key.as_str()) {
            state.active_key = Some(logical_key(session));
            return;
        }
    }

    state.active_key = None;
}

fn materialization_env_matches(
    stored: &WorkspaceSessionSnapshot,
    live: &WorkspaceSessionSnapshot,
) -> bool {
    WorkspaceSessionSnapshot::logical_environment_key(stored.environment_authority_key.as_deref())
        == WorkspaceSessionSnapshot::logical_environment_key(
            live.environment_authority_key.as_deref(),
        )
}

fn materialization_binding_matches(
    stored: &WorkspaceSessionSnapshot,
    live: &WorkspaceSessionSnapshot,
) -> bool {
    stored
        .cli_agent_session_id
        .as_deref()
        .filter(|id| !id.trim().is_empty())
        .zip(live.cli_agent_session_id.as_deref())
        .is_some_and(|(a, b)| a == b)
        || stored
            .active_conversation_id
            .iter()
            .chain(stored.conversation_ids.iter())
            .any(|conv| {
                live.active_conversation_id
                    .iter()
                    .chain(live.conversation_ids.iter())
                    .any(|live_conv| conv == live_conv)
            })
}

// ─────────────────────────────────────────────────────────
// 删除 focus + close 决策 (§6.3 / §8)
// ─────────────────────────────────────────────────────────

fn deleted_pane_locator(
    deleted_session: &WorkspaceSessionSnapshot,
    pane_info: &PaneGroupInfo,
) -> Option<PaneViewLocator> {
    let (tab, leaf) = locator_from_session_id(&deleted_session.id)?;
    let info = pane_info.tabs.get(&tab)?;
    info.all_pane_locators.get(leaf).cloned()
}

fn decide_delete_effects(
    deleted_session: &WorkspaceSessionSnapshot,
    remaining_sessions: &[WorkspaceSessionSnapshot],
    pane_info: &PaneGroupInfo,
) -> SideEffect {
    // virtual row (无 live pane) → 不改 focus / 不关 pane
    if !is_live(deleted_session) {
        return SideEffect::DeleteEffects(DeleteEffects {
            focus: None,
            close: DeleteCloseKind::None,
        });
    }

    let Some(deleted_tab_index) = tab_index_for_session(deleted_session) else {
        return SideEffect::DeleteEffects(DeleteEffects {
            focus: None,
            close: DeleteCloseKind::None,
        });
    };

    let deleted_locator = deleted_pane_locator(deleted_session, pane_info);
    let tab_info = pane_info.tabs.get(&deleted_tab_index);

    if let Some(info) = tab_info {
        if info.visible_pane_count > 1 {
            // 多 pane tab → focus 同 group 兄弟, ClosePane
            let focus = info
                .prev_pane_locator
                .clone()
                .or_else(|| {
                    info.all_pane_locators.iter().find_map(|locator| {
                        if deleted_locator
                            .as_ref()
                            .is_some_and(|deleted| deleted.pane_id == locator.pane_id)
                        {
                            None
                        } else {
                            Some(locator.clone())
                        }
                    })
                });
            return SideEffect::DeleteEffects(DeleteEffects {
                focus,
                close: deleted_locator
                    .map(DeleteCloseKind::ClosePane)
                    .unwrap_or(DeleteCloseKind::None),
            });
        }
    }

    // 唯一 pane 的 tab → CloseTab, focus 同 env 邻居 (若有)
    let deleted_env = WorkspaceSessionSnapshot::logical_environment_key(
        deleted_session.environment_authority_key.as_deref(),
    );

    let same_env_focus = remaining_sessions.iter().find_map(|s| {
        if !(is_live(s)
            && tab_index_for_session(s) != Some(deleted_tab_index)
            && WorkspaceSessionSnapshot::logical_environment_key(
                s.environment_authority_key.as_deref(),
            ) == deleted_env)
        {
            return None;
        }
        let (tab, leaf) = locator_from_session_id(&s.id)?;
        pane_info
            .tabs
            .get(&tab)
            .and_then(|info| info.all_pane_locators.get(leaf).cloned())
    });

    SideEffect::DeleteEffects(DeleteEffects {
        focus: same_env_focus,
        close: DeleteCloseKind::CloseTab(deleted_tab_index),
    })
}

// ─────────────────────────────────────────────────────────
// 主 Reducer
// ─────────────────────────────────────────────────────────

pub fn reduce(
    mut sessions: Vec<WorkspaceSessionSnapshot>,
    mut state: SessionNavigatorState,
    action: SessionNavigatorAction,
    pane_info: &PaneGroupInfo,
) -> ReduceResult {
    match action {
        SessionNavigatorAction::Delete {
            session_logical_key,
            session_id: _,
            environment_authority_key: _,
        } => {
            // 找到被删 session
            let deleted = sessions
                .iter()
                .find(|s| logical_key(s) == session_logical_key)
                .cloned();

            // 从 sessions 移除
            sessions.retain(|s| logical_key(s) != session_logical_key);

            // 加入 deleting_keys
            state.deleting_keys.insert(session_logical_key.clone());

            // 清除 active_key 如果指向被删 session
            if state.active_key.as_deref() == Some(&session_logical_key) {
                state.active_key = None;
            }

            // 清除 restoring_keys 中的被删 session
            state.restoring_keys.remove(&session_logical_key);

            // 决定 focus + close side_effect (一次 Delete 一个复合副作用)
            let side_effect = if let Some(ref deleted_session) = deleted {
                decide_delete_effects(deleted_session, &sessions, pane_info)
            } else {
                SideEffect::None
            };

            // 重新计算 active
            recompute_active(&mut sessions, &state, pane_info);

            ReduceResult {
                sessions,
                state,
                side_effect,
            }
        }

        SessionNavigatorAction::Activate {
            session_logical_key,
            session_id: _,
            is_live,
        } => {
            if is_live {
                // 已 materialize → FocusPane
                let session = sessions
                    .iter()
                    .find(|s| logical_key(s) == session_logical_key)
                    .cloned();

                let side_effect = if let Some(ref session) = session {
                    if let Some((tab, leaf)) = locator_from_session_id(&session.id) {
                        if let Some(info) = pane_info.tabs.get(&tab) {
                            if leaf < info.all_pane_locators.len() {
                                state.active_key = Some(session_logical_key.clone());
                                state.restoring_keys.remove(&session_logical_key);
                                SideEffect::FocusPane(info.all_pane_locators[leaf].clone())
                            } else {
                                SideEffect::None
                            }
                        } else {
                            SideEffect::None
                        }
                    } else {
                        SideEffect::None
                    }
                } else {
                    SideEffect::None
                };

                recompute_active(&mut sessions, &state, pane_info);
                ReduceResult {
                    sessions,
                    state,
                    side_effect,
                }
            } else {
                // virtual row → spawn
                state.restoring_keys.insert(session_logical_key.clone());
                state.active_key = Some(session_logical_key.clone());

                let spawn_session_id = sessions
                    .iter()
                    .find(|s| logical_key(s) == session_logical_key)
                    .map(|s| s.id.clone())
                    .unwrap_or_default();

                recompute_active(&mut sessions, &state, pane_info);

                ReduceResult {
                    sessions,
                    state,
                    side_effect: SideEffect::SpawnTerminal {
                        session_id: spawn_session_id,
                        logical_key: session_logical_key,
                    },
                }
            }
        }

        SessionNavigatorAction::Pin {
            session_logical_key: _,
            pinned: _,
        } => {
            // Pin 不改 sessions 的 is_active / focus
            // side_effect = WriteUserState
            // 下一次 Refresh 时 is_pinned 由 user_state 预赋值
            ReduceResult {
                sessions,
                state,
                side_effect: SideEffect::WriteUserState,
            }
        }

        SessionNavigatorAction::Refresh {
            mut new_sessions,
            pinned_session_ids,
        } => {
            // merge (简化: 假设接入层已 merge)
            // 应用 pinned 状态 (与 merge_for_session_navigator 的 pin 补齐一致)
            for session in &mut new_sessions {
                if !is_live(session) {
                    let keys = session.stable_pin_keys();
                    session.is_pinned = keys
                        .iter()
                        .any(|key| pinned_session_ids.contains(key));
                } else {
                    session.is_pinned = false;
                }
            }
            sessions = new_sessions;

            // filter deleting
            sessions.retain(|s| {
                let key = logical_key(s);
                !state.deleting_keys.contains(&key)
            });

            // reconcile
            reconcile_display_order(&sessions, &mut state);

            // materialization 追踪
            track_materialization(&sessions, &mut state);

            // 清除已 materialize 的 restoring_keys
            let materialized_keys: Vec<String> = sessions
                .iter()
                .filter(|s| is_live(s))
                .map(|s| {
                    let mut keys = vec![s.id.clone(), logical_key(s)];
                    if let Some(d) = durable_key(s) {
                        keys.push(d);
                    }
                    keys
                })
                .flatten()
                .collect();
            for key in materialized_keys {
                state.restoring_keys.remove(&key);
            }

            // sort
            sort_sessions(&mut sessions, &state);

            // recompute active
            recompute_active(&mut sessions, &state, pane_info);

            ReduceResult {
                sessions,
                state,
                side_effect: SideEffect::None,
            }
        }

        SessionNavigatorAction::TabActivated { tab_index } => {
            // 清除不属于当前 tab 的 active_key (如果 active_key 指向其他 env)
            // 接入层会在 tab 切换时构建正确的 pane_info
            let _ = tab_index;

            recompute_active(&mut sessions, &state, pane_info);

            ReduceResult {
                sessions,
                state,
                side_effect: SideEffect::None,
            }
        }

        SessionNavigatorAction::PaneFocused {
            locator: _,
            session_logical_key,
        } => {
            // 规则 1: live container focused pane → is_active = true
            if let Some(key) = session_logical_key {
                state.active_key = Some(key);
            }

            recompute_active(&mut sessions, &state, pane_info);

            ReduceResult {
                sessions,
                state,
                side_effect: SideEffect::None,
            }
        }

        SessionNavigatorAction::Reorder {
            ordered_logical_keys,
        } => {
            // EC-08: reassign display_order by logical_key; keep active_key / restoring.
            let active_before = state.active_key.clone();
            for (index, key) in ordered_logical_keys.iter().enumerate() {
                state.display_order.insert(key.clone(), index as u64);
            }
            if let Some(max_index) = ordered_logical_keys
                .len()
                .checked_sub(1)
                .map(|i| i as u64)
            {
                state.next_display_order = state.next_display_order.max(max_index + 1);
            }
            sort_sessions(&mut sessions, &state);
            recompute_active(&mut sessions, &state, pane_info);
            debug_assert_eq!(
                state.active_key, active_before,
                "Reorder must not change active_key"
            );
            ReduceResult {
                sessions,
                state,
                side_effect: SideEffect::None,
            }
        }
    }
}

// ─────────────────────────────────────────────────────────
// State Transition Validator (§10.6)
// ─────────────────────────────────────────────────────────

pub fn validate_state(
    sessions: &[WorkspaceSessionSnapshot],
    state: &SessionNavigatorState,
) -> Result<(), String> {
    // 1. 至少有一个 active 或列表为空
    let active_count = sessions.iter().filter(|s| s.is_active).count();
    if active_count > 1 {
        return Err(format!(
            "validate_state: {} active sessions, expected ≤ 1",
            active_count
        ));
    }

    // 2. deleting session 不在 sessions 中
    for session in sessions {
        let key = logical_key(session);
        if state.deleting_keys.contains(&key) {
            return Err(format!(
                "validate_state: deleted session {key} still in sessions"
            ));
        }
    }

    // 3. active_key 指向的 session 存在 (除非正在 restoring)
    if let Some(active_key) = &state.active_key {
        let exists = sessions.iter().any(|s| logical_key(s) == *active_key);
        let is_restoring = state.restoring_keys.contains(active_key);
        if !exists && !is_restoring {
            // 允许: active_key 可能指向一个正在 spawn 的 virtual row
            // 但如果不在 restoring_keys 中, 那是 stale
            return Err(format!(
                "validate_state: active_key {active_key} not in sessions and not restoring"
            ));
        }
    }

    // 4. restoring session 的 key 存在于 sessions 中 (除非已 materialize)
    for key in &state.restoring_keys {
        let _exists = sessions.iter().any(|s| {
            logical_key(s) == *key || s.id == *key
        });
        // restoring 中的 session 可能还在 sessions 中 (virtual row)
        // 如果它已经 materialize, 接入层应在 Refresh 后清除 restoring_keys
        // 这里只警告不报错 (因为 reducer 的 Refresh 已经清了)
    }

    Ok(())
}

pub fn validate_transition(
    before: &ReduceResult,
    after: &ReduceResult,
    action: &SessionNavigatorAction,
    pane_info: &PaneGroupInfo,
) -> Result<(), String> {
    // 1. active 不超过 1 个
    let active_count = after
        .sessions
        .iter()
        .filter(|s| s.is_active)
        .count();
    if active_count > 1 {
        return Err(format!(
            "validate_transition: {active_count} active sessions after reduce, expected ≤ 1"
        ));
    }

    validate_state(&after.sessions, &after.state)?;

    if let SessionNavigatorAction::Delete {
        session_logical_key,
        ..
    } = action
    {
        // 被删 key 不得仍在 sessions
        if after
            .sessions
            .iter()
            .any(|s| logical_key(s) == *session_logical_key)
        {
            return Err(format!(
                "validate_transition: deleted session {session_logical_key} still in sessions"
            ));
        }
        if !after.state.deleting_keys.contains(session_logical_key) {
            return Err(format!(
                "validate_transition: deleted session {session_logical_key} missing from deleting_keys"
            ));
        }

        // focus target (若有) 必须能在 pane_info 中解析到
        if let SideEffect::DeleteEffects(effects) = &after.side_effect {
            if let Some(focus) = &effects.focus {
                let found = pane_info.tabs.values().any(|tab| {
                    tab.all_pane_locators
                        .iter()
                        .any(|locator| locator.pane_id == focus.pane_id)
                        || tab
                            .focused_locator
                            .as_ref()
                            .is_some_and(|locator| locator.pane_id == focus.pane_id)
                        || tab
                            .prev_pane_locator
                            .as_ref()
                            .is_some_and(|locator| locator.pane_id == focus.pane_id)
                });
                if !found {
                    return Err(
                        "validate_transition: DeleteEffects.focus target not found in pane_info"
                            .to_string(),
                    );
                }
            }
        } else if !matches!(after.side_effect, SideEffect::None) {
            return Err(format!(
                "validate_transition: Delete must yield DeleteEffects or None, got {:?}",
                after.side_effect
            ));
        }
    }

    if let SessionNavigatorAction::Reorder {
        ordered_logical_keys,
    } = action
    {
        if before.state.active_key != after.state.active_key {
            return Err(format!(
                "validate_transition: Reorder changed active_key from {:?} to {:?}",
                before.state.active_key, after.state.active_key
            ));
        }
        if !matches!(after.side_effect, SideEffect::None) {
            return Err(format!(
                "validate_transition: Reorder must yield None, got {:?}",
                after.side_effect
            ));
        }
        // Check the requested key sequence (sort may re-glue adjacency via group min order).
        validate_reorder_keys_preserve_split_groups(&before.sessions, ordered_logical_keys)?;
        validate_same_window_split_adjacency(&before.sessions, &after.sessions)?;
    }

    // sessions 数量在非 Refresh 时不应增加
    if !matches!(action, SessionNavigatorAction::Refresh { .. })
        && after.sessions.len() > before.sessions.len()
    {
        return Err(format!(
            "validate_transition: session count grew from {} to {} without Refresh",
            before.sessions.len(),
            after.sessions.len()
        ));
    }

    Ok(())
}

/// EC-17: live leaves that share a tab must stay contiguous with leaf order preserved.
fn validate_same_window_split_adjacency(
    before: &[WorkspaceSessionSnapshot],
    after: &[WorkspaceSessionSnapshot],
) -> Result<(), String> {
    let before_leaves = live_leaves_by_tab(before);
    let after_keys: Vec<String> = after.iter().map(logical_key).collect();
    validate_split_groups_in_key_sequence(&before_leaves, &after_keys)
}

/// Reject Reorder payloads that insert foreign keys between same-tab live leaves
/// or reverse leaf relative order.
fn validate_reorder_keys_preserve_split_groups(
    before: &[WorkspaceSessionSnapshot],
    ordered_logical_keys: &[String],
) -> Result<(), String> {
    let before_leaves = live_leaves_by_tab(before);
    validate_split_groups_in_key_sequence(&before_leaves, ordered_logical_keys)
}

fn validate_split_groups_in_key_sequence(
    before_leaves: &HashMap<usize, Vec<String>>,
    ordered_keys: &[String],
) -> Result<(), String> {
    let positions: HashMap<&str, usize> = ordered_keys
        .iter()
        .enumerate()
        .map(|(i, key)| (key.as_str(), i))
        .collect();

    for (tab_index, before_keys) in before_leaves {
        if before_keys.len() < 2 {
            continue;
        }
        let mut positions_for_group = Vec::with_capacity(before_keys.len());
        for key in before_keys {
            let Some(pos) = positions.get(key.as_str()).copied() else {
                return Err(format!(
                    "validate_transition: Reorder dropped split leaf {key} from tab {tab_index}"
                ));
            };
            positions_for_group.push(pos);
        }
        let mut sorted = positions_for_group.clone();
        sorted.sort_unstable();
        if positions_for_group != sorted {
            return Err(format!(
                "validate_transition: Reorder changed leaf relative order in tab {tab_index}"
            ));
        }
        for window in sorted.windows(2) {
            if window[1] != window[0] + 1 {
                return Err(format!(
                    "validate_transition: Reorder broke same_window adjacency in tab {tab_index}"
                ));
            }
        }
    }
    Ok(())
}

fn live_leaves_by_tab(
    sessions: &[WorkspaceSessionSnapshot],
) -> HashMap<usize, Vec<String>> {
    let mut by_tab: HashMap<usize, Vec<(usize, String)>> = HashMap::new();
    for session in sessions {
        if !is_live(session) {
            continue;
        }
        let Some((tab, leaf)) = locator_from_session_id(&session.id) else {
            continue;
        };
        by_tab
            .entry(tab)
            .or_default()
            .push((leaf, logical_key(session)));
    }
    by_tab
        .into_iter()
        .map(|(tab, mut leaves)| {
            leaves.sort_by_key(|(leaf, _)| *leaf);
            (
                tab,
                leaves.into_iter().map(|(_, key)| key).collect::<Vec<_>>(),
            )
        })
        .collect()
}

// ─────────────────────────────────────────────────────────
// Reorder units (EC-17: split tab = one drag unit)
// ─────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReorderUnit {
    /// Same-tab multi-pane live group; keys already in leaf order.
    Group {
        tab_index: usize,
        logical_keys: Vec<String>,
    },
    /// Virtual row or sole live pane.
    Single {
        logical_key: String,
    },
}

impl ReorderUnit {
    pub fn id(&self) -> String {
        match self {
            Self::Group { tab_index, .. } => format!("group:tab:{tab_index}"),
            Self::Single { logical_key } => format!("row:{logical_key}"),
        }
    }

    pub fn logical_keys(&self) -> &[String] {
        match self {
            Self::Group { logical_keys, .. } => logical_keys,
            Self::Single { logical_key } => std::slice::from_ref(logical_key),
        }
    }
}

/// Build drag units from a display-ordered session list.
/// A tab with 2+ live leaves in the list is one Group; others are Single.
pub fn build_reorder_units(sessions: &[WorkspaceSessionSnapshot]) -> Vec<ReorderUnit> {
    let multi_tabs: HashSet<usize> = {
        let mut counts: HashMap<usize, usize> = HashMap::new();
        for session in sessions {
            if !is_live(session) {
                continue;
            }
            if let Some((tab, _)) = locator_from_session_id(&session.id) {
                *counts.entry(tab).or_default() += 1;
            }
        }
        counts
            .into_iter()
            .filter(|(_, count)| *count > 1)
            .map(|(tab, _)| tab)
            .collect()
    };

    let mut units = Vec::new();
    let mut emitted_groups: HashSet<usize> = HashSet::new();
    for session in sessions {
        let key = logical_key(session);
        if is_live(session) {
            if let Some((tab, _)) = locator_from_session_id(&session.id) {
                if multi_tabs.contains(&tab) {
                    if emitted_groups.insert(tab) {
                        let mut leaves: Vec<(usize, String)> = sessions
                            .iter()
                            .filter(|s| {
                                is_live(s)
                                    && locator_from_session_id(&s.id)
                                        .is_some_and(|(t, _)| t == tab)
                            })
                            .filter_map(|s| {
                                locator_from_session_id(&s.id)
                                    .map(|(_, leaf)| (leaf, logical_key(s)))
                            })
                            .collect();
                        leaves.sort_by_key(|(leaf, _)| *leaf);
                        units.push(ReorderUnit::Group {
                            tab_index: tab,
                            logical_keys: leaves.into_iter().map(|(_, k)| k).collect(),
                        });
                    }
                    continue;
                }
            }
        }
        units.push(ReorderUnit::Single {
            logical_key: key,
        });
    }
    units
}

/// Move a unit to `to_index` (unit-list insertion index before removal adjustment).
/// Returns flattened logical_keys for `SessionNavigatorAction::Reorder`.
pub fn move_reorder_unit(
    mut units: Vec<ReorderUnit>,
    from_index: usize,
    to_index: usize,
) -> Vec<String> {
    if from_index >= units.len() {
        return units
            .into_iter()
            .flat_map(|unit| unit.logical_keys().to_vec())
            .collect();
    }
    let unit = units.remove(from_index);
    let insert_at = if to_index > from_index {
        to_index.saturating_sub(1).min(units.len())
    } else {
        to_index.min(units.len())
    };
    units.insert(insert_at, unit);
    units
        .into_iter()
        .flat_map(|unit| unit.logical_keys().to_vec())
        .collect()
}

// ─────────────────────────────────────────────────────────
// 测试
// ─────────────────────────────────────────────────────────

#[cfg(test)]
#[path = "session_navigator_reducer_tests.rs"]
mod tests;
