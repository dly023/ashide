# EnvironmentTable 数据模型收口（阶段 3）

> 目标：把 Workspace 里散落的 17 个环境相关字段收口成一张 `EnvironmentTable`，
> 作为环境身份、生命周期、runtime handle、用户 intent 的唯一 source of truth。
> 本地与远程环境都在同一张表里；行为差异由 `EnvironmentEntryBackend` trait 承载，
> 不再通过数据存储分叉。

## 现状（收口前）

三轨数据源：

1. `current_environment: Option<EnvironmentSnapshot>`（workspace 单值，本地主用）
2. `environment_runtimes: EnvironmentRuntimeRegistry`（远程专用 entity，23 方法）
3. `tabs[].environment`（per-tab 快照，合法保留）

外加 14 个散落字段：`retained_environment_authorities`、3 个 generation map、
`home_roots`、2 个 indexed session map、7 个 `pending_environment_runtime_*` intent map、
`next_environment_runtime_session_id`。

EnvironmentStrip 同时读三个源（`view.rs:23303-23352`）。

## 目标类型

```rust
// app/src/workspace/environment_table.rs

struct EnvironmentTable {
    entries: HashMap<String, EnvironmentEntry>,       // authority -> entry
    active_authority: Option<String>,                 // 替代 current_environment
    session_to_authority: HashMap<SessionId, String>, // 远程 synthetic session 反查
    next_runtime_session_id: u64,
}

struct EnvironmentEntry {
    // 身份
    snapshot: EnvironmentSnapshot,

    // 生命周期归属
    retained: bool,                                   // 替代 retained_environment_authorities

    // Runtime handle（本地恒 Connected，session/host/control 全 None）
    status: EnvironmentRuntimeStatus,
    synthetic_session_id: Option<SessionId>,
    host_id: Option<HostId>,
    control_path: Option<PathBuf>,
    last_error: Option<String>,

    // Transport generations（本地恒 0）
    heartbeat_generation: u64,
    preparation_generation: u64,

    // Roots
    home_root: Option<String>,                        // 替代 environment_runtime_home_roots

    // 用户 intent（本地恒 None/false——本地是同步的）
    pending_terminal: bool,
    pending_startup_command: Option<String>,
    pending_agent_view: Option<AgentTabEntry>,
    pending_forked_conversation: Option<ForkEntry>,
    pending_restore: Option<PendingEnvironmentRuntimeSessionRestore>,
    pending_split_pane_loading_id: Option<PaneId>,

    // CLI agent sessions（本地从 terminal model 扫描，远程从 runtime 扫描）
    indexed_cli_agent_sessions: Vec<WorkspaceSessionSnapshot>,
    cli_agent_session_user_state: EnvironmentCliAgentSessionUserState,
}
```

### 本地 vs 远程在表里的差异

| 字段 | Local entry | Remote entry |
|------|-------------|--------------|
| `status` | 恒 `Connected` | `Dormant/Connecting/Installing/Connected/Error` |
| `synthetic_session_id` | `None` | `Some(SessionId)` |
| `host_id` / `control_path` | `None` | `Some(...)` |
| `heartbeat/preparation_generation` | `0` | 单调递增 |
| `pending_*` | 全空（同步） | 有值（异步 materialize） |
| `home_root` | `None`（本地走 cwd） | `Some(home)` |
| `indexed_cli_agent_sessions` | 本机 terminal 扫描 | 远端 runtime 扫描 |
| `cli_agent_session_user_state` | `Default`（本地走 sidecar） | 远端 RPC 读 |

行为差异由 `EnvironmentEntryBackend` trait（已存在于 `environment_backend.rs`）承载，
不通过存储分叉。

## 迁移步骤

### Step 1 — 定义类型（additive，零风险）

新建 `app/src/workspace/environment_table.rs`，定义 `EnvironmentTable` + `EnvironmentEntry`。
提供与现有访问模式对齐的 accessor 方法，使后续迁移是机械替换。

### Step 2 — 数据存储收口（一次性，机械替换）

用单个 `environments: EnvironmentTable` 字段替换 Workspace 的 17 个环境字段。
所有 `self.retained_environment_authorities.X` → `self.environments.X`，
`self.environment_runtimes.X` → `self.environments.X`，等等。

`current_environment: Option<EnvironmentSnapshot>` → `self.environments.active_authority()` +
`self.environments.current_snapshot()`。初始化处 `current_environment: None` → 表空。

`indexed_cli_agent_sessions`（本地 flat Vec）→ 移入 local entry 的 `indexed_cli_agent_sessions`。

**这是一次性替换，不留双存储过渡态**（遵循 memory 61：high-cohesion rewrite as one coherent pass）。

### Step 3 — 消费者迁移

- **EnvironmentStrip**：从读三源改为读 `self.environments`。
- **Session Navigator**：`indexed_cli_agent_sessions_for_authority` 统一走表。
- **File Browser**：lifecycle 注入从表读。
- **Spawn 路径**：`spawn_plan_for_environment` 从表读。

### Step 4 — 绞杀产品入口泄漏

1. conversation restore 三分叉（`view.rs:16465/16523/16584`）→ 走 `EnvironmentEntryBackend` 统一 deliver。
2. skill manager scope（`view.rs:6056`）→ 问 capability 而非 authority。
3. delete 焦点回拉（`session_navigator.rs:2004`）→ 统一行为。

### Step 5 — 清理死代码

- 删 `EnvironmentLifecycleState::Reconnecting`（死 variant，`EnvironmentRuntimeStatus` 无对应）。
- 把 `terminal_bootstrap_*` 3 方法从 registry 拆回本地 backend。
- 删 `current_environment_uses_terminal_bootstrap` helper（如果还有调用点）。

## 不收口的部分

- `tabs[].environment`：tab 指向环境是合法的 per-tab 状态，保留。
- `restored_workspace_sessions`：flat restore 列表，不按环境分。
- `session_navigator_display_order` / `workspace_sessions_refresh_state`：UI 状态，不属于环境。
- `restoring_workspace_session_keys` / `active_restored_workspace_session_key`：UI 状态。

## 验证

- `cargo check` 绿。
- 全量测试失败数 ≤ 63（全是 shell_integration/ui_tests 环境失败）。
- 改动模块单测全过（view_test、call_mcp_tool_tests、blocklist controller_tests 等）。
