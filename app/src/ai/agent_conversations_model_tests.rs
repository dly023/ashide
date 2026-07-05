use chrono::{DateTime, Utc};
use std::{
    collections::{HashMap, HashSet},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use warpui::{App, EntityId};

use crate::ai::agent::conversation::AIConversationId;
use crate::ai::ambient_agents::task::{AmbientAgentTaskState, TaskCreatorInfo};
use crate::ai::ambient_agents::{AmbientAgentTask, AmbientAgentTaskId};
use crate::ai::blocklist::history_model::BlocklistAIHistoryEvent;
use crate::workspace::WorkspaceAction;

use super::{
    AgentConversationsModel, AgentConversationsModelEvent, AgentManagementFilters,
    ConversationOrTask, HarnessFilter,
};
use warp_cli::agent::Harness;

fn create_test_task(
    task_id: &str,
    creator_uid: &str,
    updated_at: DateTime<Utc>,
) -> AmbientAgentTask {
    AmbientAgentTask {
        task_id: task_id.parse().unwrap(),
        parent_run_id: None,
        title: format!("Task {task_id}"),
        state: AmbientAgentTaskState::Succeeded,
        prompt: "test".to_string(),
        created_at: updated_at,
        started_at: Some(updated_at),
        updated_at,
        status_message: None,
        source: None,
        session_id: None,
        session_link: None,
        creator: Some(TaskCreatorInfo {
            creator_type: "USER".to_string(),
            uid: creator_uid.to_string(),
            display_name: Some(format!("User {creator_uid}")),
        }),
        conversation_id: None,
        request_usage: None,
        agent_config_snapshot: None,
        artifacts: vec![],
        is_sandbox_running: false,
        last_event_sequence: None,
        children: vec![],
    }
}

fn make_uuid(index: usize) -> String {
    format!("00000000-0000-0000-0000-{index:012}")
}

fn create_test_model() -> AgentConversationsModel {
    AgentConversationsModel {
        tasks: HashMap::new(),
        conversations: HashMap::new(),
        manually_opened_task_ids: HashSet::new(),
    }
}

#[test]
fn test_conversation_status_update_emits_conversation_updated() {
    App::test((), |mut app| async move {
        let agent_model = app.add_singleton_model(|_| create_test_model());
        let saw_conversation_updated = Arc::new(AtomicBool::new(false));

        app.update(|ctx| {
            let saw_conversation_updated = saw_conversation_updated.clone();
            ctx.subscribe_to_model(&agent_model, move |_, event, _| {
                if matches!(event, AgentConversationsModelEvent::ConversationUpdated) {
                    saw_conversation_updated.store(true, Ordering::SeqCst);
                }
            });
        });

        agent_model.update(&mut app, |model, ctx| {
            model.handle_history_event(
                &BlocklistAIHistoryEvent::UpdatedConversationStatus {
                    conversation_id: AIConversationId::new(),
                    terminal_view_id: EntityId::new(),
                    is_restored: false,
                },
                ctx,
            );
        });

        assert!(saw_conversation_updated.load(Ordering::SeqCst));
    });
}

#[test]
fn test_harness_filter_is_filtering_and_reset() {
    let mut filters = AgentManagementFilters::default();
    assert!(!filters.is_filtering());

    filters.harness = HarnessFilter::Specific(Harness::Claude);
    assert!(filters.is_filtering());

    filters.reset_all_but_owner();
    assert_eq!(filters.harness, HarnessFilter::default());
    assert!(!filters.is_filtering());
}

#[test]
fn test_task_open_action_opens_transcript_when_conversation_token_exists() {
    let now = Utc::now();
    let mut task = create_test_task(&make_uuid(7100), "user-a", now);
    task.conversation_id = Some("server-conversation-token".to_string());

    let item = ConversationOrTask::Task(&task);
    let action = item.get_open_action(None);

    match action {
        Some(WorkspaceAction::OpenConversationTranscriptViewer {
            conversation_id,
            ambient_agent_task_id,
        }) => {
            assert_eq!(conversation_id.as_str(), "server-conversation-token");
            assert_eq!(ambient_agent_task_id, Some(task.task_id));
        }
        other => panic!("expected transcript open action for task, got {other:?}"),
    }
}

#[test]
fn test_task_open_action_falls_back_to_live_ambient_session() {
    let now = Utc::now();
    let task = create_test_task(&make_uuid(7101), "user-a", now);

    let item = ConversationOrTask::Task(&task);
    let action = item.get_open_action(None);

    match action {
        Some(WorkspaceAction::OpenAmbientAgentSession { task_id }) => {
            assert_eq!(task_id, task.task_id);
        }
        other => panic!("expected ambient session open action for task, got {other:?}"),
    }
}

#[test]
fn test_get_or_async_fetch_task_data_returns_cached_task() {
    App::test((), |mut app| async move {
        let now = Utc::now();
        let task = create_test_task(&make_uuid(7000), "user-a", now);
        let task_id = task.task_id;

        let model_handle = app.add_singleton_model(|_| {
            let mut model = create_test_model();
            model.tasks.insert(task_id, task.clone());
            model
        });

        let result = model_handle.update(&mut app, |model, _| {
            model.get_or_async_fetch_task_data(&task_id)
        });

        assert!(result.is_some(), "cached task should be returned");
    });
}

#[test]
fn test_get_or_async_fetch_task_data_does_not_fetch_missing_task() {
    App::test((), |mut app| async move {
        let task_id: AmbientAgentTaskId = make_uuid(7001).parse().unwrap();
        let model_handle = app.add_singleton_model(|_| create_test_model());

        let result = model_handle.update(&mut app, |model, _| {
            model.get_or_async_fetch_task_data(&task_id)
        });

        assert!(result.is_none());
    });
}

#[test]
fn test_agent_management_filters_serde_defaults_missing_harness_to_all() {
    let stored_without_harness = r#"{
        "owners": "PersonalOnly",
        "status": "All",
        "source": "All",
        "created_on": "All",
        "creator": "All",
        "artifact": "All"
    }"#;
    let decoded: AgentManagementFilters = serde_json::from_str(stored_without_harness)
        .expect("stored payload without harness must deserialize");
    assert_eq!(decoded.harness, HarnessFilter::All);

    let original = AgentManagementFilters {
        harness: HarnessFilter::Specific(Harness::Claude),
        ..Default::default()
    };
    let encoded = serde_json::to_string(&original).unwrap();
    assert!(
        encoded.contains("\"harness\":\"claude\""),
        "expected serialized form to contain \"harness\":\"claude\", got {encoded}"
    );
    let decoded: AgentManagementFilters = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, original);

    let forward = r#"{
        "owners": "PersonalOnly",
        "status": "All",
        "source": "All",
        "created_on": "All",
        "creator": "All",
        "artifact": "All",
        "harness": "some-future-harness"
    }"#;
    let decoded: AgentManagementFilters = serde_json::from_str(forward).unwrap();
    assert_eq!(decoded.harness, HarnessFilter::All);
}
