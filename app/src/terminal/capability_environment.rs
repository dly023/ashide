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

pub(crate) fn terminal_capability_environment_variables_to_remove() -> &'static [&'static str] {
    &["NO_COLOR"]
}

#[cfg(test)]
mod tests {
    use super::{
        terminal_capability_environment_variables,
        terminal_capability_environment_variables_to_remove,
    };

    #[test]
    fn terminal_capability_environment_enables_color_by_default() {
        let environment_variables = terminal_capability_environment_variables();

        assert_eq!(
            environment_variables.get("TERM").map(String::as_str),
            Some("xterm-256color")
        );
        assert_eq!(
            environment_variables.get("COLORTERM").map(String::as_str),
            Some("truecolor")
        );
    }

    #[test]
    fn terminal_capability_environment_strips_inherited_color_disable_flags() {
        assert!(
            terminal_capability_environment_variables_to_remove().contains(&"NO_COLOR"),
            "terminal sessions should not inherit process-level no-color defaults"
        );
        assert!(
            !terminal_capability_environment_variables().contains_key("NO_COLOR"),
            "terminal capability overrides must enable color rather than explicitly disable it"
        );
    }
}
