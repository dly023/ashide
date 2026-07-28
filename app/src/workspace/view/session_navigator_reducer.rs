//! Session Navigator 纯 Reducer —— 状态机核心。
//!
//! 这是 Session Navigator 所有列表变化的唯一决策入口。
//! 纯函数: 输入 (sessions, state, action, pane_group_info) → (sessions, state, side_effect)。
//! 不依赖 Workspace 的可变状态,不调用 ctx,不执行 IO。
//! 接入层 (session_navigator.rs) 负责执行 side_effect。
//!
//! 契约: `docs/SESSION_NAVIGATOR_SPEC.yaml` v6。
//!
//! ## 给人和 AI 修改者的边界
//! - `SessionNavigatorState` 由 canonical EnvironmentKey 独占；authority alias 共享，
//!   不同 Environment 禁止共用。
//! - `display_order` 只在 `Refresh.FirstObserved` 和 `Reorder` 中写入。
//! - Resume/materialize 通过 `row_id_by_identity` 增加 alias，绝不新建位置。
//! - 组件只能发送 typed `SessionNavigatorAction`，不能直接排序或回写 selection。
//! - 新增 action/state slice 时，必须先更新 SPEC 的 `write_ownership`、
//!   `ux_contract_matrix` 及其绑定测试；矩阵链接测试会阻止漏项。

use std::collections::{HashMap, HashSet};

use crate::app_state::WorkspaceSessionSnapshot;
use crate::workspace::util::PaneViewLocator;

// ─────────────────────────────────────────────────────────
// 状态
// ─────────────────────────────────────────────────────────

/// 单个 Environment 独占的 Session Navigator 状态。
///
/// 它由 `EnvironmentTable` 按 canonical navigation key 分区。切换环境只切换
/// 被投影的 state，authority alias、重连和 workspace root 变化不能制造新分区。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionNavigatorState {
    pub selected_row_id: Option<String>,
    pub restoring_row_ids: HashSet<String>,
    /// 正在执行 provider/source 删除事务的 RowId。
    pub deleting_row_ids: HashSet<String>,
    /// 已提交删除的 RowId tombstone；阻止 stale provider/history source 复活。
    pub deleted_row_ids: HashSet<String>,
    /// 稳定 RowId → 显示位置。生命周期、焦点和环境切换无权写入。
    pub display_order: HashMap<String, u64>,
    /// 观察身份（stable container/durable/logical）→ 稳定 RowId。
    /// `tab:X:leaf:Y` 是 action locator，禁止进入该 registry。
    /// virtual → live 只增加 alias，不创建新排序身份。
    pub row_id_by_identity: HashMap<String, String>,
    /// opaque RowId 的单调分配器。RowId 绝不能复用 pane/session identity，
    /// 否则已删除行的 tombstone 会误伤未来复用同一物理坐标的新 session。
    pub next_row_id: u64,
    pub next_display_order: u64,
}

impl SessionNavigatorState {
    pub fn new() -> Self {
        Self {
            selected_row_id: None,
            restoring_row_ids: HashSet::new(),
            deleting_row_ids: HashSet::new(),
            deleted_row_ids: HashSet::new(),
            display_order: HashMap::new(),
            row_id_by_identity: HashMap::new(),
            next_row_id: 0,
            next_display_order: 0,
        }
    }
}

impl Default for SessionNavigatorState {
    fn default() -> Self {
        Self::new()
    }
}

/// 单个 Environment 的 canonical Session Navigator 模型。
///
/// 可见 carrier 与 lifecycle/identity/position state 必须由同一个 owner 原子持有；
/// 否则异步 Resume 在 source 已消费、live pane 尚未 materialize 的中间帧会丢行。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionNavigatorModel {
    pub sessions: Vec<WorkspaceSessionSnapshot>,
    pub state: SessionNavigatorState,
}

