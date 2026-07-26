use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::Local;
use smol_str::SmolStr;
use warp_editor::render::model::LineCount;
use warp_util::path::EscapeChar;
use warpui::App;

use super::{
    build_diff_hunk_prompt, build_review_prompt, build_selection_line_range_prompt,
    build_selection_substring_prompt, CLIAgent, UBER_TEAM_UID,
};
use crate::ai::agent::{AgentReviewCommentBatch, DiffSetHunk};
use crate::code::editor::line::EditorLineLocation;
use crate::code_review::comments::{
    AttachedReviewComment, AttachedReviewCommentTarget, CommentOrigin, LineDiffContent,
};
use crate::object_store::ids::StableObjectId;
use crate::workspaces::team::Team;
use crate::workspaces::user_workspaces::UserWorkspaces;
use crate::workspaces::workspace::Workspace;

/// Helper to build an alias map from pairs.
fn aliases(pairs: &[(&str, &str)]) -> HashMap<SmolStr, String> {
    pairs
        .iter()
        .map(|(k, v)| (SmolStr::new(k), v.to_string()))
        .collect()
}

// ---------------------------------------------------------------------------
// Helpers for prompt-building tests
// ---------------------------------------------------------------------------

fn make_comment(
    content: &str,
    target: AttachedReviewCommentTarget,
    outdated: bool,
) -> AttachedReviewComment {
    AttachedReviewComment {
        id: Default::default(),
        content: content.to_string(),
        target,
        last_update_time: Local::now(),
        base: None,
        head: None,
        outdated,
        origin: CommentOrigin::Native,
    }
}

fn batch(comments: Vec<AttachedReviewComment>) -> AgentReviewCommentBatch {
    AgentReviewCommentBatch {
        comments,
        diff_set: HashMap::new(),
    }
}

// ---------------------------------------------------------------------------
// build_review_prompt tests
// ---------------------------------------------------------------------------

#[test]
fn test_build_review_prompt_current_line_is_1_indexed() {
    // LineCount 0 (0-indexed) should appear as L1 in the prompt.
    let comment = make_comment(
        "fix this",
        AttachedReviewCommentTarget::Line {
            absolute_file_path: PathBuf::from("/repo/src/main.rs"),
            line: EditorLineLocation::Current {
                line_number: LineCount::from(0),
                line_range: LineCount::from(0)..LineCount::from(1),
            },
            content: LineDiffContent::default(),
        },
        false,
    );
    let prompt = build_review_prompt(&batch(vec![comment]));
    assert!(
        prompt.contains("/repo/src/main.rs L1"),
        "expected 1-indexed L1, got: {prompt}",
    );
    assert!(prompt.contains("fix this"));
}

#[test]
fn test_build_review_prompt_removed_line_is_1_indexed() {
    let comment = make_comment(
        "why was this deleted?",
        AttachedReviewCommentTarget::Line {
            absolute_file_path: PathBuf::from("/repo/old.rs"),
            line: EditorLineLocation::Removed {
                line_number: LineCount::from(9),
                line_range: LineCount::from(9)..LineCount::from(10),
                index: 0,
            },
            content: LineDiffContent::default(),
        },
        false,
    );
    let prompt = build_review_prompt(&batch(vec![comment]));
    assert!(
        prompt.contains("(deleted, was L10"),
        "expected 1-indexed L10, got: {prompt}",
    );
}

#[test]
fn test_build_review_prompt_collapsed_range_is_1_indexed_start() {
    let comment = make_comment(
        "check this hunk",
        AttachedReviewCommentTarget::Line {
            absolute_file_path: PathBuf::from("/repo/lib.rs"),
            line: EditorLineLocation::Collapsed {
                line_range: LineCount::from(4)..LineCount::from(10),
            },
            content: LineDiffContent::default(),
        },
        false,
    );
    let prompt = build_review_prompt(&batch(vec![comment]));
    // line_range is [4, 10) 0-indexed -> L5-L10 (1-indexed, both ends inclusive)
    assert!(prompt.contains("L5-L10"), "expected L5-L10, got: {prompt}",);
}

