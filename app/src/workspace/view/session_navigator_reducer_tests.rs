//! Session Navigator Reducer 测试
//!
//! 覆盖:
//! - 单步 fixture (基本 action)
//! - 序列型测试 (操作链)
//! - oracle 对照 (不变量)
//! - 排序稳定性
//! - tie-break
//! - validator

use std::collections::{HashMap, HashSet};

use super::*;
use crate::app_state::{WorkspaceSessionKind, WorkspaceSessionSnapshot};

// ─────────────────────────────────────────────────────────
// 测试辅助
// ─────────────────────────────────────────────────────────

fn make_session(id: &str, env: &str, is_live: bool, updated_at: i64) -> WorkspaceSessionSnapshot {
    let is_active = false;
    let is_pinned = false;
    let cli_agent_session_id = if !is_live {
        Some(format!("{id}-agent-session"))
    } else {
        None
    };
    let mut session = WorkspaceSessionSnapshot {
        id: id.to_string(),
        container_uuid: is_live.then(|| id.as_bytes().to_vec()),
        kind: WorkspaceSessionKind::AgentTerminal,
        label: Some(id.to_string()),
        environment_authority_key: Some(env.to_string()),
        cwd: Some("/tmp".to_string()),
        startup_directory: None,
        cli_agent: Some("codex".to_string()),
        cli_command: Some("codex".to_string()),
        cli_agent_origin: None,
        conversation_ids: Vec::new(),
        active_conversation_id: None,
        cli_agent_session_id,
        is_active,
        is_pinned,
        updated_at_unix_ms: Some(updated_at),
        is_live_container: is_live,
    };
    // 与 from_tabs 一致：live container 有 pane UUID，但没有 provider session id
    // 也仍然拥有稳定容器身份。
    if is_live {
        session.cli_agent_session_id = None;
        session.is_live_container = true;
    }
    session
}

fn make_live_session(id: &str, env: &str, updated_at: i64) -> WorkspaceSessionSnapshot {
    make_session(id, env, true, updated_at)
}

fn make_virtual_session(id: &str, env: &str, updated_at: i64) -> WorkspaceSessionSnapshot {
    make_session(id, env, false, updated_at)
}

fn make_pane_info(
    tabs: Vec<(
        usize,
        usize,
        Option<PaneViewLocator>,
        Option<PaneViewLocator>,
    )>,
) -> PaneGroupInfo {
    // (tab_index, visible_pane_count, focused_locator, prev_pane_locator)
    let mut info = PaneGroupInfo::new();
    for (tab, count, focused, prev) in tabs {
        let mut all_locators = Vec::new();
        for leaf in 0..count {
            all_locators.push(PaneViewLocator {
                pane_group_id: warpui::EntityId::from_usize(tab + 1),
                pane_id: crate::pane_group::PaneId::test_from_usize(leaf),
            });
        }
        info.tabs.insert(
            tab,
            TabPaneInfo {
                visible_pane_count: count,
                focused_locator: focused,
                prev_pane_locator: prev,
                all_pane_locators: all_locators,
            },
        );
    }
    info
}

fn default_pane_info_for(sessions: &[WorkspaceSessionSnapshot]) -> PaneGroupInfo {
    let mut tabs = HashMap::new();
    for session in sessions {
        if let Some((tab, leaf)) = locator_from_session_id(&session.id) {
            let entry = tabs.entry(tab).or_insert((0usize, HashSet::new()));
            entry.1.insert(leaf);
            entry.0 += 1;
        }
    }
    let mut info = PaneGroupInfo::new();
    for (tab, (count, leaves)) in tabs {
        let mut all_locators: Vec<PaneViewLocator> = leaves
            .iter()
            .map(|&leaf| PaneViewLocator {
                pane_group_id: warpui::EntityId::from_usize(tab + 1),
                pane_id: crate::pane_group::PaneId::test_from_usize(leaf),
            })
            .collect();
        all_locators.sort_by_key(|l| l.pane_id);
        let focused = all_locators.first().cloned();
        let prev = if all_locators.len() > 1 {
            all_locators.get(1).cloned()
        } else {
            None
        };
        info.tabs.insert(
            tab,
            TabPaneInfo {
                visible_pane_count: count,
                focused_locator: focused,
                prev_pane_locator: prev,
                all_pane_locators: all_locators,
            },
        );
    }
    info
}

fn make_locator(tab: usize, leaf: usize) -> PaneViewLocator {
    PaneViewLocator {
        pane_group_id: warpui::EntityId::from_usize(tab + 1),
        pane_id: crate::pane_group::PaneId::test_from_usize(leaf),
    }
}

// ─────────────────────────────────────────────────────────
// EC-06: 列表为空时插入新项
// ─────────────────────────────────────────────────────────

#[test]
fn test_ec06_empty_list_insert() {
    let state = SessionNavigatorState::new();
    let pane_info = PaneGroupInfo::new();

    let new_session = make_virtual_session("agent-1", "local", 1000);
    let result = reduce(
        Vec::new(),
        state,
        SessionNavigatorAction::Refresh {
            new_sessions: vec![new_session],
            pinned_session_ids: HashSet::new(),
        },
        &pane_info,
    );

    assert_eq!(result.sessions.len(), 1);
    assert!(!result.sessions[0].is_active);
    let row_id = row_id_for_session(&result.sessions[0], &result.state);
    assert_eq!(
        result.state.display_order.get(&row_id),
        Some(&0),
        "first session should get order 0"
    );
    validate_state(&result.sessions, &result.state).unwrap();
}

// ─────────────────────────────────────────────────────────
// EC-07: 多项同一时刻创建 → tie-break by logical_key
// ─────────────────────────────────────────────────────────

#[test]
fn test_ec07_same_timestamp_tiebreak() {
    let state = SessionNavigatorState::new();
    let pane_info = PaneGroupInfo::new();

    // 三个 session, 同一 timestamp
    let s_b = make_virtual_session("agent-bbb", "local", 1000);
    let s_a = make_virtual_session("agent-aaa", "local", 1000);
    let s_c = make_virtual_session("agent-ccc", "local", 1000);

    let result = reduce(
        Vec::new(),
        state,
        SessionNavigatorAction::Refresh {
            new_sessions: vec![s_b.clone(), s_a.clone(), s_c.clone()],
            pinned_session_ids: HashSet::new(),
        },
        &pane_info,
    );

    // reconcile 按 updated_at DESC → logical_key ASC 排序后分配 order
    // 三个 updated_at 相同 → logical_key ASC: agent-aaa < agent-bbb < agent-ccc
    // order: aaa=0, bbb=1, ccc=2
    // sort 后也是这个顺序
    assert_eq!(result.sessions[0].id, "agent-aaa");
    assert_eq!(result.sessions[1].id, "agent-bbb");
    assert_eq!(result.sessions[2].id, "agent-ccc");
    validate_state(&result.sessions, &result.state).unwrap();
}

// ─────────────────────────────────────────────────────────
// EC-10: pin/unpin 不改 selection 或 focus
// ─────────────────────────────────────────────────────────

#[test]
fn test_ec10_pin_does_not_change_focus() {
    let mut state = SessionNavigatorState::new();
    let s1 = make_live_session("tab:0:leaf:0", "local", 1000);
    let s2 = make_live_session("tab:1:leaf:0", "local", 2000);

    // 先 refresh 建立初始状态
    let pane_info = default_pane_info_for(&[s1.clone(), s2.clone()]);
    let result = reduce(
        Vec::new(),
        state.clone(),
        SessionNavigatorAction::Refresh {
            new_sessions: vec![s1.clone(), s2.clone()],
            pinned_session_ids: HashSet::new(),
        },
        &pane_info,
    );
    state = result.state;

    // pin s1
    let key_s1 = logical_key(&s1);
    let result = reduce(
        result.sessions,
        state,
        SessionNavigatorAction::Pin {
            session_logical_key: key_s1,
            pinned: true,
        },
        &pane_info,
    );

    // is_active 不应变
    let s1_after = result
        .sessions
        .iter()
        .find(|s| s.id == "tab:0:leaf:0")
        .unwrap();
    let s2_after = result
        .sessions
        .iter()
        .find(|s| s.id == "tab:1:leaf:0")
        .unwrap();
    assert_eq!(s1_after.is_active, false, "pin should not change is_active");
    assert_eq!(s2_after.is_active, false, "pin should not change is_active");
    assert_eq!(result.side_effect, SideEffect::WriteUserState);
    assert_eq!(
        result.state.selected_row_id, None,
        "pin should not change selected_row_id"
    );
    validate_state(&result.sessions, &result.state).unwrap();
}

// ─────────────────────────────────────────────────────────
// EC-01: 删除 split tab 中的一个 pane → focus 留在同 tab 的兄弟
// ─────────────────────────────────────────────────────────

#[test]
fn test_ec01_delete_split_pane_focus_stays_in_same_tab() {
    let mut state = SessionNavigatorState::new();

    // tab 0 有两个 pane: leaf 0 和 leaf 1
    let s0 = make_live_session("tab:0:leaf:0", "local", 1000);
    let s1 = make_live_session("tab:0:leaf:1", "local", 2000);

    let pane_info = make_pane_info(vec![(
        0,
        2,
        Some(make_locator(0, 0)),
        Some(make_locator(0, 1)),
    )]);

    // refresh
    let result = reduce(
        Vec::new(),
        state.clone(),
        SessionNavigatorAction::Refresh {
            new_sessions: vec![s0.clone(), s1.clone()],
            pinned_session_ids: HashSet::new(),
        },
        &pane_info,
    );
    state = result.state;

    // 删除 s0 (focused pane)
    let key_s0 = logical_key(&s0);
    let result = reduce(
        result.sessions,
        state,
        SessionNavigatorAction::Delete {
            session_logical_key: key_s0,
            session_id: s0.id.clone(),
            environment_authority_key: s0.environment_authority_key.clone(),
            session_identity_keys: Vec::new(),
        },
        &pane_info,
    );

    // side_effect: DeleteEffects — focus 同 tab 兄弟 + ClosePane
    match &result.side_effect {
        SideEffect::DeleteEffects(effects) => {
            let locator = effects
                .focus
                .as_ref()
                .expect("focus should go to sibling pane");
            assert_eq!(
                locator.pane_id,
                crate::pane_group::PaneId::test_from_usize(1),
                "focus should go to sibling pane (leaf 1) in same tab"
            );
            match &effects.close {
                DeleteCloseKind::ClosePane(closed) => {
                    assert_eq!(
                        closed.pane_id,
                        crate::pane_group::PaneId::test_from_usize(0),
                        "should close the deleted pane (leaf 0)"
                    );
                }
                other => panic!("expected ClosePane, got {other:?}"),
            }
        }
        other => panic!("expected DeleteEffects, got {other:?}"),
    }

    // s0 应该被移除
    assert_eq!(result.sessions.len(), 1);
    assert_eq!(result.sessions[0].id, "tab:0:leaf:1");
    validate_state(&result.sessions, &result.state).unwrap();
}

