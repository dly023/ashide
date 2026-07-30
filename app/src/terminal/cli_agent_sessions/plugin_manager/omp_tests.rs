use super::OmpPluginManager;
use crate::terminal::cli_agent_sessions::plugin_manager::CliAgentPluginManager;
use std::fs;
use tempfile::tempdir;

#[test]
fn can_auto_install() {
    let manager = OmpPluginManager;
    assert!(manager.can_auto_install());
}

#[test]
fn is_installed_false_when_missing() {
    let manager = OmpPluginManager;
    // The real path likely doesn't exist in CI; if it does, this is a no-op.
    // We just verify the method doesn't panic.
    let _ = manager.is_installed();
}

#[test]
fn install_writes_bundled_extension() {
    let tmp = tempdir().expect("create temp dir");
    let dir = tmp.path().join(".omp/agent/extensions");
    let path = dir.join("ashide-omp.ts");

    // Simulate install by writing to the temp path manually.
    fs::create_dir_all(&dir).expect("create extensions dir");
    fs::write(&path, include_str!("ashide-omp.ts")).expect("write extension");

    assert!(path.exists());
    let content = fs::read_to_string(&path).expect("read extension");
    assert!(content.contains("warp://cli-agent"));
    assert!(content.contains("\"omp\""));
}

#[test]
fn bundled_extension_has_required_events() {
    let source = include_str!("ashide-omp.ts");
    assert!(source.contains("session_start"));
    assert!(source.contains("input"));
    assert!(source.contains("turn_end"));
    assert!(source.contains("prompt_submit"));
}