#[test]
fn test_build_review_prompt_file_level_comment() {
    let comment = make_comment(
        "needs refactoring",
        AttachedReviewCommentTarget::File {
            absolute_file_path: PathBuf::from("/repo/src/utils.rs"),
        },
        false,
    );
    let prompt = build_review_prompt(&batch(vec![comment]));
    assert!(prompt.contains("/repo/src/utils.rs: needs refactoring"));
    // Not a deleted file (empty diff_set), so no "deleted file" text.
    assert!(!prompt.contains("deleted file"));
}

#[test]
fn test_build_review_prompt_deleted_file_comment() {
    let comment = make_comment(
        "why remove this?",
        AttachedReviewCommentTarget::File {
            absolute_file_path: PathBuf::from("/repo/src/old.rs"),
        },
        false,
    );
    let mut review = batch(vec![comment]);
    review.diff_set.insert(
        "src/old.rs".to_string(),
        vec![DiffSetHunk {
            line_range: LineCount::from(0)..LineCount::from(5),
            diff_content: String::new(),
            lines_added: 0,
            lines_removed: 5,
        }],
    );
    let prompt = build_review_prompt(&review);
    assert!(
        prompt.contains("(deleted file"),
        "expected deleted file annotation, got: {prompt}",
    );
}

#[test]
fn test_build_review_prompt_general_comment() {
    let comment = make_comment(
        "overall looks good",
        AttachedReviewCommentTarget::General,
        false,
    );
    let prompt = build_review_prompt(&batch(vec![comment]));
    assert!(prompt.contains("General: overall looks good"));
}

#[test]
fn test_build_review_prompt_skips_outdated_comments() {
    let active = make_comment("keep me", AttachedReviewCommentTarget::General, false);
    let outdated = make_comment("skip me", AttachedReviewCommentTarget::General, true);
    let prompt = build_review_prompt(&batch(vec![active, outdated]));
    assert!(prompt.contains("keep me"));
    assert!(!prompt.contains("skip me"));
}

#[test]
fn test_build_review_prompt_multiple_comments() {
    let c1 = make_comment(
        "first",
        AttachedReviewCommentTarget::Line {
            absolute_file_path: PathBuf::from("/repo/a.rs"),
            line: EditorLineLocation::Current {
                line_number: LineCount::from(4),
                line_range: LineCount::from(4)..LineCount::from(5),
            },
            content: LineDiffContent::default(),
        },
        false,
    );
    let c2 = make_comment("second", AttachedReviewCommentTarget::General, false);
    let prompt = build_review_prompt(&batch(vec![c1, c2]));
    assert!(prompt.contains("/repo/a.rs L5: first"));
    assert!(prompt.contains("General: second"));
}

#[test]
fn test_build_review_prompt_exports_internal_markdown_without_punctuation_escapes() {
    let comment = make_comment("Fix this\\.", AttachedReviewCommentTarget::General, false);
    let prompt = build_review_prompt(&batch(vec![comment]));
    assert!(prompt.contains("General: Fix this."));
    assert!(!prompt.contains("Fix this\\."));
}

// ---------------------------------------------------------------------------
// build_diff_hunk_prompt tests
// ---------------------------------------------------------------------------

#[test]
fn test_build_diff_hunk_prompt_format() {
    let prompt = build_diff_hunk_prompt(Path::new("/repo/src/main.rs"), 10, 20, 3, 2);
    assert_eq!(
        prompt,
        "/repo/src/main.rs L10-L20 (+3 -2) -- run `git diff` to see the full context.",
    );
}

// ---------------------------------------------------------------------------
// build_selection_line_range_prompt tests
// ---------------------------------------------------------------------------

#[test]
fn test_build_selection_line_range_prompt_format() {
    let result = build_selection_line_range_prompt("src/foo.rs", 5, 10);
    assert_eq!(result, "src/foo.rs L5-L10");
}

#[test]
fn test_build_selection_substring_prompt_format() {
    let result = build_selection_substring_prompt("src/foo.rs", 5, "let x = 42;");
    assert_eq!(result, "src/foo.rs L5: let x = 42;");
}

