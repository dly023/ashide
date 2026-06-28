use super::{ActionExecution, AnyActionExecution, ExecuteActionInput, PreprocessActionInput};
use crate::ai::agent::AIAgentActionResultType;
use crate::ai::skills::environment_skill_inventory::{
    environment_skill_inventory_command, parse_environment_skill_inventory_stdout,
};
#[cfg(feature = "local_fs")]
use crate::ai::skills::extract_skill_parent_directory;
use crate::ai::skills::SkillManager;
use ai::agent::action_result::AnyFileContent;
#[cfg(feature = "local_fs")]
use ai::skills::parse_skill;
use ai::skills::SkillReference;
use std::{collections::HashMap, path::Path};
use warpui::{ModelContext, SingletonEntity};

use crate::ai::agent::AIAgentActionType;
use crate::ai::agent::ReadSkillRequest;
use crate::ai::agent::ReadSkillResult;
use crate::terminal::model::session::{active_session::ActiveSession, SessionId, SessionType};
use crate::workspace::environment_runtime::{
    self, EnvironmentRuntimeClient, EnvironmentRuntimeFileContent, EnvironmentRuntimeReadFile,
    EnvironmentRuntimeReadFileContextRequest,
};
use ai::agent::action_result::FileContext;
use futures::future::{BoxFuture, FutureExt};
use std::sync::Arc;
use warpui::{Entity, ModelHandle};

pub struct ReadSkillExecutor {
    active_session: ModelHandle<ActiveSession>,
}

struct CurrentAppSkillReadProvider<'a> {
    manager: &'a SkillManager,
}

#[derive(Clone)]
struct EnvironmentSkillReadProvider {
    client: Arc<EnvironmentRuntimeClient>,
    session_id: SessionId,
    current_working_directory: Option<String>,
}

enum SkillReadProvider<'a> {
    CurrentApp(CurrentAppSkillReadProvider<'a>),
    Environment(EnvironmentSkillReadProvider),
}

impl<'a> CurrentAppSkillReadProvider<'a> {
    fn new(ctx: &'a ModelContext<ReadSkillExecutor>) -> Self {
        Self {
            manager: SkillManager::as_ref(ctx),
        }
    }

    fn skill_by_reference(
        &self,
        skill_ref: &SkillReference,
    ) -> Option<&'a ai::skills::ParsedSkill> {
        self.manager.skill_by_reference(skill_ref)
    }

    fn find_skill_by_name(&self, name: &str) -> Option<&'a ai::skills::ParsedSkill> {
        self.manager.find_skill_by_name(name)
    }

    #[cfg(feature = "local_fs")]
    fn can_read_current_app_path(&self, path: &Path) -> bool {
        extract_skill_parent_directory(path).is_ok()
    }
}

impl ReadSkillExecutor {
    pub fn new(active_session: ModelHandle<ActiveSession>) -> Self {
        Self { active_session }
    }

    pub(super) fn should_autoexecute(
        &self,
        _input: ExecuteActionInput,
        _ctx: &mut ModelContext<Self>,
    ) -> bool {
        // User-created skills are readable on demand.
        true
    }

