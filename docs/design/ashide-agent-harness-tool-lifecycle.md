# Ashide Agent Harness 工具调用连续性重构

> 状态：协作设计稿 / 评审 brief  
> 日期：2026-07-02  
> 背景：用户反馈内置 Ashide Agent 连续性差，尤其“经常调用着就可能是工具调用的原因挂掉”。本文件用于给多个 AI / reviewer 并行看背景、计划和取舍。

## 0. 当前交接快照

截至 2026-07-02，本轮已经落地 **阶段 B 的第一刀**：

- controller preflight 遇到 `MissingResultWithoutRepairSource { reason: NoResult }` 时，不再直接 blocked。
- 新增 `ByopToolResultRepair`，为确认已经没有 live executor / finished result / persisted result 的悬挂 tool call 合成一条 diagnostic cancellation `ToolCallResult`。
- synthetic payload 明确带 `synthetic=true` 和 `repair_source=byop_missing_tool_result`，避免调试时误判成真实工具返回。
- 已跑验证：
  - `cargo test -p warp tool_lifecycle --lib`
  - `cargo check`

仍未完成：

- controller 级集成测试还需要补：真实 history 中 assistant `ToolCall` 无 `ToolCallResult`，preflight 后应落地 synthetic result 并继续。
- MCP dead transport retry-once 还没开始实现。
- `AIAgentActionResultType::should_trigger_request_upon_completion()` 仍是 bool，后续应改成集中 continuation decision。

## 1. 一句话目标

把 Ashide Agent 的工具调用从“各工具各自返回结果、上层用 conversation status 猜最终状态”重构为一套明确的 **tool lifecycle / recovery state machine**：

- 工具调用失败默认是 **模型可见的 tool result**，不是 harness fatal。
- 权限等待、用户接管、取消、stream drop、MCP server 死连接，要有可区分状态。
- 内置 Ashide harness、本地/远程 Environment、CLI/current-app 路径共享同一套语义。
- 不追求最小闭环；如果问题暴露状态机、生命周期或抽象边界不干净，就趁早期重构。

## 2. 用户问题与产品判断

### 用户看到的问题

- Ashide Agent 连续性差。
- 工具调用中途可能让 agent “挂掉”。
- 多轮 terminal-native agent loop 不稳定：人、终端、agent 本应能交替接管同一个现场。
- 本地/远程行为不应因为底层实现不同而表现不一致。

### 产品判断

Ashide Agent 的优势不应该只是“一个 agent CLI 内嵌在终端里”，而应该是：

1. **terminal-native loop**：agent 使用真实 terminal / pane / Environment；用户可以随时接管，agent 之后能继续。
2. **现场连续性**：同一个 terminal 现场里的 shell 输出、工具结果、permission decision、用户插手，都能回到模型上下文。
3. **失败可恢复**：工具失败多数时候是任务过程的一部分，应该反馈给模型重试或改计划，而不是直接结束 run。
4. **本地/远程统一**：工具能力差异应该来自 Environment capability，而不是散落在 UI / harness / executor 的 local/remote 分支。

## 3. 当前代码入口地图

### Run / harness 层

- `app/src/ai/agent_sdk/driver.rs`
  - `AgentDriver::execute_run`
  - 监听 `BlocklistAIHistoryEvent::UpdatedConversationStatus`
  - 把 `AmbientConversationStatus::{Success, Cancelled, Error, Blocked}` 转成 `SDKConversationOutputStatus`
  - 当前问题：这里更像“终态监听器”，不是 tool lifecycle owner。它只能看最终 conversation status，无法知道工具失败是否应 resume。

### 工具执行层

- `app/src/ai/blocklist/action_model/execute.rs`
  - `BlocklistAIActionExecutor::try_to_execute_action`
  - 判定 autoexecute / needs confirmation / not ready
  - 分发到各工具 executor
  - emit：
    - `BlocklistAIActionExecutorEvent::ExecutingAction`
    - `BlocklistAIActionExecutorEvent::FinishedAction`
