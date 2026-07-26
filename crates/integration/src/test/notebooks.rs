use warp::{
    cmd_or_ctrl_shift,
    features::FeatureFlag,
    integration_testing::{
        notebook::{
            assert_notebook_contents, assert_notebook_id, assert_notebook_not_open,
            assert_notebook_open, assert_notebook_renders_mermaid_diagram,
            assert_open_in_warp_banner_open, create_a_personal_notebook,
            move_notebook_cursor_to_offset, open_notebook,
        },
        step::new_step_with_default_assertions,
        tab::{assert_pane_title, assert_tab_title},
        terminal::{
            assert_single_terminal_in_tab_bootstrapped, execute_command_for_single_terminal_in_tab,
            util::ExpectedExitStatus, wait_until_bootstrapped_single_pane_for_tab,
        },
        view_getters::{notebook_view, terminal_view, workspace_view},
        window::{add_and_save_window, close_window, save_active_window_id},
        workspace::{assert_focused_tab_index, assert_tab_count},
    },
    workspace::WorkspaceAction,
};
use warpui::integration::TestStep;

use super::{new_builder, Builder};

pub fn test_notebook_pane_tracking() -> Builder {
    new_builder()
        .with_step(
            create_a_personal_notebook("the notebook", "A test notebook")
                .add_assertion(save_active_window_id("first window")),
        )
        .with_step(
            open_notebook("first window", "the notebook")
                .add_named_assertion_with_data_from_prior_step(
                    "Verify notebook is open",
                    assert_notebook_id(0, 0, "the notebook"),
                ),
        )
        // Now, reopen the notebook (from both windows) and verify that it's only opened once.
        .with_step(
            open_notebook("first window", "the notebook")
                .add_named_assertion_with_data_from_prior_step(
                    "Verify notebook is open once",
                    assert_notebook_open("the notebook"),
                ),
        )
        .with_step(add_and_save_window("second window"))
        .with_step(
            open_notebook("second window", "the notebook")
                .add_named_assertion_with_data_from_prior_step(
                    "Verify notebook is open once",
                    assert_notebook_open("the notebook"),
                ),
        )
        // Close and then reopen the notebook.
        .with_step(
            TestStep::new("Close the open notebook")
                .with_keystrokes(&[cmd_or_ctrl_shift("w")])
                .add_named_assertion_with_data_from_prior_step(
                    "Verify notebook is closed",
                    assert_notebook_not_open("the notebook"),
                ),
        )
        .with_step(open_notebook("second window", "the notebook"))
        // This must be in a separate step so that the active window is updated.
        .with_step(
            TestStep::new("Verify notebook is open in second window")
                .add_named_assertion_with_data_from_prior_step(
                    "Verify notebook is open in the second window",
                    assert_notebook_id(0, 0, "the notebook"),
                ),
        )
}

/// This is a regression test for CLD-713.
pub fn test_close_notebook_tab() -> Builder {
    new_builder()
        .with_step(
            create_a_personal_notebook("the notebook", "Test Notebook")
                .add_assertion(save_active_window_id("the window")),
        )
        .with_step(
            open_notebook("the window", "the notebook")
                .add_named_assertion_with_data_from_prior_step(
                    "Verify notebook is open",
                    assert_notebook_id(0, 0, "the notebook"),
                ),
        )
        .with_step(
            TestStep::new("Open a tab with cmd-t")
                .with_keystrokes(&[cmd_or_ctrl_shift("t")])
                .add_assertion(|app, window_id| {
                    assert_single_terminal_in_tab_bootstrapped(app, window_id, 1)
                }),
        )
        // Give the terminal tab a stable identifier without coupling this notebook lifecycle test
        // to command-palette search or the inline rename editor.
        .with_step(
            TestStep::new("Name the second tab")
                .with_action(|app, window_id, _| {
                    let workspace_view_id = workspace_view(app, window_id).id();
                    app.dispatch_typed_action(
                        window_id,
                        &[workspace_view_id],
                        &WorkspaceAction::SetActiveTabName("tab2".to_owned()),
                    );
                })
                .add_assertion(assert_tab_title(1, "tab2"))
                .add_assertion(assert_focused_tab_index(1)),
        )
        // Refocus the first notebook pane.
        .with_step(
            open_notebook("the window", "the notebook")
                .add_assertion(assert_tab_title(0, "Test Notebook"))
                .add_assertion(assert_tab_title(1, "tab2"))
                .add_assertion(assert_focused_tab_index(0)),
        )
        // Close the notebook tab explicitly. Pane-close keyboard routing is a separate contract.
        .with_step(
            TestStep::new("Close the notebook tab")
                .with_action(|app, window_id, _| {
                    let workspace_view_id = workspace_view(app, window_id).id();
                    app.dispatch_typed_action(
                        window_id,
                        &[workspace_view_id],
                        &WorkspaceAction::CloseTab(0),
                    );
                })
                .add_assertion(assert_tab_count(1))
                .add_assertion(assert_tab_title(0, "tab2"))
                .add_assertion(assert_focused_tab_index(0)),
        )
        .with_step(
            open_notebook("the window", "the notebook")
                .add_named_assertion_with_data_from_prior_step(
                    "Verify notebook is open",
                    assert_notebook_id(0, 0, "the notebook"),
                ),
        )
}

