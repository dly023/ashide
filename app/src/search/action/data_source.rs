use fuzzy_match::{match_indices_case_insensitive, FuzzyMatchResult};
use std::collections::HashMap;
use std::sync::Arc;
use warpui::{AppContext, Entity, ModelContext, ModelHandle};

use crate::search::action::search_item::MatchedBinding;
use crate::search::command_palette::mixer::CommandPaletteItemAction;
use crate::search::data_source::{DataSourceSearchError, Query, QueryResult};
use crate::search::mixer::{DataSourceRunErrorWrapper, SyncDataSource};

use crate::util::bindings::CommandBinding;

use crate::search::binding_source::BindingSource;
use warpui::keymap::{BindingId, DescriptionContext};

/// Data source for [`CommandBinding`]s. Produces a list of in-app actions a user can currently
/// perform.
pub struct CommandBindingDataSource {
    searcher: Box<dyn ActionSearcher>,
}

impl CommandBindingDataSource {
    pub fn new(binding_source: ModelHandle<BindingSource>, ctx: &mut ModelContext<Self>) -> Self {
        // Command palette actions always use character-level fuzzy matching.
        // Tantivy's default tokenizer does not segment CJK text, while fuzzy matching
        // also lets English keywords match binding names by subsequence.
        ctx.observe(&binding_source, Self::on_binding_source_changed);

        let searcher = Box::new(FuzzyActionSearcher {
            all_bindings: Default::default(),
        });
        Self { searcher }
    }

    /// Returns a [`QueryResult`] for a binding with `binding_id`. `None` if no result was found
    /// with the given ID.
    pub fn query_result(
        &self,
        binding_id: BindingId,
    ) -> Option<QueryResult<CommandPaletteItemAction>> {
        self.searcher.bindings().get(&binding_id).map(|binding| {
            MatchedBinding::new(FuzzyMatchResult::no_match(), binding.clone()).into()
        })
    }

    fn on_binding_source_changed(
        &mut self,
        source: ModelHandle<BindingSource>,
        ctx: &mut ModelContext<Self>,
    ) {
        let (window_id, view_id, binding_filter_fn) = match source.as_ref(ctx) {
            BindingSource::None => return,
            BindingSource::View {
                window_id,
                view_id,
                binding_filter_fn,
            } => (*window_id, *view_id, binding_filter_fn.clone()),
        };

        *self.searcher.bindings_mut() = ctx
            .key_bindings_for_view(window_id, view_id)
            .into_iter()
            .filter_map(|lens| CommandBinding::from_lens(lens, ctx))
            .filter(|binding| binding_filter_fn.as_ref().is_none_or(|f| f(binding)))
            .map(Arc::new)
            .map(|binding| (binding.id, binding))
            .collect();

        self.searcher.build_index();
        ctx.emit(Event::IndexUpdated);
    }
}

impl SyncDataSource for CommandBindingDataSource {
    type Action = CommandPaletteItemAction;

    fn run_query(
        &self,
        query: &Query,
        _app: &AppContext,
    ) -> Result<Vec<QueryResult<Self::Action>>, DataSourceRunErrorWrapper> {
        self.searcher
            .search(&query.text.trim().to_lowercase())
            .map_err(|err| {
                let search_error = DataSourceSearchError {
                    message: err.to_string(),
                };
                Box::new(search_error) as DataSourceRunErrorWrapper
            })
    }
}

pub enum Event {
    IndexUpdated,
}

impl Entity for CommandBindingDataSource {
    type Event = Event;
}

type SearcherAction = <CommandBindingDataSource as SyncDataSource>::Action;

trait ActionSearcher {
    fn search(&self, _search_term: &str) -> anyhow::Result<Vec<QueryResult<SearcherAction>>>;

    fn build_index(&mut self);

    /// Set of cached bindings, keyed on [`BindingId`]. This is cached via the [`BindingSource`]
    /// model to ensure that we surface bindings to the user that were executable _before_ the
    /// command palette was opened.
    fn bindings(&self) -> &HashMap<BindingId, Arc<CommandBinding>>;

    fn bindings_mut(&mut self) -> &mut HashMap<BindingId, Arc<CommandBinding>>;
}

struct FuzzyActionSearcher {
    all_bindings: HashMap<BindingId, Arc<CommandBinding>>,
}

impl ActionSearcher for FuzzyActionSearcher {
    fn search(&self, search_term: &str) -> anyhow::Result<Vec<QueryResult<SearcherAction>>> {
        Ok(self
            .all_bindings
            .values()
            .filter_map(move |binding| {
                if is_excluded_binding(binding) {
                    return None;
                }

                // Binding descriptions are almost always upper case. If a user searches with
                // lowercase text, the fuzzy matcher will weight this match lower because the case
                // between the search term and the description differ. As a result, we lowercase
                // both the search term and the description to ensure that we are matching the two
                // with the same casing.
                //
                // Include the action identifier in the searchable text so English keywords
                // can match localized descriptions.
                let mut searchable = binding
                    .description
                    .in_context(DescriptionContext::Default)
                    .to_lowercase();
                let name_tokens = binding.name.replace([':', '_'], " ").to_lowercase();
                if !name_tokens.is_empty() {
                    searchable.push(' ');
                    searchable.push_str(&name_tokens);
                }
                let description_char_len = binding
                    .description
                    .in_context(DescriptionContext::Default)
                    .chars()
                    .count();
                match_indices_case_insensitive(
                    searchable.as_str(),
                    search_term.to_lowercase().as_str(),
                )
                .map(|mut result| {
                    // 高亮渲染针对 description,落到拼接的 binding.name 区段的索引
                    // 越界会画错位置,这里裁剪掉。
                    result
                        .matched_indices
                        .retain(|&idx| idx < description_char_len);
                    (result, binding)
                })
            })
            .map(|(match_result, binding)| {
                MatchedBinding::new(match_result, binding.clone()).into()
            })
            .collect())
    }

    fn build_index(&mut self) {}

    fn bindings(&self) -> &HashMap<BindingId, Arc<CommandBinding>> {
        &self.all_bindings
    }

    fn bindings_mut(&mut self) -> &mut HashMap<BindingId, Arc<CommandBinding>> {
        &mut self.all_bindings
    }
}
// Context on why the search_drive action is excluded can be seen here: https://github.com/warpdotdev/warp-internal/pull/11705
fn is_excluded_binding(binding: &CommandBinding) -> bool {
    binding.name == *"workspace:search_drive"
}
