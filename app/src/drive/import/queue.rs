use std::collections::HashMap;

use warpui::{Entity, ModelContext, SingletonEntity};

use crate::{
    notebooks::NotebookObjectModel,
    object_store::ids::{ClientId, ObjectStoreId},
    object_store::{
        update_manager::{InitiatedBy, UpdateManager},
        Owner, StoredObjectEventEntrypoint,
    },
    workflows::{workflow::Workflow, workflow_enum::WorkflowEnum},
};

use super::nodes::{self, FileId};

pub(super) enum ImportQueueEvent {
    FileCompleted {
        file_id: FileId,
        object_uid: String,
    },
    FolderCompleted {
        folder_id: nodes::FolderId,
        object_uid: String,
    },
    FileSavedLocally(FileId),
}

#[derive(Debug)]
pub(super) enum ParentId {
    PendingFolder(ClientId),
    InitialFolder(Option<ObjectStoreId>),
}

#[derive(Debug)]
pub(super) struct ImportQueueArgs {
    pub(super) owner: Owner,
    pub(super) parent_id: ParentId,
    pub(super) content: RequestContent,
}

#[derive(Debug)]
pub(super) enum RequestContent {
    Folder {
        name: String,
        client_id: ClientId,
        folder_id: nodes::FolderId,
    },
    Notebook {
        title: String,
        data: String,
        client_id: ClientId,
        file_id: FileId,
    },
    Workflow {
        workflows: Vec<(Workflow, ClientId)>,
        workflow_enums: HashMap<ClientId, WorkflowEnum>,
        file_id: FileId,
    },
}

#[derive(Default)]
struct FileCompletionCounter {
    client_id_to_file_id: HashMap<ClientId, FileId>,
    file_id_to_counter: HashMap<FileId, usize>,
}

impl FileCompletionCounter {
    fn request_completed(&mut self, client_id: ClientId) -> Option<FileId> {
        if let Some(file_id) = self.client_id_to_file_id.get(&client_id) {
            let completed = match self.file_id_to_counter.get_mut(file_id) {
                Some(counter) => {
                    *counter = counter.saturating_sub(1);
                    *counter == 0
                }
                None => {
                    log::error!("File completion counter should exist but it doesn't");
                    false
                }
            };

            if completed {
                return Some(*file_id);
            }
        }
        None
    }

    fn add_entry(&mut self, client_id: ClientId, file_id: FileId) {
        self.client_id_to_file_id.insert(client_id, file_id);
        *self.file_id_to_counter.entry(file_id).or_insert(0) += 1;
    }
}

pub(super) struct ImportQueue {
    queue: Vec<ImportQueueArgs>,
    client_to_folder_id: HashMap<ClientId, Option<ObjectStoreId>>,
    client_to_node_folder_id: HashMap<ClientId, nodes::FolderId>,
    file_completion: FileCompletionCounter,
}

impl ImportQueue {
    pub fn new(_ctx: &mut ModelContext<Self>) -> Self {
        Self {
            queue: Vec::new(),
            client_to_folder_id: HashMap::default(),
            file_completion: Default::default(),
            client_to_node_folder_id: HashMap::default(),
        }
    }

    // Whether all local parent objects needed by an item have been created.
    fn dependency_created(&self, item: &ImportQueueArgs) -> bool {
        match &item.parent_id {
            ParentId::PendingFolder(id) => self
                .client_to_folder_id
                .get(id)
                .map(|item| item.is_some())
                .unwrap_or(false),
            ParentId::InitialFolder(_) => true,
        }
    }

    // Enqueue a new request to the import queue.
    pub fn enqueue(&mut self, arg: ImportQueueArgs, ctx: &mut ModelContext<Self>) {
        // Update internal tracker of the object.
        match &arg.content {
            RequestContent::Folder {
                client_id,
                folder_id,
                ..
            } => {
                self.client_to_folder_id.insert(*client_id, None);
                self.client_to_node_folder_id.insert(*client_id, *folder_id);
            }
            RequestContent::Notebook {
                client_id, file_id, ..
            } => self.file_completion.add_entry(*client_id, *file_id),
            RequestContent::Workflow {
                workflows, file_id, ..
            } => {
                for (_, client_id) in workflows {
                    self.file_completion.add_entry(*client_id, *file_id);
                }
            }
        }

        self.queue.push(arg);
        self.dequeue(ctx);
    }

