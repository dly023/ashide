use std::path::{Path, PathBuf};

use futures::{future::BoxFuture, FutureExt};
use warpui::{Entity, EntityId, ModelContext, ModelHandle, SingletonEntity};

use crate::{
    ai::{
        agent::{
            AIAgentAction, AIAgentActionResultType, AIAgentActionType, ReadFilesRequest,
            ReadFilesResult,
        },
        blocklist::BlocklistAIPermissions,
        paths::host_native_absolute_path,
    },
    terminal::model::session::{active_session::ActiveSession, SessionType},
    workspace::environment_runtime,
};

use super::{
    read_current_app_file_context, ActionExecution, AnyActionExecution, ExecuteActionInput,
    PreprocessActionInput,
};

pub struct ReadFilesExecutor {
    active_session: ModelHandle<ActiveSession>,
    terminal_view_id: EntityId,
}

impl ReadFilesExecutor {
    pub fn new(active_session: ModelHandle<ActiveSession>, terminal_view_id: EntityId) -> Self {
        Self {
            active_session,
            terminal_view_id,
        }
    }

    pub(super) fn should_autoexecute(
        &self,
        input: ExecuteActionInput,
        ctx: &mut ModelContext<Self>,
    ) -> bool {
        let ExecuteActionInput {
            action:
                AIAgentAction {
                    action: AIAgentActionType::ReadFiles(ReadFilesRequest { locations }),
                    ..
                },
            conversation_id,
        } = input
        else {
            return false;
        };

        let session_type = self.active_session.as_ref(ctx).session_type(ctx);
        if session_type
            .as_ref()
            .is_some_and(SessionType::uses_environment_runtime)
        {
            return BlocklistAIPermissions::as_ref(ctx)
                .can_read_environment_files_with_conversation(
                    &conversation_id,
                    Some(self.terminal_view_id),
                    ctx,
                )
                .is_allowed();
        }

        // TODO: figure out how to avoid constructing the full paths in `should_execute`
        // and then again in `execute`, and then again on every render.
        let current_working_directory = self
            .active_session
            .as_ref(ctx)
            .current_working_directory()
            .cloned();
        let shell = self.active_session.as_ref(ctx).shell_launch_data(ctx);

        BlocklistAIPermissions::as_ref(ctx)
            .can_read_files_with_conversation(
                &conversation_id,
                locations
                    .iter()
                    .map(|file| {
                        PathBuf::from(host_native_absolute_path(
                            &file.name,
                            &shell,
                            &current_working_directory,
                        ))
                    })
                    .collect(),
                Some(self.terminal_view_id),
                ctx,
            )
            .is_allowed()
    }