// ─────────────────────────────────────────────────────────
// EC-04: 当前聚焦项被删, 同组兄弟获 focus; 其它行 selection 不变
// ─────────────────────────────────────────────────────────

#[test]
fn test_ec04_delete_focused_keeps_other_row_selection_semantics() {
    let mut state = SessionNavigatorState::new();

    let s0 = make_live_session("tab:0:leaf:0", "local", 1000);
    let s1 = make_live_session("tab:0:leaf:1", "local", 2000);
    let s_virtual = make_virtual_session("agent-codex:other-selected", "local", 3000);

    let pane_info = make_pane_info(vec![(
        0,
        2,
        Some(make_locator(0, 0)),
        Some(make_locator(0, 1)),
    )]);

    let result = reduce(
        Vec::new(),
        state.clone(),
        SessionNavigatorAction::Refresh {
            new_sessions: vec![s0.clone(), s1.clone(), s_virtual.clone()],
            pinned_session_ids: HashSet::new(),
        },
        &pane_info,
    );
    state = result.state;
    // Mark the virtual row as the restored selection while focus is on s0.
    state.selected_row_id = Some(row_id_for_session(&s_virtual, &state));

    let result = reduce(
        result.sessions,
        state,
        SessionNavigatorAction::Delete {
            session_logical_key: logical_key(&s0),
            session_id: s0.id.clone(),
            environment_authority_key: s0.environment_authority_key.clone(),
            session_identity_keys: Vec::new(),
        },
        &pane_info,
    );

    match &result.side_effect {
        SideEffect::DeleteEffects(effects) => {
            let locator = effects
                .focus
                .as_ref()
                .expect("focus should go to sibling pane");
            assert_eq!(
                locator.pane_id,
                crate::pane_group::PaneId::test_from_usize(1),
                "focus should go to sibling pane"
            );
            assert!(matches!(effects.close, DeleteCloseKind::ClosePane(_)));
        }
        other => panic!("expected DeleteEffects, got {other:?}"),
    }

    assert!(
        result.sessions.iter().all(|s| s.id != s0.id),
        "deleted focused session must be gone"
    );
    assert!(
        result
            .sessions
            .iter()
            .any(|s| logical_key(s) == logical_key(&s_virtual)),
        "unrelated virtual selection row must remain"
    );
    // Sibling becomes the live focus target; virtual row must not be invented as the focus target.
    assert_ne!(
        result.state.selected_row_id.as_deref(),
        Some(logical_key(&s0).as_str()),
        "deleted key must not remain active"
    );
    validate_state(&result.sessions, &result.state).unwrap();
}

// ─────────────────────────────────────────────────────────
// EC-05: active 被删 → Activate 恢复 → Pin 不改 focus
// ─────────────────────────────────────────────────────────

#[test]
fn test_ec05_delete_active_then_activate_then_pin() {
    let mut state = SessionNavigatorState::new();
    let s_live = make_live_session("tab:0:leaf:0", "local", 1000);
    let s_virtual = make_virtual_session("agent-codex:session-ec05", "local", 2000);

    let pane_info = make_pane_info(vec![(0, 1, Some(make_locator(0, 0)), None)]);

    let result = reduce(
        Vec::new(),
        state.clone(),
        SessionNavigatorAction::Refresh {
            new_sessions: vec![s_live.clone(), s_virtual.clone()],
            pinned_session_ids: HashSet::new(),
        },
        &pane_info,
    );
    state = result.state;

    // Activate live so it is the active selection, then delete it.
    let live_key = logical_key(&s_live);
    let result = reduce(
        result.sessions,
        state,
        SessionNavigatorAction::Activate {
            session_logical_key: live_key.clone(),
            session_id: s_live.id.clone(),
            is_live: true,
        },
        &pane_info,
    );
    let live_row_id = row_id_for_session(&s_live, &result.state);
    assert_eq!(result.state.selected_row_id, Some(live_row_id));

    let pane_info_after_delete = PaneGroupInfo::new();
    let result = reduce(
        result.sessions,
        result.state,
        SessionNavigatorAction::Delete {
            session_logical_key: live_key,
            session_id: s_live.id.clone(),
            environment_authority_key: s_live.environment_authority_key.clone(),
            session_identity_keys: Vec::new(),
        },
        &pane_info_after_delete,
    );
    assert!(result.sessions.iter().all(|s| s.id != s_live.id));

    // Restore virtual row.
    let virtual_key = logical_key(&s_virtual);
    let result = reduce(
        result.sessions,
        result.state,
        SessionNavigatorAction::Activate {
            session_logical_key: virtual_key.clone(),
            session_id: s_virtual.id.clone(),
            is_live: false,
        },
        &pane_info_after_delete,
    );
    let virtual_row_id = row_id_for_session(&s_virtual, &result.state);
    assert_eq!(result.state.selected_row_id, Some(virtual_row_id.clone()));
    assert!(result.state.restoring_row_ids.contains(&virtual_row_id));
    assert!(matches!(
        result.side_effect,
        SideEffect::SpawnTerminal { .. }
    ));

    let active_before_pin = result.state.selected_row_id.clone();
    let result = reduce(
        result.sessions,
        result.state,
        SessionNavigatorAction::Pin {
            session_logical_key: virtual_key,
            pinned: true,
        },
        &pane_info_after_delete,
    );
    assert_eq!(
        result.state.selected_row_id, active_before_pin,
        "pin must not change selected_row_id"
    );
    assert!(matches!(result.side_effect, SideEffect::WriteUserState));
    validate_state(&result.sessions, &result.state).unwrap();
}

// ─────────────────────────────────────────────────────────
// EC-02: 删除唯一 pane 的 tab → focus 同 env 邻居 tab
// ─────────────────────────────────────────────────────────

#[test]
fn test_ec02_delete_only_pane_focus_same_env_neighbor() {
    let mut state = SessionNavigatorState::new();

    let s0 = make_live_session("tab:0:leaf:0", "local", 1000);
    let s1 = make_live_session("tab:1:leaf:0", "local", 2000);

    let pane_info = make_pane_info(vec![
        (0, 1, Some(make_locator(0, 0)), None),
        (1, 1, Some(make_locator(1, 0)), None),
    ]);

    let result = reduce(
        Vec::new(),
        state.clone(),
        SessionNavigatorAction::Refresh {
            new_sessions: vec![s0.clone(), s1.clone()],
            pinned_session_ids: HashSet::new(),
        },
        &pane_info,
    );
    state = result.state;

    // 删除 s0 (tab 0 的唯一 pane)
    let key_s0 = logical_key(&s0);
    let result = reduce(
        result.sessions,
        state,
        SessionNavigatorAction::Delete {
            session_logical_key: key_s0,
            session_id: s0.id.clone(),
            environment_authority_key: s0.environment_authority_key.clone(),
            session_identity_keys: Vec::new(),
        },
        &pane_info,
    );

    // side_effect: DeleteEffects — focus 同 env 邻居 + CloseTab
    match &result.side_effect {
        SideEffect::DeleteEffects(effects) => {
            let locator = effects
                .focus
                .as_ref()
                .expect("focus should go to neighbor tab");
            assert_eq!(
                locator.pane_id,
                crate::pane_group::PaneId::test_from_usize(0),
                "focus should go to neighbor tab's pane"
            );
            assert_eq!(
                effects.close,
                DeleteCloseKind::CloseTab(0),
                "should close the deleted tab"
            );
        }
        other => panic!("expected DeleteEffects, got {other:?}"),
    }

    assert_eq!(result.sessions.len(), 1);
    assert_eq!(result.sessions[0].id, "tab:1:leaf:0");
    validate_state(&result.sessions, &result.state).unwrap();
}

// ─────────────────────────────────────────────────────────
// EC-09: 最后一项被删除时的 fallback focus
// ─────────────────────────────────────────────────────────

#[test]
fn test_ec09_delete_last_session_no_focus() {
    let mut state = SessionNavigatorState::new();

    let s0 = make_live_session("tab:0:leaf:0", "local", 1000);
    let pane_info = make_pane_info(vec![(0, 1, Some(make_locator(0, 0)), None)]);

    let result = reduce(
        Vec::new(),
        state.clone(),
        SessionNavigatorAction::Refresh {
            new_sessions: vec![s0.clone()],
            pinned_session_ids: HashSet::new(),
        },
        &pane_info,
    );
    state = result.state;

    let key_s0 = logical_key(&s0);
    let result = reduce(
        result.sessions,
        state,
        SessionNavigatorAction::Delete {
            session_logical_key: key_s0,
            session_id: s0.id.clone(),
            environment_authority_key: s0.environment_authority_key.clone(),
            session_identity_keys: Vec::new(),
        },
        &pane_info,
    );

    assert!(result.sessions.is_empty());
    match &result.side_effect {
        SideEffect::DeleteEffects(effects) => {
            assert!(
                effects.focus.is_none(),
                "no focus target when deleting last session"
            );
            assert_eq!(
                effects.close,
                DeleteCloseKind::CloseTab(0),
                "close the sole tab; adapter may refuse window close"
            );
        }
        other => panic!("expected DeleteEffects, got {other:?}"),
    }
    validate_state(&result.sessions, &result.state).unwrap();
}

// ─────────────────────────────────────────────────────────
// EC-03: 删除窗口唯一 tab 的唯一 pane → CloseTab, 无跨 env focus
// ─────────────────────────────────────────────────────────

#[test]
fn test_ec03_delete_only_tab_only_pane_close_without_cross_env_focus() {
    let mut state = SessionNavigatorState::new();

    let s0 = make_live_session("tab:0:leaf:0", "local", 1000);
    let pane_info = make_pane_info(vec![(0, 1, Some(make_locator(0, 0)), None)]);

    let result = reduce(
        Vec::new(),
        state.clone(),
        SessionNavigatorAction::Refresh {
            new_sessions: vec![s0.clone()],
            pinned_session_ids: HashSet::new(),
        },
        &pane_info,
    );
    state = result.state;

    let key_s0 = logical_key(&s0);
    let before = ReduceResult {
        sessions: result.sessions.clone(),
        state: state.clone(),
        side_effect: SideEffect::None,
    };
    let action = SessionNavigatorAction::Delete {
        session_logical_key: key_s0.clone(),
        session_id: s0.id.clone(),
        environment_authority_key: s0.environment_authority_key.clone(),
        session_identity_keys: Vec::new(),
    };
    let result = reduce(result.sessions, state, action.clone(), &pane_info);
    validate_transition(&before, &result, &action, &pane_info).unwrap();

    match &result.side_effect {
        SideEffect::DeleteEffects(effects) => {
            assert!(effects.focus.is_none());
            assert_eq!(effects.close, DeleteCloseKind::CloseTab(0));
        }
        other => panic!("expected DeleteEffects, got {other:?}"),
    }
    assert!(result.sessions.is_empty());
    validate_state(&result.sessions, &result.state).unwrap();
}