- `app/src/ai/blocklist/action_model/execute/call_mcp_tool.rs`
  - MCP tool 调用执行器
  - 当前失败会被压成 `CallMCPToolResult::Error(String)`
  - 死连接 / server missing / 工具自身错误 / JSON schema 错误没有统一分类。

### Action result 类型层

- `crates/ai/src/agent/action_result/mod.rs`
  - `AIAgentActionResultType`
  - `is_successful()`
  - `is_failed()`
  - `is_cancelled()`
  - `should_trigger_request_upon_completion()`
  - 当前问题：这些方法只描述“结果长什么样”，缺少“结果对 conversation lifecycle 意味着什么”的统一策略。

### Controller / BYOP readiness 层

- `app/src/ai/blocklist/controller.rs`
  - `send_request_input`
  - `run_byop_request_preflight`
  - `commit_byop_current_action_results`
  - `commit_byop_finished_action_results`
  - `commit_byop_cancellation_results`
  - `flush_pending_byop_request_after_finished_action`
  - synthetic tool result auto-resume 逻辑
- 当前已有框架：
  - 如果 BYOP preflight 发现 `PendingToolResults`，会暂存用户输入，等 `FinishedAction` 后再 flush。
  - 对 synthetic `invalid_arguments` / `_byop_intercepted` tool result 有 auto-resume。
  - 但对“工具 future 丢失 / 被打断后没有 result / MCP 死连接”等缺口还没有统一收敛。

## 4. 已发现的关键缺口

### 4.1 工具错误语义被压扁

以 MCP 为例，当前 `handle_call_tool_result` 大致把失败都转成：

```rust
CallMCPToolResult::Error(error_message)
```

这让上层无法区分：

- 工具自身返回 error：应给模型看，继续下一轮。
- MCP server 死连接：应重启/重连一次，再失败才给模型看。
- MCP server 不存在：可能是 Environment capability / 配置问题。
- 工具输入不是 object：模型参数错误，应立即给模型错误结果并 auto-resume。
- 用户取消：不应当和工具错误混在一起。

### 4.2 action result 与 conversation status 边界不干净

理想状态：

- tool result 是一条模型可消费的事实。
- conversation status 是 run 级生命周期状态。
- 只有 fatal provider/runtime 错误才让 conversation 进入 terminal error。

当前风险：

- 某些工具异常可能绕过正常 tool result，最后表现成 conversation error / blocked。
- `AgentDriver::execute_run` 只能被动看终态；如果上游状态被错误标成 Error，它只能结束 run。

### 4.3 缺失 tool result 会卡死

BYOP / tool-call 协议要求 tool call 与 tool result 配对。缺 result 会导致下一轮请求被阻断或模型无反馈。

当前 Ashide 有 readiness preflight，但与 `refs/zap-latest` 对照发现一个重要差异：

- `refs/zap-latest/app/src/ai/blocklist/controller.rs` 对
  `MissingResultWithoutRepairSource { reason: NoResult }`
  有“合成 cancellation ToolCallResult”的自愈分支。
- 当前 Ashide 的对应分支会归入 blocked error 类别。

这很可能解释“用户中断 / 工具 future 被 drop / long-running command 取消竞态后，agent 后续连续性变差”。

### 4.4 MCP server lifecycle 没有统一 recovery policy

`refs/deepx-code-main/mcp/manager.go` 里有一个值得参考的策略：

- `callToolWithRestart` 调工具。
- 如果错误像“连接已死”，自动 `Restart(server)`，然后重试一次。
- 非死连接错误正常透传给模型。
- 有 restart cooldown，防止并发/坏 server 导致无限重启刷屏。

Ashide 当前 MCP `reconnecting_peer` 名字上像支持重连，但 tool executor 层没有把“连接死了 → 重连/重试一次 → 失败转模型可见结果”表达成统一策略。

## 5. 参考实现对照

### 5.1 repo 内 refs

