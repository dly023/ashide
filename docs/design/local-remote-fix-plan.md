# 本地 / 远程一致性修复开发计划

基于 `docs/design/local-remote-inconsistencies.md` 的排查结果，给出综合考虑架构、性能、风险、依赖关系的修复顺序。

## 总体原则

1. **先收口入口，再收口数据模型**：P0 里 #1/#2/#3/#7 是产品入口的分支泄漏，改的是 handler 路由，不动核心数据结构，风险低、收益直接。P0 Environment table（设计文档里的 P0）是数据模型重构，风险高、周期长，放后面。
2. **先对齐行为，再抽公共逻辑**：每一步先把远程行为补到和本地一致（用户可见收益），再考虑要不要抽公共 builder / trait（架构收益）。避免一开始就大重构却迟迟看不到效果。
3. **每步可独立合入、可独立测试**：不依赖一个大 PR，每步都能单测 + GUI 验证。
4. **低风险先行，高风险殿后**：env builder 合并（#5）纯重构零行为变化，可随时插；Environment table 是大改，放最后。

---

## 实施状态（已落地）

| 项 | 状态 | 说明 |
|---|---|---|
| 1.1 `cd_to_directory` | ✅ 已修 | 远程分流到 `cd_to_environment_directory`（直接执行），不再静默 ignore |
| 1.2 `open_directory_in_new_tab` | ✅ 已修 | 删守卫；`open_directory_tab_in_current_environment` 已按 capability 分流（runtime → `open_environment_runtime_terminal_entry`，local → `open_terminal_bootstrap_directory_tab`） |
| 1.3 `open_environment_file_with_target` | ✅ 已修 | 新增 `preview` + `additional_paths` 参数透传给 `open_code`；现有调用方传 `false`/`&[]`（等价） |
| 1.4 welcome code review pane | ✅ 已修 | `PendingEnvironmentRuntimeAgentViewEntry` 增 `open_code_review_pane` 字段；welcome 远程路径设 `true`；`apply_pending_environment_runtime_entry_to_terminal` 在 terminal materialize 后 deferred 调 `toggle_code_review_pane` |
| 2.1 session 环境标签 | ✅ 已确认 | 合理 backend diff：本地以 cwd 为环境身份（terminal-native），session detail 恒有 root label，environment label 仅 fallback；远程用 host 名。不强行加 "本地" label |
| 2.2 env builder 合并 | ✅ 已完成 | 已抽取为 `terminal/capability_environment.rs::terminal_capability_environment_variables`，被 `local_tty/unix.rs`(264,744) 与 `environment_runtime.rs`(1787,2831) 共用；测试 `environment_runtime_pty_advertises_terminal_capabilities_like_local_pty` 守护一致性 |
| 2.3 本地 error lifecycle | ✅ 已确认 | 合理 backend diff：本地 terminal bootstrap 无异步 connect/install 生命周期，spawn 错误在 PTY 创建时同步上报；远程有异步生命周期才需要 error state |
| 菜单修复 | ✅ 已修 | "终端"单元素子菜单、"其他"子菜单拍平为一级菜单项；"上传/新建"多项子菜单保留 |
| tab 激活 lifecycle | ✅ 已修 / 待跑测 | 用户可见的 tab 激活入口统一走 `activate_tab`；切换/聚焦到 dormant runtime placeholder 时会 queue terminal intent、ensure transport、materialize pending terminal |
| Session Navigator user-state 归属 | ✅ 已修 / 待跑测 | 远程会话 alias / pin 改为 environment-owned store；本地写本机 `session_state.json`，远程通过 remote_server 写远端 `session_state.json`；deleted tombstone 设计已删除，扫描结果是 source of truth |

阶段 3（Environment table / RuntimeBackend trait 数据模型收敛）为高风险大重构，单独 milestone，不在本轮落地。

---

## 阶段 0：Tab lifecycle 保活修复（P0，已落地 / 待验证）