    // Dequeue a new request from the import queue.
    pub fn dequeue(&mut self, ctx: &mut ModelContext<Self>) {
        if self.queue.is_empty() {
            return;
        }

        if let Some(idx) = self
            .queue
            .iter()
            .position(|item| self.dependency_created(item))
        {
            let dequeued_item = self.queue.remove(idx);
            let parent_id = match dequeued_item.parent_id {
                ParentId::PendingFolder(client_id) => Some(
                    self.client_to_folder_id
                        .get(&client_id)
                        .expect("Client id entry should exist")
                        .expect("Folder id entry should exist"),
                ),
                ParentId::InitialFolder(folder_id) => folder_id,
            };

            match dequeued_item.content {
                RequestContent::Folder {
                    name,
                    client_id,
                    folder_id,
                } => {
                    let object_id = ObjectStoreId::ClientId(client_id);
                    let object_uid = object_id.uid();
                    UpdateManager::handle(ctx).update(ctx, |update_manager, ctx| {
                        update_manager.create_folder(
                            name,
                            dequeued_item.owner,
                            client_id,
                            parent_id,
                            false,
                            InitiatedBy::User,
                            ctx,
                        );
                    });
                    if let Some(value) = self.client_to_folder_id.get_mut(&client_id) {
                        *value = Some(object_id);
                    }
                    ctx.emit(ImportQueueEvent::FolderCompleted {
                        folder_id,
                        object_uid,
                    });
                }
                RequestContent::Notebook {
                    title,
                    data,
                    client_id,
                    file_id,
                } => {
                    let object_uid = ObjectStoreId::ClientId(client_id).uid();
                    UpdateManager::handle(ctx).update(ctx, |update_manager, ctx| {
                        update_manager.create_notebook(
                            client_id,
                            dequeued_item.owner,
                            parent_id,
                            NotebookObjectModel {
                                title,
                                data,
                                ai_document_id: None,
                                conversation_id: None,
                            },
                            StoredObjectEventEntrypoint::ImportModal,
                            false,
                            ctx,
                        );
                    });
                    ctx.emit(ImportQueueEvent::FileSavedLocally(file_id));
                    if let Some(file_id) = self.file_completion.request_completed(client_id) {
                        ctx.emit(ImportQueueEvent::FileCompleted {
                            file_id,
                            object_uid,
                        });
                    }
                }
                RequestContent::Workflow {
                    workflows,
                    workflow_enums,
                    file_id,
                } => {
                    let created_client_ids = workflows
                        .iter()
                        .map(|(_, client_id)| *client_id)
                        .collect::<Vec<_>>();
                    UpdateManager::handle(ctx).update(ctx, |update_manager, ctx| {
                        // Create any new workflow enums
                        for (client_id, workflow_enum) in workflow_enums {
                            update_manager.create_workflow_enum(
                                workflow_enum,
                                dequeued_item.owner,
                                client_id,
                                StoredObjectEventEntrypoint::ImportModal,
                                false,
                                ctx,
                            );
                        }

                        // Create the workflow
                        for (workflow, client_id) in workflows {
                            update_manager.create_workflow(
                                workflow,
                                dequeued_item.owner,
                                parent_id,
                                client_id,
                                StoredObjectEventEntrypoint::ImportModal,
                                false,
                                ctx,
                            );
                        }
                    });
                    ctx.emit(ImportQueueEvent::FileSavedLocally(file_id));
                    for client_id in created_client_ids {
                        if let Some(file_id) = self.file_completion.request_completed(client_id) {
                            ctx.emit(ImportQueueEvent::FileCompleted {
                                file_id,
                                object_uid: ObjectStoreId::ClientId(client_id).uid(),
                            });
                        }
                    }
                }
            }
            self.dequeue(ctx);
        }
    }
}

impl Entity for ImportQueue {
    type Event = ImportQueueEvent;
}