// ─────────────────────────────────────────────────────────
// EC-15: split/fork 后新 pane 获得焦点 (Activate live)
// ─────────────────────────────────────────────────────────

#[test]
fn test_ec15_split_fork_activates_new_pane() {
    let mut state = SessionNavigatorState::new();
    let s0 = make_live_session("tab:0:leaf:0", "local", 1000);
    let s1 = make_live_session("tab:0:leaf:1", "local", 2000);
    let pane_info = make_pane_info(vec![(
        0,
        2,
        Some(make_locator(0, 1)),
        Some(make_locator(0, 0)),
    )]);

    let result = reduce(
        Vec::new(),
        state.clone(),
        SessionNavigatorAction::Refresh {
            new_sessions: vec![s0.clone(), s1.clone()],
            pinned_session_ids: HashSet::new(),
        },
        &pane_info,
    );
    state = result.state;

    let key_s1 = logical_key(&s1);
    let result = reduce(
        result.sessions,
        state,
        SessionNavigatorAction::Activate {
            session_logical_key: key_s1.clone(),
            session_id: s1.id.clone(),
            is_live: true,
        },
        &pane_info,
    );

    match &result.side_effect {
        SideEffect::FocusPane(locator) => {
            assert_eq!(
                locator.pane_id,
                crate::pane_group::PaneId::test_from_usize(1),
                "forked pane should receive focus"
            );
        }
        other => panic!("expected FocusPane for forked pane, got {other:?}"),
    }
    let row_id_s1 = row_id_for_session(&s1, &result.state);
    assert_eq!(
        result.state.selected_row_id.as_deref(),
        Some(row_id_s1.as_str())
    );
    validate_state(&result.sessions, &result.state).unwrap();
}

// ─────────────────────────────────────────────────────────
// EC-13: 被删 session 的 active 不猜测转移
// ─────────────────────────────────────────────────────────

#[test]
fn test_ec13_deleted_active_no_guess_transfer() {
    let mut state = SessionNavigatorState::new();

    let s0 = make_live_session("tab:0:leaf:0", "local", 1000);
    let s1 = make_live_session("tab:1:leaf:0", "local", 2000);
    let pane_info = make_pane_info(vec![
        (0, 1, Some(make_locator(0, 0)), None),
        (1, 1, Some(make_locator(1, 0)), None),
    ]);

    let result = reduce(
        Vec::new(),
        state.clone(),
        SessionNavigatorAction::Refresh {
            new_sessions: vec![s0.clone(), s1.clone()],
            pinned_session_ids: HashSet::new(),
        },
        &pane_info,
    );
    state = result.state;

    // 设 s0 为 active
    state.selected_row_id = Some(row_id_for_session(&s0, &state));

    // 删除 s0
    let key_s0 = logical_key(&s0);
    let result = reduce(
        vec![s0.clone(), s1.clone()],
        state,
        SessionNavigatorAction::Delete {
            session_logical_key: key_s0.clone(),
            session_id: s0.id.clone(),
            environment_authority_key: s0.environment_authority_key.clone(),
            session_identity_keys: Vec::new(),
        },
        &pane_info,
    );

    // selected_row_id 应该被清除 (不猜测转移到 s1)
    assert_eq!(
        result.state.selected_row_id, None,
        "selected_row_id should be cleared, not guessed transfer"
    );
    validate_state(&result.sessions, &result.state).unwrap();
}

// ─────────────────────────────────────────────────────────
// EC-11: virtual → live consume 时 RowId 不迁移
// ─────────────────────────────────────────────────────────

#[test]
fn test_ec11_materialization_preserves_selected_row_id() {
    let mut state = SessionNavigatorState::new();

    // 先有 virtual row (cli_agent_session_id = "session-123" to match live session)
    let mut virtual_session = make_virtual_session("agent-codex:session-123", "local", 1000);
    virtual_session.cli_agent_session_id = Some("session-123".to_string());
    let pane_info = PaneGroupInfo::new();

    let result = reduce(
        Vec::new(),
        state.clone(),
        SessionNavigatorAction::Refresh {
            new_sessions: vec![virtual_session.clone()],
            pinned_session_ids: HashSet::new(),
        },
        &pane_info,
    );
    state = result.state;

    // selected_row_id 保存首次观察得到的稳定 RowId
    let virtual_key = logical_key(&virtual_session);
    let virtual_row_id = row_id_for_session(&virtual_session, &state);
    state.selected_row_id = Some(virtual_row_id.clone());

    // 现在 virtual row 被 consume: live container 出现, durable key 匹配
    let mut live_session = make_live_session("tab:0:leaf:0", "local", 2000);
    // live session 通过相同 durable identity 绑定到既有 RowId
    live_session.cli_agent_session_id = Some("session-123".to_string());

    let pane_info = default_pane_info_for(&[live_session.clone()]);

    let live_session_clone = live_session.clone();
    let result = reduce(
        vec![live_session],
        state,
        SessionNavigatorAction::Refresh {
            new_sessions: vec![live_session_clone],
            pinned_session_ids: HashSet::new(),
        },
        &pane_info,
    );

    let live_key = logical_key(&result.sessions[0]);
    assert_ne!(
        live_key, virtual_key,
        "live logical identity should differ from virtual identity"
    );
    assert_eq!(
        result.state.selected_row_id,
        Some(virtual_row_id.clone()),
        "materialization must add an identity alias without migrating selected RowId"
    );
    assert_eq!(
        row_id_for_session(&result.sessions[0], &result.state),
        virtual_row_id,
        "live identity must resolve to the original virtual RowId"
    );
    validate_state(&result.sessions, &result.state).unwrap();
}

// ─────────────────────────────────────────────────────────
// EC-14: 跨 environment 删除 → focus 留在被删 session 的 environment
// ─────────────────────────────────────────────────────────

#[test]
fn test_ec14_cross_env_delete_stays_in_deleted_env() {
    let mut state = SessionNavigatorState::new();

    let local_session = make_live_session("tab:0:leaf:0", "local", 1000);
    let remote_session = make_live_session("tab:1:leaf:0", "ssh:host1", 2000);

    let pane_info = make_pane_info(vec![
        (0, 1, Some(make_locator(0, 0)), None),
        (1, 1, Some(make_locator(1, 0)), None),
    ]);

    let result = reduce(
        Vec::new(),
        state.clone(),
        SessionNavigatorAction::Refresh {
            new_sessions: vec![local_session.clone(), remote_session.clone()],
            pinned_session_ids: HashSet::new(),
        },
        &pane_info,
    );
    state = result.state;

    // 删除 remote session
    let key_remote = logical_key(&remote_session);
    let result = reduce(
        result.sessions,
        state,
        SessionNavigatorAction::Delete {
            session_logical_key: key_remote,
            session_id: remote_session.id.clone(),
            environment_authority_key: remote_session.environment_authority_key.clone(),
            session_identity_keys: Vec::new(),
        },
        &pane_info,
    );

    // side_effect: CloseTab(remote), 不 focus 到 local (不同 env)
    match &result.side_effect {
        SideEffect::DeleteEffects(effects) => {
            assert!(
                effects.focus.is_none(),
                "should not focus cross-environment tab, got {:?}",
                effects.focus
            );
            assert_eq!(effects.close, DeleteCloseKind::CloseTab(1));
        }
        other => panic!("unexpected side effect: {other:?}"),
    }
    validate_state(&result.sessions, &result.state).unwrap();
}

// ─────────────────────────────────────────────────────────
// 序列测试: create → activate → pin → remove → restore
// ─────────────────────────────────────────────────────────

#[test]
fn test_sequence_create_activate_pin_remove_restore() {
    let mut state = SessionNavigatorState::new();
    let mut sessions = Vec::new();

    // 1. 新建 terminal pane
    let s1 = make_live_session("tab:0:leaf:0", "local", 1000);
    let pane_info = default_pane_info_for(&[s1.clone()]);

    let result = reduce(
        sessions,
        state,
        SessionNavigatorAction::Refresh {
            new_sessions: vec![s1.clone()],
            pinned_session_ids: HashSet::new(),
        },
        &pane_info,
    );
    sessions = result.sessions;
    state = result.state;
    validate_state(&sessions, &state).unwrap();

    // 2. activate s1
    let key_s1 = logical_key(&s1);
    let result = reduce(
        sessions,
        state,
        SessionNavigatorAction::Activate {
            session_logical_key: key_s1.clone(),
            session_id: s1.id.clone(),
            is_live: true,
        },
        &pane_info,
    );
    sessions = result.sessions;
    state = result.state;
    let row_id_s1 = row_id_for_session(&s1, &state);
    assert_eq!(state.selected_row_id, Some(row_id_s1.clone()));
    validate_state(&sessions, &state).unwrap();

    // 3. pin s1
    let result = reduce(
        sessions,
        state,
        SessionNavigatorAction::Pin {
            session_logical_key: key_s1.clone(),
            pinned: true,
        },
        &pane_info,
    );
    sessions = result.sessions;
    state = result.state;
    assert_eq!(state.selected_row_id, Some(row_id_s1.clone()));
    validate_state(&sessions, &state).unwrap();

    // 3b. EC-08 reorder — active stays on logical_key
    let result = reduce(
        sessions,
        state,
        SessionNavigatorAction::Reorder {
            ordered_logical_keys: vec![key_s1.clone()],
        },
        &pane_info,
    );
    sessions = result.sessions;
    state = result.state;
    assert_eq!(state.selected_row_id, Some(row_id_s1));
    assert!(matches!(result.side_effect, SideEffect::None));
    validate_state(&sessions, &state).unwrap();

    // 4. remove s1
    let result = reduce(
        sessions,
        state,
        SessionNavigatorAction::Delete {
            session_logical_key: key_s1.clone(),
            session_id: s1.id.clone(),
            environment_authority_key: s1.environment_authority_key.clone(),
            session_identity_keys: Vec::new(),
        },
        &pane_info,
    );
    sessions = result.sessions;
    state = result.state;
    assert!(sessions.is_empty());
    assert_eq!(state.selected_row_id, None);
    validate_state(&sessions, &state).unwrap();

    // 5. restore: 新 virtual session 出现
    let s2 = make_virtual_session("agent-codex:session-restore", "local", 3000);
    let pane_info = PaneGroupInfo::new();
    let result = reduce(
        sessions,
        state,
        SessionNavigatorAction::Refresh {
            new_sessions: vec![s2.clone()],
            pinned_session_ids: HashSet::new(),
        },
        &pane_info,
    );
    sessions = result.sessions;
    state = result.state;
    assert_eq!(sessions.len(), 1);
    validate_state(&sessions, &state).unwrap();
}