目标：用户切换标签、快捷键跳转标签、Session Navigator 删除后重选、以及跨窗口 transfer 后聚焦时，只应该改变 view focus，不应该让已激活过的 Environment runtime 变成“盲状态”。

### 已落地实现

- `activate_tab` 成为用户可见 tab 激活的统一入口：
  - `activate_tab_internal`：只负责 active index / focus / title。
  - `prepare_active_environment_after_visible_tab_activation`：负责 Environment runtime hydration。
- `prepare_active_environment_after_visible_tab_activation` 统一执行：
  - `queue_active_environment_runtime_placeholder_terminal_if_needed`
  - `ensure_current_environment_runtime_transport_if_needed`
  - `open_pending_environment_runtime_terminal_for_current_environment`
- `focus_pane` 默认也触发 Environment activation hook；仅 Settings / Notebook / inspector 等“只聚焦已有 pane、不能切当前 Environment”的路径使用 `focus_pane_preserving_current_environment`。
- `ActivateTab` / `ActivateTabByNumber` / next / prev / last / transferred tab / Session Navigator delete reselect 等入口已接到统一激活路径。

### 当前判断

普通 tab switch 没有发现 teardown：`activate_tab_internal` 只更新 UI active state；真正 teardown 在 close/window close/显式 disconnect。用户遇到“切回来盲”的核心风险更像是 dormant placeholder 没重新走 runtime hydrate，而不是切走时杀 PTY。

### 待验证

待另一个构建进程空闲后跑局部测试：

```bash
cargo test -p warp --lib workspace::view_test::test_activate_next_tab_environment_runtime_placeholder_queues_terminal_intent
cargo test -p warp --lib workspace::view_test::test_activate_prev_tab_environment_runtime_placeholder_queues_terminal_intent
cargo test -p warp --lib workspace::view_test::test_activate_last_tab_environment_runtime_placeholder_queues_terminal_intent
cargo test -p warp --lib workspace::view_test::test_focus_pane_environment_runtime_placeholder_queues_terminal_intent
cargo test -p warp --lib workspace::view_test::test_close_active_tab_activating_environment_runtime_placeholder_queues_terminal_intent
```

GUI 验证仍需单独做：打开远程 Environment → 切到本地 tab → 切回远程 tab → 确认 terminal 不盲、pending terminal 能 materialize、runtime 仍 connected/reconnecting。

---

## 阶段 1：入口行为对齐（P0，低风险，1-2 天）

目标：消除用户可见的"远程动作没反应 / 能力弱"。

### Step 1.1 — 修 #1 `cd_to_directory` 远程静默无效

**改 `app/src/workspace/view.rs:12387`**

把静默 `Ignoring` return 改成按 capability 分流：

```rust
fn cd_to_directory(&mut self, path: PathBuf, ctx: &mut ViewContext<Self>) {
    if self.current_environment_uses_terminal_bootstrap(ctx) {
        // 本地：填入输入框（保持现状）
        let Some(input_handle) = self.get_active_input_view_handle(ctx) else { return; };
        let Some(path_str) = path.to_str() else { return; };
        let cd_command = format!("cd {}", shell_words::quote(path_str));
        input_handle.update(ctx, |view, ctx| view.replace_buffer_content(&cd_command, ctx));
    } else {
        // 远程：走已有的 cd_to_environment_directory（直接执行）
        let Some(path_str) = path.to_str() else { return; };
        self.cd_to_environment_directory(path_str, ctx);
    }
}
```

**注意**：本地"填输入框"vs 远程"直接执行"是已存在的行为差异。这步先消除"远程无效"，**不强行统一交互**（统一交互是产品决策，需要单独讨论）。如果要统一，建议都改成"填入输入框"——给用户回车确认的机会，远程直接执行容易误触。

**测试**：补一个 `cd_to_directory` 在 runtime-backed environment 下调用 `cd_to_environment_directory` 的单测。

### Step 1.2 — 修 #2 `open_directory_in_new_tab` 远程静默无效

**改 `app/src/workspace/view.rs:12426`**

远程没有现成的 `open_environment_directory_tab`，需要新增——复用 `open_environment_runtime_terminal` 的路径，把 cwd 设成目标 path：

