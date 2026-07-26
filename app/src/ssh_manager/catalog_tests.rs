use std::path::PathBuf;

use warp_ssh_manager::{AuthType, LoadOutcome, LoadResult, SshConfigCandidate, SshServerInfo};

use super::{
    SshTargetCatalog, SshTargetCatalogEntry, SshTargetCatalogRefreshIntent,
    SshTargetCatalogSnapshot,
};

fn candidate(alias: &str) -> SshConfigCandidate {
    SshConfigCandidate {
        alias: alias.to_owned(),
        hostname: None,
        user: None,
        port: None,
        identity_file: None,
    }
}

fn server(node_id: &str, host: &str) -> SshServerInfo {
    SshServerInfo {
        node_id: node_id.to_owned(),
        host: host.to_owned(),
        port: 22,
        username: "root".to_owned(),
        auth_type: AuthType::Password,
        key_path: None,
        startup_command: None,
        notes: None,
        last_connected_at: None,
    }
}

fn snapshot(saved: &[(&str, &str, &str)], config: &[&str]) -> SshTargetCatalogSnapshot {
    let config = LoadResult {
        path: Some(PathBuf::from("/home/u/.ssh/config")),
        outcome: LoadOutcome::Loaded(config.iter().map(|alias| candidate(alias)).collect()),
        has_unexpanded_includes: false,
    };
    SshTargetCatalogSnapshot::merge(
        config,
        saved
            .iter()
            .map(|(node_id, name, host)| (name.to_string(), server(node_id, host)))
            .collect(),
    )
}

fn identities(catalog: &SshTargetCatalog) -> Vec<String> {
    catalog
        .entries()
        .iter()
        .map(SshTargetCatalogEntry::stable_identity)
        .collect()
}

#[test]
fn unavailable_config_source_never_fabricates_open_target() {
    for outcome in [
        LoadOutcome::NotFound,
        LoadOutcome::Error("denied".to_owned()),
    ] {
        let catalog = SshTargetCatalog::with_config_snapshot(LoadResult {
            path: None,
            outcome,
            has_unexpanded_includes: false,
        });

        assert!(catalog.config_open_target().is_none());
        assert!(catalog.config_path_display().is_none());
        assert!(catalog.entries().is_empty());
    }
}

#[test]
fn ssh_config_catalog_refresh_replaces_path_outcome_and_candidates_atomically() {
    let mut catalog = SshTargetCatalog::with_config_snapshot(LoadResult {
        path: Some(PathBuf::from("/old/config")),
        outcome: LoadOutcome::Loaded(vec![candidate("old")]),
        has_unexpanded_includes: false,
    });
    let generation = catalog.begin_refresh_for_test(SshTargetCatalogRefreshIntent::ExplicitRefresh);

    assert!(catalog.finish_refresh_for_test(
        generation,
        Ok(SshTargetCatalogSnapshot::merge(
            LoadResult {
                path: Some(PathBuf::from("C:/Users/u/.ssh/config")),
                outcome: LoadOutcome::Loaded(vec![candidate("new")]),
                has_unexpanded_includes: false,
            },
            Vec::new(),
        )),
    ));
    assert_eq!(
        catalog.config_open_target(),
        Some(PathBuf::from("C:/Users/u/.ssh/config").as_path())
    );
    assert!(catalog.find_candidate("old").is_none());
    assert_eq!(
        catalog
            .find_candidate("new")
            .map(|candidate| candidate.alias.as_str()),
        Some("new")
    );
}

#[test]
fn config_and_saved_targets_merge_with_stable_identity_conflict_and_order_rules() {
    let catalog = SshTargetCatalog::with_catalog_snapshot(snapshot(
        &[("node-b", "shared", "shared"), ("node-a", "alpha", "a")],
        &["shared", "zeta"],
    ));

    assert_eq!(
        identities(&catalog),
        vec![
            "saved:node-b",
            "saved:node-a",
            "config:shared",
            "config:zeta"
        ]
    );
}

#[test]
fn explicit_and_tree_changed_refresh_share_generation_gate() {
    let mut catalog = SshTargetCatalog::with_catalog_snapshot(snapshot(&[], &["old"]));
    let explicit = catalog.begin_refresh_for_test(SshTargetCatalogRefreshIntent::ExplicitRefresh);
    let mutation = catalog.begin_refresh_for_test(SshTargetCatalogRefreshIntent::TreeChanged);

    assert!(catalog.is_loading());
    assert_eq!(mutation, explicit + 1);
    assert_eq!(
        catalog.active_intent(),
        Some(SshTargetCatalogRefreshIntent::TreeChanged)
    );
}

#[test]
fn stale_completion_cannot_replace_newer_committed_collection() {
    let mut catalog = SshTargetCatalog::with_catalog_snapshot(snapshot(&[], &["old"]));
    let stale = catalog.begin_refresh_for_test(SshTargetCatalogRefreshIntent::ExplicitRefresh);
    let current = catalog.begin_refresh_for_test(SshTargetCatalogRefreshIntent::ExplicitRefresh);

    assert!(!catalog.finish_refresh_for_test(stale, Ok(snapshot(&[], &["stale"]))));
    assert!(catalog.finish_refresh_for_test(current, Ok(snapshot(&[], &["current"]))));
    assert_eq!(identities(&catalog), vec!["config:current"]);
}

#[test]
fn source_error_retains_target_plus_two_unrelated_committed_rows() {
    let mut catalog = SshTargetCatalog::with_catalog_snapshot(snapshot(
        &[("one", "one", "one"), ("two", "two", "two")],
        &["three"],
    ));
    let before = identities(&catalog);
    let generation = catalog.begin_refresh_for_test(SshTargetCatalogRefreshIntent::ExplicitRefresh);

    assert!(catalog.finish_refresh_for_test(generation, Err("sqlite unavailable".to_owned())));
    assert_eq!(identities(&catalog), before);
    assert_eq!(catalog.error(), Some("sqlite unavailable"));
    assert!(!catalog.is_loading());
}

#[test]
fn successful_generation_reflects_permanent_delete_without_reordering_unrelated_rows() {
    let mut catalog = SshTargetCatalog::with_catalog_snapshot(snapshot(
        &[
            ("target", "target", "target"),
            ("one", "one", "one"),
            ("two", "two", "two"),
        ],
        &[],
    ));
    let generation = catalog.begin_refresh_for_test(SshTargetCatalogRefreshIntent::TreeChanged);

    assert!(catalog.finish_refresh_for_test(
        generation,
        Ok(snapshot(
            &[("one", "one", "one"), ("two", "two", "two")],
            &[]
        )),
    ));
    assert_eq!(identities(&catalog), vec!["saved:one", "saved:two"]);
}

#[test]
fn zero_one_many_targets_have_deterministic_cardinality() {
    for (snapshot, expected) in [
        (snapshot(&[], &[]), 0),
        (snapshot(&[("one", "one", "one")], &[]), 1),
        (
            snapshot(&[("one", "one", "one"), ("two", "two", "two")], &["three"]),
            3,
        ),
    ] {
        assert_eq!(
            SshTargetCatalog::with_catalog_snapshot(snapshot)
                .entries()
                .len(),
            expected
        );
    }
}
