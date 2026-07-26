use crate::workflows::categories::{CategoriesView, WorkflowMatchType};
use crate::workflows::workflow::Workflow;
use crate::workflows::WorkflowType;
use std::sync::Arc;

#[cfg(feature = "local_fs")]
use super::ProjectWorkflowLoadState;

#[test]
fn test_workflow_matches() {
    let workflow = Arc::new(WorkflowType::Local(Workflow::Command {
        name: "g workflow_name it ".into(),
        command: "command_name git".to_string(),
        tags: vec!["foo".into(), "bar".into()],
        description: None,
        arguments: vec![],
        source_url: None,
        author: None,
        author_url: None,
        shells: vec![],
        environment_variables: None,
    }));

    assert_eq!(
        CategoriesView::matches_workflow(&workflow, "foo"),
        WorkflowMatchType::Tag
    );
    assert_eq!(
        CategoriesView::matches_workflow(&workflow, "bar"),
        WorkflowMatchType::Tag
    );

    // The Workflow name has higher precedence than the command.
    assert!(matches!(
        CategoriesView::matches_workflow(&workflow, "name"),
        WorkflowMatchType::Name { .. }
    ));

    // Git matches both the name and the command, but fuzzy matches command with a higher score.
    assert!(matches!(
        CategoriesView::matches_workflow(&workflow, "git"),
        WorkflowMatchType::Command { .. }
    ));

    assert!(matches!(
        CategoriesView::matches_workflow(&workflow, "command"),
        WorkflowMatchType::Command { .. }
    ));

    assert!(matches!(
        CategoriesView::matches_workflow(&workflow, "command"),
        WorkflowMatchType::Command { .. }
    ));

    assert_eq!(
        CategoriesView::matches_workflow(&workflow, "gibberish"),
        WorkflowMatchType::Unmatched
    );
}

#[cfg(feature = "local_fs")]
#[test]
fn project_workflow_load_state_rejects_stale_path_and_generation() {
    let mut state = ProjectWorkflowLoadState::default();
    let first_path = std::path::PathBuf::from("/first");
    let second_path = std::path::PathBuf::from("/second");

    let first_generation = state.begin(Some(first_path.clone()));
    assert!(state.owns(first_generation, &Some(first_path.clone())));

    let second_generation = state.begin(Some(second_path.clone()));
    assert!(!state.owns(first_generation, &Some(first_path)));
    assert!(!state.owns(second_generation, &Some(std::path::PathBuf::from("/other"))));
    assert!(state.owns(second_generation, &Some(second_path)));

    let cleared_generation = state.begin(None);
    assert!(!state.owns(second_generation, &None));
    assert!(state.owns(cleared_generation, &None));
}
