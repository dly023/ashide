use std::collections::HashMap;

pub const SESSION_EXECUTION_CONTEXT_MARKER: &str = "ASHIDE_SESSION_EXECUTION_CONTEXT";
pub const SESSION_EXECUTION_CONTEXT_MARKER_VALUE: &str = "1";
pub const AUTHORITATIVE_SESSION_ENVIRONMENT_VARIABLES: [&str; 5] = [
    "HOME",
    "PATH",
    "CODEX_HOME",
    "CLAUDE_CONFIG_DIR",
    SESSION_EXECUTION_CONTEXT_MARKER,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SessionExecutionContextError {
    #[error("missing target shell path")]
    MissingShellPath,
    #[error("missing target working directory")]
    MissingWorkingDirectory,
    #[error("missing target HOME")]
    MissingHome,
    #[error("missing target PATH")]
    MissingPath,
    #[error("missing validated execution-context marker")]
    MissingMarker,
}

fn non_empty(value: Option<&str>) -> bool {
    value.is_some_and(|value| !value.trim().is_empty())
}

pub fn validate_target_session_snapshot(
    shell_path: Option<&str>,
    working_directory: Option<&str>,
    environment_variables: &HashMap<String, String>,
) -> Result<(), SessionExecutionContextError> {
    if !non_empty(shell_path) {
        return Err(SessionExecutionContextError::MissingShellPath);
    }
    if !non_empty(working_directory) {
        return Err(SessionExecutionContextError::MissingWorkingDirectory);
    }
    if !non_empty(environment_variables.get("HOME").map(String::as_str)) {
        return Err(SessionExecutionContextError::MissingHome);
    }
    if !non_empty(environment_variables.get("PATH").map(String::as_str)) {
        return Err(SessionExecutionContextError::MissingPath);
    }
    Ok(())
}

pub fn mark_validated_target_session_snapshot(
    shell_path: Option<&str>,
    working_directory: Option<&str>,
    environment_variables: &mut HashMap<String, String>,
) -> Result<(), SessionExecutionContextError> {
    validate_target_session_snapshot(shell_path, working_directory, environment_variables)?;
    environment_variables.insert(
        SESSION_EXECUTION_CONTEXT_MARKER.to_owned(),
        SESSION_EXECUTION_CONTEXT_MARKER_VALUE.to_owned(),
    );
    Ok(())
}

pub fn validate_marked_target_session_snapshot(
    shell_path: Option<&str>,
    working_directory: Option<&str>,
    environment_variables: &HashMap<String, String>,
) -> Result<(), SessionExecutionContextError> {
    validate_target_session_snapshot(shell_path, working_directory, environment_variables)?;
    if environment_variables
        .get(SESSION_EXECUTION_CONTEXT_MARKER)
        .is_none_or(|value| value != SESSION_EXECUTION_CONTEXT_MARKER_VALUE)
    {
        return Err(SessionExecutionContextError::MissingMarker);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn complete_environment() -> HashMap<String, String> {
        HashMap::from([
            ("HOME".to_owned(), "/home/test".to_owned()),
            ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
        ])
    }

    #[test]
    fn marker_is_only_added_to_complete_target_snapshot() {
        let mut complete = complete_environment();
        mark_validated_target_session_snapshot(
            Some("/bin/bash"),
            Some("/workspace"),
            &mut complete,
        )
        .unwrap();
        assert_eq!(
            complete.get(SESSION_EXECUTION_CONTEXT_MARKER),
            Some(&SESSION_EXECUTION_CONTEXT_MARKER_VALUE.to_owned())
        );

        let mut incomplete = complete_environment();
        assert_eq!(
            mark_validated_target_session_snapshot(None, Some("/workspace"), &mut incomplete),
            Err(SessionExecutionContextError::MissingShellPath)
        );
        assert!(!incomplete.contains_key(SESSION_EXECUTION_CONTEXT_MARKER));
    }
}
