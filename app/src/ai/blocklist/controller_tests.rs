use std::sync::Arc;
use std::time::Duration;

use ::settings::Setting;

use crate::{
    ai::{
        agent::conversation::{AIConversation, AIConversationId},
        agent_providers::{llm_id, AgentProviderSecrets},
        blocklist::{
            action_model::BlocklistAIActionModel,
            agent_view::{AgentViewController, EphemeralMessageModel},
            context_model::BlocklistAIContextModel,
            input_model::BlocklistAIInputModel,
            BlocklistAIPermissions,
        },
        byop_readiness::{
            LiveToolCallState, ReadinessState, RedactedToolKind, ToolCallKey, ToolCallRef,
        },
        execution_profiles::profiles::AIExecutionProfilesModel,
        llms::LLMPreferences,
        mcp::templatable_manager::TemplatableMCPServerManager,
    },
    object_store::model::persistence::ObjectStoreModel,
    persistence::ModelEvent,
    settings::{AISettings, AgentProvider, AgentProviderApiType, AgentProviderModel},
    terminal::{
        cli_agent_sessions::CLIAgentSessionsModel,
        model::{
            session::{active_session::ActiveSession, Sessions},
            terminal_model::TerminalModel,
        },
        model_events::ModelEventDispatcher,
        view::ambient_agent::AmbientAgentViewModel,
    },
    test_util::settings::initialize_settings_for_tests,
    workspaces::user_workspaces::UserWorkspaces,
    GlobalResourceHandles, GlobalResourceHandlesProvider,
};
use parking_lot::FairMutex;
use warp_multi_agent_api::{self as api, message};
use warpui::{App, Entity, EntityId, ModelHandle, SingletonEntity};

use super::{BlocklistAIController, BlocklistAIHistoryModel, RequestInput};

struct TestHarness;

impl Entity for TestHarness {
    type Event = ();
}

struct ControllerPreflightFixture {
    controller: ModelHandle<BlocklistAIController>,
    history_model: ModelHandle<BlocklistAIHistoryModel>,
    active_session: ModelHandle<ActiveSession>,
    terminal_view_id: EntityId,
}

fn initialize_controller_history_test_app(app: &mut App) -> std::sync::mpsc::Receiver<ModelEvent> {
    initialize_settings_for_tests(app);
    let (sender, receiver) = std::sync::mpsc::sync_channel(2);
    let mut global_resource_handles = GlobalResourceHandles::mock(app);
    global_resource_handles.model_event_sender = Some(sender);
    app.add_singleton_model(|_| GlobalResourceHandlesProvider::new(global_resource_handles));
    receiver
}

fn initialize_controller_preflight_test_app(
    app: &mut App,
) -> (
    std::sync::mpsc::Receiver<ModelEvent>,
    ControllerPreflightFixture,
) {
    let receiver = initialize_controller_history_test_app(app);
    app.add_singleton_model(AgentProviderSecrets::new);
    app.add_singleton_model(LLMPreferences::new);
    app.add_singleton_model(ObjectStoreModel::mock);
    app.add_singleton_model(UserWorkspaces::default_mock);
    app.add_singleton_model(BlocklistAIPermissions::new);
    app.add_singleton_model(|_| TemplatableMCPServerManager::default());
    app.add_singleton_model(|ctx| {
        AIExecutionProfilesModel::new(&crate::LaunchMode::new_for_unit_test(), ctx)
    });
    app.add_singleton_model(|_| CLIAgentSessionsModel::new());

    let terminal_view_id = EntityId::new();
    let terminal_model = Arc::new(FairMutex::new(TerminalModel::mock(None, None)));
    let sessions = app.add_model(|_| Sessions::new_for_test());
    let (_events_tx, events_rx) = async_channel::unbounded();
    let sessions_for_dispatcher = sessions.clone();
    let model_events = app
        .add_model(move |ctx| ModelEventDispatcher::new(events_rx, sessions_for_dispatcher, ctx));
    let active_session = {
        let sessions = sessions.clone();
        let model_events = model_events.clone();
        app.add_model(move |ctx| ActiveSession::new(sessions, model_events, ctx))
    };
    let ambient_agent_view_model =
        app.add_model(|ctx| AmbientAgentViewModel::new(terminal_view_id, false, ctx));
    let ephemeral_message_model = app.add_model(|_| EphemeralMessageModel::new());
    let agent_view_controller = {
        let terminal_model = terminal_model.clone();
        app.add_model(|ctx| {
            AgentViewController::new(
                terminal_model,
                terminal_view_id,
                ambient_agent_view_model,
                ephemeral_message_model,
                ctx,
            )
        })
    };
    let context_model = {
        let terminal_model = terminal_model.clone();
        let agent_view_controller = agent_view_controller.clone();
        app.add_model(|_| {
            BlocklistAIContextModel::new_for_test(
                terminal_model,
                terminal_view_id,
                agent_view_controller,
            )
        })
    };
    let input_model = {
        let terminal_model = terminal_model.clone();
        let context_model = context_model.clone();
        let agent_view_controller = agent_view_controller.clone();
        app.add_model(|ctx| {
            BlocklistAIInputModel::new(
                terminal_model,
                agent_view_controller,
                context_model,
                terminal_view_id,
                ctx,
            )
        })
    };
    let action_model = {
        let terminal_model = terminal_model.clone();
        let active_session = active_session.clone();
        let model_events = model_events.clone();
        app.add_model(|ctx| {
            BlocklistAIActionModel::new(
                terminal_model,
                active_session,
                &model_events,
                terminal_view_id,
                ctx,
            )
        })
    };
    let history_model = app.add_singleton_model(|_| BlocklistAIHistoryModel::new(vec![], &[]));
    let controller = {
        let input_model = input_model.clone();
        let context_model = context_model.clone();
        let action_model = action_model.clone();
        let active_session = active_session.clone();
        let agent_view_controller = agent_view_controller.clone();
        app.add_model(|ctx| {
            BlocklistAIController::new(
                input_model,
                context_model,
                action_model,
                active_session,
                agent_view_controller,
                terminal_model,
                terminal_view_id,
                ctx,
            )
        })
    };

    (
        receiver,
        ControllerPreflightFixture {
            controller,
            history_model,
            active_session,
            terminal_view_id,
        },
    )
}