| 来源 | 可参考点 | 优点 | 风险 / 不可直接照搬点 |
|---|---|---|---|
| `refs/zap-latest` | BYOP readiness；缺失 tool result 合成 cancellation；synthetic tool result auto-resume | 与当前代码同源，迁移成本低；已有 issue 注释和测试方向 | 只是局部自愈，不是完整 tool lifecycle；可能继续把策略散落在 controller |
| `refs/deepx-code-main` | MCP `callToolWithRestart`；`sanitizeToolPairs`；codegraph 工具 | 策略简单清晰：死连接重启重试一次，工具错误透传；有并发/冷却测试 | Go CLI 模型较轻，没有 Ashide 的 UI/history/conversation 状态复杂度 |
| `refs/chatbridge-main` | session 恢复、错误 envelope、TUI cancel 恢复 | 错误展示与恢复路径明确 | 不是在线 agent harness，不能指导 tool execution state machine |
| `refs/memory-forge-rs-main` | UI 错误展示、操作状态恢复 | 可参考 UI status/error tone | 与 agent tool lifecycle 关系弱 |

### 5.2 外部开源仓库对照

已拉取到 `refs/external/` 并做第一轮对照：

| 仓库 / 项目 | 重点发现 | 可吸收点 | 不直接照搬点 |
|---|---|---|---|
| `openai-codex` | SDK item lifecycle 与 turn lifecycle 分离：`item.started/updated/completed` 独立于 `turn.completed/turn.failed`；MCP tool call item 有 `in_progress/completed/failed` | Ashide 不应让单个 tool failure 直接污染 run status；driver 只看 turn/run 级状态 | SDK event 模型比 Ashide GUI/history 轻，不能替代本地持久化 repair |
| `sst-opencode` | `ToolCall / ToolPartialCall / ToolResult` 有 processor 边界；`updateToolCall` / `completeToolCall` 收敛 session mutation | 工具状态更新应集中，避免 executor 到处写 session/history | TypeScript TUI 架构与 Ashide entity/history 模型不同 |
| `aider` | 更像传统 CLI：provider/file write retry 与 tool_error 文案明确 | 可以参考“错误反馈给模型而不是 crash”的 UX tone | 没有独立 tool protocol lifecycle，不能作为主架构参考 |
| `block-goose` | 有 `max_tool_repetitions`、`max_turns`、permission prompt allow/deny/cancel、extension load 失败继续 | 后续补 loop prevention / permission lifecycle 时可参考 | Runtime/extension 模型不同，MCP recovery 不应照搬 |

对照维度不是“抄代码”，而是回答：

1. 工具失败是否终止 run？
2. 哪些失败会作为 tool result 返回模型？
3. 哪些失败会触发自动 retry / reconnect？
4. 用户取消和工具错误是否区分？
5. 缺失 tool result 如何修复？
6. stream drop 是否可 resume？等待多久？
7. UI blocked / needs confirmation 是否会被 CLI harness 误判成完成？

## 6. 目标架构草案

### 6.1 引入 ToolLifecyclePolicy

建议新增一个集中策略层，名字可选：

- `ToolLifecyclePolicy`
- `AIAgentToolLifecycle`
- `ToolContinuationPolicy`

职责：

```text
AIAgentAction + execution outcome
  -> ToolLifecycleDecision
       - PersistToolResult
       - RetryExecutionOnce
       - WaitForUser
       - MarkCancelled
       - BlockConversation
       - FatalRunError
```

关键点：

- executor 负责“怎么执行工具”。
- policy 负责“执行结果对 agent loop 意味着什么”。
- controller 负责“把 decision 写入 history / queue resume / pending request flush”。
- driver 只负责 run process lifecycle，不再猜工具语义。

### 6.2 结果分类从 String error 升级为结构化原因

不要再只靠 `Error(String)` 驱动上层策略。可以引入内部结构，不一定暴露给 API：

```rust
enum ToolFailureKind {
    ModelInvalidArguments,
    ToolReturnedError,
    ToolUnavailable,
    ToolTransportDead,
    ToolTimedOut,
    PermissionDenied,
    UserCancelled,
    EnvironmentUnavailable,
    InternalBug,
}
```