    pub(super) fn execute(
        &mut self,
        input: ExecuteActionInput,
        ctx: &mut ModelContext<Self>,
    ) -> impl Into<AnyActionExecution> {
        let ExecuteActionInput { action, .. } = input;
        let AIAgentActionType::ReadSkill(ReadSkillRequest { skill: skill_ref }) = &action.action
        else {
            return ActionExecution::InvalidAction;
        };

        let skill_provider = match self.skill_read_provider(ctx) {
            Ok(provider) => provider,
            Err(error) => return ActionExecution::Sync(ReadSkillResult::Error(error).into()),
        };

        let skill_provider = match skill_provider {
            SkillReadProvider::Environment(provider) => return provider.execute_read(skill_ref),
            SkillReadProvider::CurrentApp(provider) => provider,
        };

        // Cache hit:proto 的 `SkillReference::Path(p)` 在这一步只在 p 恰好就是
        // 索引中真实 SKILL.md 绝对路径时命中。
        if let Some(skill) = skill_provider.skill_by_reference(skill_ref) {
            return success_execution(skill);
        }

        // BYOP `read_skill` 工具的实参是 skill **name**,被 `from_args` 装进
        // `SkillReference::SkillPath(name)` 槽位(避免 proto schema 变更)。
        // 这里在 cache miss 时按 name 反查真实 SKILL.md 路径,覆盖 Skill 管理器
        // 能看到的所有 skill(文件 skill + bundled skill)。
        if let SkillReference::Path(p) = skill_ref {
            if let Some(candidate_name) = name_candidate(p) {
                if let Some(skill) = skill_provider.find_skill_by_name(candidate_name) {
                    return success_execution(skill);
                }
            }
        }

        // Cache miss 兜底:对于 `SkillReference::Path` 形式的引用,
        // 如果路径形状是合法的 skill 文件
        // (`.../<provider>/skills/<name>/SKILL.md` 或 warp managed skill 目录下),
        // 直接读盘解析,修复 issue #99 中描述的「skill 已存在但 cache 未热」场景。
        //
        // 设计取舍:
        // - 不主动 warm SkillManager cache。Cache 由 SkillWatcher 单向维护,
        //   在这里写入会破坏数据流。重复 read_skill 同一路径会重复读盘,
        //   但 SKILL.md 通常很小,可忽略。
        // - `extract_skill_parent_directory` 只校验路径形状,与 cache hit 时
        //   返回的 path 安全等级一致 —— 都不限定家目录前缀。这是有意的:
        //   project 内 skill (`/some/repo/.agents/skills/...`) 也需要能读。
        // - Windows 下正则用反斜杠分隔,Linux 风格 `/home/<u>/...` 路径会被
        //   拒绝;这意味着本兜底对 "Windows 主进程 + WSL session" 不生效,
        //   是 issue #99 的已知限制(见 PR 描述)。
        // Cache miss fallback 仅在拥有本地文件系统的构建中可用;
        // WASM 等无 fs 构建里 `extract_skill_parent_directory` / `parse_skill`
        // 不存在,自然也无从读盘。
        #[cfg(feature = "local_fs")]
        if let SkillReference::Path(path) = skill_ref {
            if skill_provider.can_read_current_app_path(path) {
                let path = path.clone();
                let skill_ref_for_async = skill_ref.clone();
                return ActionExecution::new_async(
                    async move {
                        parse_skill(&path)
                            .map(|skill| file_context_for_skill(&skill))
                            .map_err(|err| {
                                format!("Skill not found: {skill_ref_for_async:?} ({err})")
                            })
                    },
                    move |result, _app| match result {
                        Ok(content) => {
                            AIAgentActionResultType::ReadSkill(ReadSkillResult::Success { content })
                        }
                        Err(err) => AIAgentActionResultType::ReadSkill(ReadSkillResult::Error(err)),
                    },
                );
            }
        }

        ActionExecution::Sync(
            ReadSkillResult::Error(format!("Skill not found: {:?}", skill_ref)).into(),
        )
    }

