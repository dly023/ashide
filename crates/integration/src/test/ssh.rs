use crate::{test::ssh_fixture::SshFixture, Builder};
use warp::{
    integration_testing::{
        step::new_step_with_default_assertions,
        subshell::{enter_ssh_command, run_exit_command, wait_for_ssh_fixture_ready},
        terminal::{
            execute_command_for_single_terminal_in_tab,
            util::{current_shell_starter_and_version, ExpectedExitStatus},
            wait_until_bootstrapped_single_pane_for_tab,
        },
        view_getters::single_terminal_view_for_tab,
    },
    terminal::shell::ShellType,
};
use warpui::{
    async_assert, async_assert_eq,
    integration::{AssertionCallback, AssertionOutcome},
};

use super::new_builder;

const REMOTE_SHELL: &str = "zsh";
const SSH_WARPIFY_SETTINGS: &str =
    "[warpify.ssh]\nenable_ssh_warpification = true\nuse_ssh_tmux_wrapper = false\n";
const SSH_WITH_STARTUP_OVERRIDE_SETTINGS: &str = "[warpify.ssh]\nenable_ssh_warpification = true\nuse_ssh_tmux_wrapper = false\n\n[session]\nstartup_shell_override = \"bash\"\n";

fn assert_active_block_is_remote(expected_user: String) -> AssertionCallback {
    Box::new(move |app, window_id| {
        let terminal_view = single_terminal_view_for_tab(app, window_id, 0);
        terminal_view.read(app, |view, ctx| {
            let model = view.model.lock();
            let active_block = model.block_list().active_block();

            let Some(session_id) = active_block.session_id() else {
                return AssertionOutcome::PreconditionFailed(
                    "Active block returned None from session_id()".into(),
                );
            };
            let Some(session) = view.sessions(ctx).get(session_id) else {
                return AssertionOutcome::PreconditionFailed(
                    "Active block should be part of a known session".into(),
                );
            };

            match async_assert!(
                !session.uses_current_app_environment(),
                "Active block should be part of a remote session"
            ) {
                AssertionOutcome::Success => {}
                failure => return failure,
            }

            let Some(shell_host) = active_block.shell_host() else {
                return AssertionOutcome::PreconditionFailed(
                    "Active block returned None from shell_host()".into(),
                );
            };

            async_assert_eq!(
                shell_host.user,
                expected_user,
                "Remote session did not have the fixture user"
            )
        })
    })
}

fn write_public_settings(toml: &'static str) {
    let path = warp::settings::user_preferences_toml_file_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("should create public settings directory");
    }
    std::fs::write(&path, toml).expect("should write public settings TOML");
}

fn hermetic_ssh_warpify_builder(
    settings_toml: &'static str,
    verify_startup_shell_override: bool,
) -> Builder {
    let fixture = SshFixture::start().expect("hermetic loopback sshd fixture should start");
    let expected_user = whoami::username();
    let mut fixture = Some(fixture);

    let mut builder = new_builder()
        .with_setup(move |_| write_public_settings(settings_toml))
        .with_cleanup(move |_| {
            if let Some(mut fixture) = fixture.take() {
                fixture.shutdown();
            }
        })
        .with_step(wait_until_bootstrapped_single_pane_for_tab(0));

    if verify_startup_shell_override {
        builder = builder.with_step(execute_command_for_single_terminal_in_tab(
            0,
            "test -x \"$SHELL\" && test \"${SHELL##*/}\" = bash".into(),
            ExpectedExitStatus::Success,
            (),
        ));
    }

    builder
        .with_step(enter_ssh_command(REMOTE_SHELL))
        .with_step(wait_for_ssh_fixture_ready(0, REMOTE_SHELL))
        .with_step(wait_until_bootstrapped_single_pane_for_tab(0))
        .with_step(
            new_step_with_default_assertions("Assert active block belongs to hermetic SSH")
                .add_assertion(assert_active_block_is_remote(expected_user)),
        )
        .with_step(execute_command_for_single_terminal_in_tab(
            0,
            "[[ -o login ]]".into(),
            ExpectedExitStatus::Success,
            (),
        ))
        .with_step(run_exit_command())
        .with_step(new_step_with_default_assertions(
            "Assert hermetic SSH session has completed",
        ))
}

pub fn test_hermetic_ssh_warpify() -> Builder {
    hermetic_ssh_warpify_builder(SSH_WARPIFY_SETTINGS, false)
}

pub use test_hermetic_ssh_warpify as test_legacy_ssh_into_bash;
pub use test_hermetic_ssh_warpify as test_legacy_ssh_into_zsh;
pub use test_hermetic_ssh_warpify as test_ssh_into_ash;
pub use test_hermetic_ssh_warpify as test_ssh_into_fish;
pub use test_hermetic_ssh_warpify as test_ssh_into_sh;
pub use test_hermetic_ssh_warpify as test_tmux_ssh_into_bash;
pub use test_hermetic_ssh_warpify as test_tmux_ssh_into_zsh;

pub fn test_ssh_with_shell_override() -> Builder {
    let (starter, _) = current_shell_starter_and_version();
    assert_ne!(
        starter.shell_type(),
        ShellType::PowerShell,
        "macOS hermetic SSH integration requires a POSIX startup shell"
    );
    hermetic_ssh_warpify_builder(SSH_WITH_STARTUP_OVERRIDE_SETTINGS, true)
}
