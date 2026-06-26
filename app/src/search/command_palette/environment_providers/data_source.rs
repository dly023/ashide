use fuzzy_match::{match_indices_case_insensitive, FuzzyMatchResult};
use itertools::Itertools;
use warpui::{AppContext, Entity};

use super::EnvironmentProviderSearchItem;
use crate::search::command_palette::mixer::CommandPaletteItemAction;
use crate::search::data_source::{Query, QueryResult};
use crate::search::mixer::{DataSourceRunErrorWrapper, SyncDataSource};

/// 上限。Provider target 一般几个到几十个,不会爆。
const MAX_ENVIRONMENT_PROVIDER_TARGETS_CONSIDERED: usize = 200;

#[derive(Default)]
pub struct EnvironmentProvidersDataSource;

impl EnvironmentProvidersDataSource {
    pub fn new() -> Self {
        Self
    }
}

impl Entity for EnvironmentProvidersDataSource {
    type Event = ();
}

impl SyncDataSource for EnvironmentProvidersDataSource {
    type Action = CommandPaletteItemAction;

    fn run_query(
        &self,
        query: &Query,
        _app: &AppContext,
    ) -> Result<Vec<QueryResult<Self::Action>>, DataSourceRunErrorWrapper> {
        // 走自家的 with_conn(独立写连接),不污染 PaneGroup 的主写线程。
        // DataSourceRunErrorWrapper 是 Box<dyn DataSourceRunError> 自定义 trait,
        // 包装成本太高 — 失败时 log + 返回空结果(palette 里不显示环境 provider target,但其他
        // source 不受影响)。
        let provider_targets =
            match crate::workspace::environment_provider::load_saved_provider_search_targets(
                MAX_ENVIRONMENT_PROVIDER_TARGETS_CONSIDERED,
            ) {
                Ok(targets) => targets,
                Err(error) => {
                    log::warn!(
                        "command palette environment providers: failed to load targets: {error}"
                    );
                    return Ok(Vec::new());
                }
            };

        let query_str = query.text.as_str();
        let results = provider_targets
            .into_iter()
            .filter_map(|provider_target| {
                let match_result = if query_str.is_empty() {
                    Some(FuzzyMatchResult::no_match())
                } else {
                    match_indices_case_insensitive(&provider_target.search_text, query_str)
                }?;

                let mut item = EnvironmentProviderSearchItem::new(
                    provider_target.target,
                    provider_target.detail,
                    provider_target.title,
                );
                let mut mr = match_result;
                // 跟 RepoDataSource 一样略 boost,让 provider target 结果在混合面板里有竞争力。
                mr.score *= 4;
                item.match_result = mr;
                Some(item.into())
            })
            .collect_vec();

        Ok(results)
    }
}
