use warpui::{App, SingletonEntity};

use super::Prompt;
use crate::auth::AuthStateProvider;
use crate::settings::WarpPromptSeparator;
use crate::test_util::settings::initialize_settings_for_tests;
use crate::workspaces::user_workspaces::UserWorkspaces;
use crate::{
    context_chips::{
        prompt::{PromptConfiguration, PromptSelection},
        ContextChipKind,
    },
    terminal::session_settings::SessionSettings,
};

fn initialize_app(app: &mut App) {
    initialize_settings_for_tests(app);
    app.add_singleton_model(UserWorkspaces::default_mock);
    app.add_singleton_model(|_| AuthStateProvider::new_for_test());
}

#[test]
fn test_prompt_settings() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let session_settings = SessionSettings::handle(&app);
        let default_prompt = PromptConfiguration::default_prompt();

        let prompt = app.add_singleton_model(Prompt::new);

        // First, the default prompt should be set.
        let current_prompt_chips = prompt.read(&app, |prompt, _| prompt.chip_kinds());
        assert_eq!(current_prompt_chips, default_prompt.chip_kinds());
        session_settings.read(&app, |settings, _| {
            assert_eq!(settings.saved_prompt.to_owned(), PromptSelection::Default)
        });

        // Now, set a new prompt.
        let new_chips = [ContextChipKind::Ssh, ContextChipKind::WorkingDirectory];
        prompt.update(&mut app, |prompt, ctx| {
            prompt
                .update(new_chips.clone(), false, WarpPromptSeparator::None, ctx)
                .expect("Saving prompt failed")
        });

        // The configuration should be updated both in-memory and in settings.
        let new_prompt_chips = prompt.read(&app, |prompt, _| prompt.chip_kinds());
        assert_eq!(
            new_prompt_chips,
            vec![ContextChipKind::Ssh, ContextChipKind::WorkingDirectory]
        );
        session_settings.read(&app, |settings, _| {
            assert_eq!(
                settings.saved_prompt.to_owned(),
                PromptConfiguration::from_chips(new_chips, false, WarpPromptSeparator::None).into()
            );
        });

        // If we reset the prompt, settings are cleared.
        prompt.update(&mut app, |prompt, ctx| {
            prompt.reset(ctx).expect("Saving prompt failed");
        });
        let reset_prompt_chips = prompt.read(&app, |prompt, _| prompt.chip_kinds());
        assert_eq!(reset_prompt_chips, default_prompt.chip_kinds());
        session_settings.read(&app, |settings, _| {
            assert_eq!(settings.saved_prompt.to_owned(), PromptSelection::Default);
        });
    });
}
