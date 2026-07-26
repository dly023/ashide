use super::*;

#[test]
fn test_program_invalid_bash() {
    // This test assumes there is no bash binary at /some/weird/path/bash.
    let shell_path = "/some/weird/path/bash".to_owned();
    assert!(supported_shell_path_and_type(&shell_path).is_none());
}

#[test]
fn test_program_invalid_zsh() {
    // This test assumes there is no bash zsh at /some/weird/path/bash.
    let shell_path = "/some/weird/path/zsh".to_owned();
    assert!(supported_shell_path_and_type(&shell_path).is_none());
}

#[test]
fn test_program_unknown_shell() {
    let shell_path = "/some/weird/path/wtfsh".to_owned();
    assert!(supported_shell_path_and_type(&shell_path).is_none());
}

#[test]
fn test_trim_wsl_err_from_output() {
    assert_eq!(
        take_until_utf16_crlf(b"/bin/bash\n".to_vec()),
        b"/bin/bash\n".to_vec()
    );
    assert_eq!(
        take_until_utf16_crlf(b"/bin/bash\n\r\0\n\0W\0A\0R\0N\0I\0N\0G\0".to_vec()),
        b"/bin/bash\n".to_vec()
    );
}

#[test]
fn wsl_spawn_uses_stable_exec_command_contract() {
    let args = wsl_arguments_for_session_spawning_command(
        "Debian GNU/Linux 13",
        "/bin/bash",
        ShellType::Bash,
    );
    let args = args
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect_vec();

    assert_eq!(
        &args[..4],
        [
            "--distribution",
            "Debian GNU/Linux 13",
            "--exec",
            "/bin/bash",
        ]
    );
    assert!(
        !args.iter().any(|arg| arg == "--shell-type"),
        "explicit WSL launch must not depend on an optional CLI argument: {args:?}"
    );
}

#[test]
fn wsl_distribution_name_is_preserved_in_spawn_arguments() {
    let distribution = "Debian GNU/Linux 13";
    let args = wsl_arguments_for_session_spawning_command(distribution, "/bin/zsh", ShellType::Zsh);

    assert_eq!(args[1], OsString::from(distribution));
}

#[test]
fn explicit_wsl_selection_failure_never_falls_back_to_windows_shell() {
    let error = resolved_wsl_shell_starter_source(
        "Debian GNU/Linux 13",
        Err(anyhow::anyhow!("WSL probe failed")),
    )
    .expect_err("an explicit WSL failure must remain an error");
    let error = format!("{error:#}");

    assert!(error.contains("Debian GNU/Linux 13"), "{error}");
    assert!(error.contains("WSL probe failed"), "{error}");
}
