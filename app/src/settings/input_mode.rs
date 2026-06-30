use crate::terminal::block_list_viewport::InputMode;
use settings::{macros::define_settings_group, Setting, SupportedPlatforms};

define_settings_group!(InputModeSettings, settings: [
    input_mode: InputModeState {
        type: InputMode,
        // Note that for new users, we now override this default value in SettingsInitializer
        // to set it to InputMode::Waterfall.
        default: InputMode::PinnedToBottom,
        supported_platforms: SupportedPlatforms::ALL,
        private: false,
        storage_key: "InputMode",
        toml_path: "appearance.input.input_mode",
        description: "The position of the terminal input.",
    },
]);

impl InputModeSettings {
    pub fn is_pinned_to_top(&self) -> bool {
        *self.input_mode.value() == InputMode::PinnedToTop
    }
}
