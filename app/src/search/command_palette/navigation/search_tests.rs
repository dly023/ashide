use super::full_text_searcher::byte_indices_to_char_indices;
use super::{SearchableSessionStringRanges, SessionHighlightIndices};

// ── byte_indices_to_char_indices ─────────────────────────────────────

#[test]
fn ascii_only_is_identity() {
    let text = "hello world";
    assert_eq!(
        byte_indices_to_char_indices(text, vec![0, 6, 10]),
        vec![0, 6, 10]
    );
}

#[test]
fn multi_byte_chars_shift_indices() {
    // '→' is 3 bytes.  Layout:
    //   byte 0..3  = '→'  (char 0)
    //   byte 3     = ' '  (char 1)
    //   byte 4     = 'l'  (char 2)
    //   byte 5     = 's'  (char 3)
    let text = "→ ls";
    assert_eq!(
        byte_indices_to_char_indices(text, vec![0, 3, 4, 5]),
        vec![0, 1, 2, 3]
    );
}

#[test]
fn continuation_bytes_are_filtered_out() {
    // '→' occupies bytes 0, 1, 2.  Only byte 0 is a char boundary.
    let text = "→ls";
    assert_eq!(
        byte_indices_to_char_indices(text, vec![0, 1, 2, 3, 4]),
        vec![0, 1, 2] // char 0='→', char 1='l', char 2='s'
    );
}

#[test]
fn empty_inputs() {
    assert_eq!(
        byte_indices_to_char_indices("", vec![]),
        Vec::<usize>::new()
    );
    assert_eq!(
        byte_indices_to_char_indices("abc", vec![]),
        Vec::<usize>::new()
    );
}

#[test]
fn mixed_width_characters() {
    // 'é' is 2 bytes, '→' is 3 bytes, 'a' is 1 byte.
    // Layout: é(0..2) →(2..5) a(5)
    let text = "é→a";
    assert_eq!(
        byte_indices_to_char_indices(text, vec![0, 2, 5]),
        vec![0, 1, 2] // char 0='é', char 1='→', char 2='a'
    );
}

#[test]
fn out_of_bounds_byte_indices_are_dropped() {
    let text = "ab";
    assert_eq!(
        byte_indices_to_char_indices(text, vec![0, 1, 99]),
        vec![0, 1]
    );
}

// ── End-to-end: highlight pipeline with multi-byte prompt ────────────

/// Simulates the same range construction that `searchable_session_string_and_ranges`
/// performs, then verifies that char-converted Tantivy byte indices produce
/// correct per-element highlights.
#[test]
fn highlight_indices_correct_after_byte_to_char_conversion() {
    // Prompt with multi-byte chars: "→⇒≠" = 3 chars, 9 bytes.
    let prompt = "→⇒≠";
    let command = "ls";
    let hint = "Running...";

    // Build the searchable string the same way the production code does.
    let mut searchable = prompt.to_string();
    let prompt_end = prompt.chars().count(); // 3

    searchable.push(' ');
    searchable.push_str(command);
    let cmd_start = prompt_end + 1; // 4
    let cmd_end = cmd_start + command.chars().count(); // 6
    let command_range = Some(cmd_start..cmd_end);

    searchable.push(' ');
    searchable.push_str(hint);
    let hint_start = cmd_end + 1; // 7
    let hint_end = hint_start + hint.chars().count(); // 17
    let hint_text_range = hint_start..hint_end;

    // Simulate Tantivy returning byte offsets for "ls" in the searchable
    // string.  "→⇒≠ ls Running..." — 'l' is at byte 10, 's' at byte 11.
    let byte_of_l = searchable.find('l').unwrap();
    let byte_of_s = byte_of_l + 1;
    assert_eq!(byte_of_l, 10, "precondition: 'l' should be at byte 10");

    // Without conversion these byte offsets (10, 11) would NOT fall in the
    // char-based command_range (4..6), so highlights would be lost.
    let char_indices = byte_indices_to_char_indices(&searchable, vec![byte_of_l, byte_of_s]);

    let ranges = SearchableSessionStringRanges {
        command_range,
        hint_text_range,
    };
    let highlights = SessionHighlightIndices::new(char_indices, ranges);

    // 'l' and 's' should map to command-relative indices 0 and 1.
    assert_eq!(highlights.command_indices, Some(vec![0, 1]));
    assert!(highlights.hint_text_indices.is_empty());
}

/// Same scenario but without the conversion — demonstrates the bug.
#[test]
fn raw_byte_indices_produce_wrong_highlights() {
    let prompt = "→⇒≠";
    let command = "ls";
    let hint = "Running...";

    let mut searchable = prompt.to_string();
    let prompt_end = prompt.chars().count(); // 3

    searchable.push(' ');
    searchable.push_str(command);
    let cmd_start = prompt_end + 1;
    let cmd_end = cmd_start + command.chars().count();
    let command_range = Some(cmd_start..cmd_end);

    searchable.push(' ');
    searchable.push_str(hint);
    let hint_start = cmd_end + 1;
    let hint_end = hint_start + hint.chars().count();
    let hint_text_range = hint_start..hint_end;

    // Feed raw byte offsets (10, 11) directly — the bug path.
    let byte_of_l = searchable.find('l').unwrap(); // 10
    let byte_of_s = byte_of_l + 1; // 11

    let ranges = SearchableSessionStringRanges {
        command_range,
        hint_text_range,
    };
    let highlights = SessionHighlightIndices::new(vec![byte_of_l, byte_of_s], ranges);

    // Byte 10 and 11 fall in the char-based hint_text_range (7..17), NOT the
    // command_range (4..6), so command highlights are lost and hint highlights
    // land on wrong characters.
    assert_eq!(highlights.command_indices, Some(vec![]));
    assert_eq!(highlights.hint_text_indices, vec![3, 4]); // wrong!
}