fn byop_test_task(task_id: &str, messages: Vec<api::Message>) -> api::Task {
    api::Task {
        id: task_id.to_owned(),
        messages,
        dependencies: None,
        description: String::new(),
        summary: String::new(),
        server_data: String::new(),
    }
}

fn configure_test_byop_provider(app: &mut App) -> crate::ai::llms::LLMId {
    let provider_id = "test-provider".to_owned();
    let model_id = "test-model".to_owned();
    let provider = AgentProvider {
        id: provider_id.clone(),
        name: "Test Provider".to_owned(),
        kind: Default::default(),
        api_type: AgentProviderApiType::OpenAi,
        base_url: "http://127.0.0.1:9/v1".to_owned(),
        models: vec![AgentProviderModel::from_id(model_id.clone())],
        extra_headers: vec![],
    };
    AISettings::handle(app).update(app, |settings, ctx| {
        settings
            .agent_providers
            .set_value(vec![provider], ctx)
            .expect("test provider should be persisted");
    });
    llm_id::encode(&provider_id, &model_id)
}

fn byop_tool_call_message(task_id: &str, message_id: &str, call_id: &str) -> api::Message {
    api::Message {
        id: message_id.to_owned(),
        task_id: task_id.to_owned(),
        server_message_data: String::new(),
        citations: vec![],
        message: Some(message::Message::ToolCall(message::ToolCall {
            tool_call_id: call_id.to_owned(),
            tool: None,
        })),
        request_id: "request-1".to_owned(),
        timestamp: None,
    }
}

fn restored_byop_conversation(
    conversation_id: AIConversationId,
    task_id: &str,
    messages: Vec<api::Message>,
) -> AIConversation {
    AIConversation::new_restored(
        conversation_id,
        vec![byop_test_task(task_id, messages)],
        None,
    )
    .expect("restored BYOP conversation")
}

fn tool_call_ref(task_id: &str, assistant_message_id: &str, tool_call_id: &str) -> ToolCallRef {
    ToolCallRef::new(
        ToolCallKey::new(task_id, assistant_message_id, tool_call_id),
        RedactedToolKind::new("shell"),
    )
}

fn persisted_tool_result_payloads(
    history_model: &BlocklistAIHistoryModel,
    conversation_id: AIConversationId,
    task_id: &str,
    tool_call_id: &str,
) -> Vec<String> {
    history_model
        .conversation(&conversation_id)
        .and_then(|conversation| {
            conversation.get_task(&crate::ai::agent::task::TaskId::new(task_id.to_owned()))
        })
        .into_iter()
        .flat_map(|task| task.messages())
        .filter_map(|message| {
            matches!(
                message.message.as_ref(),
                Some(message::Message::ToolCallResult(result))
                    if result.tool_call_id == tool_call_id
            )
            .then(|| message.server_message_data.clone())
        })
        .collect()
}

