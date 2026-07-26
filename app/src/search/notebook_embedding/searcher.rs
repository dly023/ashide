use crate::object_store::ids::ObjectStoreId;
use crate::search::mixer::SearchMixer;

pub type EmbeddingSearchMixer = SearchMixer<EmbeddingSearchItemAction>;

#[derive(Clone, Debug)]
pub enum EmbeddingSearchItemAction {
    AcceptWorkflow(ObjectStoreId),
}