    pub(super) fn execute(
        &mut self,
        input: ExecuteActionInput,
        ctx: &mut ModelContext<Self>,
    ) -> impl Into<AnyActionExecution> {
        let ExecuteActionInput {
            action,
            conversation_id,
            ..
        } = input;
        let AIAgentAction {
            action: AIAgentActionType::ReadFiles(ReadFilesRequest { locations }),
            ..
        } = action
        else {
            return ActionExecution::InvalidAction;
        };

        let session_type = self.active_session.as_ref(ctx).session_type(ctx);
        if !session_type
            .as_ref()
            .is_some_and(SessionType::uses_environment_runtime)
        {
            BlocklistAIPermissions::handle(ctx).update(ctx, |model, _ctx| {
                model.add_temporary_file_read_permissions(
                    conversation_id,
                    locations.iter().map(|file| Path::new(&file.name)),
                );
            });
        }

        let current_working_directory = self
            .active_session
            .as_ref(ctx)
            .current_working_directory()
            .cloned();
        let shell = self.active_session.as_ref(ctx).shell_launch_data(ctx);

        let locations = locations.clone();

        // Environment Runtime sessions need a connected host client; current-app
        // sessions continue to use the regular file system path.
        let environment_client = session_type
            .as_ref()
            .and_then(SessionType::environment_runtime_host_id)
            .and_then(|host_id| environment_runtime::client_for_host(host_id, ctx));

        if session_type
            .as_ref()
            .is_some_and(SessionType::uses_environment_runtime)
            && environment_client.is_none()
        {
            return ActionExecution::Sync(AIAgentActionResultType::ReadFiles(
                ReadFilesResult::Error(
                    "The file read/edit tool is not available until this Environment Runtime session is connected. \
                     Try again after the environment finishes connecting, or use a different tool."
                        .to_string(),
                ),
            ));
        }

        if let Some(client) = environment_client {
            return ActionExecution::Async {
                execute_future: Box::pin(async move {
                    let request = environment_runtime::EnvironmentRuntimeReadFileContextRequest {
                        files: locations
                            .iter()
                            .map(|loc| {
                                let absolute_path = host_native_absolute_path(
                                    &loc.name,
                                    &shell,
                                    &current_working_directory,
                                );
                                environment_runtime::EnvironmentRuntimeReadFile {
                                    path: absolute_path,
                                    line_ranges: loc
                                        .lines
                                        .iter()
                                        .map(|r| r.start as u32..r.end as u32)
                                        .collect(),
                                }
                            })
                            .collect(),
                        max_file_bytes: None,
                        max_batch_bytes: None,
                    };

                    let response = environment_runtime::read_file_context(&client, request)
                        .await
                        .map_err(|e| anyhow::anyhow!("Remote read failed: {e}"))?;

                    if !response.failed_files.is_empty() && response.file_contexts.is_empty() {
                        let failed = response
                            .failed_files
                            .iter()
                            .map(|f| {
                                let reason = f.message.as_deref().unwrap_or("unknown error");
                                format!("{}: {reason}", f.path)
                            })
                            .collect::<Vec<_>>()
                            .join(", ");
                        return Ok(ReadFilesResult::Error(format!(
                            "Failed to read files: {failed}"
                        )));
                    }

                    let file_contexts = response
                        .file_contexts
                        .into_iter()
                        .filter_map(|fc| {
                            let content = match fc.content? {
                                environment_runtime::EnvironmentRuntimeFileContent::Text(text) => {
                                    crate::ai::agent::AnyFileContent::StringContent(text)
                                }
                                environment_runtime::EnvironmentRuntimeFileContent::Binary(
                                    bytes,
                                ) => crate::ai::agent::AnyFileContent::BinaryContent(bytes),
                            };
                            Some(crate::ai::agent::FileContext {
                                file_name: fc.file_name,
                                content,
                                line_range: fc.line_range,
                                last_modified: fc.last_modified,
                                line_count: fc.line_count,
                            })
                        })
                        .collect();

                    Ok(ReadFilesResult::Success {
                        files: file_contexts,
                    })
                }),
                on_complete: Box::new(|res: Result<ReadFilesResult, anyhow::Error>, _ctx| {
                    let action_result =
                        res.unwrap_or_else(|e| ReadFilesResult::Error(e.to_string()));
                    AIAgentActionResultType::ReadFiles(action_result)
                }),
            };
        }

        // Local path.
        ActionExecution::Async {
            execute_future: Box::pin(async move {
                let result = read_current_app_file_context(
                    &locations,
                    current_working_directory,
                    shell,
                    None,
                    None,
                )
                .await?;
                if result.missing_files.is_empty() {
                    Ok(ReadFilesResult::Success {
                        files: result.file_contexts,
                    })
                } else {
                    let missing_files = result.missing_files.join(", ");
                    Ok(ReadFilesResult::Error(format!(
                        "These files do not exist: {missing_files}"
                    )))
                }
            }),
            on_complete: Box::new(|res: Result<ReadFilesResult, anyhow::Error>, _ctx| {
                let action_result = res.unwrap_or_else(|e| ReadFilesResult::Error(e.to_string()));
                AIAgentActionResultType::ReadFiles(action_result)
            }),
        }
    }

    pub(super) fn preprocess_action(
        &mut self,
        _input: PreprocessActionInput,
        _ctx: &mut ModelContext<Self>,
    ) -> BoxFuture<'static, ()> {
        futures::future::ready(()).boxed()
    }
}

impl Entity for ReadFilesExecutor {
    type Event = ();
}