```rust
fn open_directory_in_new_tab(&mut self, path: PathBuf, ctx: &mut ViewContext<Self>) {
    if self.current_environment_uses_terminal_bootstrap(ctx) {
        self.open_directory_tab_in_current_environment(path, false, ctx);
    } else {
        // 远程：在当前 environment runtime 开一个新 terminal，cwd = path
        let Some(path_str) = path.to_str() else { return; };
        self.open_environment_runtime_terminal_for_cwd(path_str.to_owned(), ctx);
    }
}
```

`open_environment_runtime_terminal_for_cwd` 可基于现有 `open_environment_runtime_terminal(target, root, startup_command_override, ctx)` 实现，`root = Some(path_str)`。

**测试**：单测 `open_directory_in_new_tab` 在 runtime environment 下注册了带 cwd 的新 runtime terminal。

### Step 1.3 — 修 #3 `open_environment_file_with_target` 能力对齐

**改 `app/src/workspace/view.rs:10427`**

把 `preview` 和 `additional_paths` 从写死改成参数：

```rust
pub fn open_environment_file_with_target(
    &mut self,
    environment_file_path: EnvironmentFilePath,
    line_col: Option<LineAndColumnArg>,
    preview: bool,                    // 新增
    additional_paths: &[PathBuf],     // 新增
    ctx: &mut ViewContext<Self>,
) {
    let layout = *EditorSettings::as_ref(ctx).open_file_layout.value();
    self.open_code(
        CodeSource::EnvironmentFileTree { environment_file_path },
        layout, line_col, preview, additional_paths, ctx,
    );
}
```

调用方更新：
- `view.rs:7818`（`OpenEnvironmentFile` 事件）：从 file browser 打开，`preview=false`、`&[]`（保持现行为）。
- `view.rs:18531/19783`（Ctrl/Cmd 点击路径）：传 `preview` / `additional_paths`，和本地 `open_file_with_target` 一致。
- `open_environment_file`（10416，无 target 版）保持 `preview=false, &[]` 调用新签名。

**风险**：纯参数透传，行为变化只在"远程 Ctrl+点击路径现在支持 preview/多文件"，符合预期。

**测试**：单测 `open_environment_file_with_target` 透传 preview/additional_paths 到 `open_code`。

### Step 1.4 — 修 #7 welcome view code review pane 远程缺失

**改 `app/src/pane_group/pane/welcome_view.rs:174-189` + `app/src/workspace/view.rs:6663`**

本地 `terminal_ready == true` 同步开 code review pane；远程 `false` 只 queue intent。问题是远程 terminal materialize 后没人补开 code review pane。

方案：在 `open_agent_directory_tab_in_current_environment` 远程路径里，把"开 code review pane"也作为一个 intent 一起 queue（或注册到 environment runtime terminal-ready 回调）。terminal materialize 时触发。

```rust
// welcome_view.rs
let terminal_ready = workspace.open_agent_directory_tab_in_current_environment(
    path_buf.clone(), false, /* want_code_review = */ true, ctx,
);
// 本地：terminal_ready=true，立即开（保持现状）
// 远程：terminal_ready=false，但 want_code_review intent 已 queue，
//       terminal materialize 事件里检测到就开
```

`open_agent_directory_tab_in_current_environment` 远程分支把 `want_code_review` 放进 `PendingEnvironmentRuntimeAgentViewEntry` 或单独的 intent 字段；`open_environment_runtime_terminal` materialize 完成后检查该字段，触发 `toggle_code_review_pane`。

**风险**：涉及 intent 队列 + materialize 回调，比 1.1-1.3 略复杂。建议放在 1.1-1.3 验证通过后再做。

**测试**：单测远程路径 queue 了 code review intent；materialize 后触发了 toggle。

---

## 阶段 2：细节一致性（P1，低-中风险，1-2 天）

目标：消除信息量 / 状态机不对称，为后续 Environment table 铺路。

### Step 2.1 — 修 #4 session 环境标签不对称

