use super::*;
use clap::Parser;

use crate::agent::{AgentCommand, Harness};
use crate::memory::{MemoryCommand, MemoryKind};
// Ashide hard-cut:`environment` CLI 随 cloud ambient agent 主体物理删。

#[test]
fn agent_run_accepts_model() {
    let args = Args::try_parse_from([
        "warp", "agent", "run", "--prompt", "hello", "--model", "gpt-4o",
    ])
    .unwrap();

    let Some(Command::CommandLine(boxed_cmd)) = args.command else {
        panic!("Expected `warp agent run` command");
    };
    let CliCommand::Agent(AgentCommand::Run(run_args)) = boxed_cmd.as_ref() else {
        panic!("Expected `warp agent run` command");
    };

    assert_eq!(run_args.model.model.as_deref(), Some("gpt-4o"));
}

#[test]
fn model_list_parses() {
    let args = Args::try_parse_from(["warp", "model", "list"]).unwrap();

    let Some(Command::CommandLine(boxed_cmd)) = args.command else {
        panic!("Expected `warp model list` command");
    };
    let CliCommand::Model(model_cmd) = boxed_cmd.as_ref() else {
        panic!("Expected `warp model` command");
    };

    assert!(matches!(model_cmd, crate::model::ModelCommand::List));
}

#[test]
fn session_bridge_list_parses() {
    let args = Args::try_parse_from(["oz", "session-bridge", "list"]).unwrap();

    let Some(Command::CommandLine(boxed_cmd)) = args.command else {
        panic!("Expected `oz session-bridge list` command");
    };
    let CliCommand::SessionBridge(session_bridge_cmd) = boxed_cmd.as_ref() else {
        panic!("Expected `oz session-bridge` command");
    };

    assert!(matches!(
        session_bridge_cmd,
        crate::session_bridge::SessionBridgeCommand::List
    ));
}

#[test]
fn session_bridge_export_parses_required_session_and_optional_out_dry_run() {
    let args = Args::try_parse_from([
        "oz",
        "session-bridge",
        "export",
        "--session",
        "abc-123",
        "--out",
        "exports/session.json",
        "--dry-run",
    ])
    .unwrap();

    let Some(Command::CommandLine(boxed_cmd)) = args.command else {
        panic!("Expected `oz session-bridge export` command");
    };
    let CliCommand::SessionBridge(crate::session_bridge::SessionBridgeCommand::Export(export_args)) =
        boxed_cmd.as_ref()
    else {
        panic!("Expected `oz session-bridge export` command");
    };

    assert_eq!(export_args.session, "abc-123");
    assert_eq!(
        export_args.out.as_ref().and_then(|path| path.to_str()),
        Some("exports/session.json")
    );
    assert!(export_args.dry_run);
}

#[test]
fn session_bridge_export_requires_session() {
    let result = Args::try_parse_from(["oz", "session-bridge", "export", "--dry-run"]);
    assert!(result.is_err());
}

#[test]
fn session_bridge_fork_parses_session_new_session_and_dry_run() {
    let args = Args::try_parse_from([
        "ashide",
        "session-bridge",
        "fork",
        "--session",
        "abc-123",
        "--new-session",
        "fork-456",
        "--dry-run",
    ])
    .unwrap();

    let Some(Command::CommandLine(boxed_cmd)) = args.command else {
        panic!("Expected `ashide session-bridge fork` command");
    };
    let CliCommand::SessionBridge(crate::session_bridge::SessionBridgeCommand::Fork(fork_args)) =
        boxed_cmd.as_ref()
    else {
        panic!("Expected `ashide session-bridge fork` command");
    };

    assert_eq!(fork_args.session, "abc-123");
    assert_eq!(fork_args.new_session.as_deref(), Some("fork-456"));
    assert!(fork_args.dry_run);
}

#[test]
fn session_bridge_edit_parses_redact_and_trim() {
    let args = Args::try_parse_from([
        "ashide",
        "session-bridge",
        "edit",
        "--session",
        "abc-123",
        "--redact",
        "secret",
        "--redact",
        "token",
        "--trim-after",
        "3",
        "--new-session",
        "edited-456",
        "--dry-run",
    ])
    .unwrap();

    let Some(Command::CommandLine(boxed_cmd)) = args.command else {
        panic!("Expected `ashide session-bridge edit` command");
    };
    let CliCommand::SessionBridge(crate::session_bridge::SessionBridgeCommand::Edit(edit_args)) =
        boxed_cmd.as_ref()
    else {
        panic!("Expected `ashide session-bridge edit` command");
    };

    assert_eq!(edit_args.session, "abc-123");
    assert_eq!(edit_args.redact, vec!["secret", "token"]);
    assert_eq!(edit_args.trim_after, Some(3));
    assert_eq!(edit_args.new_session.as_deref(), Some("edited-456"));
    assert!(edit_args.dry_run);
}

