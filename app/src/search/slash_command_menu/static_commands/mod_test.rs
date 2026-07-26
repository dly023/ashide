use super::Availability;

/// Helper: constructs a session context for an agent view in a current-app environment session with a repo,
/// no active LRC, and an active conversation.
fn current_app_agent_view_with_repo() -> Availability {
    Availability::AGENT_VIEW
        | Availability::CURRENT_APP_ENVIRONMENT
        | Availability::REPOSITORY
        | Availability::NO_LRC_CONTROL
        | Availability::ACTIVE_CONVERSATION
}

/// Helper: constructs a session context for a terminal view in a current-app environment session with a repo,
/// no active LRC, and an active conversation.
fn current_app_terminal_view_with_repo() -> Availability {
    Availability::TERMINAL_VIEW
        | Availability::CURRENT_APP_ENVIRONMENT
        | Availability::REPOSITORY
        | Availability::NO_LRC_CONTROL
        | Availability::ACTIVE_CONVERSATION
}

// --- ALWAYS ---

#[test]
fn always_available_in_any_context() {
    let command = Availability::ALWAYS;
    assert!(current_app_agent_view_with_repo().contains(command));
    assert!(current_app_terminal_view_with_repo().contains(command));
    // Even a minimal Environment Runtime session satisfies ALWAYS.
    assert!(Availability::AGENT_VIEW.contains(command));
}

// --- View flag tests ---

#[test]
fn no_view_requirement_available_in_any_view() {
    let command = Availability::ALWAYS;
    assert!(current_app_agent_view_with_repo().contains(command));
    assert!(current_app_terminal_view_with_repo().contains(command));
}

#[test]
fn agent_view_requirement_only_in_agent_view() {
    let command = Availability::AGENT_VIEW;
    assert!(current_app_agent_view_with_repo().contains(command));
    assert!(!current_app_terminal_view_with_repo().contains(command));
}

#[test]
fn terminal_view_requirement_only_in_terminal_view() {
    let command = Availability::TERMINAL_VIEW;
    assert!(!current_app_agent_view_with_repo().contains(command));
    assert!(current_app_terminal_view_with_repo().contains(command));
}

#[test]
fn both_view_bits_satisfy_either_view_requirement() {
    // When AgentView feature flag is disabled, both view bits are set.
    let session = Availability::AGENT_VIEW
        | Availability::TERMINAL_VIEW
        | Availability::CURRENT_APP_ENVIRONMENT;
    assert!(session.contains(Availability::AGENT_VIEW));
    assert!(session.contains(Availability::TERMINAL_VIEW));
    assert!(session.contains(Availability::ALWAYS));
}

// --- Repository flag tests ---

#[test]
fn repository_requirement_satisfied_when_in_repo() {
    let command = Availability::REPOSITORY;
    let session =
        Availability::AGENT_VIEW | Availability::CURRENT_APP_ENVIRONMENT | Availability::REPOSITORY;
    assert!(session.contains(command));
}

#[test]
fn repository_requirement_not_satisfied_when_not_in_repo() {
    let command = Availability::REPOSITORY;
    let session = Availability::AGENT_VIEW | Availability::CURRENT_APP_ENVIRONMENT;
    assert!(!session.contains(command));
}

#[test]
fn no_repository_requirement_available_regardless() {
    let command = Availability::ALWAYS;
    let session_with_repo =
        Availability::AGENT_VIEW | Availability::CURRENT_APP_ENVIRONMENT | Availability::REPOSITORY;
    let session_without_repo = Availability::AGENT_VIEW | Availability::CURRENT_APP_ENVIRONMENT;
    assert!(session_with_repo.contains(command));
    assert!(session_without_repo.contains(command));
}

// --- CURRENT_APP_ENVIRONMENT flag tests ---

#[test]
fn current_app_environment_requirement_satisfied_in_current_app_environment_session() {
    let command = Availability::CURRENT_APP_ENVIRONMENT;
    let session = Availability::AGENT_VIEW | Availability::CURRENT_APP_ENVIRONMENT;
    assert!(session.contains(command));
}

#[test]
fn current_app_environment_requirement_not_satisfied_in_environment_runtime_session() {
    let command = Availability::CURRENT_APP_ENVIRONMENT;
    let session = Availability::AGENT_VIEW; // Environment Runtime: no CURRENT_APP_ENVIRONMENT flag
    assert!(!session.contains(command));
}

#[test]
fn no_current_app_environment_requirement_available_in_any_session_type() {
    let command = Availability::ALWAYS;
    let current_app_environment_session =
        Availability::AGENT_VIEW | Availability::CURRENT_APP_ENVIRONMENT;
    let environment_runtime_session = Availability::AGENT_VIEW;
    assert!(current_app_environment_session.contains(command));
    assert!(environment_runtime_session.contains(command));
}

// --- NO_LRC_CONTROL flag tests ---

#[test]
fn no_lrc_control_requirement_satisfied_when_not_in_control() {
    let command = Availability::NO_LRC_CONTROL;
    let session = Availability::AGENT_VIEW | Availability::NO_LRC_CONTROL;
    assert!(session.contains(command));
}

#[test]
fn no_lrc_control_requirement_not_satisfied_when_in_control() {
    let command = Availability::NO_LRC_CONTROL;
    let session = Availability::AGENT_VIEW; // agent is in control: no NO_LRC_CONTROL flag
    assert!(!session.contains(command));
}