取舍：

- 对模型可见的 tool result 仍可以是自然语言 / JSON error。
- 对 Ashide 内部必须保留分类，用来决定 retry、resume、UI blocked、日志级别。

### 6.3 缺失 result 的统一补洞

任何已进入 history 的 tool call，都必须最终进入以下之一：

- `Success`
- `Error`
- `Cancelled`
- `Skipped / NotExecuted`（如果协议允许；否则映射为 cancelled/error）

禁止长期存在“有 tool_call 无 tool_result”的悬挂状态。

实现策略：

- preflight 发现 `PendingToolResults`：
  - 如果 live action 仍存在：暂存用户输入，等待 `FinishedAction`。
  - 如果 live action 不存在且无 repair source：合成 cancellation result。
- 合成 result 必须带诊断 metadata，避免以后误以为是真实用户取消。
- 循环必须有 iteration cap，避免坏历史无限修。

### 6.4 MCP recovery policy

建议分三层：

1. `MCPPeer` / manager 层：判断 transport 是否死，支持 reconnect/restart。
2. executor 层：同一 tool call 最多自动重试一次。
3. lifecycle policy 层：重试后仍失败，把结构化 error 作为 tool result 返回模型，不让 harness fatal。

必要保护：

- per-server cooldown。
- 并发 coalescing：多个 tool call 遇到同一个死 server，不应同时重启 N 次。
- telemetry/log 区分：
  - first failure
  - restart attempt
  - retry success
  - retry failed

### 6.5 AgentDriver 边界收窄

`AgentDriver::execute_run` 不应该理解每种工具失败。它只处理：

- Success
- Cancelled
- Blocked waiting for explicit user action
- Fatal provider/runtime error
- bounded auto-resume wait

如果工具失败能变成模型可见 result，就不应该冒泡成 `SDKConversationOutputStatus::Error`。

## 7. 具体实施计划

### 阶段 A：补文档和对照

- [x] 写本文档。
- [x] 拉取/检查外部开源 agent 的工具调用实现。
- [x] 形成对照表：失败分类、retry、resume、UI blocked、缺 result 修复。

### 阶段 B：先收敛已有 readiness 缺口

- [x] 对比并吸收 `refs/zap-latest` 的 missing result cancellation synthesis。
- [x] 不直接散贴代码：先抽成 `controller/tool_lifecycle.rs` 里的 `ByopToolResultRepair` 可调用能力；后续再扩成完整 `ToolLifecyclePolicy`。
- [x] 补 pure repair helper 测试：
  - synthetic message 保持 `(task_id, tool_call_id)` 配对。
  - synthetic payload 带 `status/reason/synthetic/repair_source`。
- [ ] 补 controller/history 级测试：
  - 有 tool_call，无 live action，无 result → 合成 cancellation result。
  - 有 live action → pending request 等待 finished action，不合成。
  - 重复 preflight 不重复合成。

### 阶段 C：MCP tool recovery

- [ ] 给 MCP 调用结果分类。
- [ ] 对 dead transport 做 reconnect/restart + retry once。
- [ ] retry failed 后写模型可见 error result。
- [ ] 补测试：
  - 工具自身 error 不 retry，直接 result。
  - dead connection retry success。
  - dead connection retry failed → result error，不 fatal。
  - cooldown / concurrent restart coalescing。

### 阶段 D：统一 action result continuation

- [ ] 新增集中函数判断 result 完成后是否要 follow-up request。
- [ ] 避免各处直接用 `is_failed()` / `is_cancelled()` 推导 run 终态。
- [ ] CLI/current-app 和 GUI agent view 共用策略。

### 阶段 E：本地/远程一致性

- [ ] MCP/file/shell 工具执行前统一读 Environment capability。
- [ ] current-app file-based MCP 在 remote Environment 下不可用时，结果应是模型可见 capability error，而不是 harness fatal。
- [ ] 文档化 local/remote 分叉只允许存在于 backend adapter / capability provider。

