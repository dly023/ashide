use itertools::Itertools;
use warpui::{AppContext, SingletonEntity};

use crate::object_store::{Space, StoredObject};
use crate::search::notebook_embedding::embedded_fuzzy_match::FuzzyMatchEmbeddedObjectResult;
use crate::search::notebook_embedding::searcher::EmbeddingSearchItemAction;
use crate::workflows::WorkflowObject;

use crate::object_store::model::persistence::ObjectStoreModel;
use crate::search::data_source::{Query, QueryResult};
use crate::search::mixer::{DataSourceRunErrorWrapper, SyncDataSource};

use super::workflow_search_item::WorkflowSearchItem;

pub struct EmbeddedWorkflowsDataSource {
    /// The space containing the object we are embedding into.
    embedding_space: Space,
    workflows: Vec<WorkflowObject>,
}

impl EmbeddedWorkflowsDataSource {
    pub fn new(notebook_space: Space, app: &mut AppContext) -> Self {
        let object_store_model = ObjectStoreModel::as_ref(app);
        Self {
            embedding_space: notebook_space,
            workflows: object_store_model
                .get_all_active_workflows()
                .filter(|workflow| workflow.id.into_stable().is_some())
                .cloned()
                .collect(),
        }
    }
}

impl SyncDataSource for EmbeddedWorkflowsDataSource {
    type Action = EmbeddingSearchItemAction;

    fn run_query(
        &self,
        query: &Query,
        app: &AppContext,
    ) -> Result<Vec<QueryResult<Self::Action>>, DataSourceRunErrorWrapper> {
        let query_str = query.text.as_str();
        Ok(self
            .workflows
            .clone()
            .into_iter()
            .filter_map(move |workflow| -> Option<QueryResult<Self::Action>> {
                FuzzyMatchEmbeddedObjectResult::try_match(
                    query_str,
                    workflow.model().data.name(),
                    workflow.breadcrumbs(app).as_str(),
                )
                .map(|match_result| {
                    WorkflowSearchItem {
                        workflow,
                        fuzzy_matched_workflow: match_result,
                    }
                    .into()
                })
            })
            .collect_vec())
    }
}
