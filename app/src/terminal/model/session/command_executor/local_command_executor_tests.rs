use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use serial_test::serial;

use super::{ExecuteCommandOptions, LocalCommandExecutionContext, LocalCommandExecutor};
use crate::terminal::shell::ShellType;

struct EnvironmentRestore {
    values: Vec<(&'static str, Option<String>)>,
}

impl EnvironmentRestore {
    fn capture(names: &[&'static str]) -> Self {
        Self {
            values: names
                .iter()
                .map(|name| (*name, std::env::var(name).ok()))
                .collect(),
        }
    }
}

impl Drop for EnvironmentRestore {
    fn drop(&mut self) {
        for (name, value) in &self.values {
            match value {
                Some(value) => std::env::set_var(name, value),
                None => std::env::remove_var(name),
            }
        }
    }
}

#[tokio::test]
#[serial]
async fn run_command_uses_session_context_instead_of_daemon_environment() {
    let names = ["HOME", "PATH", "CODEX_HOME", "CLAUDE_CONFIG_DIR"];
    let _restore = EnvironmentRestore::capture(&names);
    std::env::set_var("HOME", "/daemon/home");
    std::env::set_var("PATH", "/daemon/bin");
    std::env::set_var("CODEX_HOME", "/daemon/codex");
    std::env::set_var("CLAUDE_CONFIG_DIR", "/daemon/claude");

    let cwd = tempfile::tempdir().unwrap();
    let environment_variables = HashMap::from([
        ("HOME".to_owned(), "/session/home".to_owned()),
        ("PATH".to_owned(), "/session/bin".to_owned()),
        ("CODEX_HOME".to_owned(), "/session/codex".to_owned()),
        ("CLAUDE_CONFIG_DIR".to_owned(), "/session/claude".to_owned()),
    ]);
    let context = LocalCommandExecutionContext {
        working_directory: Some(cwd.path().to_path_buf()),
        environment_variables,
        authoritative_environment_variable_names: names
            .into_iter()
            .map(str::to_owned)
            .collect::<HashSet<_>>(),
    };
    let executor =
        LocalCommandExecutor::new(Some(PathBuf::from("/bin/bash")), ShellType::Bash, context);

    let output = executor
        .execute_local_command(
            "printf '%s\\n%s\\n%s\\n%s\\n%s\\n' \"$PWD\" \"$HOME\" \"$PATH\" \"$CODEX_HOME\" \"$CLAUDE_CONFIG_DIR\"",
            None,
            None,
            ExecuteCommandOptions::default(),
        )
        .await
        .unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let lines = stdout.lines().collect::<Vec<_>>();
    assert_eq!(
        lines[0],
        std::fs::canonicalize(cwd.path()).unwrap().to_string_lossy()
    );
    assert_eq!(
        lines[1..],
        [
            "/session/home",
            "/session/bin",
            "/session/codex",
            "/session/claude",
        ]
    );
}

#[tokio::test]
#[serial]
async fn missing_session_root_variable_does_not_inherit_daemon_value() {
    let _restore = EnvironmentRestore::capture(&["CODEX_HOME"]);
    std::env::set_var("CODEX_HOME", "/daemon/codex");

    let context = LocalCommandExecutionContext {
        working_directory: None,
        environment_variables: HashMap::new(),
        authoritative_environment_variable_names: HashSet::from(["CODEX_HOME".to_owned()]),
    };
    let executor =
        LocalCommandExecutor::new(Some(PathBuf::from("/bin/bash")), ShellType::Bash, context);

    let output = executor
        .execute_local_command(
            "printf '%s' \"${CODEX_HOME-unset}\"",
            None,
            None,
            ExecuteCommandOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(output.stdout, b"unset");
}