## 8. 重要取舍

### 8.1 不把所有错误都 auto-resume

理由：

- 无限 resume 会掩盖真实 bug。
- 模型反复输出坏参数时会死循环。
- 当前 synthetic invalid args 已经有 `can_attempt_resume_on_error=false` 的防循环思想。

原则：

- 模型参数错误：可 auto-resume 一次。
- 工具自身错误：通常让模型看到即可；是否继续由下一轮模型决定。
- transport 死连接：自动修复一次。
- provider stream drop：按现有 bounded auto-resume，必须有 timeout。
- 内部 bug / schema 不变量破坏：不要静默吞掉，但也要尽量写诊断 result，避免历史悬挂。

### 8.2 cancellation result 是修复缺 result 的安全默认

当 tool call 已存在、没有 live executor、也没有真实 result 时，继续等待没有意义。

可选方案：

| 方案 | 优点 | 缺点 |
|---|---|---|
| 阻断请求 | fail-closed，不伪造结果 | 用户会卡死；连续性最差 |
| 合成 error result | 模型可见，可继续 | 可能让模型误以为工具失败，而不是被取消 |
| 合成 cancellation result | 表达“这次工具调用不会再产生结果”最贴近事实 | 需要 metadata 标注 synthetic，方便调试 |

当前倾向：合成 cancellation result，并带 synthetic metadata。

### 8.3 结构化内部错误，不一定扩大外部协议

短期内不要为了内部策略立刻改 public protobuf/API。可以：

- 内部保留 `ToolFailureKind`。
- 转模型时仍输出已有 `CallMcpToolResult::Error { message }` 等结构。
- 日后需要 UI 精细展示时再扩协议。

### 8.4 不把 policy 塞进每个 executor

每个 executor 都自己决定 retry/resume，会造成下一轮维护困难。

executor 应该返回：

- 成功 payload
- 失败 kind + message
- cancellation
- not ready / permission needed

统一 policy 决定：

- 是否重试
- 是否写 history
- 是否 flush pending request
- 是否让 conversation blocked/error

## 9. 给其他 AI / reviewer 的具体问题

请重点 review 以下问题：

1. `ToolLifecyclePolicy` 应放在 `crates/ai` 还是 `app/src/ai/blocklist`？
   - `crates/ai` 更通用，但可能需要依赖 app-only 类型。
   - `blocklist` 更贴近 history/controller，但会继续加重旧模块。
2. 缺失 tool result 合成 cancellation 是否足够安全？
3. MCP restart 应放在 `TemplatableMCPServerManager`、`ReconnectingPeer` 还是 `CallMCPToolExecutor`？
4. `AIAgentActionResultType::should_trigger_request_upon_completion()` 是否应该改成返回 enum，而不是 bool？
5. `AgentDriver::execute_run` 对 `Blocked` 的处理是否应该区分：
   - waiting for permission
   - unrecoverable policy block
   - user-input-needed
6. local/remote capability error 是 tool result 还是 blocked UI？
7. 是否需要把 tool lifecycle events 持久化到 history，方便 session resume 后继续 repair？

## 10. 验证口径

不能只用 cargo check 证明行为正确。分层验证：

### 单元测试

- action result classification
- readiness repair
- MCP dead connection retry
- no duplicate synthetic result
- no infinite auto-resume

### 集成/模型前测试

- 构造一段历史：assistant tool_call 无 tool_result，下一轮用户输入应自动补 cancellation 并继续。
- 构造 MCP server 第一次调用断开、重启后成功，模型应收到 success。
- 构造 MCP server 工具自身 error，模型应收到 error result，harness 不退出。

### GUI/CLI 行为验证

- CLI `ashide agent run` / 当前旧入口：工具失败后进程不应立刻 fatal，除非是真 fatal。
- GUI agent view：显示工具 error，但输入/继续状态可恢复。
- 远程 Environment：不可用 capability 应作为模型可见错误或明确 blocked，不应表现为本地路径分支崩掉。