impl SessionNavigatorModel {
    pub fn new(sessions: Vec<WorkspaceSessionSnapshot>, state: SessionNavigatorState) -> Self {
        Self { sessions, state }
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
        /// 本次删除事务覆盖的全部 identity alias；reducer 必须一次性登记，
        /// 禁止接入层在物理副作用之后再补写 lifecycle。
        session_identity_keys: Vec<String>,
    },
    /// 激活 / 恢复一个 session
    Activate {
        session_logical_key: String,
        session_id: String,
        is_live: bool,
    },
    /// 显式修改 Navigator selection；focus projection 禁止使用此 action。
    SelectionChanged { session_logical_key: Option<String> },
    /// 恢复开始；一次性登记生命周期，并可选择对应行。
    RestoreStarted {
        session_keys: Vec<String>,
        selected_logical_key: Option<String>,
    },
    /// 恢复成功、失败或取消后清理生命周期。
    RestoreFinished { session_keys: Vec<String> },
    /// provider/source 删除失败：回滚 in-flight tombstone。
    DeleteRolledBack { session_keys: Vec<String> },
    /// provider/source 删除成功：提交稳定 RowId tombstone，并释放 volatile identity alias。
    DeleteCommitted {
        session_keys: Vec<String>,
        volatile_identity_keys: Vec<String>,
    },
    /// Pin / unpin
    Pin {
        session_logical_key: String,
        pinned: bool,
    },
    /// 刷新 (merge + reconcile + sort + recompute active)
    Refresh {
        new_sessions: Vec<WorkspaceSessionSnapshot>,
        pinned_identity_keys: HashSet<String>,
    },
    /// Tab 被激活；当前 pane 拓扑已经由 `pane_info` 表达，action 不重复携带无主数据。
    TabActivated,
    /// Pane 被聚焦；只携带 active projection 真正需要的会话 identity。
    PaneFocused { session_logical_key: Option<String> },
    /// 重排列表 (EC-08): 只改 display_order；focus/selected_row_id 绑定的 RowId 不变。
    Reorder { ordered_logical_keys: Vec<String> },
}

/// Action 对持久状态切片的写权限。这个穷尽 `match` 是有意为之：新增 action 时
/// 编译器会强制修改者（包括 AI agent）先声明它能改什么，再进入 reducer 实现。
#[derive(Clone, Copy, Debug)]
struct TransitionPermissions {
    position: bool,
    identity: bool,
    selection: bool,
    lifecycle: bool,
}

fn transition_permissions(action: &SessionNavigatorAction) -> TransitionPermissions {
    match action {
        SessionNavigatorAction::Delete { .. } | SessionNavigatorAction::Activate { .. } => {
            TransitionPermissions {
                position: false,
                identity: true,
                selection: true,
                lifecycle: true,
            }
        }
        SessionNavigatorAction::SelectionChanged { .. } => TransitionPermissions {
            position: false,
            identity: true,
            selection: true,
            lifecycle: false,
        },
        SessionNavigatorAction::RestoreStarted { .. } => TransitionPermissions {
            position: false,
            identity: true,
            selection: true,
            lifecycle: true,
        },
        SessionNavigatorAction::RestoreFinished { .. } => TransitionPermissions {
            position: false,
            identity: false,
            selection: true,
            lifecycle: true,
        },
        SessionNavigatorAction::DeleteRolledBack { .. } => TransitionPermissions {
            position: false,
            identity: false,
            selection: false,
            lifecycle: true,
        },
        SessionNavigatorAction::DeleteCommitted { .. } => TransitionPermissions {
            position: false,
            identity: true,
            selection: false,
            lifecycle: true,
        },
        SessionNavigatorAction::Pin { .. } => TransitionPermissions {
            position: false,
            identity: false,
            selection: false,
            lifecycle: false,
        },
        SessionNavigatorAction::Refresh { .. } => TransitionPermissions {
            position: true,
            identity: true,
            selection: true,
            lifecycle: true,
        },
        SessionNavigatorAction::TabActivated | SessionNavigatorAction::PaneFocused { .. } => {
            TransitionPermissions {
                position: false,
                identity: false,
                selection: false,
                lifecycle: false,
            }
        }
        SessionNavigatorAction::Reorder { .. } => TransitionPermissions {
            position: true,
            identity: false,
            selection: false,
            lifecycle: false,
        },
    }
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
    session.durable_identity_key()
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

fn session_identity_keys(session: &WorkspaceSessionSnapshot) -> Vec<String> {
    session.observed_identity_keys()
}

fn row_id_for_identity(identity: &str, state: &SessionNavigatorState) -> Option<String> {
    state.row_id_by_identity.get(identity).cloned()
}

fn allocate_row_id(state: &mut SessionNavigatorState) -> String {
    let row_id = format!("row:{}", state.next_row_id);
    state.next_row_id += 1;
    row_id
}

fn existing_row_id_for_identities<'a>(
    identities: impl IntoIterator<Item = &'a String>,
    state: &SessionNavigatorState,
) -> Option<String> {
    identities
        .into_iter()
        .find_map(|identity| state.row_id_by_identity.get(identity).cloned())
}