// ─────────────────────────────────────────────────────────
// 序列测试: split → fork → delete one pane → focus stays
// ─────────────────────────────────────────────────────────

#[test]
fn test_sequence_split_fork_delete_focus_stays() {
    let mut state = SessionNavigatorState::new();

    // 初始: 1 tab 1 pane
    let s0 = make_live_session("tab:0:leaf:0", "local", 1000);
    let mut pane_info = make_pane_info(vec![(0, 1, Some(make_locator(0, 0)), None)]);

    let result = reduce(
        Vec::new(),
        state.clone(),
        SessionNavigatorAction::Refresh {
            new_sessions: vec![s0.clone()],
            pinned_session_ids: HashSet::new(),
        },
        &pane_info,
    );
    state = result.state;

    // split: 新 pane 出现
    let s1 = make_live_session("tab:0:leaf:1", "local", 2000);
    pane_info = make_pane_info(vec![(
        0,
        2,
        Some(make_locator(0, 1)),
        Some(make_locator(0, 0)),
    )]);

    let result = reduce(
        vec![s0.clone(), s1.clone()],
        state,
        SessionNavigatorAction::Refresh {
            new_sessions: vec![s0.clone(), s1.clone()],
            pinned_session_ids: HashSet::new(),
        },
        &pane_info,
    );
    state = result.state;

    // delete s1 (focused pane) → focus 应该回到 s0
    let key_s1 = logical_key(&s1);
    let result = reduce(
        result.sessions,
        state,
        SessionNavigatorAction::Delete {
            session_logical_key: key_s1,
            session_id: s1.id.clone(),
            environment_authority_key: s1.environment_authority_key.clone(),
            session_identity_keys: Vec::new(),
        },
        &pane_info,
    );

    match &result.side_effect {
        SideEffect::DeleteEffects(effects) => {
            let locator = effects
                .focus
                .as_ref()
                .expect("focus should go to sibling pane");
            assert_eq!(
                locator.pane_id,
                crate::pane_group::PaneId::test_from_usize(0),
                "focus should go to sibling pane (leaf 0)"
            );
            assert!(matches!(effects.close, DeleteCloseKind::ClosePane(_)));
        }
        other => panic!("expected DeleteEffects for sibling, got {other:?}"),
    }
    assert_eq!(result.sessions.len(), 1);
    assert_eq!(result.sessions[0].id, "tab:0:leaf:0");
    validate_state(&result.sessions, &result.state).unwrap();
}

// ─────────────────────────────────────────────────────────
// 排序稳定性: 多次排序结果相同
// ─────────────────────────────────────────────────────────

#[test]
fn test_sort_stability_multiple_sorts() {
    let state = SessionNavigatorState::new();
    let sessions = vec![
        make_virtual_session("agent-c", "local", 1000),
        make_virtual_session("agent-a", "local", 1000),
        make_virtual_session("agent-b", "local", 1000),
    ];

    let pane_info = PaneGroupInfo::new();
    let result = reduce(
        Vec::new(),
        state.clone(),
        SessionNavigatorAction::Refresh {
            new_sessions: sessions,
            pinned_session_ids: HashSet::new(),
        },
        &pane_info,
    );

    let order1: Vec<String> = result.sessions.iter().map(|s| s.id.clone()).collect();

    // 再排一次
    let mut sessions2 = result.sessions.clone();
    sort_sessions(&mut sessions2, &result.state);
    let order2: Vec<String> = sessions2.iter().map(|s| s.id.clone()).collect();

    assert_eq!(
        order1, order2,
        "sort should be stable across multiple sorts"
    );
}

#[test]
fn test_new_session_inserts_at_top_of_unpinned() {
    let pane_info = PaneGroupInfo::new();
    let older = make_virtual_session("agent-older", "local", 1000);
    let newer = make_virtual_session("agent-newer", "local", 2000);

    let first = reduce(
        Vec::new(),
        SessionNavigatorState::new(),
        SessionNavigatorAction::Refresh {
            new_sessions: vec![older.clone()],
            pinned_session_ids: HashSet::new(),
        },
        &pane_info,
    );
    assert_eq!(first.sessions[0].id, "agent-older");

    let second = reduce(
        first.sessions,
        first.state,
        SessionNavigatorAction::Refresh {
            new_sessions: vec![older.clone(), newer.clone()],
            pinned_session_ids: HashSet::new(),
        },
        &pane_info,
    );

    assert_eq!(
        second
            .sessions
            .iter()
            .map(|s| s.id.as_str())
            .collect::<Vec<_>>(),
        vec!["agent-newer", "agent-older"],
        "newly reconciled row must sit above existing unpinned rows"
    );
    let newer_order = second.state.display_order[&row_id_for_session(&newer, &second.state)];
    let older_order = second.state.display_order[&row_id_for_session(&older, &second.state)];
    assert!(
        newer_order > older_order,
        "newer row must receive a larger display_order ({newer_order} <= {older_order})"
    );
}

// ─────────────────────────────────────────────────────────
// tie-break: updated_at 为 None 排最后
// ─────────────────────────────────────────────────────────

#[test]
fn test_tie_break_none_updated_at_last() {
    let state = SessionNavigatorState::new();

    let s_with_time = make_virtual_session("agent-with-time", "local", 1000);
    let mut s_no_time = make_virtual_session("agent-no-time", "local", 1000);
    s_no_time.updated_at_unix_ms = None;

    let pane_info = PaneGroupInfo::new();
    let result = reduce(
        Vec::new(),
        state.clone(),
        SessionNavigatorAction::Refresh {
            new_sessions: vec![s_no_time.clone(), s_with_time.clone()],
            pinned_session_ids: HashSet::new(),
        },
        &pane_info,
    );

    // reconcile: updated_at DESC → Some(1000) 排前, None 排后
    // s_with_time order=0, s_no_time order=1
    // sort 后 s_with_time 在前
    assert_eq!(result.sessions[0].id, "agent-with-time");
    assert_eq!(result.sessions[1].id, "agent-no-time");
}

// ─────────────────────────────────────────────────────────
// EC-08: reorder 后 focus 跟随稳定 logical_key
// ─────────────────────────────────────────────────────────

#[test]
fn test_ec08_reorder_keeps_active_on_logical_key() {
    let s_a = make_virtual_session("agent-a", "local", 3000);
    let s_b = make_virtual_session("agent-b", "local", 2000);
    let s_c = make_virtual_session("agent-c", "local", 1000);
    let pane_info = PaneGroupInfo::new();

    let result = reduce(
        Vec::new(),
        SessionNavigatorState::new(),
        SessionNavigatorAction::Refresh {
            new_sessions: vec![s_a.clone(), s_b.clone(), s_c.clone()],
            pinned_session_ids: HashSet::new(),
        },
        &pane_info,
    );
    let key_b = logical_key(&s_b);
    let result = reduce(
        result.sessions,
        result.state,
        SessionNavigatorAction::Activate {
            session_logical_key: key_b.clone(),
            session_id: s_b.id.clone(),
            is_live: false,
        },
        &pane_info,
    );
    let row_id_b = row_id_for_session(&s_b, &result.state);
    assert_eq!(result.state.selected_row_id, Some(row_id_b.clone()));

    let before = ReduceResult {
        sessions: result.sessions.clone(),
        state: result.state.clone(),
        side_effect: SideEffect::None,
    };
    // Reverse display order by logical_key.
    let ordered = vec![logical_key(&s_c), logical_key(&s_b), logical_key(&s_a)];
    let action = SessionNavigatorAction::Reorder {
        ordered_logical_keys: ordered.clone(),
    };
    let result = reduce(result.sessions, result.state, action.clone(), &pane_info);
    validate_transition(&before, &result, &action, &pane_info).unwrap();

    assert_eq!(
        result.state.selected_row_id,
        Some(row_id_b),
        "reorder must keep selected_row_id on the same RowId"
    );
    let active = result
        .sessions
        .iter()
        .find(|s| s.is_active)
        .expect("one active row");
    assert_eq!(logical_key(active), key_b);
    // Order follows the reorder keys for rows that have an assigned order.
    let positions: HashMap<String, usize> = result
        .sessions
        .iter()
        .enumerate()
        .map(|(i, s)| (logical_key(s), i))
        .collect();
    assert!(
        positions[&logical_key(&s_c)] < positions[&logical_key(&s_a)],
        "reordered keys should control relative order"
    );
    validate_state(&result.sessions, &result.state).unwrap();
}

// ─────────────────────────────────────────────────────────
// EC-17: 同屏组拖拽 — 整组平移, leaf 邻接与相对序不变
// ─────────────────────────────────────────────────────────

#[test]
fn test_ec17_reorder_moves_split_group_as_unit() {
    let leaf0 = make_live_session("tab:0:leaf:0", "local", 1000);
    let leaf1 = make_live_session("tab:0:leaf:1", "local", 2000);
    let virtual_a = make_virtual_session("agent-ec17", "local", 3000);
    let pane_info = make_pane_info(vec![(
        0,
        2,
        Some(make_locator(0, 0)),
        Some(make_locator(0, 1)),
    )]);

    let result = reduce(
        Vec::new(),
        SessionNavigatorState::new(),
        SessionNavigatorAction::Refresh {
            new_sessions: vec![leaf0.clone(), leaf1.clone(), virtual_a.clone()],
            pinned_session_ids: HashSet::new(),
        },
        &pane_info,
    );
    let key0 = logical_key(&leaf0);
    let result = reduce(
        result.sessions,
        result.state,
        SessionNavigatorAction::Activate {
            session_logical_key: key0.clone(),
            session_id: leaf0.id.clone(),
            is_live: true,
        },
        &pane_info,
    );
    let row_id0 = row_id_for_session(&leaf0, &result.state);
    assert_eq!(result.state.selected_row_id, Some(row_id0.clone()));

    let units = build_reorder_units(&result.sessions);
    assert_eq!(units.len(), 2, "split group + virtual = 2 units");
    assert!(
        units
            .iter()
            .any(|unit| matches!(unit, ReorderUnit::Group { tab_index: 0, .. })),
        "expected tab:0 group unit among {units:?}"
    );

    // Construct unit list with group first, then drag past the virtual unit.
    let group_unit = units
        .iter()
        .find(|unit| matches!(unit, ReorderUnit::Group { .. }))
        .cloned()
        .expect("group");
    let single_unit = units
        .iter()
        .find(|unit| matches!(unit, ReorderUnit::Single { .. }))
        .cloned()
        .expect("single");
    let ordered = move_reorder_unit(vec![group_unit, single_unit], 0, 2);
    assert_eq!(
        ordered,
        vec![
            logical_key(&virtual_a),
            logical_key(&leaf0),
            logical_key(&leaf1),
        ]
    );

    let before = ReduceResult {
        sessions: result.sessions.clone(),
        state: result.state.clone(),
        side_effect: SideEffect::None,
    };
    let action = SessionNavigatorAction::Reorder {
        ordered_logical_keys: ordered,
    };
    let result = reduce(result.sessions, result.state, action.clone(), &pane_info);
    validate_transition(&before, &result, &action, &pane_info).unwrap();

    assert_eq!(result.state.selected_row_id, Some(row_id0));
    let ids: Vec<&str> = result.sessions.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["agent-ec17", "tab:0:leaf:0", "tab:0:leaf:1"],
        "group must stay contiguous after crossing virtual"
    );
    validate_state(&result.sessions, &result.state).unwrap();
}

