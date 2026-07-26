#[test]
fn environment_execution_context_precedes_session_executor_selection() {
    const MODEL_EVENTS_RS: &str = include_str!("model_events.rs");
    let completion = MODEL_EVENTS_RS
        .split_once("fn complete_bootstrapped_session(")
        .expect("bootstrapped-session completion boundary must exist")
        .1
        .split_once("/// Emits an event so `TerminalView`")
        .expect("bootstrapped-session completion boundary must remain auditable")
        .0;

    let execution_context_commit = completion
        .find("notify_bootstrapped_session(")
        .expect("Environment Runtime bootstrap must commit its execution context");
    let session_initialization = completion
        .find("sessions.initialize_bootstrapped_session(")
        .expect("bootstrapped terminal session must be initialized");

    assert!(
        execution_context_commit < session_initialization,
        "execution context and runtime alias must be committed before Session initialization selects and stores its CommandExecutor"
    );
}