#[test]
fn test_detect_known_agents() {
    App::test((), |mut app| async move {
        app.update(|ctx| {
            for (command, expected) in [
                ("claude", CLIAgent::Claude),
                ("codex", CLIAgent::Codex),
                ("deepseek", CLIAgent::DeepSeek),
                ("deepseek-tui", CLIAgent::DeepSeek),
                ("agy", CLIAgent::Antigravity),
                ("amp", CLIAgent::Amp),
                ("droid", CLIAgent::Droid),
                ("opencode", CLIAgent::OpenCode),
                ("copilot", CLIAgent::Copilot),
                ("agent", CLIAgent::CursorCli),
                ("goose", CLIAgent::Goose),
                ("omp", CLIAgent::Omp),
            ] {
                assert_eq!(
                    CLIAgent::detect(command, None, None, ctx),
                    Some(expected),
                    "failed to detect {command}",
                );
            }
        });
    });
}

#[test]
fn jcode_is_a_first_class_detectable_cli_agent() {
    App::test((), |mut app| async move {
        app.update(|ctx| {
            assert_eq!(
                CLIAgent::detect("jcode", None, None, ctx),
                Some(CLIAgent::Jcode),
            );
            assert_eq!(CLIAgent::Jcode.command_prefix(), "jcode");
            assert_eq!(
                CLIAgent::from_command_prefix("jcode"),
                Some(CLIAgent::Jcode),
            );
        });
    });
}

#[test]
fn test_from_command_prefix_resolves_persisted_agent_command() {
    assert_eq!(
        CLIAgent::from_command_prefix("codex"),
        Some(CLIAgent::Codex)
    );
    assert_eq!(
        CLIAgent::from_command_prefix("deepseek-tui"),
        Some(CLIAgent::DeepSeek)
    );
    assert_eq!(CLIAgent::from_command_prefix("gemini"), None);
    assert_eq!(CLIAgent::from_command_prefix("unknown-agent"), None);
}

#[test]
fn test_resume_session_id_from_command_parses_explicit_restore_commands() {
    assert_eq!(
        CLIAgent::Codex
            .resume_session_id_from_command("codex resume 019e3a0f-2fa7-78d2-ac9d-09b9c6b228ed"),
        Some("019e3a0f-2fa7-78d2-ac9d-09b9c6b228ed".to_owned())
    );
    assert_eq!(
        CLIAgent::Codex.resume_session_id_from_command(
            "cd /repo && codex resume 019e3a0f-2fa7-78d2-ac9d-09b9c6b228ed"
        ),
        Some("019e3a0f-2fa7-78d2-ac9d-09b9c6b228ed".to_owned())
    );
    assert_eq!(
        CLIAgent::Codex.resume_session_id_from_command(
            "codex resume 2026-05-18T15-48-54-019e3a0f-2fa7-78d2-ac9d-09b9c6b228ed"
        ),
        Some("019e3a0f-2fa7-78d2-ac9d-09b9c6b228ed".to_owned())
    );
    assert_eq!(
        CLIAgent::Claude.resume_session_id_from_command("claude --resume claude-123"),
        Some("claude-123".to_owned())
    );
    assert_eq!(
        CLIAgent::Claude.resume_session_id_from_command("claude --resume=claude-123"),
        Some("claude-123".to_owned())
    );
    assert_eq!(
        CLIAgent::Claude.resume_session_id_from_command("claude --continue"),
        None
    );
    assert_eq!(
        CLIAgent::Omp.resume_session_id_from_command("omp --resume omp-123"),
        Some("omp-123".to_owned())
    );
    assert_eq!(
        CLIAgent::Omp.resume_session_id_from_command("omp --resume=omp-123"),
        Some("omp-123".to_owned())
    );
    assert_eq!(
        CLIAgent::Omp.resume_session_id_from_command("omp -r omp-123"),
        Some("omp-123".to_owned())
    );
}

#[test]
fn test_codex_explicit_resume_command_uses_cli_session_uuid() {
    assert_eq!(
        CLIAgent::Codex.explicit_resume_command(
            Some("2026-05-18T15-48-54-019e3a0f-2fa7-78d2-ac9d-09b9c6b228ed"),
            Some("/root"),
        ),
        Some("codex resume 019e3a0f-2fa7-78d2-ac9d-09b9c6b228ed".to_owned())
    );
    assert_eq!(
        CLIAgent::Codex.explicit_resume_command(
            Some("rollout-2026-05-18T15-48-54-019e3a0f-2fa7-78d2-ac9d-09b9c6b228ed"),
            Some("/root"),
        ),
        Some("codex resume 019e3a0f-2fa7-78d2-ac9d-09b9c6b228ed".to_owned())
    );
    assert_eq!(
        CLIAgent::Codex.explicit_resume_command(
            Some("msg_06435be93b11cbcc016a55deda46808197a7e8894330ebe948"),
            Some("/root"),
        ),
        None,
        "Codex response-item message IDs must never become resume commands"
    );
    assert_eq!(
        CLIAgent::Codex.resume_session_id_from_command(
            "codex resume msg_06435be93b11cbcc016a55deda46808197a7e8894330ebe948"
        ),
        None,
        "invalid manual commands must not poison live durable session bindings"
    );
}