fn task_tool_result_count(tasks: &[api::Task], task_id: &str, tool_call_id: &str) -> usize {
    tasks
        .iter()
        .find(|task| task.id == task_id)
        .into_iter()
        .flat_map(|task| task.messages.iter())
        .filter(|message| {
            matches!(
                message.message.as_ref(),
                Some(message::Message::ToolCallResult(result))
                    if result.tool_call_id == tool_call_id
            )
        })
        .count()
}

#[test]
fn missing_result_repair_appends_synthetic_tool_result_to_history() {
    App::test((), |mut app| async move {
        let receiver = initialize_controller_history_test_app(&mut app);
        let history_model = app.add_singleton_model(|_| BlocklistAIHistoryModel::new(vec![], &[]));
        let harness = app.add_model(|_| TestHarness);

        let terminal_view_id = EntityId::new();
        let conversation_id = AIConversationId::new();
        let conversation = restored_byop_conversation(
            conversation_id,
            "root-task",
            vec![byop_tool_call_message("root-task", "assistant-1", "call-1")],
        );
        history_model.update(&mut app, |history_model, ctx| {
            history_model.restore_conversations(terminal_view_id, vec![conversation], ctx);
        });

        let repair = tool_call_ref("root-task", "assistant-1", "call-1");
        let progress = harness.update(&mut app, |_, ctx| {
            BlocklistAIController::synthesize_byop_missing_cancellation_results(
                conversation_id,
                &[repair],
                ctx,
            )
        });

        assert_eq!(progress.unwrap(), 1);
        let payloads = history_model.read(&app, |history_model, _| {
            persisted_tool_result_payloads(history_model, conversation_id, "root-task", "call-1")
        });
        assert_eq!(payloads.len(), 1);
        let payload: serde_json::Value =
            serde_json::from_str(&payloads[0]).expect("valid synthetic repair payload");
        assert_eq!(payload["status"], "cancelled");
        assert_eq!(payload["synthetic"], true);
        assert_eq!(payload["repair_source"], "byop_missing_tool_result");

        let event = receiver.recv_timeout(Duration::from_secs(1)).unwrap();
        let ModelEvent::UpdateMultiAgentConversation {
            conversation_id: persisted_conversation_id,
            ..
        } = event
        else {
            panic!("expected UpdateMultiAgentConversation event");
        };
        assert_eq!(persisted_conversation_id, conversation_id.to_string());
    });
}

#[test]
fn missing_result_repair_is_idempotent_when_result_already_persisted() {
    App::test((), |mut app| async move {
        let _receiver = initialize_controller_history_test_app(&mut app);
        let history_model = app.add_singleton_model(|_| BlocklistAIHistoryModel::new(vec![], &[]));
        let harness = app.add_model(|_| TestHarness);

        let terminal_view_id = EntityId::new();
        let conversation_id = AIConversationId::new();
        let conversation = restored_byop_conversation(
            conversation_id,
            "root-task",
            vec![byop_tool_call_message("root-task", "assistant-1", "call-1")],
        );
        history_model.update(&mut app, |history_model, ctx| {
            history_model.restore_conversations(terminal_view_id, vec![conversation], ctx);
        });

        let repair = tool_call_ref("root-task", "assistant-1", "call-1");
        let first_progress = harness.update(&mut app, |_, ctx| {
            BlocklistAIController::synthesize_byop_missing_cancellation_results(
                conversation_id,
                std::slice::from_ref(&repair),
                ctx,
            )
        });
        let second_progress = harness.update(&mut app, |_, ctx| {
            BlocklistAIController::synthesize_byop_missing_cancellation_results(
                conversation_id,
                &[repair],
                ctx,
            )
        });

        assert_eq!(first_progress.unwrap(), 1);
        assert_eq!(second_progress.unwrap(), 1);
        let payloads = history_model.read(&app, |history_model, _| {
            persisted_tool_result_payloads(history_model, conversation_id, "root-task", "call-1")
        });
        assert_eq!(payloads.len(), 1);
    });
}