// --- ACTIVE_CONVERSATION flag tests ---

#[test]
fn active_conversation_requirement_satisfied_when_conversation_active() {
    let command = Availability::ACTIVE_CONVERSATION;
    let session = Availability::AGENT_VIEW | Availability::ACTIVE_CONVERSATION;
    assert!(session.contains(command));
}

#[test]
fn active_conversation_requirement_not_satisfied_when_no_conversation() {
    let command = Availability::ACTIVE_CONVERSATION;
    let session = Availability::AGENT_VIEW;
    assert!(!session.contains(command));
}

// --- AI_ENABLED flag tests ---

#[test]
fn ai_enabled_requirement_satisfied_when_ai_on() {
    let command = Availability::AI_ENABLED;
    let session = Availability::AGENT_VIEW | Availability::AI_ENABLED;
    assert!(session.contains(command));
}

#[test]
fn ai_enabled_requirement_not_satisfied_when_ai_off() {
    let command = Availability::AI_ENABLED;
    let session = Availability::AGENT_VIEW;
    assert!(!session.contains(command));
}

#[test]
fn commands_without_ai_enabled_remain_available_when_ai_off() {
    // Commands like `/open-file`, `/rename-tab`, `/changelog` only set session-context bits.
    // With AI off, `session_context` has no `AI_ENABLED` bit, but these should still match.
    let command_current_app_environment = Availability::CURRENT_APP_ENVIRONMENT;
    let command_always = Availability::ALWAYS;
    let session_ai_off = Availability::TERMINAL_VIEW | Availability::CURRENT_APP_ENVIRONMENT;
    assert!(session_ai_off.contains(command_current_app_environment));
    assert!(session_ai_off.contains(command_always));
}

// --- Combined flag tests ---

#[test]
fn agent_view_and_repository_both_required() {
    let command = Availability::AGENT_VIEW | Availability::REPOSITORY;

    // Agent view + repo → available
    let session =
        Availability::AGENT_VIEW | Availability::CURRENT_APP_ENVIRONMENT | Availability::REPOSITORY;
    assert!(session.contains(command));

    // Terminal view + repo → not available (missing AGENT_VIEW)
    let session = Availability::TERMINAL_VIEW
        | Availability::CURRENT_APP_ENVIRONMENT
        | Availability::REPOSITORY;
    assert!(!session.contains(command));

    // Agent view, no repo → not available (missing REPOSITORY)
    let session = Availability::AGENT_VIEW | Availability::CURRENT_APP_ENVIRONMENT;
    assert!(!session.contains(command));
}

#[test]
fn agent_view_and_current_app_environment_both_required() {
    let command = Availability::AGENT_VIEW | Availability::CURRENT_APP_ENVIRONMENT;

    // Agent view + current app environment → available
    let session = Availability::AGENT_VIEW | Availability::CURRENT_APP_ENVIRONMENT;
    assert!(session.contains(command));

    // Agent view without current app environment → not available
    let session = Availability::AGENT_VIEW;
    assert!(!session.contains(command));

    // Terminal view + current app environment → not available (missing AGENT_VIEW)
    let session = Availability::TERMINAL_VIEW | Availability::CURRENT_APP_ENVIRONMENT;
    assert!(!session.contains(command));
}

#[test]
fn fork_like_command_requires_agent_view_active_conversation_no_lrc() {
    let command =
        Availability::AGENT_VIEW | Availability::ACTIVE_CONVERSATION | Availability::NO_LRC_CONTROL;

    // All conditions met → available
    let session = Availability::AGENT_VIEW
        | Availability::ACTIVE_CONVERSATION
        | Availability::NO_LRC_CONTROL
        | Availability::CURRENT_APP_ENVIRONMENT;
    assert!(session.contains(command));

    // Agent in control of LRC → not available
    let session = Availability::AGENT_VIEW | Availability::ACTIVE_CONVERSATION;
    assert!(!session.contains(command));

    // No active conversation → not available
    let session = Availability::AGENT_VIEW | Availability::NO_LRC_CONTROL;
    assert!(!session.contains(command));

    // Wrong view → not available
    let session = Availability::TERMINAL_VIEW
        | Availability::ACTIVE_CONVERSATION
        | Availability::NO_LRC_CONTROL;
    assert!(!session.contains(command));
}

#[test]
fn all_flags_required() {
    let command = Availability::AGENT_VIEW
        | Availability::CURRENT_APP_ENVIRONMENT
        | Availability::REPOSITORY
        | Availability::NO_LRC_CONTROL
        | Availability::ACTIVE_CONVERSATION;

    let full_session = current_app_agent_view_with_repo();
    assert!(full_session.contains(command));

    // Missing any single flag → not available
    let missing_current_app_environment = Availability::AGENT_VIEW
        | Availability::REPOSITORY
        | Availability::NO_LRC_CONTROL
        | Availability::ACTIVE_CONVERSATION;
    assert!(!missing_current_app_environment.contains(command));

    let missing_repo = Availability::AGENT_VIEW
        | Availability::CURRENT_APP_ENVIRONMENT
        | Availability::NO_LRC_CONTROL
        | Availability::ACTIVE_CONVERSATION;
    assert!(!missing_repo.contains(command));

    let missing_view = Availability::CURRENT_APP_ENVIRONMENT
        | Availability::REPOSITORY
        | Availability::NO_LRC_CONTROL
        | Availability::ACTIVE_CONVERSATION;
    assert!(!missing_view.contains(command));
}
