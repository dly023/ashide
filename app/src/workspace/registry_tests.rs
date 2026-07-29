use super::WorkspaceRegistry;
use super::{
    LocalCliAgentSessionScanKey, LocalCliAgentSessionScanParticipant,
    LocalCliAgentSessionScanRequest,
};
use crate::session_management::{
    CommandContext, SessionNavigationData, SessionNavigationPromptElements,
};
use crate::terminal::CLIAgent;
use crate::workspace::environment_backend::EnvironmentSessionRefreshIntent;
use crate::workspace::environment_table::EnvironmentTable;
use crate::workspace::PaneViewLocator;
use std::collections::HashSet;
use std::path::PathBuf;
use warpui::WindowId;

fn session(label: &str, window_id: WindowId) -> SessionNavigationData {
    SessionNavigationData::new(
        label.to_owned(),
        SessionNavigationPromptElements::from_display_label(label.to_owned()),
        CommandContext::None,
        PaneViewLocator::placeholder(),
        None,
        false,
        window_id,
        Default::default(),
    )
}

#[test]
fn session_search_projection_is_window_scoped_and_unregister_clears_it() {
    let first_window = WindowId::from_usize(11);
    let second_window = WindowId::from_usize(22);
    let mut registry = WorkspaceRegistry::new();

    registry.replace_session_search_documents(first_window, vec![session("first", first_window)]);
    let (first_generation, documents) = registry.session_search_snapshot();
    assert_eq!(documents.len(), 1);
    assert_eq!(documents[0].prompt(), "first");

    registry
        .replace_session_search_documents(second_window, vec![session("second", second_window)]);
    let (second_generation, documents) = registry.session_search_snapshot();
    assert!(second_generation > first_generation);
    assert_eq!(documents.len(), 2);

    registry.unregister(first_window);
    let (_, documents) = registry.session_search_snapshot();
    assert_eq!(documents.len(), 1);
    assert_eq!(documents[0].prompt(), "second");
}

#[test]
fn session_search_projection_event_reaches_derived_index_owner() {
    use warpui::{App, Entity, ModelContext, SingletonEntity};

    struct SearchIndexObserver {
        generations: Vec<u64>,
    }

    impl Entity for SearchIndexObserver {
        type Event = ();
    }

    App::test((), |mut app| async move {
        app.add_singleton_model(|_| WorkspaceRegistry::new());
        let observer = app.add_model(|ctx: &mut ModelContext<SearchIndexObserver>| {
            ctx.subscribe_to_model(&WorkspaceRegistry::handle(ctx), |observer, event, _| {
                let super::WorkspaceRegistryEvent::SessionSearchProjectionChanged { generation } =
                    event;
                observer.generations.push(*generation);
            });
            SearchIndexObserver {
                generations: Vec::new(),
            }
        });
        let window_id = WindowId::from_usize(33);
        WorkspaceRegistry::handle(&app).update(&mut app, |registry, ctx| {
            let generation = registry
                .replace_session_search_documents(window_id, vec![session("published", window_id)]);
            ctx.emit(super::WorkspaceRegistryEvent::SessionSearchProjectionChanged { generation });
        });

        app.update(|ctx| {
            assert_eq!(observer.as_ref(ctx).generations, vec![1]);
        });
    })
}

fn local_scan_participant(
    table: &mut EnvironmentTable,
    refresh_generation: Option<u64>,
) -> LocalCliAgentSessionScanParticipant {
    LocalCliAgentSessionScanParticipant {
        authority: "local".to_owned(),
        scan_token: table.begin_indexed_cli_agent_session_scan("local", None),
        refresh_generation,
    }
}

