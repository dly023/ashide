use settings::{macros::define_settings_group, SupportedPlatforms};

use super::DriveSortOrder;

pub const HAS_AUTO_OPENED_WELCOME_FOLDER: &str = "HasAutoOpenedWelcomeFolder";

define_settings_group!(LocalDriveSettings, settings: [
    sorting_choice: LocalDriveSortingChoice {
        type: DriveSortOrder,
        default: DriveSortOrder::ByObjectType,
        supported_platforms: SupportedPlatforms::ALL,
        private: false,
        toml_path: "local_drive.sorting_choice",
        description: "The sort order for items in Ashide Drive.",
    },
    sharing_onboarding_block_shown: LocalDriveSharingOnboardingBlockShown {
        type: bool,
        default: false,
        supported_platforms: SupportedPlatforms::ALL,
        private: true,
    },
    // Controls whether Ashide Drive appears in the tools panel, command palette, and command search.
    enable_local_drive: EnableLocalDrive {
        type: bool,
        default: true,
        supported_platforms: SupportedPlatforms::ALL,
        private: false,
        toml_path: "local_drive.enabled",
        description: "Whether Ashide Drive is enabled.",
    },
]);

impl LocalDriveSettings {
    /// Returns whether Ashide Drive should be considered enabled.
    pub fn is_local_drive_enabled(app: &warpui::AppContext) -> bool {
        use warpui::SingletonEntity as _;
        *Self::as_ref(app).enable_local_drive
    }
}
