use super::commit_complete_cli_agent_session_scan;

#[test]
fn session_refresh_scan_failure_preserves_cached_rows() {
    let mut cached_rows = vec!["existing-session"];

    let result = commit_complete_cli_agent_session_scan(
        &mut cached_rows,
        Err::<Vec<&str>, _>("traversal failed"),
    );

    assert_eq!(result, Err("traversal failed"));
    assert_eq!(cached_rows, vec!["existing-session"]);
}

#[test]
fn session_refresh_complete_scan_replaces_cached_rows() {
    let mut cached_rows = vec!["existing-session"];

    commit_complete_cli_agent_session_scan(&mut cached_rows, Ok::<_, &str>(vec!["new-session"]))
        .expect("complete scan commits");

    assert_eq!(cached_rows, vec!["new-session"]);
}