#[test]
fn local_cli_agent_scan_coalesces_matching_workspaces_and_explicit_refresh_preempts_safely() {
    let mut registry = WorkspaceRegistry::new();
    let enabled_agents = vec![CLIAgent::Claude, CLIAgent::Omp];
    let key = LocalCliAgentSessionScanKey::new(&enabled_agents, &HashSet::new(), &[]);
    let first_window = WindowId::from_usize(41);
    let second_window = WindowId::from_usize(42);
    let mut first_table = EnvironmentTable::default();
    let mut second_table = EnvironmentTable::default();

    assert!(matches!(
        registry.request_local_cli_agent_session_scan(
            key.clone(),
            first_window,
            local_scan_participant(&mut first_table, None),
            EnvironmentSessionRefreshIntent::PassiveProjection,
        ),
        LocalCliAgentSessionScanRequest::Started { generation: 1 },
    ));
    assert!(matches!(
        registry.request_local_cli_agent_session_scan(
            key.clone(),
            second_window,
            local_scan_participant(&mut second_table, None),
            EnvironmentSessionRefreshIntent::PassiveProjection,
        ),
        LocalCliAgentSessionScanRequest::Joined,
    ));
    assert!(matches!(
        registry.request_local_cli_agent_session_scan(
            key.clone(),
            second_window,
            local_scan_participant(&mut second_table, Some(7)),
            EnvironmentSessionRefreshIntent::UserInitiated { generation: 7 },
        ),
        LocalCliAgentSessionScanRequest::Started { generation: 2 },
    ));
    assert!(
        registry
            .complete_local_cli_agent_session_scan(&key, 1)
            .is_empty(),
        "a stale worker must not clear passive followers after explicit refresh preempts it"
    );

    let mut windows = registry
        .complete_local_cli_agent_session_scan(&key, 2)
        .into_iter()
        .map(|(window_id, _)| window_id)
        .collect::<Vec<_>>();
    windows.sort_unstable();
    assert_eq!(windows, vec![first_window, second_window]);
}

#[test]
fn local_cli_agent_scan_never_coalesces_different_source_scopes() {
    let mut registry = WorkspaceRegistry::new();
    let enabled_agents = vec![CLIAgent::Omp];
    let first_key = LocalCliAgentSessionScanKey::new(&enabled_agents, &HashSet::new(), &[]);
    let second_key = LocalCliAgentSessionScanKey::new(
        &enabled_agents,
        &HashSet::new(),
        &[PathBuf::from("/tmp/other-workspace")],
    );
    let mut first_table = EnvironmentTable::default();
    let mut second_table = EnvironmentTable::default();

    assert!(matches!(
        registry.request_local_cli_agent_session_scan(
            first_key,
            WindowId::from_usize(51),
            local_scan_participant(&mut first_table, None),
            EnvironmentSessionRefreshIntent::PassiveProjection,
        ),
        LocalCliAgentSessionScanRequest::Started { generation: 1 },
    ));
    assert!(matches!(
        registry.request_local_cli_agent_session_scan(
            second_key,
            WindowId::from_usize(52),
            local_scan_participant(&mut second_table, None),
            EnvironmentSessionRefreshIntent::PassiveProjection,
        ),
        LocalCliAgentSessionScanRequest::Started { generation: 1 },
    ));
}

#[test]
fn local_cli_agent_scan_never_coalesces_different_observed_provider_sets() {
    let mut registry = WorkspaceRegistry::new();
    let enabled_agents = vec![CLIAgent::Claude, CLIAgent::Omp];
    let first_key = LocalCliAgentSessionScanKey::new(&enabled_agents, &HashSet::new(), &[]);
    let second_key =
        LocalCliAgentSessionScanKey::new(&enabled_agents, &HashSet::from([CLIAgent::Omp]), &[]);
    let mut first_table = EnvironmentTable::default();
    let mut second_table = EnvironmentTable::default();

    assert!(matches!(
        registry.request_local_cli_agent_session_scan(
            first_key,
            WindowId::from_usize(61),
            local_scan_participant(&mut first_table, None),
            EnvironmentSessionRefreshIntent::PassiveProjection,
        ),
        LocalCliAgentSessionScanRequest::Started { generation: 1 },
    ));
    assert!(matches!(
        registry.request_local_cli_agent_session_scan(
            second_key,
            WindowId::from_usize(62),
            local_scan_participant(&mut second_table, None),
            EnvironmentSessionRefreshIntent::PassiveProjection,
        ),
        LocalCliAgentSessionScanRequest::Started { generation: 1 },
    ));
}
