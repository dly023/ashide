use warpui::elements::Padding;

use super::{session_sidebar_surface_metrics, session_sidebar_unit_padding};

#[test]
fn session_sidebar_surfaces_share_horizontal_content_padding() {
    let metrics = session_sidebar_surface_metrics();

    assert_eq!(metrics.content_pad_left, 4.);
    assert_eq!(metrics.content_pad_right, 4.);
}

#[test]
fn live_units_and_flat_preview_have_identical_visible_spacing() {
    let first = session_sidebar_unit_padding(true, false);
    let middle = session_sidebar_unit_padding(false, false);
    let last = session_sidebar_unit_padding(false, true);
    let preview = session_sidebar_unit_padding(true, true);

    assert_eq!(first, Padding::uniform(0.).with_top(4.).with_bottom(3.));
    assert_eq!(middle, Padding::uniform(0.).with_bottom(3.));
    assert_eq!(last, Padding::uniform(0.).with_bottom(4.));
    assert_eq!(preview, Padding::uniform(0.).with_top(4.).with_bottom(4.));
}