#[test]
fn session_bridge_import_parses_bundle_new_session_and_dry_run() {
    let args = Args::try_parse_from([
        "ashide",
        "session-bridge",
        "import",
        "--bundle",
        "exports/session.json",
        "--new-session",
        "00000000-0000-4000-8000-000000000001",
        "--dry-run",
    ])
    .unwrap();

    let Some(Command::CommandLine(boxed_cmd)) = args.command else {
        panic!("Expected `ashide session-bridge import` command");
    };
    let CliCommand::SessionBridge(crate::session_bridge::SessionBridgeCommand::Import(import_args)) =
        boxed_cmd.as_ref()
    else {
        panic!("Expected `ashide session-bridge import` command");
    };

    assert_eq!(
        import_args.bundle.as_path().to_str(),
        Some("exports/session.json")
    );
    assert_eq!(
        import_args.new_session.as_deref(),
        Some("00000000-0000-4000-8000-000000000001")
    );
    assert!(import_args.dry_run);
}

#[test]
fn remember_parses_implicit_fact_text() {
    let args =
        Args::try_parse_from(["ashide", "remember", "Memory", "lives", "in", ".agents"]).unwrap();

    let Some(Command::CommandLine(boxed_cmd)) = args.command else {
        panic!("Expected `ashide remember` command");
    };
    let CliCommand::Remember(remember_args) = boxed_cmd.as_ref() else {
        panic!("Expected `ashide remember` command");
    };

    let write_args = remember_args.clone().into_write_args().unwrap();
    assert_eq!(write_args.kind, MemoryKind::Fact);
    assert_eq!(write_args.text, "Memory lives in .agents");
}

#[test]
fn remember_parses_kind_prefix() {
    let args = Args::try_parse_from([
        "ashide",
        "remember",
        "decision",
        "SQLite WAL is the memory source of truth",
    ])
    .unwrap();

    let Some(Command::CommandLine(boxed_cmd)) = args.command else {
        panic!("Expected `ashide remember` command");
    };
    let CliCommand::Remember(remember_args) = boxed_cmd.as_ref() else {
        panic!("Expected `ashide remember` command");
    };

    let write_args = remember_args.clone().into_write_args().unwrap();
    assert_eq!(write_args.kind, MemoryKind::Decision);
    assert_eq!(write_args.text, "SQLite WAL is the memory source of truth");
}

#[test]
fn recall_parses_default_focused_query() {
    let args = Args::try_parse_from([
        "ashide", "recall", "--top-k", "3", "SQLite", "WAL", "--json",
    ])
    .unwrap();

    let Some(Command::CommandLine(boxed_cmd)) = args.command else {
        panic!("Expected `ashide recall` command");
    };
    let CliCommand::Recall(recall_args) = boxed_cmd.as_ref() else {
        panic!("Expected `ashide recall` command");
    };

    assert_eq!(recall_args.tier().as_str(), "focused");
    assert_eq!(recall_args.top_k, 3);
    assert_eq!(recall_args.query_text().as_deref(), Some("SQLite WAL"));
    assert!(recall_args.json);
}

