//! Entity submodel that encapsulates all filesystem access for diff application.
//!
//! The executor holds a [`ModelHandle<ApplyDiffModel>`] and calls
//! [`ApplyDiffModel::apply_diffs`] without knowing which filesystem backend
//! backs the active session. Internally the method resolves the session context
//! and dispatches through either a current-app filesystem closure or an
//! Environment Runtime client-backed closure.

use ai::diff_validation::AIRequestedCodeDiff;
use futures::FutureExt;
use vec1::Vec1;
use warpui::r#async::BoxFuture;
use warpui::{Entity, ModelContext, ModelHandle};

use crate::ai::agent::FileEdit;
use crate::ai::blocklist::SessionContext;
use crate::terminal::model::session::active_session::ActiveSession;
use crate::workspace::environment_runtime::{self, EnvironmentRuntimeClient};

use super::diff_application::{apply_edits, DiffApplicationError, FileReadResult};

/// Entity submodel that encapsulates filesystem access for diff application.
///
/// Held as a [`ModelHandle`] by the [`super::RequestFileEditsExecutor`].
pub(crate) struct ApplyDiffModel {
    active_session: ModelHandle<ActiveSession>,
}

impl Entity for ApplyDiffModel {
    type Event = ();
}

impl ApplyDiffModel {
    pub fn new(active_session: ModelHandle<ActiveSession>) -> Self {
        Self { active_session }
    }

    /// Resolves session context and environment client from the model context, then
    /// returns a future that applies the edits through the active filesystem backend.
    pub fn apply_diffs(
        &self,
        edits: Vec<FileEdit>,
        ctx: &mut ModelContext<Self>,
    ) -> BoxFuture<'static, Result<Vec<AIRequestedCodeDiff>, Vec1<DiffApplicationError>>> {
        let session_context = SessionContext::from_session(self.active_session.as_ref(ctx), ctx);
        let environment_client = session_context
            .host_id()
            .and_then(|host_id| environment_runtime::client_for_host(host_id, ctx));

        let uses_environment_runtime = session_context.uses_environment_runtime();
        let fut = async move {
            if uses_environment_runtime {
                match environment_client {
                    Some(client) => {
                        apply_edits(edits, &session_context, |path| {
                            let client = client.clone();
                            async move { read_environment_file(&client, &path).await }
                        })
                        .await
                    }
                    None => Err(vec1::vec1![
                        DiffApplicationError::EnvironmentFileOperationsUnsupported
                    ]),
                }
            } else {
                apply_edits(edits, &session_context, |path| async move {
                    FileReadResult::from(std::fs::read_to_string(path))
                })
                .await
            }
        };
        cfg_if::cfg_if! {
            if #[cfg(target_family = "wasm")] {
                fut.boxed_local()
            } else {
                fut.boxed()
            }
        }
    }
}

// ── Environment Runtime file reading ───────────────────────────────────────────────

/// Per-file byte limit for Environment Runtime diff application (10 MB).
const MAX_DIFF_READ_BYTES: u32 = 10_000_000;

async fn read_environment_file(client: &EnvironmentRuntimeClient, path: &str) -> FileReadResult {
    let request = environment_runtime::EnvironmentRuntimeReadFileContextRequest {
        files: vec![environment_runtime::EnvironmentRuntimeReadFile {
            path: path.to_string(),
            line_ranges: vec![],
        }],
        max_file_bytes: Some(MAX_DIFF_READ_BYTES),
        max_batch_bytes: None,
    };
    match environment_runtime::read_file_context(client, request).await {
        Ok(response) => {
            if let Some(fc) = response.file_contexts.into_iter().next() {
                // A whole-file read that was truncated by the byte limit will
                // have line_range_start/end set even though no ranges were
                // requested. Detect this and fail explicitly rather than
                // applying the diff to partial content.
                if fc.line_range.is_some() {
                    return FileReadResult::ReadError(format!(
                        "File exceeds the {MAX_DIFF_READ_BYTES}-byte limit for Environment Runtime diff \
                         application and was truncated. The diff cannot be applied safely."
                    ));
                }
                match fc.content {
                    Some(environment_runtime::EnvironmentRuntimeFileContent::Text(content)) => {
                        FileReadResult::Found(content)
                    }
                    Some(environment_runtime::EnvironmentRuntimeFileContent::Binary(_)) => {
                        // apply-diff only works with text files
                        FileReadResult::ReadError("File is binary".to_string())
                    }
                    None => FileReadResult::Found(String::new()),
                }
            } else if let Some(failed) = response.failed_files.into_iter().next() {
                let message = failed
                    .message
                    .unwrap_or_else(|| "Unknown error".to_string());
                if message.contains("not found") || message.contains("Not found") {
                    FileReadResult::NotFound
                } else {
                    FileReadResult::ReadError(message)
                }
            } else {
                FileReadResult::NotFound
            }
        }
        Err(err) => FileReadResult::ReadError(format!("{err}")),
    }
}
