//! Session Navigator / Environment rail 共享密度常量。
//!
//! 紧凑但大气：单行 36px、图标 22px、行间略有呼吸；搜索条保持扁平无描边。

use warpui::elements::Padding;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SessionSidebarSurfaceMetrics {
    pub(crate) search_height: f32,
    pub(crate) row_min_height: f32,
    pub(crate) unit_gap: f32,
    pub(crate) unit_outer_pad_y: f32,
    pub(crate) content_pad_left: f32,
    pub(crate) content_pad_right: f32,
    pub(crate) row_pad_y: f32,
    pub(crate) icon_size: f32,
    pub(crate) icon_radius: f32,
    pub(crate) icon_text_gap: f32,
    pub(crate) text_fade_width: f32,
    pub(crate) title_font_size: f32,
}

pub(crate) fn session_sidebar_surface_metrics() -> SessionSidebarSurfaceMetrics {
    SessionSidebarSurfaceMetrics {
        search_height: 30.0,
        row_min_height: 36.0,
        unit_gap: 3.0,
        unit_outer_pad_y: 4.0,
        content_pad_left: 4.0,
        content_pad_right: 4.0,
        row_pad_y: 6.0,
        icon_size: 20.0,
        icon_radius: 5.0,
        icon_text_gap: 8.0,
        text_fade_width: 16.0,
        title_font_size: 13.0,
    }
}

/// live reorder unit 与 inactive preview 共用的可见边距。
///
/// List 的 unit 边界只是虚拟化/拖拽边界，不得泄漏成额外视觉间距：首尾各保留
/// `unit_outer_pad_y`，任意相邻 unit 之间只保留一个 `unit_gap`。横向 inset
/// 由具体 surface 统一消费 `content_pad_left/right`，这里不得重复叠加。
pub(crate) fn session_sidebar_unit_padding(is_first: bool, is_last: bool) -> Padding {
    let metrics = session_sidebar_surface_metrics();
    Padding::uniform(0.)
        .with_top(if is_first {
            metrics.unit_outer_pad_y
        } else {
            0.
        })
        .with_bottom(if is_last {
            metrics.unit_outer_pad_y
        } else {
            metrics.unit_gap
        })
}

#[cfg(test)]
#[path = "session_sidebar_metrics_tests.rs"]
mod tests;
