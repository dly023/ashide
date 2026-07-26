use warp::{
    integration_testing::{
        self,
        command_palette::{open_command_palette_and_run_action, TestStepsExt},
        step::new_step_with_default_assertions,
        terminal::{
            execute_command_for_single_terminal_in_tab, util::ExpectedExitStatus,
            wait_until_bootstrapped_single_pane_for_tab,
        },
        view_of_type,
        window::save_active_window_id,
        workflow::{
            assert_no_workflow_pane_open, assert_open_workflow_pane_count_equals,
            assert_workflow_id, create_a_personal_workflow, open_workflow,
        },
    },
    workflows::CategoriesView,
};
use warpui::{async_assert_eq, integration::TestStep, ViewHandle};

use crate::Builder;

use super::{new_builder, TEST_ONLY_ASSETS};

pub fn test_open_workflow_in_pane() -> Builder {
    new_builder()
        .with_step(wait_until_bootstrapped_single_pane_for_tab(0))
        .with_step(
            create_a_personal_workflow("workflow_2_key")
                .add_assertion(save_active_window_id("first window")),
        )
        .with_step(
            open_workflow("first window", "workflow_2_key")
                .add_named_assertion_with_data_from_prior_step(
                    "Verify workflow is open",
                    assert_workflow_id(0, 0, "workflow_2_key"),
                ),
        )
}

pub fn test_create_personal_workflow_pane_from_command_palette() -> Builder {
    new_builder()
        .with_step(wait_until_bootstrapped_single_pane_for_tab(0))
        .with_step(TestStep::new("Noop step").add_named_assertion(
            "Make sure no workflow panes are open",
            assert_no_workflow_pane_open(),
        ))
        .with_steps(
            open_command_palette_and_run_action("Create a New Personal Workflow")
                .add_named_assertion(
                    "There should be one workflow pane open",
                    assert_open_workflow_pane_count_equals(1),
                ),
        )
}

/// 在 Git 仓库的项目配置目录中写入包含两个 workflow 的文件，
/// 并验证它们出现在 workflow 菜单中。
pub fn test_loading_project_workflows() -> Builder {
    new_builder()
        .with_step(wait_until_bootstrapped_single_pane_for_tab(0))
        .with_step(
            TestStep::new("Should have no project workflows").add_named_assertion(
                "Should have no project workflows",
                |app, window_id| {
                    let workflows: ViewHandle<CategoriesView> = view_of_type(app, window_id, 0);

                    workflows.read(app, |workflows, _| {
                        // 初始列表在浏览器打开时已经完成加载，因此这里可以同步断言。
                        async_assert_eq!(
                            workflows.project_workflows().count(),
                            0,
                            "There should not be any project workflows"
                        )
                    })
                },
            ),
        )
        // Create a git repository in the `repo` subdirectory.
        .with_step(execute_command_for_single_terminal_in_tab(
            0,
            "git init repo && cd repo".into(),
            ExpectedExitStatus::Success,
            (),
        ))
        .with_step(
            TestStep::new("Write a new file containing two workflows").with_setup(|utils| {
                integration_testing::create_file_from_assets(
                    TEST_ONLY_ASSETS,
                    "test_workflow.yaml",
                    &utils
                        .test_dir()
                        .join("repo")
                        .join(warp_core::paths::WARP_CONFIG_DIR)
                        .join("workflows/test_workflow.yaml"),
                );
            }),
        )
        .with_step(
            new_step_with_default_assertions(
                "Open the workflows browser to refresh the list of project workflows",
            )
            .with_keystrokes(&["ctrl-shift-R"]),
        )
        .with_step(
            TestStep::new("Verify the workflows were loaded successfully").add_named_assertion(
                "The two added workflows should be in the view",
                |app, window_id| {
                    let workflows: ViewHandle<CategoriesView> = view_of_type(app, window_id, 0);

                    let num_workflows =
                        workflows.read(app, |workflows, _| workflows.project_workflows().count());
                    async_assert_eq!(num_workflows, 2)
                },
            ),
        )
}