#[test]
fn jcode_uses_its_official_icon_in_all_agent_entrypoints() {
    assert_eq!(
        CLIAgent::Jcode.icon(),
        Some(warp_core::ui::icons::Icon::JcodeLogo)
    );
}

#[test]
fn jcode_explicit_resume_command_uses_public_cli_contract() {
    assert_eq!(
        CLIAgent::Jcode.explicit_resume_command(Some("session-123"), Some("/repo")),
        Some("jcode --resume session-123".to_owned()),
    );
    assert_eq!(
        CLIAgent::Jcode.resume_session_id_from_command("jcode --resume session-123"),
        Some("session-123".to_owned()),
    );
    assert_eq!(
        CLIAgent::Jcode.explicit_resume_command(
            Some("session_bug_1784897411999_c3c24cb8ea67c6a2"),
            Some("/repo"),
        ),
        Some("jcode --resume session_bug_1784897411999_c3c24cb8ea67c6a2".to_owned()),
    );
    assert_eq!(CLIAgent::Jcode.explicit_resume_command(None, None), None);
}

#[test]
fn jcode_discovery_and_resume_do_not_require_a_daemon_handshake() {
    let capabilities = CLIAgent::Jcode.capabilities();

    assert!(capabilities.can_detect);
    assert!(capabilities.can_bind_live_session);
    assert!(capabilities.can_index_sessions);
    assert!(capabilities.can_list_sessions);
    assert!(capabilities.can_cold_restore);
    assert!(capabilities.can_resume);
    assert_eq!(
        CLIAgent::Jcode.explicit_resume_command(Some("offline-session"), None),
        Some("jcode --resume offline-session".to_owned()),
    );
}

#[test]
fn every_agent_has_one_layered_capability_record() {
    for agent in enum_iterator::all::<CLIAgent>() {
        let record = agent.record();
        assert_eq!(record.agent, agent);
        assert_eq!(record.capabilities, agent.capabilities());
    }

    let codex = CLIAgent::Codex.capabilities();
    assert!(codex.can_detect && codex.can_bind_live_session);
    assert!(codex.can_index_sessions && codex.can_list_sessions && codex.can_cold_restore);
    assert!(codex.can_resume && codex.can_attach && codex.can_target_environment_runtime);
    assert!(codex.can_read_session_ir);
    assert!(codex.can_fork && codex.can_edit_preview);
    assert!(codex.can_write_native_history && codex.can_launch_derived_session);

    let antigravity = CLIAgent::Antigravity.capabilities();
    assert!(antigravity.can_detect && antigravity.can_bind_live_session);
    assert!(!antigravity.can_index_sessions && !antigravity.can_cold_restore);
    assert!(antigravity.can_resume && antigravity.can_attach);
    assert!(!antigravity.can_read_session_ir && !antigravity.can_fork);
    assert!(!antigravity.can_write_native_history && !antigravity.can_launch_derived_session);

    let pi = CLIAgent::Pi.capabilities();
    assert!(pi.can_detect && pi.can_bind_live_session);
    assert!(!pi.can_index_sessions && !pi.can_list_sessions && !pi.can_cold_restore);
    assert!(!pi.can_resume && !pi.can_attach && !pi.can_target_environment_runtime);
    assert!(!pi.can_read_session_ir && !pi.can_fork && !pi.can_edit_preview);
    assert!(!pi.can_write_native_history && !pi.can_launch_derived_session);

    let unknown = CLIAgent::Unknown.capabilities();
    assert!(!unknown.can_detect && !unknown.can_bind_live_session);
}

