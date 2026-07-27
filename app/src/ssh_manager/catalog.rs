//! App-global Environment SSH target catalog.
//!
//! This model is the only owner of the committed target collection consumed by the
//! Environment Strip, SSH Manager and runtime lookup. It atomically merges OpenSSH
//! config candidates with saved SQLite servers. Render code never performs source IO.

use std::collections::BTreeMap;
use std::path::Path;

use warp_ssh_manager::{
    LoadOutcome, LoadResult, NodeKind, SshConfigCandidate, SshNode, SshServerInfo,
};
use warpui::{Entity, ModelContext, SingletonEntity};

use super::{SshTreeChangedEvent, SshTreeChangedNotifier};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SshTargetCatalogRefreshIntent {
    ExplicitRefresh,
    TreeChanged,
}

#[derive(Clone, Debug)]
pub enum SshTargetCatalogEntry {
    Saved { name: String, server: SshServerInfo },
    Config(SshConfigCandidate),
}

impl SshTargetCatalogEntry {
    pub fn stable_identity(&self) -> String {
        match self {
            Self::Saved { server, .. } => format!("saved:{}", server.node_id),
            Self::Config(candidate) => format!("config:{}", candidate.alias),
        }
    }

    pub fn saved(&self) -> Option<(&str, &SshServerInfo)> {
        match self {
            Self::Saved { name, server } => Some((name, server)),
            Self::Config(_) => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SshTargetCatalogSnapshot {
    config: LoadResult,
    entries: Vec<SshTargetCatalogEntry>,
}

impl SshTargetCatalogSnapshot {
    pub(crate) fn merge(config: LoadResult, saved: Vec<(String, SshServerInfo)>) -> Self {
        let mut entries = saved
            .into_iter()
            .map(|(name, server)| SshTargetCatalogEntry::Saved { name, server })
            .collect::<Vec<_>>();
        if let LoadOutcome::Loaded(candidates) = &config.outcome {
            entries.extend(
                candidates
                    .iter()
                    .cloned()
                    .map(SshTargetCatalogEntry::Config),
            );
        }
        Self { config, entries }
    }
}

pub struct SshTargetCatalog {
    committed: SshTargetCatalogSnapshot,
    requested_generation: u64,
    committed_generation: u64,
    active_intent: Option<SshTargetCatalogRefreshIntent>,
    loading: bool,
    error: Option<String>,
}

impl SshTargetCatalog {
    pub fn new(ctx: &mut ModelContext<Self>) -> Self {
        ctx.subscribe_to_model(
            &SshTreeChangedNotifier::handle(ctx),
            |catalog, event, ctx| match event {
                SshTreeChangedEvent::TreeChanged => {
                    catalog.refresh(SshTargetCatalogRefreshIntent::TreeChanged, ctx)
                }
            },
        );

        match load_snapshot() {
            Ok(committed) => Self::with_committed(committed),
            Err(error) => {
                let mut catalog = Self::with_committed(SshTargetCatalogSnapshot::merge(
                    empty_config_snapshot(),
                    Vec::new(),
                ));
                catalog.error = Some(error);
                catalog
            }
        }
    }

    fn with_committed(committed: SshTargetCatalogSnapshot) -> Self {
        Self {
            committed,
            requested_generation: 0,
            committed_generation: 0,
            active_intent: None,
            loading: false,
            error: None,
        }
    }

    pub fn refresh(&mut self, intent: SshTargetCatalogRefreshIntent, ctx: &mut ModelContext<Self>) {
        let generation = self.begin_refresh(intent);
        ctx.notify();
        ctx.spawn(async { load_snapshot() }, move |catalog, result, ctx| {
            if catalog.finish_refresh(generation, result) {
                ctx.notify();
            }
        });
    }

    pub fn entries(&self) -> &[SshTargetCatalogEntry] {
        &self.committed.entries
    }

    pub fn is_loading(&self) -> bool {
        self.loading
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    #[cfg(test)]
    pub fn active_intent(&self) -> Option<SshTargetCatalogRefreshIntent> {
        self.active_intent
    }

    pub fn config_open_target(&self) -> Option<&Path> {
        self.committed.config.path.as_deref()
    }

    pub fn config_path_display(&self) -> Option<String> {
        self.config_open_target()
            .map(|path| path.display().to_string())
    }

    pub fn outcome(&self) -> &LoadOutcome {
        &self.committed.config.outcome
    }

    pub fn find_candidate(&self, alias: &str) -> Option<&SshConfigCandidate> {
        match self.outcome() {
            LoadOutcome::Loaded(candidates) => {
                candidates.iter().find(|candidate| candidate.alias == alias)
            }
            LoadOutcome::NotFound | LoadOutcome::Error(_) => None,
        }
    }

    pub fn find_entry(&self, stable_identity: &str) -> Option<&SshTargetCatalogEntry> {
        self.entries()
            .iter()
            .find(|entry| entry.stable_identity() == stable_identity)
    }

    fn begin_refresh(&mut self, intent: SshTargetCatalogRefreshIntent) -> u64 {
        self.requested_generation = self.requested_generation.wrapping_add(1);
        self.active_intent = Some(intent);
        self.loading = true;
        self.error = None;
        self.requested_generation
    }

    fn finish_refresh(
        &mut self,
        generation: u64,
        result: Result<SshTargetCatalogSnapshot, String>,
    ) -> bool {
        if generation != self.requested_generation {
            return false;
        }

        self.loading = false;
        self.active_intent = None;
        match result {
            Ok(snapshot) => {
                self.committed = snapshot;
                self.committed_generation = generation;
                self.error = None;
            }
            Err(error) => {
                self.error = Some(error);
            }
        }
        true
    }

    #[cfg(test)]
    pub(crate) fn with_snapshot(config: LoadResult) -> Self {
        Self::with_committed(SshTargetCatalogSnapshot::merge(config, Vec::new()))
    }

    #[cfg(test)]
    pub(crate) fn with_catalog_snapshot(snapshot: SshTargetCatalogSnapshot) -> Self {
        Self::with_committed(snapshot)
    }

    #[cfg(test)]
    pub(crate) fn begin_refresh_for_test(&mut self, intent: SshTargetCatalogRefreshIntent) -> u64 {
        self.begin_refresh(intent)
    }

    #[cfg(test)]
    pub(crate) fn finish_refresh_for_test(
        &mut self,
        generation: u64,
        result: Result<SshTargetCatalogSnapshot, String>,
    ) -> bool {
        self.finish_refresh(generation, result)
    }

    #[cfg(test)]
    pub(crate) fn with_config_snapshot(config: LoadResult) -> Self {
        Self::with_committed(SshTargetCatalogSnapshot::merge(config, Vec::new()))
    }

    #[cfg(test)]
    pub(crate) fn commit_for_test(&mut self, config: LoadResult) {
        self.committed = SshTargetCatalogSnapshot::merge(config, Vec::new());
        self.committed_generation = self.committed_generation.wrapping_add(1);
    }
}

impl Default for SshTargetCatalog {
    fn default() -> Self {
        Self::with_committed(load_snapshot().unwrap_or_else(|_| {
            SshTargetCatalogSnapshot::merge(empty_config_snapshot(), Vec::new())
        }))
    }
}

impl Entity for SshTargetCatalog {
    type Event = ();
}

impl SingletonEntity for SshTargetCatalog {}

fn load_snapshot() -> Result<SshTargetCatalogSnapshot, String> {
    let config = warp_ssh_manager::load_candidates();
    if let LoadOutcome::Error(error) = &config.outcome {
        return Err(format!("SSH config: {error}"));
    }

    let saved = load_saved_servers()?;
    Ok(SshTargetCatalogSnapshot::merge(config, saved))
}

fn load_saved_servers() -> Result<Vec<(String, SshServerInfo)>, String> {
    warp_ssh_manager::with_conn(|conn| {
        let nodes = warp_ssh_manager::SshRepository::list_nodes(conn)?;
        let ordered = sort_ssh_nodes_for_display(nodes);
        let mut saved = Vec::new();
        for node in ordered {
            if !matches!(node.kind, NodeKind::Server) {
                continue;
            }
            let Some(server) = warp_ssh_manager::SshRepository::get_server(conn, &node.id)? else {
                return Err(warp_ssh_manager::SshRepositoryError::NotFound(node.id).into());
            };
            saved.push((node.name, server));
        }
        Ok(saved)
    })
    .map_err(|error| format!("saved SSH targets: {error}"))
}

pub(crate) fn sort_ssh_nodes_for_display(nodes: Vec<SshNode>) -> Vec<SshNode> {
    let mut by_parent: BTreeMap<Option<String>, Vec<SshNode>> = BTreeMap::new();
    for node in nodes {
        by_parent
            .entry(node.parent_id.clone())
            .or_default()
            .push(node);
    }
    for children in by_parent.values_mut() {
        children.sort_by(|left, right| {
            (left.sort_order, &left.name).cmp(&(right.sort_order, &right.name))
        });
    }

    fn walk(
        parent: Option<&String>,
        by_parent: &BTreeMap<Option<String>, Vec<SshNode>>,
        ordered: &mut Vec<SshNode>,
    ) {
        if let Some(children) = by_parent.get(&parent.cloned()) {
            for child in children {
                ordered.push(child.clone());
                walk(Some(&child.id), by_parent, ordered);
            }
        }
    }

    let mut ordered = Vec::new();
    walk(None, &by_parent, &mut ordered);
    ordered
}

fn empty_config_snapshot() -> LoadResult {
    LoadResult {
        path: None,
        outcome: LoadOutcome::NotFound,
        has_unexpanded_includes: false,
    }
}

#[cfg(test)]
#[path = "catalog_tests.rs"]
mod tests;