#[test]
fn test_ec17_validate_rejects_split_adjacency_break() {
    let leaf0 = make_live_session("tab:0:leaf:0", "local", 1000);
    let leaf1 = make_live_session("tab:0:leaf:1", "local", 2000);
    let virtual_a = make_virtual_session("agent-ec17-break", "local", 3000);
    let pane_info = make_pane_info(vec![(
        0,
        2,
        Some(make_locator(0, 0)),
        Some(make_locator(0, 1)),
    )]);

    let result = reduce(
        Vec::new(),
        SessionNavigatorState::new(),
        SessionNavigatorAction::Refresh {
            new_sessions: vec![leaf0.clone(), leaf1.clone(), virtual_a.clone()],
            pinned_session_ids: HashSet::new(),
        },
        &pane_info,
    );
    let before = ReduceResult {
        sessions: result.sessions.clone(),
        state: result.state.clone(),
        side_effect: SideEffect::None,
    };
    // Intentionally interleave virtual between split leaves.
    let action = SessionNavigatorAction::Reorder {
        ordered_logical_keys: vec![
            logical_key(&leaf0),
            logical_key(&virtual_a),
            logical_key(&leaf1),
        ],
    };
    let after = reduce(result.sessions, result.state, action.clone(), &pane_info);
    let err = validate_transition(&before, &after, &action, &pane_info)
        .expect_err("breaking same_window adjacency must fail validation");
    assert!(
        err.contains("adjacency") || err.contains("leaf relative order"),
        "unexpected validator message: {err}"
    );
}

#[test]
fn test_build_reorder_units_groups_same_tab_leaves() {
    let leaf0 = make_live_session("tab:1:leaf:0", "local", 1000);
    let leaf1 = make_live_session("tab:1:leaf:1", "local", 2000);
    let sole = make_live_session("tab:2:leaf:0", "local", 1500);
    let virtual_a = make_virtual_session("agent-units", "local", 3000);
    let sessions = vec![
        leaf0.clone(),
        leaf1.clone(),
        sole.clone(),
        virtual_a.clone(),
    ];
    let units = build_reorder_units(&sessions);
    assert_eq!(units.len(), 3);
    match &units[0] {
        ReorderUnit::Group {
            tab_index,
            logical_keys,
        } => {
            assert_eq!(*tab_index, 1);
            assert_eq!(
                logical_keys,
                &vec![logical_key(&leaf0), logical_key(&leaf1)]
            );
        }
        other => panic!("expected group, got {other:?}"),
    }
    assert_eq!(
        units[1],
        ReorderUnit::Single {
            logical_key: logical_key(&sole)
        }
    );
    assert_eq!(
        units[2],
        ReorderUnit::Single {
            logical_key: logical_key(&virtual_a)
        }
    );
}

// ─────────────────────────────────────────────────────────
// oracle: active 不超过 1 个
// ─────────────────────────────────────────────────────────

#[test]
fn test_oracle_at_most_one_active() {
    let scenarios = vec![
        vec![make_live_session("tab:0:leaf:0", "local", 1000)],
        vec![
            make_live_session("tab:0:leaf:0", "local", 1000),
            make_live_session("tab:0:leaf:1", "local", 2000),
        ],
        vec![
            make_live_session("tab:0:leaf:0", "local", 1000),
            make_virtual_session("agent-a", "local", 2000),
        ],
        vec![make_virtual_session("agent-a", "local", 1000)],
    ];

    for sessions in scenarios {
        let state = SessionNavigatorState::new();
        let pane_info = default_pane_info_for(&sessions);
        let result = reduce(
            Vec::new(),
            state,
            SessionNavigatorAction::Refresh {
                new_sessions: sessions,
                pinned_session_ids: HashSet::new(),
            },
            &pane_info,
        );

        let active_count = result.sessions.iter().filter(|s| s.is_active).count();
        assert!(active_count <= 1, "expected ≤ 1 active, got {active_count}");
        validate_state(&result.sessions, &result.state).unwrap();
    }
}

/// Combinatorial oracle: for each action family, check invariants from SPEC §10.3.
#[test]
fn test_oracle_combinatorial_action_invariants() {
    let live0 = make_live_session("tab:0:leaf:0", "local", 1000);
    let live1 = make_live_session("tab:0:leaf:1", "local", 2000);
    let virtual_a = make_virtual_session("agent-oracle-a", "local", 3000);
    let virtual_b = make_virtual_session("agent-oracle-b", "local", 4000);

    let pane_info = make_pane_info(vec![(
        0,
        2,
        Some(make_locator(0, 0)),
        Some(make_locator(0, 1)),
    )]);

    let base = reduce(
        Vec::new(),
        SessionNavigatorState::new(),
        SessionNavigatorAction::Refresh {
            new_sessions: vec![
                live0.clone(),
                live1.clone(),
                virtual_a.clone(),
                virtual_b.clone(),
            ],
            pinned_session_ids: HashSet::new(),
        },
        &pane_info,
    );
    validate_state(&base.sessions, &base.state).unwrap();

    let actions: Vec<SessionNavigatorAction> = vec![
        SessionNavigatorAction::Activate {
            session_logical_key: logical_key(&virtual_a),
            session_id: virtual_a.id.clone(),
            is_live: false,
        },
        SessionNavigatorAction::Pin {
            session_logical_key: logical_key(&virtual_a),
            pinned: true,
        },
        SessionNavigatorAction::Reorder {
            // Keep same-tab live leaves contiguous (EC-17); move virtuals ahead of the group.
            ordered_logical_keys: vec![
                logical_key(&virtual_b),
                logical_key(&virtual_a),
                logical_key(&live0),
                logical_key(&live1),
            ],
        },
        SessionNavigatorAction::Delete {
            session_logical_key: logical_key(&live0),
            session_id: live0.id.clone(),
            environment_authority_key: live0.environment_authority_key.clone(),
            session_identity_keys: Vec::new(),
        },
        SessionNavigatorAction::PaneFocused {
            session_logical_key: Some(logical_key(&live1)),
        },
        SessionNavigatorAction::TabActivated,
        SessionNavigatorAction::Refresh {
            new_sessions: vec![live1.clone(), virtual_a.clone(), virtual_b.clone()],
            pinned_session_ids: HashSet::from([logical_key(&virtual_a)]),
        },
    ];

    let mut current = base;
    for action in actions {
        let before = current.clone();
        let active_before_pin = before.state.selected_row_id.clone();
        let is_pin = matches!(action, SessionNavigatorAction::Pin { .. });
        let is_reorder = matches!(action, SessionNavigatorAction::Reorder { .. });
        current = reduce(
            before.sessions.clone(),
            before.state.clone(),
            action.clone(),
            &pane_info,
        );
        validate_transition(&before, &current, &action, &pane_info).unwrap();

        let active_count = current.sessions.iter().filter(|s| s.is_active).count();
        assert!(active_count <= 1, "oracle: ≤1 active after {action:?}");

        if is_pin {
            assert_eq!(
                current.state.selected_row_id, active_before_pin,
                "oracle: pin must not change selected_row_id"
            );
            assert!(matches!(current.side_effect, SideEffect::WriteUserState));
        }
        if is_reorder {
            assert_eq!(
                current.state.selected_row_id, before.state.selected_row_id,
                "oracle: reorder must keep selected_row_id"
            );
            assert!(matches!(current.side_effect, SideEffect::None));
        }
        if let SessionNavigatorAction::Delete {
            session_logical_key,
            ..
        } = &action
        {
            assert!(
                current
                    .sessions
                    .iter()
                    .all(|s| logical_key(s) != *session_logical_key),
                "oracle: deleted session gone"
            );
            if let SideEffect::DeleteEffects(effects) = &current.side_effect {
                if let Some(focus) = &effects.focus {
                    let found = pane_info.tabs.values().any(|tab| {
                        tab.all_pane_locators
                            .iter()
                            .any(|locator| locator.pane_id == focus.pane_id)
                    });
                    assert!(found, "oracle: delete focus target must exist in pane_info");
                }
            }
        }
    }
}

// ─────────────────────────────────────────────────────────
// validator: 多个 active → 报错
// ─────────────────────────────────────────────────────────

#[test]
fn test_validator_multiple_active_fails() {
    let mut sessions = vec![
        make_live_session("tab:0:leaf:0", "local", 1000),
        make_live_session("tab:1:leaf:0", "local", 2000),
    ];
    sessions[0].is_active = true;
    sessions[1].is_active = true;

    let state = SessionNavigatorState::new();
    let result = validate_state(&sessions, &state);
    assert!(
        result.is_err(),
        "validator should fail with 2 active sessions"
    );
}

// ─────────────────────────────────────────────────────────
// validator: deleted session 在列表中 → 报错
// ─────────────────────────────────────────────────────────

#[test]
fn test_validator_deleted_session_in_list_fails() {
    let sessions = vec![make_live_session("tab:0:leaf:0", "local", 1000)];
    let mut state = SessionNavigatorState::new();
    let key = logical_key(&sessions[0]);
    state.deleting_row_ids.insert(key);

    let result = validate_state(&sessions, &state);
    assert!(
        result.is_err(),
        "validator should fail with deleted session in list"
    );
}

// ─────────────────────────────────────────────────────────
// validator: stale selected_row_id → 报错
// ─────────────────────────────────────────────────────────

#[test]
fn test_validator_stale_selected_row_id_fails() {
    let sessions = vec![make_live_session("tab:0:leaf:0", "local", 1000)];
    let mut state = SessionNavigatorState::new();
    state.selected_row_id = Some("nonexistent-key".to_string());

    let result = validate_state(&sessions, &state);
    assert!(
        result.is_err(),
        "validator should fail with stale selected_row_id"
    );
}

// ─────────────────────────────────────────────────────────
// EC-12: restoring 中用户切走 → focus 改 projection，selection/lifecycle 保留
// ─────────────────────────────────────────────────────────

