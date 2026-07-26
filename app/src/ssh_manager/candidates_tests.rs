//! `CandidatesViewModel` presentation mapping tests.
//! Source IO and path ownership are covered by `SshTargetCatalog` tests.

use std::collections::HashSet;
use std::path::PathBuf;

use warp_ssh_manager::SshConfigCandidate;

use super::{
    fake_load_result_error, fake_load_result_loaded, fake_load_result_not_found, CandidateRow,
    CandidatesViewModel,
};
use crate::ssh_manager::SshTargetCatalog;

fn candidate(alias: &str) -> SshConfigCandidate {
    SshConfigCandidate {
        alias: alias.into(),
        hostname: None,
        user: None,
        port: None,
        identity_file: None,
    }
}

fn full_candidate() -> SshConfigCandidate {
    SshConfigCandidate {
        alias: "prodbox".into(),
        hostname: Some("prod.example.com".into()),
        user: Some("alice".into()),
        port: Some(2222),
        identity_file: Some(PathBuf::from("/home/alice/.ssh/id_ed25519")),
    }
}

#[test]
fn rows_when_not_found_returns_header_plus_not_found() {
    let vm = CandidatesViewModel::new();
    let catalog =
        SshTargetCatalog::with_snapshot(fake_load_result_not_found("/home/u/.ssh/config"));

    let rows = vm.rows(&catalog);
    assert_eq!(rows.len(), 2);
    assert!(matches!(
        rows[0],
        CandidateRow::Header {
            count: 0,
            can_refresh: true,
            ..
        }
    ));
    assert_eq!(
        rows[1],
        CandidateRow::NotFound {
            path_display: "/home/u/.ssh/config".into()
        }
    );
}

#[test]
fn rows_when_error_returns_header_plus_error_with_message() {
    let vm = CandidatesViewModel::new();
    let catalog = SshTargetCatalog::with_snapshot(fake_load_result_error(
        "/home/u/.ssh/config",
        "permission denied",
    ));

    let rows = vm.rows(&catalog);
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows[1],
        CandidateRow::Error {
            path_display: "/home/u/.ssh/config".into(),
            message: "permission denied".into(),
        }
    );
}

#[test]
fn rows_when_loaded_empty_returns_header_plus_empty() {
    let vm = CandidatesViewModel::new();
    let catalog =
        SshTargetCatalog::with_snapshot(fake_load_result_loaded("/home/u/.ssh/config", vec![]));

    let rows = vm.rows(&catalog);
    assert_eq!(rows.len(), 2);
    assert!(matches!(rows[0], CandidateRow::Header { count: 0, .. }));
    assert!(matches!(rows[1], CandidateRow::Empty { .. }));
}

#[test]
fn rows_preserve_committed_candidate_order_and_fields() {
    let vm = CandidatesViewModel::new();
    let catalog = SshTargetCatalog::with_snapshot(fake_load_result_loaded(
        "/home/u/.ssh/config",
        vec![candidate("a"), full_candidate(), candidate("c")],
    ));

    let rows = vm.rows(&catalog);
    assert_eq!(rows.len(), 4);
    assert!(matches!(rows[0], CandidateRow::Header { count: 3, .. }));
    assert!(matches!(
        &rows[2],
        CandidateRow::Candidate {
            alias,
            hostname: Some(hostname),
            user: Some(user),
            port: Some(2222),
            identity_file: Some(identity_file),
            ..
        } if alias == "prodbox"
            && hostname == "prod.example.com"
            && user == "alice"
            && identity_file == "/home/alice/.ssh/id_ed25519"
    ));
}

#[test]
fn rows_marks_only_imported_aliases_as_added() {
    let mut added = HashSet::new();
    added.insert("b".to_owned());
    let vm = CandidatesViewModel::with_state(added, true);
    let catalog = SshTargetCatalog::with_snapshot(fake_load_result_loaded(
        "/home/u/.ssh/config",
        vec![candidate("a"), candidate("b"), candidate("c")],
    ));

    let rows = vm.rows(&catalog);
    assert!(matches!(
        rows[1],
        CandidateRow::Candidate { added: false, .. }
    ));
    assert!(matches!(
        rows[2],
        CandidateRow::Candidate { added: true, .. }
    ));
    assert!(matches!(
        rows[3],
        CandidateRow::Candidate { added: false, .. }
    ));
}

#[test]
fn collapsed_model_keeps_only_header_for_every_catalog_outcome() {
    let vm = CandidatesViewModel::with_state(HashSet::new(), false);
    let catalogs = [
        SshTargetCatalog::with_snapshot(fake_load_result_not_found("/a/config")),
        SshTargetCatalog::with_snapshot(fake_load_result_error("/b/config", "io error")),
        SshTargetCatalog::with_snapshot(fake_load_result_loaded(
            "/c/config",
            vec![candidate("a"), candidate("b")],
        )),
    ];

    for catalog in catalogs {
        let rows = vm.rows(&catalog);
        assert_eq!(rows.len(), 1);
        assert!(matches!(rows[0], CandidateRow::Header { .. }));
    }
}

#[test]
fn unavailable_source_renders_error_without_fabricated_path() {
    let vm = CandidatesViewModel::new();
    let catalog = SshTargetCatalog::with_snapshot(warp_ssh_manager::LoadResult {
        path: None,
        outcome: warp_ssh_manager::LoadOutcome::Error("home unavailable".into()),
        has_unexpanded_includes: false,
    });

    let rows = vm.rows(&catalog);
    assert_eq!(
        rows[1],
        CandidateRow::Error {
            path_display: String::new(),
            message: "home unavailable".into(),
        }
    );
}
