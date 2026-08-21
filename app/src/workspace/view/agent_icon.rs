//! Session Navigator 会话行图标 — 全局统一正方形圆角 plate。
//!
//! # 全局机制（禁止按 agent 特判）
//! 1. **外框**：正方形 glyph 槽 + uniform padding，沿用 IconWithStatus 的 plate 几何。
//! 2. **光学内缩**：glyph 槽 = `icon_size * (1 - 2 * PLATE_GLYPH_INSET_RATIO)`，
//!    任意满幅 SVG 也不会盖住圆角。
//! 3. **资源契约**：SVG 保留品牌图形自身比例，由 Icon 的 Contain fit 收进统一正方形 glyph 槽。

use pathfinder_color::ColorU;
use warp_core::ui::theme::color::internal_colors;
use warp_core::ui::theme::{Fill as WarpThemeFill, WarpTheme};
use warpui::elements::{ConstrainedBox, Container, CornerRadius, Element, Icon, Radius};

use crate::terminal::CLIAgent;

use super::session_sidebar_metrics::session_sidebar_surface_metrics;

const TERMINAL_ICON_PATH: &str = "bundled/svg/terminal.svg";

/// 每侧相对 plate 边长的 glyph 内缩比例。满幅框标 / 异形 viewBox 都留出圆角底板。
pub(crate) const PLATE_GLYPH_INSET_RATIO: f32 = 0.22;

fn rgb_u32_to_color_u(rgb: u32) -> ColorU {
    ColorU::new(
        ((rgb >> 16) & 0xFF) as u8,
        ((rgb >> 8) & 0xFF) as u8,
        (rgb & 0xFF) as u8,
        255,
    )
}

fn plate_glyph_slot(box_size: f32) -> f32 {
    box_size * (1.0 - 2.0 * PLATE_GLYPH_INSET_RATIO)
}

/// 全局唯一 plate 几何：正方形 glyph 槽 + 对称 padding 正方形底板。
fn rounded_rect_icon_plate(
    glyph: Box<dyn Element>,
    plate_fill: WarpThemeFill,
    box_size: f32,
    radius: f32,
) -> Box<dyn Element> {
    let glyph_size = plate_glyph_slot(box_size);
    let padding = (box_size - glyph_size) / 2.;
    Container::new(
        ConstrainedBox::new(glyph)
            .with_width(glyph_size)
            .with_height(glyph_size)
            .finish(),
    )
    .with_uniform_padding(padding)
    .with_background(plate_fill)
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(radius)))
    .finish()
}

/// Accent-plate agent badge（品牌底 + 对比色 currentColor SVG）。
pub(crate) fn agent_icon_badge(agent: CLIAgent, box_size: f32) -> Box<dyn Element> {
    let metrics = session_sidebar_surface_metrics();
    let accent = agent
        .accent_rgb()
        .map(rgb_u32_to_color_u)
        .unwrap_or(ColorU::new(64, 64, 64, 255));
    let glyph_color = rgb_u32_to_color_u(agent.glyph_rgb());
    let glyph = Icon::new(agent.icon_path(), glyph_color).finish();
    rounded_rect_icon_plate(
        glyph,
        WarpThemeFill::Solid(accent),
        box_size,
        metrics.icon_radius,
    )
}

/// Session Navigator 行图标唯一入口。
pub(crate) fn session_navigator_row_icon(
    agent: Option<CLIAgent>,
    theme: &WarpTheme,
) -> Box<dyn Element> {
    let metrics = session_sidebar_surface_metrics();
    match agent {
        Some(agent) if !matches!(agent, CLIAgent::Unknown) && agent.accent_rgb().is_some() => {
            agent_icon_badge(agent, metrics.icon_size)
        }
        Some(_) | None => {
            let glyph_color = theme.main_text_color(theme.background()).into_solid();
            let glyph = Icon::new(TERMINAL_ICON_PATH, glyph_color).finish();
            rounded_rect_icon_plate(
                glyph,
                internal_colors::fg_overlay_2(theme),
                metrics.icon_size,
                metrics.icon_radius,
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::PLATE_GLYPH_INSET_RATIO;

    #[test]
    fn plate_is_padding_square_with_global_inset() {
        let source = include_str!("agent_icon.rs");
        let prod = source.split("#[cfg(test)]").next().unwrap_or(source);
        assert!(
            prod.contains("Container::new(")
                && prod.contains("with_uniform_padding(padding)")
                && prod.contains("plate_glyph_slot")
                && prod.contains("PLATE_GLYPH_INSET_RATIO")
                && !prod.contains("Align::new")
                && !prod.contains("Stack::new")
                && !prod.contains("Rect::new"),
            "plate must use the shared padding-square geometry and global glyph inset"
        );
        assert!(
            (PLATE_GLYPH_INSET_RATIO - 0.22).abs() < f32::EPSILON,
            "inset ratio is part of the visual contract"
        );
    }

    #[test]
    fn terminal_and_agent_share_icon_element() {
        let source = include_str!("agent_icon.rs");
        let prod = source.split("#[cfg(test)]").next().unwrap_or(source);
        assert!(
            prod.contains("Icon::new(TERMINAL_ICON_PATH")
                && prod.contains("Icon::new(agent.icon_path()"),
            "terminal and agents must share Icon::new"
        );
    }
}
