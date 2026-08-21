//! Append-stable left Environment rail layout（ENV-SESSION-FIRST-RAIL-56）。
//!
//! 纯布局与 section 投影；渲染见 `vertical_tabs` / `environment_rail_view`。

use crate::app_state::{EnvironmentKind, EnvironmentLifecycleState, EnvironmentSnapshot};
use crate::environment_authority::ParsedEnvironmentAuthority;
use crate::workspace::environment_backend::EnvironmentSessionRefreshAvailability;
use pathfinder_color::ColorU;

use super::session_sidebar_metrics::session_sidebar_surface_metrics;

/// 紧凑会话行高 — 与 session_sidebar_surface_metrics 同步。
pub(crate) const RAIL_SESSION_ROW_HEIGHT: f32 = 36.0;

/// Environment identity header 块高度（search 以下、会话 viewport 以上）。
pub(crate) const RAIL_ENV_HEADER_HEIGHT: f32 = 36.0;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RailSessionLayoutPlan {
    pub viewport_heights_px: Vec<f32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EnvironmentSessionCounts {
    pub live: usize,
    pub total: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EnvironmentRailIssueKind {
    HelperUpdateRequired,
    ConnectionFailed,
    DiscoveryFailed,
    DiscoverySourceMissing,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EnvironmentRailIssue {
    pub kind: EnvironmentRailIssueKind,
    pub label: String,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EnvironmentRailSection {
    /// 逻辑 Environment 分区键（`navigation_key`）。
    pub navigation_key: String,
    pub authority_key: String,
    pub label: String,
    pub is_current: bool,
    pub collapsed: bool,
    /// 同行 trailing meta，例如 `"31"` 或 `"2·31"`。
    pub trailing: String,
    pub lifecycle_state: EnvironmentLifecycleState,
    pub refresh_availability: EnvironmentSessionRefreshAvailability,
    pub supports_disconnect: bool,
    pub supports_connect: bool,
    /// Persistent, actionable status owned by this Environment row.
    pub issue: Option<EnvironmentRailIssue>,
}

pub(crate) fn rail_session_viewport_slots(
    before: &[EnvironmentRailSection],
    current: &Option<EnvironmentRailSection>,
    after: &[EnvironmentRailSection],
    current_row_count: usize,
    preview_row_count: impl Fn(&str) -> usize,
) -> Vec<usize> {
    let mut slots = Vec::new();
    for section in before.iter().chain(current.as_ref()).chain(after.iter()) {
        if section.collapsed {
            continue;
        }
        if section.is_current {
            if current_row_count > 0 {
                slots.push(current_row_count);
            }
        } else {
            let rows = preview_row_count(&section.navigation_key);
            if rows > 0 {
                slots.push(rows);
            }
        }
    }
    slots
}

pub(crate) fn rail_available_session_area_px(
    window_height: f32,
    search_block_height: f32,
    visible_env_headers: usize,
) -> f32 {
    let chrome = search_block_height + 8.0;
    let headers = visible_env_headers as f32 * RAIL_ENV_HEADER_HEIGHT;
    (window_height - chrome - headers).max(RAIL_SESSION_ROW_HEIGHT)
}

pub(crate) fn rail_session_layout_plan(
    slot_row_counts: &[usize],
    rail_height_px: f32,
    single_section_cap_px: f32,
) -> RailSessionLayoutPlan {
    let expanded_nonempty_sections = slot_row_counts.len();
    let viewport_cap = if expanded_nonempty_sections > 1 {
        rail_height_px / 2.0
    } else {
        single_section_cap_px
    }
    .max(RAIL_SESSION_ROW_HEIGHT);

    RailSessionLayoutPlan {
        viewport_heights_px: slot_row_counts
            .iter()
            .map(|row_count| rail_session_natural_height_px(*row_count).min(viewport_cap))
            .collect(),
    }
}

/// 一个 Environment session viewport 的自然内容高度。
///
/// 横向 inset 由 row surface 消费；这里仅计算行高、行间距和首尾纵向留白。
/// 0 条与 collapsed section 都没有 viewport。
pub(crate) fn rail_session_natural_height_px(row_count: usize) -> f32 {
    if row_count == 0 {
        return 0.0;
    }
    let metrics = session_sidebar_surface_metrics();
    row_count as f32 * RAIL_SESSION_ROW_HEIGHT
        + row_count.saturating_sub(1) as f32 * metrics.unit_gap
        + metrics.unit_outer_pad_y * 2.0
}

pub(crate) fn rail_search_block_height() -> f32 {
    let metrics = session_sidebar_surface_metrics();
    metrics.search_height + 16.0
}

pub(crate) fn rail_trailing_count(counts: EnvironmentSessionCounts) -> String {
    if counts.total == 0 {
        return String::new();
    }
    if counts.live == 0 {
        return counts.total.to_string();
    }
    format!("{}·{}", counts.live, counts.total)
}

pub(crate) fn section_viewport_layout<'a>(
    section: &EnvironmentRailSection,
    preview_or_current_rows: usize,
    viewport_heights: &mut std::slice::Iter<'a, f32>,
) -> Option<f32> {
    if section.collapsed || preview_or_current_rows == 0 {
        return None;
    }
    viewport_heights.next().copied()
}

/// 从已打开 Environment 列表构建左栏 section；Local 始终排第一。
pub(crate) fn build_environment_rail_sections(
    open_environments: &[EnvironmentSnapshot],
    active_navigation_key: &str,
    collapsed: impl Fn(&str) -> bool,
    runtime_refresh_availability: impl Fn(&str) -> EnvironmentSessionRefreshAvailability,
    operational_issue: impl Fn(&str) -> Option<EnvironmentRailIssue>,
    session_counts: impl Fn(&str, bool) -> EnvironmentSessionCounts,
) -> Vec<EnvironmentRailSection> {
    let mut sections = Vec::new();
    let mut seen = std::collections::HashSet::new();

    let push_section = |sections: &mut Vec<EnvironmentRailSection>,
                        seen: &mut std::collections::HashSet<String>,
                        snapshot: &EnvironmentSnapshot,
                        is_current: bool| {
        let navigation_key = ParsedEnvironmentAuthority::parse(&snapshot.authority_key)
            .navigation_key()
            .to_owned();
        if !seen.insert(navigation_key.clone()) {
            return;
        }
        let counts = session_counts(&navigation_key, is_current);
        let display =
            crate::workspace::environment_runtime::environment_display_info_for_environment(
                snapshot,
            );
        let refresh_availability = if ParsedEnvironmentAuthority::parse(&snapshot.authority_key)
            .uses_terminal_bootstrap()
        {
            EnvironmentSessionRefreshAvailability::Ready
        } else if matches!(
            snapshot.lifecycle_state,
            EnvironmentLifecycleState::Connected
        ) {
            runtime_refresh_availability(&snapshot.authority_key)
        } else {
            EnvironmentSessionRefreshAvailability::Unavailable
        };
        sections.push(EnvironmentRailSection {
            navigation_key: navigation_key.clone(),
            authority_key: snapshot.authority_key.clone(),
            label: display.chip_label.unwrap_or_else(|| snapshot.label.clone()),
            is_current,
            collapsed: collapsed(&navigation_key),
            trailing: rail_trailing_count(counts),
            lifecycle_state: snapshot.lifecycle_state.clone(),
            refresh_availability,
            supports_disconnect: display.supports_disconnect,
            supports_connect: display.supports_connect,
            issue: operational_issue(&snapshot.authority_key),
        });
    };

    if let Some(local) = open_environments
        .iter()
        .find(|env| matches!(env.kind, EnvironmentKind::Local))
    {
        let is_current = ParsedEnvironmentAuthority::parse(&local.authority_key).navigation_key()
            == active_navigation_key;
        push_section(&mut sections, &mut seen, local, is_current);
    }

    for snapshot in open_environments {
        if matches!(snapshot.kind, EnvironmentKind::Local) {
            continue;
        }
        let is_current = ParsedEnvironmentAuthority::parse(&snapshot.authority_key)
            .navigation_key()
            == active_navigation_key;
        push_section(&mut sections, &mut seen, snapshot, is_current);
    }

    let has_active = sections
        .iter()
        .any(|section| section.navigation_key == active_navigation_key);
    if !has_active {
        if let Some(snapshot) = open_environments.iter().find(|env| {
            ParsedEnvironmentAuthority::parse(&env.authority_key).navigation_key()
                == active_navigation_key
        }) {
            push_section(&mut sections, &mut seen, snapshot, true);
        }
    }

    sections
}

pub(crate) fn split_rail_sections(
    sections: Vec<EnvironmentRailSection>,
) -> (
    Vec<EnvironmentRailSection>,
    Option<EnvironmentRailSection>,
    Vec<EnvironmentRailSection>,
) {
    let current_index = sections.iter().position(|section| section.is_current);
    match current_index {
        Some(index) => {
            let mut before = sections;
            let after = before.split_off(index + 1);
            let current_section = before.pop();
            (before, current_section, after)
        }
        None => (sections, None, Vec::new()),
    }
}

pub(crate) fn lifecycle_dot_color(state: EnvironmentLifecycleState) -> ColorU {
    match state {
        EnvironmentLifecycleState::Connected => ColorU::new(34, 197, 94, 255),
        EnvironmentLifecycleState::Connecting | EnvironmentLifecycleState::Installing => {
            ColorU::new(234, 179, 8, 255)
        }
        EnvironmentLifecycleState::Dormant | EnvironmentLifecycleState::Error => {
            ColorU::new(239, 68, 68, 255)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_state::{EnvironmentKind, EnvironmentSnapshot};

    fn sample_local() -> EnvironmentSnapshot {
        EnvironmentSnapshot::local(Some("/Users/dev".into()))
    }

    fn sample_remote() -> EnvironmentSnapshot {
        EnvironmentSnapshot {
            label: "Build".into(),
            authority_key: "remote:ssh://dev@build.example".into(),
            kind: EnvironmentKind::Ssh,
            connection_ref: None,
            active_workspace_root: None,
            lifecycle_state: EnvironmentLifecycleState::Connected,
        }
    }

    #[test]
    fn rail_session_layout_plan_uses_natural_height_and_only_caps_competing_sections() {
        let one_row = rail_session_natural_height_px(1);
        let two_rows = rail_session_natural_height_px(2);
        let many_rows = rail_session_natural_height_px(40);

        let single_small = rail_session_layout_plan(&[2], 720.0, 640.0);
        assert_eq!(single_small.viewport_heights_px, vec![two_rows]);

        let single_large = rail_session_layout_plan(&[40], 720.0, 640.0);
        assert_eq!(single_large.viewport_heights_px, vec![640.0]);

        let competing = rail_session_layout_plan(&[1, 2, 40], 720.0, 600.0);
        assert_eq!(
            competing.viewport_heights_px,
            vec![one_row, two_rows, many_rows.min(360.0)]
        );

        let both_large = rail_session_layout_plan(&[40, 40], 720.0, 600.0);
        assert_eq!(both_large.viewport_heights_px, vec![360.0, 360.0]);
    }

    #[test]
    fn rail_session_viewport_slots_skip_collapsed() {
        let current = EnvironmentRailSection {
            navigation_key: "local:/".into(),
            authority_key: "local:/".into(),
            label: "Local".into(),
            is_current: true,
            collapsed: false,
            trailing: String::new(),
            lifecycle_state: EnvironmentLifecycleState::Connected,
            refresh_availability: EnvironmentSessionRefreshAvailability::Ready,
            supports_disconnect: false,
            supports_connect: false,
            issue: None,
        };
        let remote = EnvironmentRailSection {
            navigation_key: "remote:build".into(),
            authority_key: "remote:ssh://dev@build".into(),
            label: "Build".into(),
            is_current: false,
            collapsed: true,
            trailing: String::new(),
            lifecycle_state: EnvironmentLifecycleState::Connected,
            refresh_availability: EnvironmentSessionRefreshAvailability::Ready,
            supports_disconnect: true,
            supports_connect: false,
            issue: None,
        };
        let slots = rail_session_viewport_slots(&[], &Some(current), &[remote], 5, |_| 3);
        assert_eq!(slots, vec![5]);
    }

    #[test]
    fn rail_session_viewport_slots_current_and_preview() {
        let current = EnvironmentRailSection {
            navigation_key: "local:/".into(),
            authority_key: "local:/".into(),
            label: "Local".into(),
            is_current: true,
            collapsed: false,
            trailing: String::new(),
            lifecycle_state: EnvironmentLifecycleState::Connected,
            refresh_availability: EnvironmentSessionRefreshAvailability::Ready,
            supports_disconnect: false,
            supports_connect: false,
            issue: None,
        };
        let remote = EnvironmentRailSection {
            navigation_key: "remote:build".into(),
            authority_key: "remote:ssh://dev@build".into(),
            label: "Build".into(),
            is_current: false,
            collapsed: false,
            trailing: String::new(),
            lifecycle_state: EnvironmentLifecycleState::Connected,
            refresh_availability: EnvironmentSessionRefreshAvailability::Ready,
            supports_disconnect: true,
            supports_connect: false,
            issue: None,
        };
        let slots = rail_session_viewport_slots(&[remote], &Some(current), &[], 5, |_| 3);
        assert_eq!(slots, vec![3, 5]);
    }

    #[test]
    fn rail_session_viewport_slots_treat_zero_rows_like_collapsed() {
        let current = EnvironmentRailSection {
            navigation_key: "local:/".into(),
            authority_key: "local:/".into(),
            label: "Local".into(),
            is_current: true,
            collapsed: false,
            trailing: String::new(),
            lifecycle_state: EnvironmentLifecycleState::Connected,
            refresh_availability: EnvironmentSessionRefreshAvailability::Ready,
            supports_disconnect: false,
            supports_connect: false,
            issue: None,
        };
        let slots = rail_session_viewport_slots(&[], &Some(current), &[], 0, |_| 0);
        assert!(slots.is_empty());
    }

    #[test]
    fn build_sections_local_first() {
        let open = vec![sample_remote(), sample_local()];
        let sections = build_environment_rail_sections(
            &open,
            "local:/Users/dev",
            |_| false,
            |_| EnvironmentSessionRefreshAvailability::Ready,
            |_| None,
            |_, _| EnvironmentSessionCounts { live: 0, total: 0 },
        );
        assert_eq!(sections.len(), 2);
        assert!(
            ParsedEnvironmentAuthority::parse(&sections[0].authority_key).uses_terminal_bootstrap()
        );
        assert!(
            ParsedEnvironmentAuthority::parse(&sections[1].authority_key)
                .uses_runtime_environment()
        );
    }

    #[test]
    fn build_sections_projects_connect_for_dormant_and_error_runtime() {
        for lifecycle_state in [
            EnvironmentLifecycleState::Dormant,
            EnvironmentLifecycleState::Error,
        ] {
            let mut remote = sample_remote();
            remote.lifecycle_state = lifecycle_state;
            let active_key = ParsedEnvironmentAuthority::parse(&remote.authority_key)
                .navigation_key()
                .to_owned();
            let sections = build_environment_rail_sections(
                &[sample_local(), remote],
                &active_key,
                |_| false,
                |_| EnvironmentSessionRefreshAvailability::Ready,
                |_| None,
                |_, _| EnvironmentSessionCounts { live: 0, total: 0 },
            );

            assert!(!sections[0].supports_disconnect);
            assert!(!sections[0].supports_connect);
            assert_eq!(
                sections[0].refresh_availability,
                EnvironmentSessionRefreshAvailability::Ready
            );
            assert!(sections[1].supports_disconnect);
            assert!(sections[1].supports_connect);
            assert_eq!(
                sections[1].refresh_availability,
                EnvironmentSessionRefreshAvailability::Unavailable
            );
        }
    }

    #[test]
    fn build_sections_keeps_refresh_and_connect_transport_semantics_disjoint() {
        for lifecycle_state in [
            EnvironmentLifecycleState::Connecting,
            EnvironmentLifecycleState::Installing,
        ] {
            let mut remote = sample_remote();
            remote.lifecycle_state = lifecycle_state;
            let sections = build_environment_rail_sections(
                &[sample_local(), remote],
                "local",
                |_| false,
                |_| EnvironmentSessionRefreshAvailability::Ready,
                |_| None,
                |_, _| EnvironmentSessionCounts { live: 0, total: 0 },
            );

            assert_eq!(
                sections[1].refresh_availability,
                EnvironmentSessionRefreshAvailability::Unavailable
            );
            assert!(!sections[1].supports_connect);
            assert!(sections[1].supports_disconnect);
        }
    }

    #[test]
    fn build_sections_keeps_connected_runtime_refresh_ready_without_terminal() {
        let remote = sample_remote();
        let ready = build_environment_rail_sections(
            &[sample_local(), remote.clone()],
            "local",
            |_| false,
            |_| EnvironmentSessionRefreshAvailability::Ready,
            |_| None,
            |_, _| EnvironmentSessionCounts { live: 0, total: 0 },
        );
        assert_eq!(
            ready[1].refresh_availability,
            EnvironmentSessionRefreshAvailability::Ready
        );
        assert!(!ready[1].supports_connect);
        assert!(ready[1].supports_disconnect);

        let unavailable = build_environment_rail_sections(
            &[sample_local(), remote],
            "local",
            |_| false,
            |_| EnvironmentSessionRefreshAvailability::Unavailable,
            |_| None,
            |_, _| EnvironmentSessionCounts { live: 0, total: 0 },
        );
        assert_eq!(
            unavailable[1].refresh_availability,
            EnvironmentSessionRefreshAvailability::Unavailable
        );
    }
}