    pub(super) fn preprocess_action(
        &mut self,
        _input: PreprocessActionInput,
        _ctx: &mut ModelContext<Self>,
    ) -> BoxFuture<'static, ()> {
        futures::future::ready(()).boxed()
    }

    fn skill_read_provider<'a>(
        &self,
        ctx: &'a ModelContext<Self>,
    ) -> Result<SkillReadProvider<'a>, String> {
        let session_type = self.active_session.as_ref(ctx).session_type(ctx);
        if !session_type
            .as_ref()
            .is_some_and(SessionType::uses_environment_runtime)
        {
            return Ok(SkillReadProvider::CurrentApp(
                CurrentAppSkillReadProvider::new(ctx),
            ));
        }

        let Some(session) = self.active_session.as_ref(ctx).session(ctx) else {
            return Err(
                "The read_skill tool is not available until this Environment Runtime session is active."
                    .to_string(),
            );
        };
        let session_id = session.id();

        let Some(host_id) = session_type
            .as_ref()
            .and_then(SessionType::environment_runtime_host_id)
        else {
            return Err(
                "The read_skill tool is not available until this Environment Runtime session is connected. \
                 Try again after the environment finishes connecting."
                    .to_string(),
            );
        };

        let Some(client) = environment_runtime::client_for_host(host_id, ctx) else {
            return Err(format!(
                "The read_skill tool cannot reach Environment Runtime host {host_id}. \
                 Try reconnecting the environment."
            ));
        };

        Ok(SkillReadProvider::Environment(
            EnvironmentSkillReadProvider {
                client,
                session_id,
                current_working_directory: self
                    .active_session
                    .as_ref(ctx)
                    .current_working_directory()
                    .cloned(),
            },
        ))
    }
}

impl EnvironmentSkillReadProvider {
    fn execute_read(
        self,
        skill_ref: &SkillReference,
    ) -> ActionExecution<Result<FileContext, String>> {
        let SkillReference::Path(path) = skill_ref else {
            return ActionExecution::Sync(
                ReadSkillResult::Error(
                    "Bundled skills are current-app skills; Environment Runtime read_skill only supports remote SKILL.md paths until EnvironmentSkillProvider indexing is available."
                        .to_string(),
                )
                .into(),
            );
        };

        if !is_skill_path(path) {
            if let Some(skill_name) = name_candidate(path) {
                let skill_name = skill_name.to_string();
                return ActionExecution::new_async(
                    async move { self.read_by_name(skill_name).await },
                    |result, _app| {
                        AIAgentActionResultType::ReadSkill(match result {
                            Ok(content) => ReadSkillResult::Success { content },
                            Err(error) => ReadSkillResult::Error(error),
                        })
                    },
                );
            }

            return ActionExecution::Sync(
                ReadSkillResult::Error(format!(
                    "Skill not found in this environment: {:?}",
                    skill_ref
                ))
                .into(),
            );
        }

        let path = path.to_string_lossy().into_owned();
        ActionExecution::new_async(
            async move { read_environment_skill_file(self.client, path).await },
            |result, _app| {
                AIAgentActionResultType::ReadSkill(match result {
                    Ok(content) => ReadSkillResult::Success { content },
                    Err(error) => ReadSkillResult::Error(error),
                })
            },
        )
    }

    async fn read_by_name(self, skill_name: String) -> Result<FileContext, String> {
        let path = resolve_environment_skill_name(
            self.client.clone(),
            self.session_id,
            self.current_working_directory.clone(),
            &skill_name,
        )
        .await?;
        read_environment_skill_file(self.client, path).await
    }
}

async fn resolve_environment_skill_name(
    client: Arc<EnvironmentRuntimeClient>,
    session_id: SessionId,
    current_working_directory: Option<String>,
    skill_name: &str,
) -> Result<String, String> {
    if !is_plain_skill_name(skill_name) {
        return Err(format!(
            "Skill not found in this environment: {skill_name:?}"
        ));
    }

    let output = environment_runtime::run_command_output(
        &client,
        session_id,
        environment_skill_inventory_command(),
        current_working_directory,
        HashMap::new(),
    )
    .await?;

    match output.exit_code {
        Some(0) => {
            for skill in parse_environment_skill_inventory_stdout(&output.stdout)? {
                if skill.name == skill_name {
                    let SkillReference::Path(path) = skill.reference else {
                        continue;
                    };
                    return Ok(path.to_string_lossy().into_owned());
                }
            }
        }
        _ => {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            if stderr.is_empty() {
                return Err("Remote skill inventory scan failed".to_string());
            }
            return Err(format!("Remote skill inventory scan failed: {stderr}"));
        }
    }

    Err(format!(
        "Skill not found in this environment: {skill_name:?}"
    ))
}

