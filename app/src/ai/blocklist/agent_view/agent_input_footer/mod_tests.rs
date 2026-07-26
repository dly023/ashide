use super::*;
use crate::terminal::cli_agent_sessions::{
    CLIAgentInputState, CLIAgentSessionContext, CLIAgentSessionStatus,
};

#[test]
fn plugin_chip_policy_scopes_custom_runtime_session_by_environment_host() {
    let session = CLIAgentSession {
        agent: CLIAgent::Codex,
        status: CLIAgentSessionStatus::InProgress,
        session_context: CLIAgentSessionContext::default(),
        input_state: CLIAgentInputState::Closed,
        should_auto_toggle_input: false,
        listener: None,
        plugin_version: None,
        environment_host_key: Some("container:devbox".to_owned()),
        draft_text: None,
        custom_command_prefix: None,
    };

    assert_eq!(
        plugin_chip_key_for_session(&session),
        "codex@container:devbox",
        "plugin dismissal/install policy must stay scoped to the runtime host carried by the registered session model"
    );
}