fn bind_identities_to_row(
    identities: impl IntoIterator<Item = String>,
    row_id: &str,
    state: &mut SessionNavigatorState,
) {
    for identity in identities {
        state.row_id_by_identity.insert(identity, row_id.to_owned());
    }
}

fn migrate_row_state(from_row_id: &str, to_row_id: &str, state: &mut SessionNavigatorState) {
    if from_row_id == to_row_id {
        return;
    }

    for row_id in state.row_id_by_identity.values_mut() {
        if row_id == from_row_id {
            *row_id = to_row_id.to_owned();
        }
    }
    if state.selected_row_id.as_deref() == Some(from_row_id) {
        state.selected_row_id = Some(to_row_id.to_owned());
    }
    for row_ids in [
        &mut state.restoring_row_ids,
        &mut state.deleting_row_ids,
        &mut state.deleted_row_ids,
    ] {
        if row_ids.remove(from_row_id) {
            row_ids.insert(to_row_id.to_owned());
        }
    }

    if let Some(order) = state.display_order.remove(from_row_id) {
        state
            .display_order
            .entry(to_row_id.to_owned())
            .or_insert(order);
    }
}

fn canonical_existing_row_id_for_session(
    session: &WorkspaceSessionSnapshot,
    identities: &[String],
    state: &SessionNavigatorState,
) -> Option<String> {
    durable_key(session)
        .and_then(|identity| row_id_for_identity(&identity, state))
        .or_else(|| row_id_for_identity(&logical_key(session), state))
        .or_else(|| existing_row_id_for_identities(identities, state))
}

fn row_id_for_session(session: &WorkspaceSessionSnapshot, state: &SessionNavigatorState) -> String {
    let logical = logical_key(session);
    state
        .row_id_by_identity
        .get(&logical)
        .cloned()
        .or_else(|| {
            durable_key(session)
                .and_then(|identity| state.row_id_by_identity.get(&identity).cloned())
        })
        .unwrap_or(logical)
}

fn display_order_for_session(
    session: &WorkspaceSessionSnapshot,
    state: &SessionNavigatorState,
) -> u64 {
    state
        .display_order
        .get(&row_id_for_session(session, state))
        .copied()
        .unwrap_or(0)
}

