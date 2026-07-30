//! Session Navigator 的真实应用事件循环交互测试。
//!
//! 这些测试只操作隔离 HOME 下的 fixture。它们验证 WarpUI 的实际 click /
//! key routing，不依赖 macOS Accessibility，也绝不执行第三方 agent 的 resume。

use std::fs;
use std::path::Path;

use warp::{
    integration_testing::{
        command_palette::{assert_command_palette_has_results, assert_command_palette_is_closed},
        navigation_palette::open_navigation_palette_step,
        terminal::wait_until_bootstrapped_single_pane_for_tab,
        view_getters::command_palette_view,
    },
    workspace::{Workspace, WorkspaceAction},
};
use warpui::{async_assert, async_assert_eq, integration::TestStep, ViewHandle};

use super::{new_builder, Builder};

const ENVIRONMENT_PROVIDER_PICKER_POSITION_ID: &str =
    "workspace:environment_provider_picker_button";
const SESSION_NAVIGATOR_SEARCH_INPUT_POSITION_ID: &str = "workspace:session_navigator_search_input";
const OMP_FIXTURE_ID: &str = "019f0a0b-1111-4222-8333-444444444444";
const OMP_FIXTURE_TITLE: &str = "Integration Omp Navigator Fixture";
const OMP_UNRELATED_FIXTURE_IDS: [&str; 2] = [
    "019f0a0b-1111-4222-8333-555555555555",
    "019f0a0b-1111-4222-8333-666666666666",
];
const OMP_UNRELATED_FIXTURE_TITLES: [&str; 2] = [
    "Integration Omp Unrelated Fixture One",
    "Integration Omp Unrelated Fixture Two",
];

fn workspace_view(app: &mut warpui::App, window_id: warpui::WindowId) -> ViewHandle<Workspace> {
    app.views_of_type::<Workspace>(window_id)
        .and_then(|views| views.first().cloned())
        .expect("workspace view must exist")
}

fn write_isolated_omp_fixture(home: &Path) {
    let project = home.join("navigator-project");
    fs::create_dir_all(&project).expect("must create isolated Omp project");
    let sessions_dir = home.join(".omp/agent/sessions/-ashide");
    fs::create_dir_all(&sessions_dir).expect("must create isolated Omp session directory");
    for (index, (id, title)) in std::iter::once((OMP_FIXTURE_ID, OMP_FIXTURE_TITLE))
        .chain(
            OMP_UNRELATED_FIXTURE_IDS
                .iter()
                .copied()
                .zip(OMP_UNRELATED_FIXTURE_TITLES.iter().copied()),
        )
        .enumerate()
    {
        fs::write(
            sessions_dir.join(format!("178489700000{index}_{id}.jsonl")),
            format!(
                "{}\n{}\n",
                serde_json::json!({"type": "title", "title": title}),
                serde_json::json!({"type": "session", "id": id, "cwd": project}),
            ),
        )
        .expect("must write isolated read-only Omp discovery fixture");
    }
}

fn refresh_isolated_omp_fixture_step() -> TestStep {
    TestStep::new("Explicitly refresh the isolated Omp discovery fixture").with_action(
        |app, window_id, _| {
            let workspace = workspace_view(app, window_id);
            app.dispatch_typed_action(
                window_id,
                &[workspace.id()],
                &WorkspaceAction::RefreshWorkspaceSessions,
            );
        },
    )
}

fn wait_for_isolated_omp_fixture_step() -> TestStep {
    TestStep::new("Wait for committed Navigator projection to contain Omp fixture")
        .set_timeout(std::time::Duration::from_secs(30))
        .add_assertion(|app, window_id| {
            let workspace = workspace_view(app, window_id);
            workspace.read(app, |workspace, _| {
                async_assert!(
                    workspace.integration_session_navigator_contains_labels(&[
                        OMP_FIXTURE_TITLE,
                        OMP_UNRELATED_FIXTURE_TITLES[0],
                        OMP_UNRELATED_FIXTURE_TITLES[1],
                    ]),
                    "explicit refresh must preserve the target Omp row and two unrelated Omp rows"
                )
            })
        })
}