## 11. 当前已知相关改动状态

本轮之前已有未提交改动：

- `crates/warp_cli/src/agent.rs`
- `crates/warp_cli/src/skill.rs`
- `app/src/ai/skills/file_watchers/utils_tests.rs`
- `app/src/warp_managed_paths_watcher.rs`

本文件新增：

- `docs/design/ashide-agent-harness-tool-lifecycle.md`

注意：命名 hard-cut（`Oz` → `Ashide`）先暂停；当前优先级是工具调用连续性。

## 12. 本轮落地决策记录

### 12.1 为什么先修 missing result，而不是先碰 MCP

这是最靠近用户“工具调用着挂掉 / 连续性差”的已知 deterministic gap：

1. BYOP 协议要求 tool call 与 tool result 配对。
2. 当前 readiness 已能发现缺口，但 `NoResult` 被当作 blocked error。
3. 同源 `zap-latest` 已验证过“合成 cancellation result”是可行修复。
4. 这条路径不依赖真实 MCP server，可先用 history/request preflight 测出闭环。

MCP dead connection recovery 仍然重要，但它属于 executor/transport 层；在缺 result 仍会卡死的前提下，先修 controller repair 能给后续 MCP recovery 一个稳定兜底。

### 12.2 为什么合成 cancellation，而不是 error

`MissingResultWithoutRepairSource { NoResult }` 的语义是：

- history 里已有 assistant tool_call；
- 没有持久化 tool_result；
- 没有 current input / finished action result；
- 没有 live action 仍在跑；
- 没有 repair record 授权可用。

此时事实不是“工具返回 error”，而是“这次调用不会再产生结果”。因此 cancellation 比 error 更准确。为了调试不混淆真实用户取消，合成 payload 带：

```json
{
  "status": "cancelled",
  "reason": "interrupted_by_user",
  "synthetic": true,
  "repair_source": "byop_missing_tool_result"
}
```

### 12.3 为什么先放在 `app/src/ai/blocklist/controller/tool_lifecycle.rs`

完整 `ToolLifecyclePolicy` 最终可能应该下沉到更通用的 agent/action 层，但当前第一刀依赖：

- BYOP readiness 的 `ToolCallRef`；
- blocklist history append；
- controller preflight rebuild；
- `BlocklistAIHistoryModel::append_byop_preflight_messages_to_task`。

直接放 `crates/ai` 会引入 app-only 类型倒挂。先把 pure repair helper 从 controller 主文件拆出，保留清晰 seam；等 MCP/action result continuation 一起收敛后，再判断是否抽到 `crates/ai`。

### 12.4 本轮新增/计划中的代码路径

已新增：

- `app/src/ai/blocklist/controller/tool_lifecycle.rs`
  - `ByopToolResultRepair::missing_result_cancellation_message`
  - 单测覆盖 synthetic message 的 tool_call 配对与诊断 payload。

已修改：

- `app/src/ai/blocklist/controller.rs`
  - `run_byop_request_preflight` 对 `MissingResultWithoutRepairSource { reason: NoResult }` 不再直接 blocked。
  - 新增 `synthesize_byop_missing_cancellation_results`，追加 synthetic cancellation `ToolCallResult` 后 rebuild request。

还需要补强：

- controller 级测试：构造 history 中 assistant tool_call 无 result，preflight 后应落地 synthetic result。
- pending live action 测试：有 live action 时仍走 `PendingToolResults`，不能提前合成 cancellation。
- duplicate 防护测试：已有 persisted result 时不重复 append。

## 13. 多 AI 并行 review 建议

为了避免多个 AI 重复看同一块，可以按下面拆：

1. **Reviewer A：controller/readiness**
   - 看 `run_byop_request_preflight` 的状态机是否会循环、误修或漏修。
   - 重点确认 `PendingToolResults` 与 `NoResult` 的边界。
