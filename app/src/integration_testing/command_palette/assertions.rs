use crate::integration_testing::view_getters::{command_palette_view, workspace_view};
use warpui::integration::AssertionCallback;
use warpui::keymap::{BindingDescription, DescriptionContext};
use warpui::{async_assert, async_assert_eq};

/// Asserts that the command palette is currently open.
pub fn assert_command_palette_is_open() -> AssertionCallback {
    Box::new(move |app, window_id| {
        let workspace = workspace_view(app, window_id);

        workspace.read(app, |workspace, _| {
            async_assert!(
                workspace.is_palette_open(),
                "Expected palette to be open, but it was closed"
            )
        })
    })
}

/// Asserts that the command palette is currently closed.
pub fn assert_command_palette_is_closed() -> AssertionCallback {
    Box::new(move |app, window_id| {
        let workspace = workspace_view(app, window_id);

        workspace.read(app, |workspace, _| {
            async_assert!(
                !workspace.is_palette_open(),
                "Expected palette to be closed, but it was open"
            )
        })
    })
}

/// Asserts that the command palette currently has at least one search result.
pub fn assert_command_palette_has_results() -> AssertionCallback {
    Box::new(move |app, window_id| {
        let palette = command_palette_view(app, window_id);

        palette.read(app, |palette, ctx| {
            async_assert!(
                palette.search_results(ctx).next().is_some(),
                "Expected command palette to have results, but it was empty"
            )
        })
    })
}

pub fn assert_command_palette_selected_binding(action: &str) -> AssertionCallback {
    let expected = BindingDescription::new(action)
        .in_context(DescriptionContext::Default)
        .to_owned();
    Box::new(move |app, window_id| {
        let palette = command_palette_view(app, window_id);

        palette.read(app, |palette, ctx| {
            let selected = palette.selected_binding_description(ctx);
            async_assert_eq!(
                selected.as_deref(),
                Some(expected.as_str()),
                "Expected the requested action binding to own the selected result before Enter"
            )
        })
    })
}