// ── Bug-1 fix: restored sessions surface in search with a restore target ──

#[test]
fn restored_session_snapshot_yields_searchable_prompt_and_restore_target() {
    use crate::app_state::{WorkspaceSessionKind, WorkspaceSessionSnapshot};
    use crate::session_management::SessionNavigationData;
    use warpui::WindowId;

    // A restored CLI-agent session like the navigator would show but the old
    // search (live-panes-only) would miss.
    let snapshot = WorkspaceSessionSnapshot {
        id: "restored:agent:abc-123".to_string(),
        container_uuid: None,
        kind: WorkspaceSessionKind::AgentTerminal,
        label: Some("Fix the login bug".to_string()),
        environment_authority_key: Some("local".to_string()),
        cwd: Some("/Users/me/proj".to_string()),
        startup_directory: None,
        cli_agent: Some("claude-code".to_string()),
        cli_command: Some("claude --resume abc-123".to_string()),
        cli_agent_origin: None,
        conversation_ids: vec![],
        active_conversation_id: None,
        cli_agent_session_id: Some("abc-123".to_string()),
        is_active: false,
        is_pinned: false,
        updated_at_unix_ms: Some(1_700_000_000_000),
        is_live_container: false,
    };

    let window_id = WindowId::new();
    let data = SessionNavigationData::from_workspace_session_snapshot(&snapshot, window_id);

    // The restore target must be set so activation routes through
    // ActivateRestoredWorkspaceSession instead of pane focus.
    let target = data.restore_target().expect("restore target should be set");
    assert_eq!(target.session_id, "restored:agent:abc-123");
    assert_eq!(target.environment_authority_key, "local");

    // The searchable prompt must contain the label and the agent identity so a
    // fuzzy match on "login" or "claude" can hit this row.
    let prompt = data.prompt();
    assert!(
        prompt.contains("Fix the login bug"),
        "prompt should contain the label fragment; got: {prompt:?}"
    );
    assert!(
        prompt.contains("claude"),
        "prompt should contain the agent fragment; got: {prompt:?}"
    );

    // The display label fallback must be populated (used when no PS1/chip).
    assert_eq!(
        data.prompt_elements().display_label.as_deref(),
        Some("Fix the login bug")
    );
}

#[cfg(not(target_family = "wasm"))]
#[test]
fn full_text_session_search_reuses_published_projection_until_generation_changes() {
    use super::{FullTextSessionSearcher, SessionSearcher};
    use crate::session_management::{
        CommandContext, SessionNavigationData, SessionNavigationPromptElements, SessionSource,
    };
    use crate::workspace::{PaneViewLocator, WorkspaceRegistry};
    use warpui::{App, SingletonEntity, WindowId};

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

    App::test((), |mut app| async move {
        app.add_singleton_model(|_| WorkspaceRegistry::new());
        let source = app.add_model(|_| SessionSource::None);
        let window_id = WindowId::from_usize(101);
        WorkspaceRegistry::handle(&app).update(&mut app, |registry, _| {
            registry.replace_session_search_documents(
                window_id,
                vec![session("first agent", window_id)],
            );
        });

        let mut searcher = app.update(|ctx| FullTextSessionSearcher::new(source, ctx));
        app.update(|ctx| {
            assert_eq!(searcher.search("first", ctx).unwrap().len(), 1);
            assert!(searcher.search("second", ctx).unwrap().is_empty());
        });

        WorkspaceRegistry::handle(&app).update(&mut app, |registry, _| {
            registry.replace_session_search_documents(
                window_id,
                vec![session("second agent", window_id)],
            );
        });
        app.update(|ctx| {
            assert!(searcher.search("second", ctx).is_err());
        });
        app.update(|ctx| {
            let generation = WorkspaceRegistry::as_ref(ctx).session_search_generation();
            searcher.refresh_search_index(generation, ctx).unwrap();
        });
        app.update(|ctx| {
            assert!(searcher.search("first", ctx).unwrap().is_empty());
            assert_eq!(searcher.search("second", ctx).unwrap().len(), 1);
        });
    })
}

#[test]
fn session_search_static_guard() {
    let search = include_str!("search.rs");
    let data_source = include_str!("data_source.rs");
    let registry = include_str!("../../../workspace/registry.rs");
    let navigator = include_str!("../../../workspace/view/session_navigator.rs");

    assert!(search.contains("WorkspaceRegistry::as_ref(app).session_search_generation()"));
    assert!(search.contains("fn refresh_search_index("));
    assert!(!search.contains("SessionNavigationData::all_sessions("));
    assert!(!search.contains("app.window_ids("));
    assert!(!search.contains("app.views_of_type::<Workspace>("));

    let full_text_query = search
        .split("impl SessionSearcher for FullTextSessionSearcher {")
        .nth(1)
        .expect("full-text Session Navigator searcher must exist")
        .split("fn active_session_id(")
        .next()
        .expect("full-text query must precede active-session lookup");
    assert!(!full_text_query.contains("create_searcher("));
    assert!(!full_text_query.contains("build_index("));

    assert!(data_source.contains("subscribe_to_model("));
    assert!(data_source.contains("WorkspaceRegistry::handle(ctx)"));
    assert!(data_source.contains("refresh_search_index(*generation, ctx)"));
    assert!(registry.contains("SessionSearchProjectionChanged"));
    assert!(navigator.contains("publish_session_navigator_search_documents(ctx)"));
    assert!(navigator.contains("SessionNavigationData::from_workspace_session_snapshot"));
}