**改 `app/src/workspace/environment_runtime.rs:537`**

本地 authority 返回 `None` 导致 session 详情没环境标签。给本地也返回 label：

```rust
pub(crate) fn session_environment_display_label(authority: &str) -> Option<String> {
    let authority = authority.trim();
    if authority.is_empty() {
        return None;
    }
    if authority_uses_terminal_bootstrap(authority) {
        return Some(t_static!("workspace-environment-kind-local").to_string()); // "Local"
    }
    // 远程保持现状
    Some(authority.strip_prefix("ssh:ssh-config:")...)
}
```

`restored_session_environment_label`（`session_display.rs:185`）会据此给本地 session 也显示 "Environment runtime: Local"——或改 i18n key 让本地显示更自然的标签。

**注意**：要确认 session navigator 里本地 session 显示 "Local" 不会和 chip label 重复/冗余。可能需要调整 `restored_session_detail` 的渲染优先级。

**测试**：单测本地 authority 返回 "Local"；GUI 验证 session navigator 本地/远程 subtitle 结构一致。

### Step 2.2 — 修 #5 terminal capability env 合并

**改 `app/src/workspace/environment_runtime.rs:1792` + `app/src/terminal/local_tty/unix.rs:264`**

抽单一 builder：

```rust
// 放在 environment_runtime.rs 或新模块 terminal_capability.rs
pub(crate) fn terminal_capability_environment_variables() -> HashMap<String, String> {
    let mut vars = HashMap::new();
    vars.insert("TERM".into(), "xterm-256color".into());
    vars.insert("TERM_PROGRAM".into(), "WarpTerminal".into());
    vars.insert("COLORTERM".into(), "truecolor".into());
    if let Some(v) = ChannelState::app_version() {
        vars.insert("TERM_PROGRAM_VERSION".into(), v.to_string());
        vars.insert("WARP_CLIENT_VERSION".into(), v.to_string());
    } else {
        vars.insert("WARP_CLIENT_VERSION".into(), "local".into());
    }
    if FeatureFlag::HOANotifications.is_enabled() {
        vars.insert("WARP_CLI_AGENT_PROTOCOL_VERSION".into(), current_protocol_version().to_string());
    }
    vars
}
```

`environment_runtime_terminal_environment_variables` 和 `local_tty/unix.rs` 那段都改成调用它。

**风险**：纯重构，零行为变化。可随时做，建议作为阶段 1 之间的"休息任务"插入。已有测试 `environment_runtime_pty_advertises_terminal_capabilities_like_local_pty` 保护。

**性能**：builder 每次返回 `HashMap`，调用频率低（spawn PTY 时），无影响。

### Step 2.3 — 修 #6 本地 Environment error lifecycle

**这是设计决策，先讨论再改。**

当前本地 terminal 出错走 terminal view 渲染，远程走 EnvironmentLifecycleState::Error。要统一有两种方向：

- **方向 A（收口）**：给本地 Environment 也加 error lifecycle。本地 terminal view 的 fatal error 映射到 `EnvironmentLifecycleState::Error`，Environment Strip 显示 error dot。工作量大，要改 terminal error 路径 + Environment Strip 渲染。
- **方向 B（承认差异）**：在设计文档明确"本地无 transport，error 不进 Environment lifecycle"是合理 backend 差异，不收口。本地 error UI 维持现状。

**建议**：先 B（在文档承认），等阶段 3 Environment table 落地时再评估要不要 A。不要为单独这一项提前做本地 error lifecycle。

---

## 阶段 3：数据模型收口（设计文档 P0/P1，高风险，长周期）

目标：把 retained authority / current_environment / registry 收口成 Environment table，绞杀 local/remote 分支。这是设计文档本身规划的，不在本次排查清单里，但前面 1-2 阶段的修复都在为它铺路。

### Step 3.1 — Environment table（设计文档 P0）

`retained_environment_authorities: HashSet<String>` 升级为 `EnvironmentTable`：