#[test]
fn missing_result_repair_keeps_live_action_pending_without_synthesis() {
    App::test((), |mut app| async move {
        let _receiver = initialize_controller_history_test_app(&mut app);
        let history_model = app.add_singleton_model(|_| BlocklistAIHistoryModel::new(vec![], &[]));

        let terminal_view_id = EntityId::new();
        let conversation_id = AIConversationId::new();
        let tool_call_message = byop_tool_call_message("root-task", "assistant-1", "call-1");
        let conversation = restored_byop_conversation(
            conversation_id,
            "root-task",
            vec![tool_call_message.clone()],
        );
        history_model.update(&mut app, |history_model, ctx| {
            history_model.restore_conversations(terminal_view_id, vec![conversation], ctx);
        });

        let params = crate::ai::agent::api::RequestParams::new_for_test(
            vec![],
            vec![byop_test_task("root-task", vec![tool_call_message])],
        );
        let live_tool_calls = BlocklistAIController::collect_byop_unfinished_live_tool_calls(
            &params,
            |task_id, tool_call_id| task_id == "root-task" && tool_call_id == "call-1",
        );

        assert_eq!(live_tool_calls.len(), 1);
        assert_eq!(live_tool_calls[0].state, LiveToolCallState::Running);
        assert_eq!(
            live_tool_calls[0].tool_call.key,
            ToolCallKey::new("root-task", "assistant-1", "call-1")
        );

        let report =
            crate::ai::agent_providers::chat_stream::classify_byop_controller_readiness_with_live_tool_calls(
                &params,
                live_tool_calls,
            );
        let ReadinessState::PendingToolResults { tool_calls } = report.state else {
            panic!("live action should keep missing tool result pending");
        };
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(
            tool_calls[0].key,
            ToolCallKey::new("root-task", "assistant-1", "call-1")
        );

        let payloads = history_model.read(&app, |history_model, _| {
            persisted_tool_result_payloads(history_model, conversation_id, "root-task", "call-1")
        });
        assert!(
            payloads.is_empty(),
            "pending live action must not synthesize a cancellation result"
        );
    });
}

#[test]
fn byop_preflight_synthesizes_missing_result_and_rebuilds_request() {
    App::test((), |mut app| async move {
        let (receiver, fixture) = initialize_controller_preflight_test_app(&mut app);
        let byop_model_id = configure_test_byop_provider(&mut app);

        let conversation_id = AIConversationId::new();
        let conversation = restored_byop_conversation(
            conversation_id,
            "root-task",
            vec![byop_tool_call_message("root-task", "assistant-1", "call-1")],
        );
        fixture
            .history_model
            .update(&mut app, |history_model, ctx| {
                history_model.restore_conversations(
                    fixture.terminal_view_id,
                    vec![conversation],
                    ctx,
                );
            });

        let (rebuilt_tasks, attempt_id) = fixture.controller.update(&mut app, |controller, ctx| {
            let mut request_input = RequestInput::for_task(
                vec![],
                crate::ai::agent::task::TaskId::new("root-task".to_owned()),
                &fixture.active_session,
                None,
                conversation_id,
                fixture.terminal_view_id,
                ctx,
            );
            request_input.model_id = byop_model_id;
            let snapshot = controller
                .conversation_snapshot_for_request(&request_input, ctx)
                .expect("test conversation snapshot");
            let mut conversation_data = snapshot.conversation_data;
            let mut request_params = controller.build_request_params_for_input(
                &request_input,
                conversation_data.clone(),
                None,
                snapshot.parent_agent_id,
                snapshot.agent_name,
                ctx,
            );

            controller
                .run_byop_request_preflight(
                    &mut request_input,
                    &mut conversation_data,
                    &mut request_params,
                    None,
                    ctx,
                )
                .expect("missing result should be repaired by preflight");

            (
                request_params.tasks.clone(),
                request_params.byop_readiness_attempt_id.clone(),
            )
        });

        assert!(
            attempt_id.is_some(),
            "preflight progress should preserve readiness attempt id"
        );
        assert_eq!(
            task_tool_result_count(&rebuilt_tasks, "root-task", "call-1"),
            1,
            "rebuilt request params should include the synthetic tool result"
        );

        let payloads = fixture.history_model.read(&app, |history_model, _| {
            persisted_tool_result_payloads(history_model, conversation_id, "root-task", "call-1")
        });
        assert_eq!(payloads.len(), 1);
        let payload: serde_json::Value =
            serde_json::from_str(&payloads[0]).expect("valid synthetic repair payload");
        assert_eq!(payload["status"], "cancelled");
        assert_eq!(payload["synthetic"], true);
        assert_eq!(payload["repair_source"], "byop_missing_tool_result");

        let event = receiver.recv_timeout(Duration::from_secs(1)).unwrap();
        let ModelEvent::UpdateMultiAgentConversation {
            conversation_id: persisted_conversation_id,
            ..
        } = event
        else {
            panic!("expected UpdateMultiAgentConversation event");
        };
        assert_eq!(persisted_conversation_id, conversation_id.to_string());
    });
}