#[test]
fn test_ec12_restoring_user_switch_away() {
    let mut state = SessionNavigatorState::new();

    // 有一个 live session (tab 0) 和一个 virtual session
    let s_live = make_live_session("tab:0:leaf:0", "local", 1000);
    let s_virtual = make_virtual_session("agent-codex:session-restore", "local", 2000);

    let pane_info = make_pane_info(vec![(0, 1, Some(make_locator(0, 0)), None)]);

    let result = reduce(
        Vec::new(),
        state.clone(),
        SessionNavigatorAction::Refresh {
            new_sessions: vec![s_live.clone(), s_virtual.clone()],
            pinned_session_ids: HashSet::new(),
        },
        &pane_info,
    );
    state = result.state;

    // activate virtual session → restoring
    let virtual_key = logical_key(&s_virtual);
    let result = reduce(
        result.sessions,
        state.clone(),
        SessionNavigatorAction::Activate {
            session_logical_key: virtual_key.clone(),
            session_id: s_virtual.id.clone(),
            is_live: false,
        },
        &pane_info,
    );

    // restoring session 应该 is_active = true
    let virtual_in_result = result
        .sessions
        .iter()
        .find(|s| logical_key(s) == virtual_key)
        .unwrap();
    assert!(
        virtual_in_result.is_active,
        "restoring session should be active"
    );

    // 用户切到 live session (PaneFocused)
    let selected_before_focus = result.state.selected_row_id.clone();
    let result = reduce(
        result.sessions,
        result.state,
        SessionNavigatorAction::PaneFocused {
            session_logical_key: Some(logical_key(&s_live)),
        },
        &pane_info,
    );

    // live session 应该 is_active = true (规则 1: live focus 优先)
    let live_in_result = result
        .sessions
        .iter()
        .find(|s| s.id == "tab:0:leaf:0")
        .unwrap();
    assert!(
        live_in_result.is_active,
        "live focused session should be active (priority 1)"
    );

    // restoring session 也可以 is_active = true (规则 2)
    // 但 normalize 后只能有一个 active → live 优先
    let active_count = result.sessions.iter().filter(|s| s.is_active).count();
    assert_eq!(active_count, 1, "only one active after normalize");
    assert_eq!(
        result.state.selected_row_id, selected_before_focus,
        "PaneFocused 只能改变 projection，不能污染 Environment selection"
    );

    validate_state(&result.sessions, &result.state).unwrap();
}

#[test]
fn test_restore_finished_clears_selection_only_when_row_has_no_rendered_entity() {
    let virtual_session = make_virtual_session("agent-codex:restore", "local", 1000);
    let logical_key = logical_key(&virtual_session);
    let pane_info = make_pane_info(Vec::new());
    let started = reduce(
        Vec::new(),
        SessionNavigatorState::new(),
        SessionNavigatorAction::RestoreStarted {
            session_keys: vec![virtual_session.id.clone(), logical_key.clone()],
            selected_logical_key: Some(logical_key.clone()),
        },
        &pane_info,
    );
    let restoring_row_id = started
        .state
        .selected_row_id
        .clone()
        .expect("restore start should select its stable RowId");

    let refused = reduce(
        Vec::new(),
        started.state.clone(),
        SessionNavigatorAction::RestoreFinished {
            session_keys: vec![virtual_session.id.clone(), logical_key.clone()],
        },
        &pane_info,
    );
    assert_eq!(refused.state.selected_row_id, None);
    assert!(!refused.state.restoring_row_ids.contains(&restoring_row_id));
    validate_state(&refused.sessions, &refused.state).unwrap();

    let materialized = reduce(
        vec![virtual_session],
        started.state,
        SessionNavigatorAction::RestoreFinished {
            session_keys: vec![logical_key],
        },
        &pane_info,
    );
    assert_eq!(
        materialized.state.selected_row_id,
        Some(restoring_row_id),
        "materialized row must retain the same selected RowId"
    );
    validate_state(&materialized.sessions, &materialized.state).unwrap();
}

// ─────────────────────────────────────────────────────────
// 序列测试: multi-tab delete → same-env neighbor
// ─────────────────────────────────────────────────────────

#[test]
fn test_sequence_multi_tab_delete_same_env_neighbor() {
    let mut state = SessionNavigatorState::new();

    // 3 tabs: 2 local + 1 remote
    let s_local1 = make_live_session("tab:0:leaf:0", "local", 1000);
    let s_local2 = make_live_session("tab:1:leaf:0", "local", 2000);
    let s_remote = make_live_session("tab:2:leaf:0", "ssh:host1", 3000);

    let pane_info = make_pane_info(vec![
        (0, 1, Some(make_locator(0, 0)), None),
        (1, 1, Some(make_locator(1, 0)), None),
        (2, 1, Some(make_locator(2, 0)), None),
    ]);

    let result = reduce(
        Vec::new(),
        state.clone(),
        SessionNavigatorAction::Refresh {
            new_sessions: vec![s_local1.clone(), s_local2.clone(), s_remote.clone()],
            pinned_session_ids: HashSet::new(),
        },
        &pane_info,
    );
    state = result.state;

    // 删除中间的 local tab (tab 1)
    let key_local2 = logical_key(&s_local2);
    let result = reduce(
        result.sessions,
        state,
        SessionNavigatorAction::Delete {
            session_logical_key: key_local2,
            session_id: s_local2.id.clone(),
            environment_authority_key: s_local2.environment_authority_key.clone(),
            session_identity_keys: Vec::new(),
        },
        &pane_info,
    );

    // side_effect 应该 focus 到同 env (local) 的 tab + CloseTab
    match &result.side_effect {
        SideEffect::DeleteEffects(effects) => {
            let locator = effects
                .focus
                .as_ref()
                .expect("should focus same-env neighbor");
            assert_eq!(
                locator.pane_group_id,
                warpui::EntityId::from_usize(1),
                "should focus same-env (local) neighbor tab"
            );
            assert_eq!(effects.close, DeleteCloseKind::CloseTab(1));
        }
        other => panic!("expected DeleteEffects, got {other:?}"),
    }

    assert_eq!(result.sessions.len(), 2);
    validate_state(&result.sessions, &result.state).unwrap();
}

// ─────────────────────────────────────────────────────────
// tie-break: materialize 后 id 变化 → order 不丢
// ─────────────────────────────────────────────────────────

#[test]
fn test_tie_break_materialize_id_change_order_preserved() {
    let mut state = SessionNavigatorState::new();

    // 先有 virtual session (cli_agent_session_id = "session-123" to match live session)
    let mut virtual_session = make_virtual_session("agent-codex:session-123", "local", 1000);
    virtual_session.cli_agent_session_id = Some("session-123".to_string());
    let pane_info = PaneGroupInfo::new();

    let result = reduce(
        Vec::new(),
        state.clone(),
        SessionNavigatorAction::Refresh {
            new_sessions: vec![virtual_session.clone()],
            pinned_session_ids: HashSet::new(),
        },
        &pane_info,
    );
    state = result.state;

    let virtual_row_id = row_id_for_session(&virtual_session, &state);
    let virtual_order = state.display_order.get(&virtual_row_id).copied();
    assert!(virtual_order.is_some(), "virtual session should have order");

    // materialize: virtual → live, id 变了但 durable key 相同
    let mut live_session = make_live_session("tab:0:leaf:0", "local", 2000);
    live_session.cli_agent_session_id = Some("session-123".to_string());

    let pane_info = default_pane_info_for(&[live_session.clone()]);

    let live_session_clone = live_session.clone();
    let result = reduce(
        vec![live_session],
        state,
        SessionNavigatorAction::Refresh {
            new_sessions: vec![live_session_clone],
            pinned_session_ids: HashSet::new(),
        },
        &pane_info,
    );

    // live session 应该继承 virtual session 的 order (通过 durable key)
    let live_row_id = row_id_for_session(&result.sessions[0], &result.state);
    let live_order = result.state.display_order.get(&live_row_id).copied();
    assert_eq!(
        live_order, virtual_order,
        "live session should inherit virtual session's order via durable key"
    );
    validate_state(&result.sessions, &result.state).unwrap();
}

// ─────────────────────────────────────────────────────────
// pin 组切换后组内顺序不变
// ─────────────────────────────────────────────────────────

#[test]
fn test_pin_group_switch_preserves_inner_order() {
    let mut state = SessionNavigatorState::new();

    let s_a = make_virtual_session("agent-aaa", "local", 1000);
    let s_b = make_virtual_session("agent-bbb", "local", 2000);
    let s_c = make_virtual_session("agent-ccc", "local", 3000);

    let pane_info = PaneGroupInfo::new();

    // 初始 refresh: order a=0, b=1, c=2
    let result = reduce(
        Vec::new(),
        state.clone(),
        SessionNavigatorAction::Refresh {
            new_sessions: vec![s_a.clone(), s_b.clone(), s_c.clone()],
            pinned_session_ids: HashSet::new(),
        },
        &pane_info,
    );
    state = result.state;

    // 排序: c, b, a (updated_at DESC)
    let order_before: Vec<String> = result.sessions.iter().map(|s| s.id.clone()).collect();
    assert_eq!(order_before, vec!["agent-ccc", "agent-bbb", "agent-aaa"]);

    // pin agent-aaa → 它应该排到最前面
    let mut pinned = HashSet::new();
    pinned.insert("agent-aaa".to_string());

    let result = reduce(
        result.sessions,
        state,
        SessionNavigatorAction::Refresh {
            new_sessions: vec![s_a.clone(), s_b.clone(), s_c.clone()],
            pinned_session_ids: pinned,
        },
        &pane_info,
    );

    // agent-aaa pinned → 排第一
    // b, c 的相对顺序不变
    let order_after: Vec<String> = result.sessions.iter().map(|s| s.id.clone()).collect();
    assert_eq!(
        order_after,
        vec!["agent-aaa", "agent-ccc", "agent-bbb"],
        "pinned session first, rest preserve order"
    );
    validate_state(&result.sessions, &result.state).unwrap();
}

#[test]
fn test_resume_materialization_preserves_exact_display_position() {
    let env = "local";
    let a = make_virtual_session("a", env, 300);
    let b = make_virtual_session("b", env, 200);
    let c = make_virtual_session("c", env, 100);
    let pane_info = PaneGroupInfo::new();

    let initial = reduce(
        Vec::new(),
        SessionNavigatorState::new(),
        SessionNavigatorAction::Refresh {
            new_sessions: vec![c.clone(), a.clone(), b.clone()],
            pinned_session_ids: HashSet::new(),
        },
        &pane_info,
    );
    let initial_keys: Vec<String> = initial.sessions.iter().map(logical_key).collect();
    assert_eq!(
        initial_keys,
        vec![logical_key(&a), logical_key(&b), logical_key(&c)]
    );
    let b_row_id = row_id_for_session(&b, &initial.state);
    let b_order = initial.state.display_order[&b_row_id];

    let activated = reduce(
        initial.sessions,
        initial.state,
        SessionNavigatorAction::Activate {
            session_logical_key: logical_key(&b),
            session_id: b.id.clone(),
            is_live: false,
        },
        &pane_info,
    );

    let mut b_live = make_live_session("tab:1:leaf:0", env, 400);
    b_live.cli_agent_session_id = b.cli_agent_session_id.clone();
    let materialized = reduce(
        activated.sessions,
        activated.state,
        SessionNavigatorAction::Refresh {
            new_sessions: vec![c.clone(), b_live.clone(), a.clone()],
            pinned_session_ids: HashSet::new(),
        },
        &default_pane_info_for(std::slice::from_ref(&b_live)),
    );

    assert_eq!(
        materialized
            .sessions
            .iter()
            .map(|session| session.id.as_str())
            .collect::<Vec<_>>(),
        vec!["a", "tab:1:leaf:0", "c"]
    );
    assert_eq!(row_id_for_session(&b_live, &materialized.state), b_row_id);
    assert_eq!(materialized.state.display_order[&b_row_id], b_order);
}

