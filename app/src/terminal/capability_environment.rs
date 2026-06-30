use std::collections::HashMap;

use warp_core::channel::ChannelState;
use warp_core::features::FeatureFlag;

use crate::terminal::cli_agent_sessions::event::current_protocol_version;

pub(crate) fn terminal_capability_environment_variables() -> HashMap<String, String> {
    let mut environment_variables = HashMap::new();
    environment_variables.insert("TERM".to_string(), "xterm-256color".to_string());
    environment_variables.insert("TERM_PROGRAM".to_string(), "WarpTerminal".to_string());
    environment_variables.insert("COLORTERM".to_string(), "truecolor".to_string());

    if let Some(version) = ChannelState::app_version() {
        environment_variables.insert("TERM_PROGRAM_VERSION".to_string(), version.to_string());
        environment_variables.insert("WARP_CLIENT_VERSION".to_string(), version.to_string());
    } else {
        environment_variables.insert("WARP_CLIENT_VERSION".to_string(), "local".to_string());
    }

    if FeatureFlag::HOANotifications.is_enabled() {
        environment_variables.insert(
            "WARP_CLI_AGENT_PROTOCOL_VERSION".to_string(),
            current_protocol_version().to_string(),
        );
    }

    environment_variables
}