#[test]
fn jcode_readonly_discovery_capabilities_are_separate_from_advanced_session_actions() {
    let capabilities = CLIAgent::Jcode.capabilities();

    assert!(capabilities.can_detect);
    assert!(capabilities.can_bind_live_session);
    assert!(capabilities.can_index_sessions);
    assert!(capabilities.can_list_sessions);
    assert!(capabilities.can_cold_restore);
    assert!(capabilities.can_resume);
    assert!(capabilities.can_target_environment_runtime);

    assert!(!capabilities.can_attach);
    assert!(!capabilities.can_read_session_ir);
}

#[test]
fn omp_readonly_discovery_has_an_independent_explicit_resume_adapter() {
    let capabilities = CLIAgent::Omp.capabilities();

    assert!(capabilities.can_index_sessions);
    assert!(capabilities.can_list_sessions);
    assert!(capabilities.can_cold_restore);
    assert!(capabilities.can_resume);
    assert_eq!(
        CLIAgent::Omp.explicit_resume_command(Some("019f9401-bf1b-7000-996f-756210becb26"), None),
        Some("omp --resume 019f9401-bf1b-7000-996f-756210becb26".to_owned())
    );
    assert!(!capabilities.can_attach);
    assert!(!capabilities.can_read_session_ir);
    assert!(!capabilities.can_fork);
    assert!(!capabilities.can_edit_preview);
    assert!(!capabilities.can_write_native_history);
    assert!(!capabilities.can_launch_derived_session);
}

#[test]
fn jcode_fork_edit_and_writeback_remain_disabled() {
    let capabilities = CLIAgent::Jcode.capabilities();

    assert!(!capabilities.can_fork);
    assert!(!capabilities.can_edit_preview);
    assert!(!capabilities.can_write_native_history);
    assert!(!capabilities.can_launch_derived_session);
}

#[test]
fn discovery_descriptors_are_exhaustive_and_match_index_capability() {
    for agent in enum_iterator::all::<CLIAgent>() {
        let descriptor = agent.session_discovery_provider();
        assert_eq!(
            descriptor.is_some(),
            agent.capabilities().can_index_sessions,
            "{agent:?} index capability must have exactly one discovery descriptor"
        );
    }
}

#[test]
fn test_detect_with_arguments() {
    App::test((), |mut app| async move {
        app.update(|ctx| {
            assert_eq!(
                CLIAgent::detect("claude --model opus", None, None, ctx),
                Some(CLIAgent::Claude),
            );
        });
    });
}

#[test]
fn test_detect_with_leading_whitespace() {
    App::test((), |mut app| async move {
        app.update(|ctx| {
            assert_eq!(
                CLIAgent::detect("  claude", None, None, ctx),
                Some(CLIAgent::Claude),
            );
            assert_eq!(
                CLIAgent::detect("\tclaude --help", None, None, ctx),
                Some(CLIAgent::Claude),
            );
        });
    });
}

#[test]
fn test_detect_no_match() {
    App::test((), |mut app| async move {
        app.update(|ctx| {
            assert_eq!(CLIAgent::detect("ls -la", None, None, ctx), None);
            assert_eq!(CLIAgent::detect("vim", None, None, ctx), None);
            assert_eq!(CLIAgent::detect("claude_wrapper", None, None, ctx), None);
        });
    });
}

#[test]
fn test_detect_with_alias() {
    App::test((), |mut app| async move {
        app.update(|ctx| {
            let map = aliases(&[("c", "claude")]);
            assert_eq!(
                CLIAgent::detect("c", None, Some(&map), ctx),
                Some(CLIAgent::Claude),
            );
            assert_eq!(
                CLIAgent::detect("c --help", None, Some(&map), ctx),
                Some(CLIAgent::Claude),
            );
        });
    });
}

#[test]
fn test_detect_alias_not_matching() {
    App::test((), |mut app| async move {
        app.update(|ctx| {
            let map = aliases(&[("c", "cat")]);
            assert_eq!(CLIAgent::detect("c", None, Some(&map), ctx), None);
        });
    });
}

#[test]
fn test_detect_with_env_var_prefix() {
    App::test((), |mut app| async move {
        app.update(|ctx| {
            assert_eq!(
                CLIAgent::detect(
                    "EXAMPLE=true opencode",
                    Some(EscapeChar::Backslash),
                    None,
                    ctx,
                ),
                Some(CLIAgent::OpenCode),
            );
        });
    });
}