```
key: authority
snapshot: label, kind, root/home, connection ref, capabilities
lifecycle: dormant / connecting / connected / reconnecting / error
runtime handle: session id, host id, control path, heartbeat generation
user intent: open terminal / open agent / restore / startup command
```

Environment Strip / Session Navigator / File Browser / new terminal 都读这张表，不再散落读 `current_environment` / `retained_authorities` / `environment_runtimes`。

**这是大改，建议单独一个 milestone，前面阶段全部合入后再启动。**

### Step 3.2 — RuntimeBackend trait（设计文档 P1）

引入 `LocalBackend` / `RemoteBackend` trait，上层只问 capability，不直接问 `EnvironmentKind::Local/Ssh`。`capabilities_for_environment` 内部按 backend 分流，外部统一。

### Step 3.3 — teardown policy 状态机（设计文档 P1）

把 reconnect/heartbeat/session-match 守卫正式枚举化：

```
Retained + Connected / Retained + Disconnected / Released + Connected / Released + Disconnected / Error (transient vs user-action-required)
```

顺便修上一个评审发现的 `reconnect_environment_runtime_authority` 缺 retained 守卫问题（在状态机入口统一加检查）。

### Step 3.4 — local root/home 走 Environment roots（设计文档 P2）

本地 root/home 用统一接口（provider 是 local fs），Project Explorer / File Browser / Skill Manager 不感知 backend。

---

## 阶段 4：Session Navigator user-state 归属收口（P0.5，中风险，已落地 / 待验证）

目标：会话 alias / pin 是用户对某个 Environment 内会话的个性化状态，必须由对应 Environment 拥有。本地会话写本机配置；远程会话写远端配置。删除不再保存 UI tombstone：Session Navigator 以 provider scan 结果为 source of truth，删除动作先修改 provider-owned source，成功后只清理 alias / pin。

### 当前问题

原链路：

- `session_navigator.rs::finish_workspace_session_alias_rename`
  → `set_workspace_session_alias_for_keys`
  → `set_cli_agent_session_alias`
  → `terminal/cli_agent_session_index.rs::set_session_alias`
- `set_session_alias` / `set_session_pinned` 原先只写本机 sidecar。
- 远程 scan 只通过 remote_server 返回 session records；合并时仍用本机 `pinned_session_ids` / `session_aliases`。

这意味着远程环境 A 上的会话别名、置顶状态曾是“本地客户端状态”，不是“远程环境状态”。换一台本地机器或重装本地 Ashide，远程别名会丢；同一个远程环境被多台客户端打开也无法共享用户意图。

### Step 4.1 — 抽离 UI 对本地 sidecar 的直接依赖

已落地为 Workspace 层 environment-aware 读写入口：

- `workspace_session_user_state_for_authority(authority)`
- `mutate_workspace_session_user_state_for_authority(authority, keys, mutation, ctx)`

本地 authority 走 `terminal::cli_agent_session_index`；远程 authority 走 `EnvironmentRuntimeClient` RPC。

### Step 4.2 — 本地 store 保持本机 sidecar，但改成统一 state 文件

本地 store 继续落在本机 Ashide config 目录，并从三个散文件 hard-cut 收口为一个 state 文件：

```text
~/.ashide/session_state.json
```

结构只保存 UI personalization，不保存 deleted：

```json
{
  "aliases": {},
  "pinned": []
}
```

本仓库未发布，无历史兼容要求；已 hard-cut 旧 `session_aliases.json` / `session_pins.json` / `session_deleted.json` 路径。

### Step 4.3 — 远程 store 写远端 `~/.ashide/session_state.json`

remote_server 新增专门的 user-state RPC，不复用 `MutateCliAgentSession`：

- `GetCliAgentSessionUserState`
- `MutateCliAgentSessionUserState`

独立 RPC 的原因：`MutateCliAgentSession` 语义是“改 provider session source（archive/delete 原始文件或 index entry）”，而 alias / pin 是 Ashide UI 个性化状态；两者混在一起会让 source mutation 和 UI state ownership 继续纠缠。

远端 daemon 直接读写远端：