fn sort_sessions(sessions: &mut Vec<WorkspaceSessionSnapshot>, state: &SessionNavigatorState) {
    let original_positions: HashMap<String, usize> = sessions
        .iter()
        .enumerate()
        .map(|(idx, session)| (row_id_for_session(session, state), idx))
        .collect();

    // 同屏组位置：tab_index → 该组最大 display_order（值越大越靠上）。
    let mut same_window_group_orders: HashMap<usize, u64> = HashMap::new();
    for session in sessions.iter() {
        if let Some((tab_index, _)) = locator_from_session_id(&session.id) {
            let order = display_order_for_session(session, state);
            same_window_group_orders
                .entry(tab_index)
                .and_modify(|value| *value = (*value).max(order))
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
            .then_with(|| right_group_order.cmp(&left_group_order))
            .then_with(|| {
                if let (Some((left_tab, left_leaf)), Some((right_tab, right_leaf))) =
                    (left_tab, right_tab)
                {
                    if left_tab == right_tab {
                        left_leaf.cmp(&right_leaf)
                    } else {
                        std::cmp::Ordering::Equal
                    }
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .then_with(|| right_order.cmp(&left_order))
            .then_with(|| right.updated_at_unix_ms.cmp(&left.updated_at_unix_ms))
            .then_with(|| {
                let left_position = original_positions
                    .get(&row_id_for_session(left, state))
                    .copied()
                    .unwrap_or(usize::MAX);
                let right_position = original_positions
                    .get(&row_id_for_session(right, state))
                    .copied()
                    .unwrap_or(usize::MAX);
                left_position.cmp(&right_position)
            })
            .then_with(|| row_id_for_session(left, state).cmp(&row_id_for_session(right, state)))
    });
}

// ─────────────────────────────────────────────────────────
// Reconcile (唯一允许为新行分配 display_order 的入口)
// ─────────────────────────────────────────────────────────

fn reconcile_display_order(
    sessions: &[WorkspaceSessionSnapshot],
    state: &mut SessionNavigatorState,
) {
    let mut sorted = sessions.to_vec();
    sorted.sort_by(|left, right| {
        right
            .is_pinned
            .cmp(&left.is_pinned)
            .then_with(|| right.updated_at_unix_ms.cmp(&left.updated_at_unix_ms))
            .then_with(|| logical_key(left).cmp(&logical_key(right)))
    });

    let mut pending_row_ids = Vec::new();
    for session in &sorted {
        let identities = session_identity_keys(session);
        let row_id = canonical_existing_row_id_for_session(session, &identities, state)
            .unwrap_or_else(|| allocate_row_id(state));

        let competing_row_ids = identities
            .iter()
            .filter_map(|identity| row_id_for_identity(identity, state))
            .filter(|existing_row_id| existing_row_id != &row_id)
            .collect::<HashSet<_>>();
        for competing_row_id in competing_row_ids {
            // 同一 observed row 的 durable/physical alias 若曾因异步时序短暂分配到
            // 不同 RowId，Refresh 必须原子收敛全部 RowId-owned 状态。否则 identity
            // registry 虽已统一，selection/lifecycle/order 仍会悬挂在孤儿 RowId 上。
            migrate_row_state(&competing_row_id, &row_id, state);
        }
        bind_identities_to_row(identities, &row_id, state);

        if !state.display_order.contains_key(&row_id) && !pending_row_ids.contains(&row_id) {
            pending_row_ids.push(row_id);
        }
    }

    // 真正首次发现的 RowId 才拿新位置；Resume/Refresh/Focus/Environment switch
    // 只增加 identity alias，不创建 position。
    for row_id in pending_row_ids.into_iter().rev() {
        let order = state.next_display_order;
        state.next_display_order += 1;
        state.display_order.insert(row_id, order);
    }
}

/// Collapse backing projections that have already been explicitly aliased to
/// the same canonical RowId. This is the materialization boundary for restore
/// sources without a provider-native session ID: the reducer prefers the live
/// container while retaining row-owned pin/order/selection state.
fn reconcile_refresh_projection(
    previous_sessions: Vec<WorkspaceSessionSnapshot>,
    observed_sessions: Vec<WorkspaceSessionSnapshot>,
    state: &SessionNavigatorState,
) -> Vec<WorkspaceSessionSnapshot> {
    let observed_row_ids = observed_sessions
        .iter()
        .map(|session| row_id_for_session(session, state))
        .collect::<HashSet<_>>();
    let mut reconciled = observed_sessions;

    for session in previous_sessions {
        let row_id = row_id_for_session(&session, state);
        if state.restoring_row_ids.contains(&row_id) && !observed_row_ids.contains(&row_id) {
            reconciled.push(session);
        }
    }

    reconciled
}

fn collapse_materialized_session_rows(
    sessions: Vec<WorkspaceSessionSnapshot>,
    state: &SessionNavigatorState,
) -> Vec<WorkspaceSessionSnapshot> {
    let mut collapsed = Vec::<WorkspaceSessionSnapshot>::new();
    let mut index_by_row_id = HashMap::<String, usize>::new();

    for mut session in sessions {
        let row_id = row_id_for_session(&session, state);
        let Some(existing_index) = index_by_row_id.get(&row_id).copied() else {
            index_by_row_id.insert(row_id, collapsed.len());
            collapsed.push(session);
            continue;
        };

        let existing = &mut collapsed[existing_index];
        let pinned = existing.is_pinned || session.is_pinned;
        if is_live(&session) && !is_live(existing) {
            session.label = session.merged_label(existing, false, true, false);
            if session.updated_at_unix_ms.is_none() {
                session.updated_at_unix_ms = existing.updated_at_unix_ms;
            }
            session.is_pinned = pinned;
            *existing = session;
        } else {
            existing.is_pinned = pinned;
            existing.label =
                existing.merged_label(&session, false, is_live(existing), is_live(&session));
            if existing.updated_at_unix_ms.is_none() {
                existing.updated_at_unix_ms = session.updated_at_unix_ms;
            }
        }
    }

    collapsed
}

// ─────────────────────────────────────────────────────────
// Active 状态计算 (§6.1 优先级链)
// ─────────────────────────────────────────────────────────

fn recompute_active(
    sessions: &mut [WorkspaceSessionSnapshot],
    state: &SessionNavigatorState,
    active_row_override: Option<&str>,
) {
    for session in sessions.iter_mut() {
        session.is_active = false;
    }

    // active 是渲染投影：显式 focus RowId 优先于 Environment selection，随后依次是
    // selected live、restoring、selected virtual。投影输入不回写任何持久状态切片。
    let selected_row_id = active_row_override.or(state.selected_row_id.as_deref());
    let selected_live_index = selected_row_id.and_then(|selected_row_id| {
        sessions.iter().position(|session| {
            is_live(session) && row_id_for_session(session, state) == selected_row_id
        })
    });
    let restoring_index = sessions.iter().position(|session| {
        state
            .restoring_row_ids
            .contains(&row_id_for_session(session, state))
    });
    let selected_index = selected_row_id.and_then(|selected_row_id| {
        sessions
            .iter()
            .position(|session| row_id_for_session(session, state) == selected_row_id)
    });

    if let Some(index) = selected_live_index.or(restoring_index).or(selected_index) {
        sessions[index].is_active = true;
    }
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
            let focus = info.prev_pane_locator.or_else(|| {
                info.all_pane_locators.iter().find_map(|locator| {
                    if deleted_locator
                        .as_ref()
                        .is_some_and(|deleted| deleted.pane_id == locator.pane_id)
                    {
                        None
                    } else {
                        Some(*locator)
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
            session_identity_keys: delete_identity_keys,
        } => {
            let deleted = sessions
                .iter()
                .find(|session| logical_key(session) == session_logical_key)
                .cloned();
            let mut identities = delete_identity_keys;
            identities.push(session_logical_key.clone());
            if let Some(deleted) = &deleted {
                identities.extend(session_identity_keys(deleted));
            }
            identities.sort();
            identities.dedup();
            let row_id = existing_row_id_for_identities(identities.iter(), &state)
                .unwrap_or_else(|| allocate_row_id(&mut state));
            bind_identities_to_row(identities, &row_id, &mut state);

            sessions.retain(|session| row_id_for_session(session, &state) != row_id);
            state.deleting_row_ids.insert(row_id.clone());
            state.deleted_row_ids.remove(&row_id);
            if state.selected_row_id.as_deref() == Some(row_id.as_str()) {
                state.selected_row_id = None;
            }
            state.restoring_row_ids.remove(&row_id);

            let side_effect = if let Some(ref deleted_session) = deleted {
                decide_delete_effects(deleted_session, &sessions, pane_info)
            } else {
                SideEffect::None
            };
            recompute_active(&mut sessions, &state, None);
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
            let session = sessions
                .iter()
                .find(|session| logical_key(session) == session_logical_key)
                .cloned();
            let mut identities = session
                .as_ref()
                .map(session_identity_keys)
                .unwrap_or_default();
            identities.push(session_logical_key.clone());
            identities.sort();
            identities.dedup();
            let row_id = existing_row_id_for_identities(identities.iter(), &state)
                .unwrap_or_else(|| allocate_row_id(&mut state));
            bind_identities_to_row(identities, &row_id, &mut state);

            if is_live {
                let side_effect = if let Some(ref session) = session {
                    if let Some((tab, leaf)) = locator_from_session_id(&session.id) {
                        if let Some(info) = pane_info.tabs.get(&tab) {
                            if leaf < info.all_pane_locators.len() {
                                state.selected_row_id = Some(row_id.clone());
                                state.restoring_row_ids.remove(&row_id);
                                SideEffect::FocusPane(info.all_pane_locators[leaf])
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
                recompute_active(&mut sessions, &state, None);
                ReduceResult {
                    sessions,
                    state,
                    side_effect,
                }
            } else {
                state.restoring_row_ids.insert(row_id.clone());
                state.selected_row_id = Some(row_id);
                let spawn_session_id = session.map(|session| session.id).unwrap_or_default();
                recompute_active(&mut sessions, &state, None);
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

        SessionNavigatorAction::SelectionChanged {
            session_logical_key,
        } => {
            state.selected_row_id = session_logical_key.map(|identity| {
                let mut identities = sessions
                    .iter()
                    .find(|session| session_identity_keys(session).contains(&identity))
                    .map(session_identity_keys)
                    .unwrap_or_default();
                identities.push(identity);
                identities.sort();
                identities.dedup();
                let row_id = existing_row_id_for_identities(identities.iter(), &state)
                    .unwrap_or_else(|| allocate_row_id(&mut state));
                bind_identities_to_row(identities, &row_id, &mut state);
                row_id
            });
            recompute_active(&mut sessions, &state, None);
            ReduceResult {
                sessions,
                state,
                side_effect: SideEffect::None,
            }
        }

        SessionNavigatorAction::RestoreStarted {
            mut session_keys,
            selected_logical_key,
        } => {
            if let Some(selected_logical_key) = &selected_logical_key {
                session_keys.push(selected_logical_key.clone());
            }
            session_keys.sort();
            session_keys.dedup();
            let fallback_identity = selected_logical_key
                .as_ref()
                .or_else(|| session_keys.first())
                .cloned();
            if fallback_identity.is_some() {
                let row_id = existing_row_id_for_identities(session_keys.iter(), &state)
                    .unwrap_or_else(|| allocate_row_id(&mut state));
                bind_identities_to_row(session_keys, &row_id, &mut state);
                state.restoring_row_ids.insert(row_id.clone());
                if selected_logical_key.is_some() {
                    state.selected_row_id = Some(row_id);
                }
            }
            recompute_active(&mut sessions, &state, None);
            ReduceResult {
                sessions,
                state,
                side_effect: SideEffect::None,
            }
        }

        SessionNavigatorAction::RestoreFinished { session_keys } => {
            let row_ids = session_keys
                .iter()
                .filter_map(|identity| state.row_id_by_identity.get(identity).cloned())
                .collect::<HashSet<_>>();
            state
                .restoring_row_ids
                .retain(|row_id| !row_ids.contains(row_id));
            if state
                .selected_row_id
                .as_ref()
                .is_some_and(|selected_row_id| {
                    row_ids.contains(selected_row_id)
                        && !sessions
                            .iter()
                            .any(|session| row_id_for_session(session, &state) == *selected_row_id)
                })
            {
                // Restore 的成功、失败和取消共用这个生命周期收口。仅当 RowId 已无
                // 可渲染实体时清 selection；materialize 成 live row 时保持空间记忆。
                state.selected_row_id = None;
            }
            recompute_active(&mut sessions, &state, None);
            ReduceResult {
                sessions,
                state,
                side_effect: SideEffect::None,
            }
        }

        SessionNavigatorAction::DeleteRolledBack { session_keys } => {
            let row_ids = session_keys
                .iter()
                .filter_map(|identity| state.row_id_by_identity.get(identity).cloned())
                .collect::<HashSet<_>>();
            state
                .deleting_row_ids
                .retain(|row_id| !row_ids.contains(row_id));
            recompute_active(&mut sessions, &state, None);
            ReduceResult {
                sessions,
                state,
                side_effect: SideEffect::None,
            }
        }

        SessionNavigatorAction::DeleteCommitted {
            session_keys,
            volatile_identity_keys,
        } => {
            let row_ids = session_keys
                .iter()
                .filter_map(|identity| state.row_id_by_identity.get(identity).cloned())
                .collect::<HashSet<_>>();
            for row_id in &row_ids {
                state.deleting_row_ids.remove(row_id);
                state.deleted_row_ids.insert(row_id.clone());
            }
            for identity in volatile_identity_keys {
                state.row_id_by_identity.remove(&identity);
            }
            recompute_active(&mut sessions, &state, None);
            ReduceResult {
                sessions,
                state,
                side_effect: SideEffect::None,
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
            new_sessions,
            pinned_identity_keys,
        } => {
            // 先让本帧 observed source 进入 canonical identity registry，再与上一帧
            // carrier reconcile。Restore source 暂时缺席不等于删除：只要 RowId 仍处于
            // restoring，就保留原 carrier；live carrier 出现后同 RowId collapse 为 live。
            reconcile_display_order(&new_sessions, &mut state);
            let mut sessions = reconcile_refresh_projection(sessions, new_sessions, &state);
            for session in &mut sessions {
                let keys = session.stable_pin_keys();
                session.is_pinned = keys.iter().any(|key| pinned_identity_keys.contains(key));
            }

            // identity reconcile 后再按 RowId tombstone 过滤。这样 stale provider source
            // 即使换了 physical identity，也无法复活已删除行。
            sessions.retain(|session| {
                let row_id = row_id_for_session(session, &state);
                !state.deleting_row_ids.contains(&row_id)
                    && !state.deleted_row_ids.contains(&row_id)
            });
            sessions = collapse_materialized_session_rows(sessions, &state);

            let materialized_row_ids = sessions
                .iter()
                .filter(|session| is_live(session))
                .map(|session| row_id_for_session(session, &state))
                .collect::<HashSet<_>>();
            state
                .restoring_row_ids
                .retain(|row_id| !materialized_row_ids.contains(row_id));
            if state
                .selected_row_id
                .as_ref()
                .is_some_and(|selected_row_id| {
                    !state.restoring_row_ids.contains(selected_row_id)
                        && !sessions
                            .iter()
                            .any(|session| row_id_for_session(session, &state) == *selected_row_id)
                })
            {
                // restoring carrier 由 canonical model 保留；只有生命周期已经结束且
                // Refresh 再次确认没有任何 source/live carrier 时，selection 才能清理。
                state.selected_row_id = None;
            }

            sort_sessions(&mut sessions, &state);
            recompute_active(&mut sessions, &state, None);
            ReduceResult {
                sessions,
                state,
                side_effect: SideEffect::None,
            }
        }

        SessionNavigatorAction::TabActivated => {
            recompute_active(&mut sessions, &state, None);

            ReduceResult {
                sessions,
                state,
                side_effect: SideEffect::None,
            }
        }

        SessionNavigatorAction::PaneFocused {
            session_logical_key,
        } => {
            // Focus 只作为本帧 active projection 的显式输入；它不借用、不修改
            // Environment 保存的 selection，因此状态所有权在类型边界上就是单向的。
            let focused_row_id = session_logical_key
                .as_deref()
                .and_then(|identity| row_id_for_identity(identity, &state));
            recompute_active(&mut sessions, &state, focused_row_id.as_deref());

            ReduceResult {
                sessions,
                state,
                side_effect: SideEffect::None,
            }
        }

        SessionNavigatorAction::Reorder {
            ordered_logical_keys,
        } => {
            // EC-08：只按 logical key 重写 display_order，selection/restoring 保持不变。
            // ordered_logical_keys[0] 是视觉首行，因此获得最大的 display_order。
            let active_before = state.selected_row_id.clone();
            let n = ordered_logical_keys.len();
            for (index, key) in ordered_logical_keys.iter().enumerate() {
                let order = (n - 1 - index) as u64;
                if let Some(row_id) = state.row_id_by_identity.get(key).cloned() {
                    state.display_order.insert(row_id, order);
                }
            }
            if n > 0 {
                state.next_display_order = state.next_display_order.max(n as u64);
            }
            sort_sessions(&mut sessions, &state);
            recompute_active(&mut sessions, &state, None);
            debug_assert_eq!(
                state.selected_row_id, active_before,
                "Reorder must not change selected_row_id"
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
    let persisted_row_ids = state
        .row_id_by_identity
        .values()
        .chain(state.display_order.keys())
        .chain(state.restoring_row_ids.iter())
        .chain(state.deleting_row_ids.iter())
        .chain(state.deleted_row_ids.iter())
        .chain(state.selected_row_id.iter());
    for row_id in persisted_row_ids {
        if !row_id.starts_with("row:") {
            return Err(format!(
                "validate_state: persisted RowId must be opaque row:N, got {row_id}"
            ));
        }
    }

    let active_count = sessions.iter().filter(|session| session.is_active).count();
    if active_count > 1 {
        return Err(format!(
            "validate_state: {active_count} active sessions, expected ≤ 1"
        ));
    }

    for session in sessions {
        let row_id = row_id_for_session(session, state);
        if state.deleting_row_ids.contains(&row_id) || state.deleted_row_ids.contains(&row_id) {
            return Err(format!(
                "validate_state: tombstoned RowId {row_id} still in sessions"
            ));
        }
    }

    if let Some(selected_row_id) = &state.selected_row_id {
        let exists = sessions
            .iter()
            .any(|session| row_id_for_session(session, state) == *selected_row_id);
        if !exists && !state.restoring_row_ids.contains(selected_row_id) {
            return Err(format!(
                "validate_state: selected RowId {selected_row_id} not in sessions and not restoring"
            ));
        }
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
    let active_count = after.sessions.iter().filter(|s| s.is_active).count();
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
        let row_id = row_id_for_identity(session_logical_key, &after.state).ok_or_else(|| {
            format!("validate_transition: deleted identity {session_logical_key} missing RowId")
        })?;
        if !after.state.deleting_row_ids.contains(&row_id) {
            return Err(format!(
                "validate_transition: deleted RowId {row_id} missing from deleting_row_ids"
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
        if before.state.selected_row_id != after.state.selected_row_id {
            return Err(format!(
                "validate_transition: Reorder changed selected_row_id from {:?} to {:?}",
                before.state.selected_row_id, after.state.selected_row_id
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

    let permissions = transition_permissions(action);
    if !permissions.position
        && (before.state.display_order != after.state.display_order
            || before.state.next_display_order != after.state.next_display_order)
    {
        return Err(format!(
            "validate_transition: {action:?} 越权修改了 position state"
        ));
    }

    if !permissions.identity
        && (before.state.row_id_by_identity != after.state.row_id_by_identity
            || before.state.next_row_id != after.state.next_row_id)
    {
        return Err(format!(
            "validate_transition: {action:?} 越权修改了 identity registry"
        ));
    }

    if !permissions.selection && before.state.selected_row_id != after.state.selected_row_id {
        return Err(format!(
            "validate_transition: {action:?} 越权修改了 selection"
        ));
    }

    if matches!(action, SessionNavigatorAction::Refresh { .. })
        && before.state.selected_row_id != after.state.selected_row_id
    {
        let Some(before_selected) = before.state.selected_row_id.as_deref() else {
            return Err("validate_transition: Refresh 不得凭空创建 selection".to_string());
        };
        if let Some(after_selected) = after.state.selected_row_id.as_deref() {
            let selected_aliases = before
                .state
                .row_id_by_identity
                .iter()
                .filter_map(|(identity, row_id)| {
                    (row_id == before_selected).then_some(identity.as_str())
                })
                .collect::<Vec<_>>();
            if selected_aliases.is_empty()
                || selected_aliases.iter().any(|identity| {
                    after
                        .state
                        .row_id_by_identity
                        .get(*identity)
                        .map(String::as_str)
                        != Some(after_selected)
                })
            {
                return Err(format!(
                    "validate_transition: Refresh 只能把 selected RowId 收敛到其 canonical alias，got {:?} -> {:?}",
                    before.state.selected_row_id, after.state.selected_row_id
                ));
            }
        } else if after.state.restoring_row_ids.contains(before_selected)
            || after
                .sessions
                .iter()
                .any(|session| row_id_for_session(session, &after.state) == before_selected)
        {
            return Err(
                "validate_transition: Refresh 只能在恢复结束且 carrier 已确认缺失时清除 selection"
                    .to_string(),
            );
        }
    }

    if !permissions.lifecycle
        && (before.state.restoring_row_ids != after.state.restoring_row_ids
            || before.state.deleting_row_ids != after.state.deleting_row_ids
            || before.state.deleted_row_ids != after.state.deleted_row_ids)
    {
        return Err(format!(
            "validate_transition: {action:?} 越权修改了 lifecycle state"
        ));
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

fn live_leaves_by_tab(sessions: &[WorkspaceSessionSnapshot]) -> HashMap<usize, Vec<String>> {
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
    Single { logical_key: String },
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
                                    && locator_from_session_id(&s.id).is_some_and(|(t, _)| t == tab)
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
        units.push(ReorderUnit::Single { logical_key: key });
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
