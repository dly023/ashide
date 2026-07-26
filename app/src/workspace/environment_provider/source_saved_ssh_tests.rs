use std::path::PathBuf;

use warp_ssh_manager::{LoadOutcome, LoadResult, SshConfigCandidate};

use super::config_candidate_to_server;
use crate::ssh_manager::SshTargetCatalog;

fn candidate() -> SshConfigCandidate {
    SshConfigCandidate {
        alias: "remote-fixture".to_owned(),
        hostname: None,
        user: None,
        port: None,
        identity_file: None,
    }
}

fn catalog(path: Option<PathBuf>) -> SshTargetCatalog {
    SshTargetCatalog::with_snapshot(LoadResult {
        path,
        outcome: LoadOutcome::Loaded(vec![candidate()]),
        has_unexpanded_includes: false,
    })
}

#[test]
fn config_candidate_server_uses_catalog_resolved_source_path() {
    let path = PathBuf::from(r"C:\Users\Alice\.ssh\config");
    let server = config_candidate_to_server(&candidate(), &catalog(Some(path.clone())));

    assert_eq!(
        server.notes.as_deref(),
        Some(format!("Loaded from {}", path.display()).as_str())
    );
    assert!(!server.notes.as_deref().unwrap().contains('~'));
}

#[test]
fn config_candidate_server_without_source_path_has_no_origin_note() {
    let server = config_candidate_to_server(&candidate(), &catalog(None));

    assert_eq!(server.notes, None);
}
