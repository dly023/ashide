use super::WorkspaceAction;
use crate::pane_group::TerminalPaneId;
use crate::workspace::PaneViewLocator;
use warpui::EntityId;

#[test]
fn vertical_tabs_panel_toggle_still_saves_workspace_state() {
    assert!(WorkspaceAction::ToggleVerticalTabsPanel.should_save_app_state_on_action());
}

#[test]
fn toggle_skill_manager_saves_workspace_state() {
    assert!(WorkspaceAction::ToggleSkillManager.should_save_app_state_on_action());
}

#[test]
fn pane_name_actions_save_workspace_state() {
    let locator = PaneViewLocator {
        pane_group_id: EntityId::new(),
        pane_id: TerminalPaneId::dummy_terminal_pane_id().into(),
    };

    assert!(WorkspaceAction::RenamePane(locator).should_save_app_state_on_action());
    assert!(WorkspaceAction::ResetPaneName(locator).should_save_app_state_on_action());
}