#[test]
fn memory_status_write_and_recall_parse() {
    let status_args = Args::try_parse_from(["ashide", "memory", "status"]).unwrap();
    let Some(Command::CommandLine(boxed_cmd)) = status_args.command else {
        panic!("Expected `ashide memory status` command");
    };
    assert!(matches!(
        boxed_cmd.as_ref(),
        CliCommand::Memory(MemoryCommand::Status)
    ));

    let write_args = Args::try_parse_from([
        "ashide",
        "memory",
        "write",
        "--kind",
        "decision",
        "--text",
        "Do not mutate AGENTS.md",
        "--source",
        "file:docs/shared-agent-memory-architecture.md",
    ])
    .unwrap();
    let Some(Command::CommandLine(boxed_cmd)) = write_args.command else {
        panic!("Expected `ashide memory write` command");
    };
    let CliCommand::Memory(MemoryCommand::Write(write_args)) = boxed_cmd.as_ref() else {
        panic!("Expected `ashide memory write` command");
    };
    assert_eq!(write_args.kind, MemoryKind::Decision);
    assert_eq!(write_args.text, "Do not mutate AGENTS.md");
    assert_eq!(
        write_args.source.as_deref(),
        Some("file:docs/shared-agent-memory-architecture.md")
    );

    let recall_args =
        Args::try_parse_from(["ashide", "memory", "recall", "--full", "AGENTS.md"]).unwrap();
    let Some(Command::CommandLine(boxed_cmd)) = recall_args.command else {
        panic!("Expected `ashide memory recall` command");
    };
    let CliCommand::Memory(MemoryCommand::Recall(recall_args)) = boxed_cmd.as_ref() else {
        panic!("Expected `ashide memory recall` command");
    };
    assert_eq!(recall_args.tier().as_str(), "full");
    assert_eq!(recall_args.query_text().as_deref(), Some("AGENTS.md"));

    let context_args = Args::try_parse_from([
        "ashide",
        "memory",
        "context",
        "--focused",
        "--top-k",
        "5",
        "--token-budget",
        "900",
        "--task",
        "continue SessionBridge",
        "SessionBridge",
        "memory",
    ])
    .unwrap();
    let Some(Command::CommandLine(boxed_cmd)) = context_args.command else {
        panic!("Expected `ashide memory context` command");
    };
    let CliCommand::Memory(MemoryCommand::Context(context_args)) = boxed_cmd.as_ref() else {
        panic!("Expected `ashide memory context` command");
    };
    assert_eq!(context_args.recall.tier().as_str(), "focused");
    assert_eq!(context_args.recall.top_k, 5);
    assert_eq!(context_args.token_budget, 900);
    assert_eq!(context_args.task.as_deref(), Some("continue SessionBridge"));
    assert_eq!(
        context_args.recall.query_text().as_deref(),
        Some("SessionBridge memory")
    );

    let preview_args = Args::try_parse_from([
        "ashide",
        "memory",
        "preview",
        "--full",
        "--top-k",
        "6",
        "--token-budget",
        "1000",
        "--evidence-limit",
        "3",
        "--task",
        "editor recall",
        "ContextPacket",
    ])
    .unwrap();
    let Some(Command::CommandLine(boxed_cmd)) = preview_args.command else {
        panic!("Expected `ashide memory preview` command");
    };
    let CliCommand::Memory(MemoryCommand::Preview(preview_args)) = boxed_cmd.as_ref() else {
        panic!("Expected `ashide memory preview` command");
    };
    assert_eq!(preview_args.recall.tier().as_str(), "full");
    assert_eq!(preview_args.recall.top_k, 6);
    assert_eq!(preview_args.token_budget, 1000);
    assert_eq!(preview_args.evidence_limit, 3);
    assert_eq!(preview_args.task.as_deref(), Some("editor recall"));
    assert_eq!(
        preview_args.recall.query_text().as_deref(),
        Some("ContextPacket")
    );

    let evidence_args =
        Args::try_parse_from(["ashide", "memory", "evidence", "--limit", "2", "--json"]).unwrap();
    let Some(Command::CommandLine(boxed_cmd)) = evidence_args.command else {
        panic!("Expected `ashide memory evidence` command");
    };
    let CliCommand::Memory(MemoryCommand::Evidence(evidence_args)) = boxed_cmd.as_ref() else {
        panic!("Expected `ashide memory evidence` command");
    };
    assert_eq!(evidence_args.limit, 2);
    assert!(evidence_args.json);

    let mcp_server_args = Args::try_parse_from(["ashide", "memory", "mcp-server"]).unwrap();
    let Some(Command::CommandLine(boxed_cmd)) = mcp_server_args.command else {
        panic!("Expected `ashide memory mcp-server` command");
    };
    assert!(matches!(
        boxed_cmd.as_ref(),
        CliCommand::Memory(MemoryCommand::McpServer)
    ));
}

#[test]
fn agent_run_accepts_file() {
    let args = Args::try_parse_from([
        "warp",
        "agent",
        "run",
        "--prompt",
        "hello",
        "--file",
        "config.yaml",
    ])
    .unwrap();

    let Some(Command::CommandLine(boxed_cmd)) = args.command else {
        panic!("Expected `warp agent run` command");
    };
    let CliCommand::Agent(AgentCommand::Run(run_args)) = boxed_cmd.as_ref() else {
        panic!("Expected `warp agent run` command");
    };

    assert_eq!(
        run_args.config_file.file.as_ref().and_then(|p| p.to_str()),
        Some("config.yaml")
    );
}