pub fn test_close_notebook_window() -> Builder {
    new_builder()
        .with_step(
            create_a_personal_notebook("the notebook", "Test Notebook")
                .add_assertion(save_active_window_id("first window")),
        )
        .with_step(
            open_notebook("first window", "the notebook")
                .add_named_assertion_with_data_from_prior_step(
                    "Verify notebook is open",
                    assert_notebook_id(0, 0, "the notebook"),
                ),
        )
        .with_step(add_and_save_window("second window"))
        // Close the first window
        .with_step(
            close_window("first window", 1).add_named_assertion_with_data_from_prior_step(
                "Verify notebook is closed",
                assert_notebook_not_open("the notebook"),
            ),
        )
        // Reopen the notebook in the remaining window.
        .with_step(
            open_notebook("second window", "the notebook")
                .add_assertion(assert_tab_title(0, "Test Notebook")),
        )
}

pub fn test_open_in_warp_banner() -> Builder {
    new_builder()
        .with_setup(|utils| {
            std::fs::write(utils.test_dir().join("README.md"), "# Hello, world!")
                .expect("Couldn't create README.md");
        })
        .with_step(wait_until_bootstrapped_single_pane_for_tab(0))
        .with_step(
            execute_command_for_single_terminal_in_tab(
                0,
                "cat README.md".to_string(),
                ExpectedExitStatus::Success,
                (),
            )
            .add_assertion(assert_open_in_warp_banner_open(0, 0)),
        )
        .with_step(
            new_step_with_default_assertions("Click Open in Ashide banner")
                .with_click_on_saved_position_fn(|app, window_id| {
                    let view = terminal_view(app, window_id, 0, 0);
                    format!("open_in_warp_banner_button_{}", view.id())
                }),
        )
        .with_step(
            new_step_with_default_assertions("Wait for Markdown file to open")
                .add_assertion(assert_pane_title(0, 1, "README.md")),
        )
}

pub fn test_backspace_inside_rendered_mermaid_block_is_atomic() -> Builder {
    FeatureFlag::MarkdownMermaid.set_enabled(true);
    FeatureFlag::EditableMarkdownMermaid.set_enabled(true);

    let markdown = "Before\n```mermaid\ngraph TD\nA --> B\n```\nAfter";
    let mermaid_block_start = markdown
        .find("```mermaid")
        .expect("Mermaid block should exist");
    let cursor_offset = markdown
        .find("graph TD")
        .expect("Mermaid source should exist")
        + 3;

    new_builder()
        .with_step(
            create_a_personal_notebook("the notebook", "Mermaid Notebook")
                .add_assertion(save_active_window_id("the window")),
        )
        .with_step(
            open_notebook("the window", "the notebook")
                .add_named_assertion_with_data_from_prior_step(
                    "Verify notebook is open",
                    assert_notebook_id(0, 0, "the notebook"),
                ),
        )
        .with_step(
            TestStep::new("Enter notebook edit mode and set Mermaid Markdown")
                .with_action(move |app, window_id, _| {
                    let notebook = notebook_view(app, window_id, 0, 0);
                    let editor = notebook.read(app, |notebook, _| notebook.input_editor());
                    notebook.update(app, |notebook, ctx| {
                        notebook.grab_edit_access_or_display_access_dialog(ctx);
                    });
                    editor.update(app, |editor, ctx| {
                        editor.reset_with_markdown(markdown, ctx);
                        editor.focus(ctx);
                    });
                })
                .add_assertion(assert_notebook_contents(0, 0, markdown))
                .add_assertion(assert_notebook_renders_mermaid_diagram(
                    0,
                    0,
                    mermaid_block_start,
                )),
        )
        .with_step(move_notebook_cursor_to_offset(0, 0, cursor_offset))
        .with_step(
            TestStep::new("Backspace from inside rendered Mermaid")
                .with_keystrokes(&["backspace"])
                .add_assertion(assert_notebook_contents(0, 0, "Before\nAfter")),
        )
}