async fn read_environment_skill_file(
    client: Arc<EnvironmentRuntimeClient>,
    path: String,
) -> Result<FileContext, String> {
    let response = environment_runtime::read_file_context(
        &client,
        EnvironmentRuntimeReadFileContextRequest {
            files: vec![EnvironmentRuntimeReadFile {
                path: path.clone(),
                line_ranges: Vec::new(),
            }],
            max_file_bytes: None,
            max_batch_bytes: None,
        },
    )
    .await
    .map_err(|error| format!("Remote skill read failed: {error}"))?;

    if let Some(file) = response.file_contexts.into_iter().next() {
        let content = match file.content {
            Some(EnvironmentRuntimeFileContent::Text(text)) => AnyFileContent::StringContent(text),
            Some(EnvironmentRuntimeFileContent::Binary(_)) => {
                return Err(format!(
                    "Remote skill is not a text file: {}",
                    file.file_name
                ));
            }
            None => {
                return Err(format!(
                    "Remote skill did not include file content: {}",
                    file.file_name
                ));
            }
        };

        return Ok(FileContext::new(
            file.file_name,
            content,
            file.line_range,
            file.last_modified,
        ));
    }

    let failed = response
        .failed_files
        .into_iter()
        .map(|file| {
            let reason = file.message.unwrap_or_else(|| "unknown error".to_string());
            format!("{}: {reason}", file.path)
        })
        .collect::<Vec<_>>()
        .join(", ");
    if failed.is_empty() {
        Err(format!("Remote skill not found: {path}"))
    } else {
        Err(format!("Remote skill not found: {failed}"))
    }
}

/// Build a sync success execution from a parsed skill.
///
/// 抽出 helper 是为了让 `ActionExecution<T>` 的泛型 `T` 在 `success_execution`
/// 和 `new_async` 两条路径里推导到相同类型(否则 Rust 会要求函数显式声明返回类型)。
fn success_execution(
    skill: &ai::skills::ParsedSkill,
) -> ActionExecution<Result<FileContext, String>> {
    ActionExecution::Sync(
        ReadSkillResult::Success {
            content: file_context_for_skill(skill),
        }
        .into(),
    )
}

fn file_context_for_skill(skill: &ai::skills::ParsedSkill) -> FileContext {
    FileContext::new(
        skill.path.to_string_lossy().into_owned(),
        AnyFileContent::StringContent(skill.content.clone()),
        skill.line_range.clone(),
        None,
    )
}

/// 判断 `SkillReference::Path` 中的值是否应当被当作 skill **name** 反查。
///
/// 真实 SKILL.md 路径包含路径分隔符(`/` 或 `\`)或是绝对路径,而 BYOP
/// 工具调用的 name(如 `"build-feature"`)是纯字符串。把这两类区分开,
/// 避免把 `/home/.../SKILL.md` 误解为 name 而错过文件系统 fallback。
fn name_candidate(p: &Path) -> Option<&str> {
    if p.is_absolute() {
        return None;
    }
    let s = p.to_str()?;
    if s.is_empty() || s.contains('/') || s.contains('\\') {
        return None;
    }
    Some(s)
}

fn is_plain_skill_name(name: &str) -> bool {
    !name.is_empty()
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains('\0')
        && name != "."
        && name != ".."
}

#[cfg(feature = "local_fs")]
fn is_skill_path(path: &Path) -> bool {
    extract_skill_parent_directory(path).is_ok()
}

#[cfg(not(feature = "local_fs"))]
fn is_skill_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == "SKILL.md")
}

impl Entity for ReadSkillExecutor {
    type Event = ();
}

#[cfg(test)]
#[path = "read_skill_tests.rs"]
mod tests;
