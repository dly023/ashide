use super::{
    project_explorer_header_actions, LeftPanelAction, ProjectExplorerHeaderLocation, ToolPanelView,
};

fn header_action_ids(actions: &[super::ProjectExplorerHeaderAction]) -> Vec<&'static str> {
    actions
        .iter()
        .map(|action| match action.action {
            LeftPanelAction::ProjectExplorerRefresh => "refresh",
            LeftPanelAction::ProjectExplorerUploadFiles => "upload-files",
            LeftPanelAction::ProjectExplorerUploadFolder => "upload-folder",
            LeftPanelAction::ToggleHiddenFiles => "hidden-files",
            LeftPanelAction::ProjectExplorer
            | LeftPanelAction::GlobalSearch { .. }
            | LeftPanelAction::LocalDrive
            | LeftPanelAction::EnvironmentProviderManager
            | LeftPanelAction::EnvironmentProjectExplorer
            | LeftPanelAction::ServerFileBrowser
            | LeftPanelAction::SkillManager
            | LeftPanelAction::OpenProjectExplorerOverflow => {
                panic!("header projection contains a non-header action")
            }
        })
        .collect()
}

#[test]
fn project_explorer_header_projection_has_exact_accessible_labels_and_tooltips() {
    let actions = project_explorer_header_actions(
        ToolPanelView::EnvironmentProjectExplorer,
        true,
        false,
        400.0,
    );

    assert_eq!(
        header_action_ids(&actions),
        vec!["refresh", "upload-files", "upload-folder", "hidden-files"]
    );
    assert!(actions
        .iter()
        .all(|action| !action.accessible_label.is_empty() && !action.tooltip.is_empty()));
    assert!(matches!(
        actions[0].action,
        LeftPanelAction::ProjectExplorerRefresh
    ));
    assert!(matches!(
        actions[1].action,
        LeftPanelAction::ProjectExplorerUploadFiles
    ));
    assert!(matches!(
        actions[2].action,
        LeftPanelAction::ProjectExplorerUploadFolder
    ));
    assert!(matches!(
        actions[3].action,
        LeftPanelAction::ToggleHiddenFiles
    ));
}

#[test]
fn project_explorer_header_projection_models_local_and_runtime_capabilities() {
    let local =
        project_explorer_header_actions(ToolPanelView::ProjectExplorer, false, false, 400.0);
    assert_eq!(header_action_ids(&local), vec!["hidden-files"]);

    let runtime_without_transfer = project_explorer_header_actions(
        ToolPanelView::EnvironmentProjectExplorer,
        false,
        false,
        400.0,
    );
    assert_eq!(
        header_action_ids(&runtime_without_transfer),
        vec!["refresh", "hidden-files"]
    );

    let runtime_with_transfer = project_explorer_header_actions(
        ToolPanelView::EnvironmentProjectExplorer,
        true,
        false,
        400.0,
    );
    assert!(header_action_ids(&runtime_with_transfer).contains(&"upload-files"));
    assert!(header_action_ids(&runtime_with_transfer).contains(&"upload-folder"));
    assert!(header_action_ids(&runtime_with_transfer).contains(&"refresh"));
    assert!(!header_action_ids(&local).contains(&"refresh"));
    assert!(!header_action_ids(&local).contains(&"upload-files"));
    assert!(!header_action_ids(&local).contains(&"upload-folder"));
}

#[test]
fn project_explorer_header_projection_preserves_every_action_in_narrow_overflow() {
    let wide = project_explorer_header_actions(
        ToolPanelView::EnvironmentProjectExplorer,
        true,
        false,
        400.0,
    );
    let narrow = project_explorer_header_actions(
        ToolPanelView::EnvironmentProjectExplorer,
        true,
        false,
        220.0,
    );

    assert_eq!(header_action_ids(&wide), header_action_ids(&narrow));
    assert_eq!(
        narrow
            .iter()
            .map(|action| action.location)
            .collect::<Vec<_>>(),
        vec![
            ProjectExplorerHeaderLocation::Overflow,
            ProjectExplorerHeaderLocation::Overflow,
            ProjectExplorerHeaderLocation::Overflow,
            ProjectExplorerHeaderLocation::Inline,
        ]
    );
    assert!(narrow
        .iter()
        .all(|action| !action.accessible_label.is_empty() && !action.tooltip.is_empty()));
}

#[test]
fn project_explorer_hidden_action_tracks_dynamic_label_and_selected_state() {
    let shown = project_explorer_header_actions(ToolPanelView::ProjectExplorer, false, true, 400.0);
    let hidden = shown
        .iter()
        .find(|action| matches!(action.action, LeftPanelAction::ToggleHiddenFiles))
        .unwrap();
    assert!(hidden.selected);
    assert!(matches!(hidden.action, LeftPanelAction::ToggleHiddenFiles));
    assert!(!hidden.accessible_label.is_empty());

    let concealed =
        project_explorer_header_actions(ToolPanelView::ProjectExplorer, false, false, 400.0);
    let concealed = concealed
        .iter()
        .find(|action| matches!(action.action, LeftPanelAction::ToggleHiddenFiles))
        .unwrap();
    assert!(!concealed.selected);
    assert_ne!(hidden.accessible_label, concealed.accessible_label);
}