#[test]
fn agent_run_accepts_idle_on_complete_flag() {
    let args = Args::try_parse_from([
        "warp",
        "agent",
        "run",
        "--prompt",
        "hello",
        "--idle-on-complete",
    ])
    .unwrap();

    let Some(Command::CommandLine(boxed_cmd)) = args.command else {
        panic!("Expected `warp agent run` command");
    };
    let CliCommand::Agent(AgentCommand::Run(run_args)) = boxed_cmd.as_ref() else {
        panic!("Expected `warp agent run` command");
    };

    assert_eq!(
        run_args.idle_on_complete,
        Some(humantime::Duration::from(std::time::Duration::from_secs(
            45 * 60
        )))
    );
}

#[test]
fn agent_run_accepts_idle_on_complete_duration() {
    let args = Args::try_parse_from([
        "warp",
        "agent",
        "run",
        "--prompt",
        "hello",
        "--idle-on-complete",
        "10m",
    ])
    .unwrap();

    let Some(Command::CommandLine(boxed_cmd)) = args.command else {
        panic!("Expected `warp agent run` command");
    };
    let CliCommand::Agent(AgentCommand::Run(run_args)) = boxed_cmd.as_ref() else {
        panic!("Expected `warp agent run` command");
    };

    assert_eq!(
        run_args.idle_on_complete,
        Some(humantime::Duration::from(std::time::Duration::from_secs(
            10 * 60
        )))
    );
}

#[test]
fn agent_run_rejects_without_prompt_or_skill() {
    let result = Args::try_parse_from(["warp", "agent", "run", "--model", "gpt-4o"]);
    assert!(result.is_err());
    let err = result.unwrap_err();
    let err_str = err.to_string();
    assert!(err_str.contains("prompt_group") || err_str.contains("required"));
}

#[test]
fn agent_run_accepts_prompt_only() {
    let args = Args::try_parse_from(["warp", "agent", "run", "--prompt", "hello"]).unwrap();

    let Some(Command::CommandLine(boxed_cmd)) = args.command else {
        panic!("Expected `warp agent run` command");
    };
    let CliCommand::Agent(AgentCommand::Run(run_args)) = boxed_cmd.as_ref() else {
        panic!("Expected `warp agent run` command");
    };

    assert_eq!(run_args.prompt_arg.prompt.as_deref(), Some("hello"));
    assert!(run_args.prompt_arg.saved_prompt.is_none());
    assert!(run_args.skill.is_none());
}

#[test]
fn agent_run_accepts_saved_prompt_only() {
    let args = Args::try_parse_from(["warp", "agent", "run", "--saved-prompt", "sp-123"]).unwrap();

    let Some(Command::CommandLine(boxed_cmd)) = args.command else {
        panic!("Expected `warp agent run` command");
    };
    let CliCommand::Agent(AgentCommand::Run(run_args)) = boxed_cmd.as_ref() else {
        panic!("Expected `warp agent run` command");
    };

    assert!(run_args.prompt_arg.prompt.is_none());
    assert_eq!(run_args.prompt_arg.saved_prompt.as_deref(), Some("sp-123"));
    assert!(run_args.skill.is_none());
}

#[test]
fn agent_run_accepts_skill_only() {
    let args = Args::try_parse_from(["warp", "agent", "run", "--skill", "my-skill"]).unwrap();

    let Some(Command::CommandLine(boxed_cmd)) = args.command else {
        panic!("Expected `warp agent run` command");
    };
    let CliCommand::Agent(AgentCommand::Run(run_args)) = boxed_cmd.as_ref() else {
        panic!("Expected `warp agent run` command");
    };

    assert!(run_args.prompt_arg.prompt.is_none());
    assert!(run_args.skill.is_some());
}

#[test]
fn agent_run_accepts_prompt_and_skill() {
    let args = Args::try_parse_from([
        "warp", "agent", "run", "--prompt", "do stuff", "--skill", "my-skill",
    ])
    .unwrap();

    let Some(Command::CommandLine(boxed_cmd)) = args.command else {
        panic!("Expected `warp agent run` command");
    };
    let CliCommand::Agent(AgentCommand::Run(run_args)) = boxed_cmd.as_ref() else {
        panic!("Expected `warp agent run` command");
    };

    assert_eq!(run_args.prompt_arg.prompt.as_deref(), Some("do stuff"));
    assert!(run_args.skill.is_some());
}