pub fn test_environment_provider_picker_event_loop_keyboard_lifecycle() -> Builder {
    new_builder()
        .with_setup(|utils| {
            let ssh_dir = utils.test_dir().join(".ssh");
            fs::create_dir_all(&ssh_dir).expect("must create isolated SSH config directory");
            fs::write(
                ssh_dir.join("config"),
                "Host integration-picker\n  HostName 127.0.0.1\n  Port 1\n  User integration\n",
            )
            .expect("must write isolated SSH config fixture");
        })
        .with_step(wait_until_bootstrapped_single_pane_for_tab(0))
        .with_step(
            TestStep::new("Open Environment provider picker through its painted entry")
                .with_click_on_saved_position(ENVIRONMENT_PROVIDER_PICKER_POSITION_ID)
                .add_assertion(|app, window_id| {
                    let workspace = workspace_view(app, window_id);
                    workspace.read(app, |workspace, _| {
                        let (is_open, query, selected) =
                            workspace.integration_environment_provider_picker_state();
                        async_assert_eq!(
                            (is_open, query, selected.as_deref()),
                            (true, String::new(), Some("config:integration-picker")),
                            "opening must use the committed catalog and clear stale transient state"
                        )
                    })
                }),
        )
        .with_step(
            TestStep::new("Filter picker then move selection with the actual keyboard route")
                .with_typed_characters(&["integration"])
                .with_keystrokes(&["down"])
                .add_assertion(|app, window_id| {
                    let workspace = workspace_view(app, window_id);
                    workspace.read(app, |workspace, _| {
                        let (is_open, query, selected) =
                            workspace.integration_environment_provider_picker_state();
                        async_assert_eq!(
                            (is_open, query, selected.as_deref()),
                            (
                                true,
                                "integration".to_owned(),
                                Some("config:integration-picker")
                            ),
                            "typed query and Down must stay within the committed picker catalog"
                        )
                    })
                }),
        )
        .with_step(
            TestStep::new("Escape closes picker and clears its transient state")
                .with_keystrokes(&["escape"])
                .add_assertion(|app, window_id| {
                    let workspace = workspace_view(app, window_id);
                    workspace.read(app, |workspace, _| {
                        let (is_open, query, selected) =
                            workspace.integration_environment_provider_picker_state();
                        async_assert!(
                            !is_open && query.is_empty() && selected.is_none(),
                            "Escape must close the picker and clear query/selection; actual={:?}",
                            (is_open, query, selected)
                        )
                    })
                }),
        )
}

pub fn test_session_navigator_search_keyboard_event_loop_lifecycle() -> Builder {
    new_builder()
        .with_setup(|utils| write_isolated_omp_fixture(&utils.test_dir()))
        .with_step(wait_until_bootstrapped_single_pane_for_tab(0))
        .with_step(refresh_isolated_omp_fixture_step())
        .with_step(wait_for_isolated_omp_fixture_step())
        .with_step(
            TestStep::new("Focus Session Navigator search through its painted input")
                .with_click_on_saved_position(SESSION_NAVIGATOR_SEARCH_INPUT_POSITION_ID)
                .with_typed_characters(&[OMP_FIXTURE_TITLE])
                .with_keystrokes(&["down"])
                .add_assertion(|app, window_id| {
                    let workspace = workspace_view(app, window_id);
                    workspace.read(app, |workspace, _| {
                        let (query, has_cursor) =
                            workspace.integration_session_navigator_search_state();
                        async_assert!(
                            query == OMP_FIXTURE_TITLE && has_cursor,
                            "Down must keep the target query and select its committed Omp row; actual={:?}",
                            (query, has_cursor)
                        )
                    })
                }),
        )
        .with_step(
            TestStep::new("Escape clears the Session Navigator cursor and query")
                .with_keystrokes(&["escape"])
                .add_assertion(|app, window_id| {
                    let workspace = workspace_view(app, window_id);
                    workspace.read(app, |workspace, _| {
                        let (query, has_cursor) =
                            workspace.integration_session_navigator_search_state();
                        async_assert!(
                            query.is_empty() && !has_cursor,
                            "Escape must clear the Session Navigator query and cursor"
                        )
                    })
                }),
        )
}

pub fn test_session_navigator_command_palette_event_loop_search() -> Builder {
    new_builder()
        .with_setup(|utils| write_isolated_omp_fixture(&utils.test_dir()))
        .with_step(wait_until_bootstrapped_single_pane_for_tab(0))
        .with_step(refresh_isolated_omp_fixture_step())
        .with_step(wait_for_isolated_omp_fixture_step())
        .with_step(
            open_navigation_palette_step()
                .with_typed_characters(&[OMP_FIXTURE_TITLE])
                .add_assertion(assert_command_palette_has_results())
                .add_assertion(|app, window_id| {
                    let palette = command_palette_view(app, window_id);
                    palette.read(app, |palette, ctx| {
                        async_assert!(
                            palette.selected_result_is_session_navigation(ctx),
                            "the searched Omp fixture must select the canonical Session Navigator action"
                        )
                    })
                }),
        )
        // 不按 Enter：本测试只验证 query-time projection/index 复用，隔离的 provider
        // fixture 也不得执行 `omp --resume`。
        .with_step(
            TestStep::new("Close searched command palette without activating Omp")
                .with_keystrokes(&["escape"])
                .add_assertion(assert_command_palette_is_closed()),
        )
}
