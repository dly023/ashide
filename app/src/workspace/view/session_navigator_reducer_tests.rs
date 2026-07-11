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

fn make_session(
    id: &str,
    env: &str,
    is_live: bool,
    updated_at: i64,
) -> WorkspaceSessionSnapshot {
    let is_active = false;
    let is_pinned = false;
    let cli_agent_session_id = if !is_live {
        Some(format!("{id}-agent-session"))
    } else {
        None
    };
    let mut session = WorkspaceSessionSnapshot {
        id: id.to_string(),
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
    // 确保 live container 没有 cli_agent_session_id (模拟 from_tabs)
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
    tabs: Vec<(usize, usize, Option<PaneViewLocator>, Option<PaneViewLocator>)>,
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
    let key = logical_key(&result.sessions[0]);
    assert_eq!(
        result.state.display_order.get(&key),
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
    let s1_after = result.sessions.iter().find(|s| s.id == "tab:0:leaf:0").unwrap();
    let s2_after = result.sessions.iter().find(|s| s.id == "tab:1:leaf:0").unwrap();
    assert_eq!(s1_after.is_active, false, "pin should not change is_active");
    assert_eq!(s2_after.is_active, false, "pin should not change is_active");
    assert_eq!(result.side_effect, SideEffect::WriteUserState);
    assert_eq!(
        result.state.active_key,
        None,
        "pin should not change active_key"
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

    let pane_info = make_pane_info(vec![
        (0, 2, Some(make_locator(0, 0)), Some(make_locator(0, 1))),
    ]);

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

    let pane_info = make_pane_info(vec![
        (0, 2, Some(make_locator(0, 0)), Some(make_locator(0, 1))),
    ]);

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
    state.active_key = Some(logical_key(&s_virtual));

    let result = reduce(
        result.sessions,
        state,
        SessionNavigatorAction::Delete {
            session_logical_key: logical_key(&s0),
            session_id: s0.id.clone(),
            environment_authority_key: s0.environment_authority_key.clone(),
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
        result.state.active_key.as_deref(),
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
    assert_eq!(result.state.active_key, Some(live_key.clone()));

    let pane_info_after_delete = PaneGroupInfo::new();
    let result = reduce(
        result.sessions,
        result.state,
        SessionNavigatorAction::Delete {
            session_logical_key: live_key,
            session_id: s_live.id.clone(),
            environment_authority_key: s_live.environment_authority_key.clone(),
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
    assert_eq!(result.state.active_key, Some(virtual_key.clone()));
    assert!(result.state.restoring_keys.contains(&virtual_key));
    assert!(matches!(
        result.side_effect,
        SideEffect::SpawnTerminal { .. }
    ));

    let active_before_pin = result.state.active_key.clone();
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
        result.state.active_key, active_before_pin,
        "pin must not change active_key"
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
    let pane_info = make_pane_info(vec![
        (0, 2, Some(make_locator(0, 1)), Some(make_locator(0, 0))),
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
    assert_eq!(result.state.active_key.as_deref(), Some(key_s1.as_str()));
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
    state.active_key = Some(logical_key(&s0));

    // 删除 s0
    let key_s0 = logical_key(&s0);
    let result = reduce(
        vec![s0.clone(), s1.clone()],
        state,
        SessionNavigatorAction::Delete {
            session_logical_key: key_s0.clone(),
            session_id: s0.id.clone(),
            environment_authority_key: s0.environment_authority_key.clone(),
        },
        &pane_info,
    );

    // active_key 应该被清除 (不猜测转移到 s1)
    assert_eq!(
        result.state.active_key, None,
        "active_key should be cleared, not guessed transfer"
    );
    validate_state(&result.sessions, &result.state).unwrap();
}

// ─────────────────────────────────────────────────────────
// EC-11: virtual → live consume 时 active 迁移
// ─────────────────────────────────────────────────────────

#[test]
fn test_ec11_materialization_active_migration() {
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

    // 设 active_key 指向 virtual row
    let virtual_key = logical_key(&virtual_session);
    state.active_key = Some(virtual_key.clone());

    // 现在 virtual row 被 consume: live container 出现, durable key 匹配
    let mut live_session = make_live_session("tab:0:leaf:0", "local", 2000);
    // live session 需要有相同的 durable key 来触发迁移
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

    // active_key 应该从 virtual key 迁移到 live container 的 logical_key
    let live_key = logical_key(&result.sessions[0]);
    assert_ne!(
        live_key, virtual_key,
        "live logical_key should differ from virtual key"
    );
    assert_eq!(
        result.state.active_key,
        Some(live_key),
        "active_key should migrate from virtual to live"
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
    assert_eq!(state.active_key, Some(key_s1.clone()));
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
    assert_eq!(result.side_effect, SideEffect::WriteUserState);
    validate_state(&sessions, &state).unwrap();

    // 4. remove s1
    let result = reduce(
        sessions,
        state,
        SessionNavigatorAction::Delete {
            session_logical_key: key_s1.clone(),
            session_id: s1.id.clone(),
            environment_authority_key: s1.environment_authority_key.clone(),
        },
        &pane_info,
    );
    sessions = result.sessions;
    state = result.state;
    assert!(sessions.is_empty());
    assert_eq!(state.active_key, None);
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
    pane_info = make_pane_info(vec![
        (0, 2, Some(make_locator(0, 1)), Some(make_locator(0, 0))),
    ]);

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
    let mut state = SessionNavigatorState::new();
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

    assert_eq!(order1, order2, "sort should be stable across multiple sorts");
}

// ─────────────────────────────────────────────────────────
// tie-break: updated_at 为 None 排最后
// ─────────────────────────────────────────────────────────

#[test]
fn test_tie_break_none_updated_at_last() {
    let mut state = SessionNavigatorState::new();

    let mut s_with_time = make_virtual_session("agent-with-time", "local", 1000);
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
        assert!(
            active_count <= 1,
            "expected ≤ 1 active, got {active_count}"
        );
        validate_state(&result.sessions, &result.state).unwrap();
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
    assert!(result.is_err(), "validator should fail with 2 active sessions");
}

// ─────────────────────────────────────────────────────────
// validator: deleted session 在列表中 → 报错
// ─────────────────────────────────────────────────────────

#[test]
fn test_validator_deleted_session_in_list_fails() {
    let sessions = vec![make_live_session("tab:0:leaf:0", "local", 1000)];
    let mut state = SessionNavigatorState::new();
    let key = logical_key(&sessions[0]);
    state.deleting_keys.insert(key);

    let result = validate_state(&sessions, &state);
    assert!(result.is_err(), "validator should fail with deleted session in list");
}

// ─────────────────────────────────────────────────────────
// validator: stale active_key → 报错
// ─────────────────────────────────────────────────────────

#[test]
fn test_validator_stale_active_key_fails() {
    let sessions = vec![make_live_session("tab:0:leaf:0", "local", 1000)];
    let mut state = SessionNavigatorState::new();
    state.active_key = Some("nonexistent-key".to_string());

    let result = validate_state(&sessions, &state);
    assert!(
        result.is_err(),
        "validator should fail with stale active_key"
    );
}

// ─────────────────────────────────────────────────────────
// EC-12: restoring 中用户切走 → 保持 active highlight
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
    let result = reduce(
        result.sessions,
        result.state,
        SessionNavigatorAction::PaneFocused {
            locator: make_locator(0, 0),
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

    validate_state(&result.sessions, &result.state).unwrap();
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

    let virtual_key = logical_key(&virtual_session);
    let virtual_order = state.display_order.get(&virtual_key).copied();
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
    let live_key = logical_key(&result.sessions[0]);
    let live_order = result.state.display_order.get(&live_key).copied();
    assert_eq!(
        live_order,
        virtual_order,
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
            new_sessions: vec![
                s_a.clone(),
                s_b.clone(),
                s_c.clone(),
            ],
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
