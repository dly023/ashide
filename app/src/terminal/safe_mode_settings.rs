use settings::{macros::define_settings_group, Setting, SupportedPlatforms};
use warpui::{AppContext, SingletonEntity};

use crate::{terminal::model::ObfuscateSecrets, workspaces::user_workspaces::UserWorkspaces};

/// How secrets should be displayed in the block list
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
    settings_value::SettingsValue,
)]
#[schemars(
    description = "How detected secrets are visually displayed.",
    rename_all = "snake_case"
)]
pub enum SecretDisplayMode {
    /// Fully obscure secrets with asterisks
    Asterisks,
    /// Show secrets with gray color and strikethrough styling
    #[default]
    Strikethrough,
    /// Show secrets normally with no visual treatment (but are still detected/redacted)
    AlwaysShow,
}

impl SecretDisplayMode {
    /// Convert to the corresponding ObfuscateSecrets enum for visual rendering
    pub fn to_obfuscate_secrets(self) -> ObfuscateSecrets {
        match self {
            SecretDisplayMode::Asterisks => ObfuscateSecrets::Yes,
            SecretDisplayMode::Strikethrough => ObfuscateSecrets::Strikethrough,
            SecretDisplayMode::AlwaysShow => ObfuscateSecrets::AlwaysShow,
        }
    }

    /// Display name for UI
    pub fn display_name(self) -> &'static str {
        match self {
            SecretDisplayMode::Asterisks => "Asterisks",
            SecretDisplayMode::Strikethrough => "Strikethrough",
            SecretDisplayMode::AlwaysShow => "Always show secrets",
        }
    }

    /// Get all available modes for dropdown
    pub fn all_modes() -> [SecretDisplayMode; 3] {
        [
            SecretDisplayMode::Asterisks,
            SecretDisplayMode::Strikethrough,
            SecretDisplayMode::AlwaysShow,
        ]
    }
}

define_settings_group!(SafeModeSettings, settings: [
    safe_mode_enabled: SafeModeEnabled {
        type: bool,
        default: false,
        supported_platforms: SupportedPlatforms::ALL,
        private: false,
        toml_path: "privacy.secret_redaction.enabled",
        description: "Whether secret redaction is enabled to detect and obscure secrets in terminal output.",
    },
    secret_display_mode: SecretDisplayModeSetting {
        type: SecretDisplayMode,
        default: SecretDisplayMode::Strikethrough,
        supported_platforms: SupportedPlatforms::ALL,
        private: false,
        toml_path: "privacy.secret_redaction.secret_display_mode_setting",
        description: "Controls how detected secrets are visually displayed in the terminal.",
    },
]);

/// Returns whether the rendering should obfuscate secrets given the current safe mode settings.
pub fn get_secret_obfuscation_mode(app: &AppContext) -> ObfuscateSecrets {
    let safe_mode_settings = SafeModeSettings::as_ref(app);
    let is_enterprise_secret_redaction_enabled =
        UserWorkspaces::as_ref(app).is_enterprise_secret_redaction_enabled();

    if !is_enterprise_secret_redaction_enabled && !*safe_mode_settings.safe_mode_enabled.value() {
        ObfuscateSecrets::No
    } else {
        safe_mode_settings
            .secret_display_mode
            .value()
            .to_obfuscate_secrets()
    }
}