#[test]
fn test_refresh_merges_provisional_materialization_row_state_into_canonical_row() {
    let pane_info = PaneGroupInfo::new();
    let conversation_id = "conversation-1";
    let mut historical = make_virtual_session(
        &format!("ashide-conversation:{conversation_id}"),
        "local",
        200,
    );
    historical.cli_agent = None;
    historical.cli_command = None;
    historical.cli_agent_session_id = None;
    historical.conversation_ids = vec![conversation_id.to_owned()];

    let initial = reduce(
        Vec::new(),
        SessionNavigatorState::new(),
        SessionNavigatorAction::Refresh {
            new_sessions: vec![historical.clone()],
            pinned_session_ids: HashSet::new(),
        },
        &pane_info,
    );
    let canonical_row_id = row_id_for_session(&historical, &initial.state);
    let canonical_order = initial.state.display_order[&canonical_row_id];
    let next_display_order = initial.state.next_display_order;

    // 模拟异步 materialize：空 terminal 先出现并被错误入口短暂选中，随后才注册
    // conversation durable identity。Reducer 必须能把该 provisional RowId 完整吞并。
    let provisional_live = make_live_session("tab:1:leaf:0", "local", 300);
    let provisional_key = logical_key(&provisional_live);
    let selected = reduce(
        initial.sessions,
        initial.state,
        SessionNavigatorAction::SelectionChanged {
            session_logical_key: Some(provisional_key.clone()),
        },
        &pane_info,
    );
    let provisional_row_id = selected
        .state
        .selected_row_id
        .clone()
        .expect("provisional live row should be selected");
    assert_ne!(provisional_row_id, canonical_row_id);

    let mut materialized_live = provisional_live;
    materialized_live.conversation_ids = vec![conversation_id.to_owned()];
    let action = SessionNavigatorAction::Refresh {
        new_sessions: vec![materialized_live.clone()],
        pinned_session_ids: HashSet::new(),
    };
    let before = ReduceResult {
        sessions: selected.sessions.clone(),
        state: selected.state.clone(),
        side_effect: SideEffect::None,
    };
    let materialized = reduce(
        selected.sessions,
        selected.state,
        action.clone(),
        &default_pane_info_for(std::slice::from_ref(&materialized_live)),
    );

    assert_eq!(
        row_id_for_session(&materialized_live, &materialized.state),
        canonical_row_id
    );
    assert_eq!(
        materialized.state.selected_row_id,
        Some(canonical_row_id.clone())
    );
    assert_eq!(
        materialized.state.display_order[&canonical_row_id],
        canonical_order
    );
    assert_eq!(materialized.state.next_display_order, next_display_order);
    assert!(materialized
        .state
        .row_id_by_identity
        .values()
        .all(|row_id| row_id != &provisional_row_id));
    assert!(!materialized
        .state
        .display_order
        .contains_key(&provisional_row_id));
    assert!(!materialized
        .state
        .restoring_row_ids
        .contains(&provisional_row_id));
    assert!(!materialized
        .state
        .deleting_row_ids
        .contains(&provisional_row_id));
    assert!(!materialized
        .state
        .deleted_row_ids
        .contains(&provisional_row_id));
    validate_state(&materialized.sessions, &materialized.state).unwrap();
    validate_transition(
        &before,
        &materialized,
        &action,
        &default_pane_info_for(&[materialized_live]),
    )
    .unwrap();
}

#[test]
fn test_repeated_resume_and_refresh_never_reorders_existing_rows() {
    let env = "ssh:test";
    let mut virtuals = vec![
        make_virtual_session("a", env, 400),
        make_virtual_session("b", env, 300),
        make_virtual_session("c", env, 200),
        make_virtual_session("d", env, 100),
    ];
    let mut result = reduce(
        Vec::new(),
        SessionNavigatorState::new(),
        SessionNavigatorAction::Refresh {
            new_sessions: virtuals.clone(),
            pinned_session_ids: HashSet::new(),
        },
        &PaneGroupInfo::new(),
    );

    for (resume_index, target_index) in [2usize, 0, 3, 1].into_iter().enumerate() {
        let target = virtuals[target_index].clone();
        result = reduce(
            result.sessions,
            result.state,
            SessionNavigatorAction::Activate {
                session_logical_key: logical_key(&target),
                session_id: target.id.clone(),
                is_live: false,
            },
            &PaneGroupInfo::new(),
        );

        let mut live = make_live_session(&format!("tab:{resume_index}:leaf:0"), env, 999);
        live.cli_agent_session_id = target.cli_agent_session_id.clone();
        live.label = target.label.clone();
        virtuals[target_index] = live.clone();
        let mut refresh_input = virtuals.clone();
        let refresh_len = refresh_input.len();
        refresh_input.rotate_left((resume_index + 1) % refresh_len);
        result = reduce(
            result.sessions,
            result.state,
            SessionNavigatorAction::Refresh {
                new_sessions: refresh_input,
                pinned_session_ids: HashSet::new(),
            },
            &default_pane_info_for(&virtuals),
        );

        assert_eq!(
            result
                .sessions
                .iter()
                .map(|session| session.label.as_deref().unwrap_or_default())
                .collect::<Vec<_>>(),
            vec!["a", "b", "c", "d"],
            "第 {resume_index} 次 Resume 后顺序发生变化"
        );
    }
}