```text
~/.ashide/session_state.json
```

写入用 atomic replace，避免 app/daemon crash 留半截 JSON。

### Step 4.4 — remote scan 合并远端 user-state

`scan_environment_runtime_agent_sessions` 完成后，不再用本机 sidecar 合并远程 records，而是：

1. remote scan 返回 provider records。
2. 同一 remote client 读取远端 user-state。
3. records → snapshots。
4. 用远端 user-state 做 alias override / pinned merge。
5. 再写入 `indexed_environment_cli_agent_sessions[authority]` 和 `indexed_environment_cli_agent_session_user_states[authority]`。

本地 scan 仍用 local store。

### Step 4.5 — 删除顺序保持“先 source mutation，后清 UI side-state”

删除远程会话时保持：

1. 先 remote delete/archive provider source。
2. 成功后清远端 alias / pin。
3. 失败则不清 alias / pin，也不隐藏远端真实存在的 session。

本地同理：先删本地 provider source，再清本地 alias / pin。删除不会写 deleted tombstone；如果 provider source 还存在，下一次 scan 应继续显示。

### Step 4.6 — 测试 / 验收

必须覆盖：

- local alias/pin 写 local store。
- remote alias/pin 走 remote RPC，不写本机 `~/.ashide/session_*.json`。
- remote alias 从另一台本地客户端扫描同一 remote 时仍可见（需要真实远端或 daemon integration 验证）。
- remote source delete 失败时不隐藏 session。
- scan refresh 以 provider records 为 source of truth，不能靠 UI state 隐藏行。
- key 需要包含 Environment scope，避免不同环境下相同 provider session id 冲突。

---

## #8 状态：已确认非问题

排查确认远程 runtime PTY spawn 时有 `working_directory` 字段（`pane_group/mod.rs:5189`），restore 不拼 cd 是合理的 backend 分支。CSV 里 `status` 改 `closed`。

---

## 推荐执行顺序与依赖

```
阶段 1（入口对齐，可并行）
  ├─ 1.1 cd_to_directory        ─┐
  ├─ 1.2 open_directory_new_tab ─┼─ 独立，可并行，先做
  ├─ 1.3 open_env_file_with_target ─┘
  └─ 1.4 welcome code review pane ── 依赖 1.1-1.3 的 environment handler 经验，稍后

阶段 2（细节一致）
  ├─ 2.2 env builder 合并 ── 纯重构，随时插入（建议夹在阶段 1 中间）
  ├─ 2.1 session 标签 ── 1.x 之后
  └─ 2.3 本地 error lifecycle ── 先文档承认，阶段 3 再评估

阶段 3（数据模型，大改）
  3.1 Environment table → 3.2 RuntimeBackend trait → 3.3 状态机 → 3.4 local roots
```

## 每步的验收标准

| 步骤 | 单测 | GUI 验证 |
|---|---|---|
| 1.1 | runtime env 下 cd_to_directory 调用 cd_to_environment_directory | 远程 file browser 右键 cd 有反应 |
| 1.2 | runtime env 下 open_directory_in_new_tab 注册带 cwd 的 runtime terminal | 远程右键"新 tab 打开"生效 |
| 1.3 | open_environment_file_with_target 透传 preview/additional_paths | 远程 Ctrl+点击路径能 preview/多文件 |
| 1.4 | 远程路径 queue code review intent；materialize 后触发 | welcome view 开项目远程也有 code review pane |
| 2.1 | 本地 authority 返回 "Local" | session navigator 本地/远程 subtitle 一致 |
| 2.2 | 现有 capability env 测试仍通过 | 颜色/terminal identity 不变 |
| 2.3 | — | 文档更新 |
| 3.x | 见设计文档 | 大规模 GUI 回归 |

## 风险与回退

- 阶段 1 每步都是小 handler 改动，出问题回退单个 commit 即可。
- 阶段 2.2 是纯重构，由现有测试保护，风险最低。
- 阶段 3 是大改，建议在单独分支进行，完整 CI + GUI 回归后再合入主干。