#[test]
fn test_detect_with_multiple_env_vars() {
    App::test((), |mut app| async move {
        app.update(|ctx| {
            assert_eq!(
                CLIAgent::detect(
                    "FOO=1 BAR=2 opencode --flag",
                    Some(EscapeChar::Backslash),
                    None,
                    ctx,
                ),
                Some(CLIAgent::OpenCode),
            );
        });
    });
}

#[test]
fn test_detect_with_alias_and_env_var() {
    App::test((), |mut app| async move {
        app.update(|ctx| {
            let map = aliases(&[("oc", "EXAMPLE=1 opencode")]);
            assert_eq!(
                CLIAgent::detect("oc --flag", Some(EscapeChar::Backslash), Some(&map), ctx,),
                Some(CLIAgent::OpenCode),
            );
        });
    });
}

/// Creates a workspace containing a team with the given UID.
fn workspace_with_team_uid(uid: &str) -> Workspace {
    Workspace::from_local_cache(
        StableObjectId::from_string_lossy("test-workspace-uid-001").into(),
        "Test Workspace".to_string(),
        Some(vec![Team::from_local_cache(
            StableObjectId::from_string_lossy(uid),
            "Test Team".to_string(),
            None,
            None,
            None,
        )]),
    )
}

#[test]
fn test_detect_aifx_agent_run_claude_on_uber_team() {
    App::test((), |mut app| async move {
        let uber_workspace = workspace_with_team_uid(UBER_TEAM_UID);
        app.add_singleton_model(|ctx| UserWorkspaces::mock(vec![uber_workspace], ctx));

        app.update(|ctx| {
            assert_eq!(
                CLIAgent::detect("aifx agent run claude", None, None, ctx),
                Some(CLIAgent::Claude),
            );
            // With extra args
            assert_eq!(
                CLIAgent::detect("aifx agent run claude --verbose", None, None, ctx),
                Some(CLIAgent::Claude),
            );
        });
    });
}

#[test]
fn test_detect_aifx_agent_run_claude_via_alias_on_uber_team() {
    App::test((), |mut app| async move {
        let uber_workspace = workspace_with_team_uid(UBER_TEAM_UID);
        app.add_singleton_model(|ctx| UserWorkspaces::mock(vec![uber_workspace], ctx));

        app.update(|ctx| {
            let map = aliases(&[("ai", "aifx agent run claude")]);
            assert_eq!(
                CLIAgent::detect("ai", None, Some(&map), ctx),
                Some(CLIAgent::Claude),
            );
            assert_eq!(
                CLIAgent::detect("ai --flag", None, Some(&map), ctx),
                Some(CLIAgent::Claude),
            );
        });
    });
}

#[test]
fn test_detect_aifx_agent_run_claude_not_on_uber_team() {
    App::test((), |mut app| async move {
        // Register UserWorkspaces with no Uber team membership
        app.add_singleton_model(UserWorkspaces::default_mock);

        app.update(|ctx| {
            assert_eq!(
                CLIAgent::detect("aifx agent run claude", None, None, ctx),
                None,
            );
        });
    });
}

#[test]
fn test_serialized_name_round_trips_known_agents() {
    for agent in enum_iterator::all::<CLIAgent>() {
        let name = agent.to_serialized_name();
        if agent == CLIAgent::Unknown {
            assert_eq!(name, "Unknown");
        } else {
            assert!(!name.is_empty(), "empty serialized name for {agent:?}");
        }
        assert_eq!(
            CLIAgent::from_serialized_name(&name),
            agent,
            "round-trip failed for {agent:?} with serialized name {name:?}",
        );
    }
}

#[test]
fn test_from_serialized_name_falls_back_to_unknown() {
    assert_eq!(CLIAgent::from_serialized_name(""), CLIAgent::Unknown);
    assert_eq!(CLIAgent::from_serialized_name("Gemini"), CLIAgent::Unknown);
    assert_eq!(
        CLIAgent::from_serialized_name("nonexistent"),
        CLIAgent::Unknown
    );
}

#[test]
fn test_detect_aifx_agent_run_claude_wrong_team() {
    App::test((), |mut app| async move {
        let other_workspace = workspace_with_team_uid("some-other-team-uid-01");
        app.add_singleton_model(|ctx| UserWorkspaces::mock(vec![other_workspace], ctx));

        app.update(|ctx| {
            assert_eq!(
                CLIAgent::detect("aifx agent run claude", None, None, ctx),
                None,
            );
        });
    });
}