#[test]
fn test_session_navigator_spec_matrix_is_complete_and_linked() {
    let spec: serde_yaml::Value =
        serde_yaml::from_str(include_str!("../../../../docs/SESSION_NAVIGATOR_SPEC.yaml"))
            .expect("SESSION_NAVIGATOR_SPEC.yaml 必须是合法 YAML");
    let rows = spec["ux_contract_matrix"]["rows"]
        .as_sequence()
        .expect("ux_contract_matrix.rows 必须存在");
    let reducer_tests = include_str!("session_navigator_reducer_tests.rs");
    let workspace_tests = include_str!("../view_test.rs");
    let environment_table_tests = include_str!("../environment_table.rs");
    let app_state_tests = include_str!("../../app_state_tests.rs");
    let cli_agent_session_index_tests = include_str!("../../terminal/cli_agent_session_index.rs");
    let remote_cli_agent_session_tests = include_str!("../../remote_server/cli_agent_sessions.rs");
    let persistence_tests = include_str!("../../persistence/sqlite_tests.rs");
    let mut ids = HashSet::new();
    let mut linked_tests = HashSet::new();

    for row in rows {
        let id = row["id"].as_str().expect("matrix row 缺少 id");
        assert!(ids.insert(id), "UX matrix ID 重复: {id}");
        for field in ["scope", "event", "may_change", "must_preserve", "test"] {
            assert!(!row[field].is_null(), "{id} 缺少字段 {field}");
        }
        let test = row["test"].as_str().expect("matrix test 必须是字符串");
        assert!(
            linked_tests.insert(test),
            "UX matrix test 重复绑定，必须一条契约对应一个专用回归测试: {test}"
        );
        assert!(
            reducer_tests.contains(&format!("fn {test}"))
                || workspace_tests.contains(&format!("fn {test}"))
                || environment_table_tests.contains(&format!("fn {test}"))
                || app_state_tests.contains(&format!("fn {test}"))
                || cli_agent_session_index_tests.contains(&format!("fn {test}"))
                || remote_cli_agent_session_tests.contains(&format!("fn {test}"))
                || persistence_tests.contains(&format!("fn {test}")),
            "{id} 绑定的测试不存在: {test}"
        );
    }

    for required_id in [
        "SN-INTRA-RESUME-01",
        "SN-INTRA-RESUME-02",
        "SN-INTRA-MATERIALIZE-CONFLICT-01",
        "SN-INTRA-RESTORE-ASHIDE-01",
        "SN-INTRA-REFRESH-01",
        "SN-INTRA-ALIAS-01",
        "SN-INTRA-ENRICHMENT-QUOTA-01",
        "SN-INTRA-CODEX-COLD-ALIAS-01",
        "SN-INTRA-BINDING-PERSIST-01",
        "SN-INTRA-BINDING-SAVE-01",
        "SN-INTRA-PANE-IDENTITY-01",
        "SN-INTRA-CONTAINER-PERSIST-01",
        "SN-ENV-PLACEHOLDER-IDENTITY-01",
        "SN-INTRA-PANE-REUSE-01",
        "SN-INTRA-MERGE-ORDER-01",
        "SN-INTRA-COLD-LIVE-SOURCE-01",
        "SN-INTRA-READONLY-PERSIST-01",
        "SN-INTRA-USER-STATE-HYGIENE-01",
        "SN-DISCOVERY-LIMIT-01",
        "SN-INTRA-REORDER-01",
        "SN-INTRA-PIN-01",
        "SN-INTRA-FOCUS-01",
        "SN-INTRA-CREATE-LOCAL-01",
        "SN-ENV-CREATE-REMOTE-01",
        "SN-INTRA-DELETE-01",
        "SN-INTRA-DELETE-02",
        "SN-ENV-SWITCH-01",
        "SN-ENV-PANE-01",
        "SN-ENV-SWITCH-FALLBACK-01",
        "SN-ENV-ORDER-01",
        "SN-ENV-ALIAS-01",
        "SN-ENV-METADATA-01",
        "SN-ENV-IDENTITY-01",
        "SN-ENV-BG-01",
        "SN-ENV-CLOSE-01",
        "SN-ENV-DISCONNECT-01",
    ] {
        assert!(
            ids.contains(required_id),
            "UX matrix 缺少稳定契约 {required_id}"
        );
    }

    let ownership = spec["state_model"]["action_write_ownership"]["actions"]
        .as_mapping()
        .expect("action_write_ownership.actions 必须存在");
    let action_fixtures = vec![
        (
            "Delete",
            SessionNavigatorAction::Delete {
                session_logical_key: String::new(),
                session_id: String::new(),
                environment_authority_key: None,
                session_identity_keys: Vec::new(),
            },
        ),
        (
            "Activate",
            SessionNavigatorAction::Activate {
                session_logical_key: String::new(),
                session_id: String::new(),
                is_live: false,
            },
        ),
        (
            "SelectionChanged",
            SessionNavigatorAction::SelectionChanged {
                session_logical_key: None,
            },
        ),
        (
            "RestoreStarted",
            SessionNavigatorAction::RestoreStarted {
                session_keys: Vec::new(),
                selected_logical_key: None,
            },
        ),
        (
            "RestoreFinished",
            SessionNavigatorAction::RestoreFinished {
                session_keys: Vec::new(),
            },
        ),
        (
            "DeleteRolledBack",
            SessionNavigatorAction::DeleteRolledBack {
                session_keys: Vec::new(),
            },
        ),
        (
            "DeleteCommitted",
            SessionNavigatorAction::DeleteCommitted {
                session_keys: Vec::new(),
                volatile_identity_keys: Vec::new(),
            },
        ),
        (
            "Pin",
            SessionNavigatorAction::Pin {
                session_logical_key: String::new(),
                pinned: false,
            },
        ),
        (
            "Refresh",
            SessionNavigatorAction::Refresh {
                new_sessions: Vec::new(),
                pinned_session_ids: HashSet::new(),
            },
        ),
        ("TabActivated", SessionNavigatorAction::TabActivated),
        (
            "PaneFocused",
            SessionNavigatorAction::PaneFocused {
                session_logical_key: None,
            },
        ),
        (
            "Reorder",
            SessionNavigatorAction::Reorder {
                ordered_logical_keys: Vec::new(),
            },
        ),
    ];
    assert_eq!(
        ownership.len(),
        action_fixtures.len(),
        "SPEC action ownership 必须与 reducer action variants 精确一一对应"
    );
    for (name, action) in action_fixtures {
        let permissions = transition_permissions(&action);
        let actual = [
            ("position", permissions.position),
            ("identity", permissions.identity),
            ("selection", permissions.selection),
            ("lifecycle", permissions.lifecycle),
        ]
        .into_iter()
        .filter_map(|(slice, allowed)| allowed.then_some(slice))
        .collect::<HashSet<_>>();
        let expected = ownership[&serde_yaml::Value::String(name.to_owned())]
            .as_sequence()
            .unwrap_or_else(|| panic!("SPEC 缺少 action ownership: {name}"))
            .iter()
            .map(|slice| slice.as_str().expect("ownership slice 必须是字符串"))
            .collect::<HashSet<_>>();
        assert_eq!(actual, expected, "{name} 的 reducer/SPEC 写权限不一致");
    }

    let production_sources = [
        ("session_navigator.rs", include_str!("session_navigator.rs")),
        ("view.rs", include_str!("../view.rs")),
        ("vertical_tabs.rs", include_str!("vertical_tabs.rs")),
        (
            "environment_runtime.rs",
            include_str!("../environment_runtime.rs"),
        ),
    ];
    let forbidden_direct_writes = [
        ".selected_row_id =",
        ".restoring_row_ids.insert",
        ".restoring_row_ids.remove",
        ".restoring_row_ids.retain",
        ".restoring_row_ids.extend",
        ".deleting_row_ids.insert",
        ".deleting_row_ids.remove",
        ".deleting_row_ids.retain",
        ".deleting_row_ids.extend",
        ".deleted_row_ids.insert",
        ".deleted_row_ids.remove",
        ".deleted_row_ids.retain",
        ".deleted_row_ids.extend",
        ".display_order.insert",
        ".row_id_by_identity.insert",
        ".row_id_by_identity.remove",
        ".next_row_id =",
        ".next_display_order =",
    ];
    for (path, source) in production_sources {
        for pattern in forbidden_direct_writes {
            assert!(
                !source.contains(pattern),
                "{path} 禁止绕过 reducer 直接写 SessionNavigatorState: {pattern}"
            );
        }
    }

    let ai_contract = &spec["ai_change_contract"];
    for field in [
        "typed_boundary",
        "exhaustive_ownership",
        "canonical_identity",
        "projection_boundary",
        "persistence_boundary",
        "live_identity_failure",
        "stable_traceability",
        "change_protocol",
        "comment_policy",
    ] {
        assert!(
            !ai_contract[field].is_null(),
            "AI change contract 缺少 {field}"
        );
    }
    let reducer = include_str!("session_navigator_reducer.rs");
    assert!(
        reducer.contains("fn transition_permissions")
            && reducer.contains("SessionNavigatorAction::PaneFocused"),
        "typed action 的穷尽写权限检查不得从 reducer 中移除"
    );

    let product_principles = &spec["ai_programming_ux_principles"];
    for field in [
        "spatial_memory",
        "environment_memory",
        "background_safety",
        "identity_continuity",
        "compact_native_actions",
        "deterministic_feedback",
    ] {
        assert!(
            product_principles[field].is_string(),
            "AI programming UX principle 缺少 {field}"
        );
    }

    let persistence_contract = &spec["state_model"]["app_state_persistence"];
    for field in [
        "live_authority",
        "tree_boundary",
        "transient_semantics",
        "structural_normalization",
        "container_identity",
        "hard_cut",
    ] {
        assert!(
            persistence_contract[field].is_string(),
            "app-state persistence contract 缺少 {field}"
        );
    }

    let app_state = include_str!("../../app_state.rs");
    let pane_model = include_str!("../../pane_group/pane/mod.rs");
    let sqlite = include_str!("../../persistence/sqlite.rs");
    let container_identity_migration = include_str!(
        "../../../../crates/persistence/migrations/2026-07-12-235000_persist_pane_container_identity/up.sql"
    );
    assert!(
        app_state.contains("Navigator 可见 live pane 必须拥有稳定 container UUID")
            && !app_state.contains("return format!(\"{environment_key}::source:{}\", self.id);"),
        "live identity 禁止静默退回 tab/leaf locator"
    );
    assert!(
        pane_model.contains("container_uuid: Uuid::new_v4().as_bytes().to_vec()")
            && pane_model.contains("restore_container_uuid"),
        "PaneConfiguration 必须统一拥有并恢复 container UUID"
    );
    assert!(
        sqlite.contains("pane_container_identities")
            && sqlite.contains("uuid: pane.container_uuid.clone()"),
        "SQLite 必须显式持久化 pane container identity"
    );
    assert!(
        container_identity_migration.contains("randomblob(16)")
            && !container_identity_migration.contains("DELETE FROM windows"),
        "旧 pane 只能一次性分配随机 container UUID，禁止坐标迁移或清空用户布局"
    );
}

#[test]
fn test_cli_agent_binding_updates_trigger_app_state_save() {
    let source = include_str!("session_navigator.rs");
    let handler = source
        .split("pub(super) fn handle_cli_agent_sessions_event")
        .nth(1)
        .and_then(|source| {
            source
                .split("pub(super) fn sync_session_navigator_sessions")
                .next()
        })
        .expect("CLI-agent session event handler must exist");

    assert!(handler.contains("CLIAgentSessionsModelEvent::Started"));
    assert!(handler.contains("CLIAgentSessionsModelEvent::SessionUpdated"));
    assert!(handler.contains("ctx.dispatch_global_action(\"workspace:save_app\", ())"));
}

#[test]
fn test_session_navigator_initial_refresh_and_remote_share_logical_limit() {
    assert_eq!(
        crate::app_state::WORKSPACE_SESSION_NAVIGATOR_LOGICAL_LIMIT,
        80
    );

    let session_navigator = include_str!("session_navigator.rs");
    let workspace_view = include_str!("../view.rs");
    let environment_runtime = include_str!("../environment_runtime.rs");
    assert!(
        session_navigator.contains("crate::app_state::WORKSPACE_SESSION_NAVIGATOR_LOGICAL_LIMIT")
    );
    assert!(workspace_view
        .contains("indexed_cli_agent_sessions: Self::scan_terminal_cli_agent_sessions()"));
    assert!(environment_runtime
        .contains("crate::app_state::WORKSPACE_SESSION_NAVIGATOR_LOGICAL_LIMIT as u32"));
    assert!(!workspace_view.contains("scan_terminal_cli_agent_sessions(40)"));
    assert!(!session_navigator.contains("scan_terminal_cli_agent_sessions(80)"));
}

#[test]
fn test_live_pane_layout_shift_preserves_row_id_order_and_selection() {
    let mut first = make_live_session("tab:1:leaf:0", "local", 1_000);
    first.container_uuid = Some(vec![0xaa, 0xbb, 0xcc, 0xdd]);
    let initial = reduce(
        vec![first.clone()],
        SessionNavigatorState::new(),
        SessionNavigatorAction::Refresh {
            new_sessions: vec![first.clone()],
            pinned_session_ids: HashSet::new(),
        },
        &PaneGroupInfo::new(),
    );
    let row_id = row_id_for_session(&first, &initial.state);
    let order = initial.state.display_order[&row_id];
    let mut state = initial.state;
    state.selected_row_id = Some(row_id.clone());

    let mut shifted = first;
    shifted.id = "tab:0:leaf:0".to_owned();
    let refreshed = reduce(
        vec![shifted.clone()],
        state,
        SessionNavigatorAction::Refresh {
            new_sessions: vec![shifted.clone()],
            pinned_session_ids: HashSet::new(),
        },
        &PaneGroupInfo::new(),
    );

    assert_eq!(row_id_for_session(&shifted, &refreshed.state), row_id);
    assert_eq!(refreshed.state.display_order[&row_id], order);
    assert_eq!(
        refreshed.state.selected_row_id.as_deref(),
        Some(row_id.as_str())
    );
    assert!(refreshed.sessions[0].is_active);
}

#[test]
fn test_tab_coordinate_reuse_cannot_merge_distinct_container_uuid_rows() {
    let mut first = make_live_session("tab:0:leaf:0", "local", 1_000);
    first.container_uuid = Some(vec![0x01]);
    let initial = reduce(
        Vec::new(),
        SessionNavigatorState::new(),
        SessionNavigatorAction::Refresh {
            new_sessions: vec![first.clone()],
            pinned_session_ids: HashSet::new(),
        },
        &PaneGroupInfo::new(),
    );
    let first_row_id = row_id_for_session(&first, &initial.state);

    let mut second = make_live_session("tab:0:leaf:0", "local", 2_000);
    second.container_uuid = Some(vec![0x02]);
    let refreshed = reduce(
        initial.sessions,
        initial.state,
        SessionNavigatorAction::Refresh {
            new_sessions: vec![second.clone()],
            pinned_session_ids: HashSet::new(),
        },
        &PaneGroupInfo::new(),
    );
    let second_row_id = row_id_for_session(&second, &refreshed.state);

    assert_ne!(second_row_id, first_row_id);
    assert!(!refreshed
        .state
        .row_id_by_identity
        .contains_key("tab:0:leaf:0"));
    assert_eq!(
        refreshed.state.row_id_by_identity["local::pane:02"],
        second_row_id
    );
}
