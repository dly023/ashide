use warp_cli::agent::Harness;

use super::{
    build_current_app_claude_child_command, build_current_app_opencode_child_command,
    normalize_current_app_child_harness, validate_current_app_harness_shell,
};
use crate::terminal::shell::ShellType;

#[test]
fn normalize_current_app_child_harness_accepts_supported_aliases() {
    assert_eq!(
        normalize_current_app_child_harness("claude"),
        Some(Harness::Claude)
    );
    assert_eq!(
        normalize_current_app_child_harness("claude-code"),
        Some(Harness::Claude)
    );
    assert_eq!(
        normalize_current_app_child_harness("claude_code"),
        Some(Harness::Claude)
    );
    assert_eq!(
        normalize_current_app_child_harness("opencode"),
        Some(Harness::OpenCode)
    );
    assert_eq!(
        normalize_current_app_child_harness("open-code"),
        Some(Harness::OpenCode)
    );
    assert_eq!(
        normalize_current_app_child_harness("open_code"),
        Some(Harness::OpenCode)
    );
}

#[test]
fn normalize_current_app_child_harness_rejects_unsupported_values() {
    assert_eq!(normalize_current_app_child_harness("oz"), None);
    assert_eq!(normalize_current_app_child_harness("codex"), None);
    assert_eq!(normalize_current_app_child_harness(""), None);
}

#[test]
fn validate_current_app_harness_shell_accepts_supported_shells() {
    assert_eq!(
        validate_current_app_harness_shell(Some(ShellType::Bash)),
        Ok(())
    );
    assert_eq!(
        validate_current_app_harness_shell(Some(ShellType::Zsh)),
        Ok(())
    );
    assert_eq!(
        validate_current_app_harness_shell(Some(ShellType::Fish)),
        Ok(())
    );
}

#[test]
fn validate_current_app_harness_shell_rejects_unsupported_shells() {
    assert_eq!(
        validate_current_app_harness_shell(Some(ShellType::PowerShell)),
        Err(
            "Current-app child harnesses currently require bash, zsh, or fish; PowerShell is not supported."
                .to_string()
        )
    );
    assert_eq!(
        validate_current_app_harness_shell(None),
        Err(
            "Current-app child harnesses currently require a detected bash, zsh, or fish session."
                .to_string()
        )
    );
}

#[test]
fn build_current_app_claude_child_command_quotes_the_prompt() {
    let command = build_current_app_claude_child_command("hello world");

    assert!(command.starts_with("claude --session-id "));
    assert!(command.ends_with(" --dangerously-skip-permissions 'hello world'"));
}

#[test]
fn build_current_app_opencode_child_command_quotes_the_prompt() {
    assert_eq!(
        build_current_app_opencode_child_command("hello world"),
        "opencode --prompt 'hello world'"
    );
}