2. **Reviewer B：history/protocol**
   - 看 synthetic `ToolCallResult` 的 `server_message_data` 是否会被 BYOP provider 正确序列化给模型。
   - 重点确认 `result=None` + JSON payload 与现有 `byop_action_result_message` 兼容。
3. **Reviewer C：MCP recovery**
   - 从 `call_mcp_tool.rs` 往下梳理 dead transport / server missing / tool returned error。
   - 给下一阶段 `ToolFailureKind` 与 retry-once seam 方案。
4. **Reviewer D：public harness / SDK**
   - 看 `agent_sdk/driver.rs` 是否仍把可恢复工具失败误判成 run fatal。
   - 对照 Codex item lifecycle，给 driver 边界收窄建议。

## 14. 本轮实现细节

### 14.1 改动文件

```text
AGENTS.md
docs/design/ashide-agent-harness-tool-lifecycle.md
app/src/ai/blocklist/controller.rs
app/src/ai/blocklist/controller/tool_lifecycle.rs
```

`AGENTS.md` 只写原则：

- 不以“最小闭环”压制架构重构。
- 不用“小补丁”逃避状态机、生命周期、抽象边界问题。

`controller.rs` 只负责 preflight 状态机：

- `PendingToolResults`：仍然返回 `PendingByopToolResultsError`，由 pending request 机制等待 live action 完成。
- `MissingResultWithoutRepairSource { NoResult }`：调用 `synthesize_byop_missing_cancellation_results`，append synthetic cancellation result，然后 rebuild request 继续 readiness loop。
- 其它 missing / duplicate / orphan / out-of-order：仍 blocked，避免把真实历史损坏静默吞掉。

`controller/tool_lifecycle.rs` 目前只承载 BYOP repair helper：

- `ByopToolResultRepair::missing_result_cancellation_message`
- 后续可以沿这个 seam 扩成 `ToolLifecyclePolicy` / `ToolContinuationPolicy`。

### 14.2 当前 synthetic result 形态

当 readiness 已确认 tool call 无任何可等待结果时，写入：

```rust
message::Message::ToolCallResult(message::ToolCallResult {
    tool_call_id,
    context: None,
    result: None,
})
```

并在 `server_message_data` 放：

```json
{
  "status": "cancelled",
  "reason": "interrupted_by_user",
  "synthetic": true,
  "repair_source": "byop_missing_tool_result"
}
```

这个形态故意对齐现有无结构化 result 的 BYOP 持久化路径：provider serializer 已会从 `server_message_data` 取内容给模型。

### 14.3 本轮验证结果

```text
$ cargo test -p warp tool_lifecycle --lib
running 2 tests
test ...missing_result_cancellation_payload_is_diagnostic_and_model_visible ... ok
test ...missing_result_cancellation_message_preserves_tool_call_pairing ... ok
test result: ok. 2 passed

$ cargo check
Finished `dev` profile [unoptimized + debuginfo] target(s) in 41.60s
```

`cargo check` 输出大量既有 `dead_code` / macOS deprecated API warning；本轮没有引入编译错误。

### 14.4 下一轮建议顺序

1. **补 controller/history 级测试**
   - 目标是证明真实 preflight loop 会把 missing result append 到 history，而不仅是 helper 能构造 message。
   - 优先搜索/复用 `BlocklistAIHistoryModel` 现有测试 helper，避免为了测试手搭太多 UI entity。
2. **MCP failure classification**
   - 从 `app/src/ai/blocklist/action_model/execute/call_mcp_tool.rs` 开始。
   - 先把失败拆成 `ToolReturnedError / ToolUnavailable / ToolTransportDead / ModelInvalidArguments`。
3. **MCP retry once**
   - 借鉴 `refs/deepx-code-main/mcp/manager.go` 的 `callToolWithRestart`。
   - 只对 dead transport retry；工具自身 error 不 retry。
4. **action result continuation enum**
   - 把 `should_trigger_request_upon_completion() -> bool` 升级成 decision enum。
   - controller / driver 都消费同一套 decision，避免工具失败到处各自推导 run 终态。