#[test]
fn agent_run_accepts_saved_prompt_and_skill() {
    let args = Args::try_parse_from([
        "warp",
        "agent",
        "run",
        "--saved-prompt",
        "sp-1",
        "--skill",
        "my-skill",
    ])
    .unwrap();

    let Some(Command::CommandLine(boxed_cmd)) = args.command else {
        panic!("Expected `warp agent run` command");
    };
    let CliCommand::Agent(AgentCommand::Run(run_args)) = boxed_cmd.as_ref() else {
        panic!("Expected `warp agent run` command");
    };

    assert_eq!(run_args.prompt_arg.saved_prompt.as_deref(), Some("sp-1"));
    assert!(run_args.skill.is_some());
}

#[test]
fn agent_run_rejects_prompt_and_saved_prompt() {
    let result = Args::try_parse_from([
        "warp",
        "agent",
        "run",
        "--prompt",
        "hello",
        "--saved-prompt",
        "sp-1",
    ]);
    assert!(result.is_err());
}

#[test]
fn run_command_is_removed() {
    let result = Args::try_parse_from(["warp", "run", "message"]);
    assert!(result.is_err());
}

// Ashide hard-cut:environment_image_list_parses / environment_create_accepts_description /
// environment_create_description_max_length / environment_update_accepts_description /
// environment_update_accepts_remove_description 随 cloud ambient agent 主体子系统物理删。

#[test]
fn agent_run_accepts_computer_use_flag() {
    let args = Args::try_parse_from([
        "warp",
        "agent",
        "run",
        "--prompt",
        "hello",
        "--computer-use",
    ])
    .unwrap();

    let Some(Command::CommandLine(boxed_cmd)) = args.command else {
        panic!("Expected `warp agent run` command");
    };
    let CliCommand::Agent(AgentCommand::Run(run_args)) = boxed_cmd.as_ref() else {
        panic!("Expected `warp agent run` command");
    };

    assert!(run_args.computer_use.computer_use);
    assert!(!run_args.computer_use.no_computer_use);
    assert_eq!(run_args.computer_use.computer_use_override(), Some(true));
}

#[test]
fn agent_run_accepts_no_computer_use_flag() {
    let args = Args::try_parse_from([
        "warp",
        "agent",
        "run",
        "--prompt",
        "hello",
        "--no-computer-use",
    ])
    .unwrap();

    let Some(Command::CommandLine(boxed_cmd)) = args.command else {
        panic!("Expected `warp agent run` command");
    };
    let CliCommand::Agent(AgentCommand::Run(run_args)) = boxed_cmd.as_ref() else {
        panic!("Expected `warp agent run` command");
    };

    assert!(!run_args.computer_use.computer_use);
    assert!(run_args.computer_use.no_computer_use);
    assert_eq!(run_args.computer_use.computer_use_override(), Some(false));
}

#[test]
fn agent_run_rejects_both_computer_use_flags() {
    let result = Args::try_parse_from([
        "warp",
        "agent",
        "run",
        "--prompt",
        "hello",
        "--computer-use",
        "--no-computer-use",
    ]);

    assert!(result.is_err());
}

#[test]
fn agent_run_defaults_to_no_computer_use_override() {
    let args = Args::try_parse_from(["warp", "agent", "run", "--prompt", "hello"]).unwrap();

    let Some(Command::CommandLine(boxed_cmd)) = args.command else {
        panic!("Expected `warp agent run` command");
    };
    let CliCommand::Agent(AgentCommand::Run(run_args)) = boxed_cmd.as_ref() else {
        panic!("Expected `warp agent run` command");
    };

    assert!(!run_args.computer_use.computer_use);
    assert!(!run_args.computer_use.no_computer_use);
    assert_eq!(run_args.computer_use.computer_use_override(), None);
}
#[test]
fn harness_parse_orchestration_harness_accepts_aliases() {
    assert_eq!(
        Harness::parse_orchestration_harness("claude-code"),
        Some(Harness::Claude)
    );
    assert_eq!(
        Harness::parse_orchestration_harness("open_code"),
        Some(Harness::OpenCode)
    );
}

#[test]
fn harness_parse_current_app_child_harness_rejects_oz() {
    assert_eq!(Harness::parse_current_app_child_harness("oz"), None);
    assert_eq!(
        Harness::parse_current_app_child_harness("opencode"),
        Some(Harness::OpenCode)
    );
}
