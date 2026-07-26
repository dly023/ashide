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
| File Browser symlink 语义 | ✅ 已修 / 待 GUI 复验 | RPC 硬切区分请求 `path` 与目标 `resolved_path`；Server File Browser 与 SFTP 都保留链接身份并单独存 target kind；导航按目标类型，删除/重命名按链接自身 |

阶段 3（Environment table / RuntimeBackend trait 数据模型收敛）为高风险大重构，单独 milestone，不在本轮落地。

---

## 阶段 0：Tab lifecycle 保活修复（P0，已落地 / 待验证）

目标：用户切换标签、快捷键跳转标签、Session Navigator 删除后重选、以及跨窗口 transfer 后聚焦时，只应该改变 view focus，不应该让已激活过的 Environment runtime 变成“盲状态”。

### 已落地实现

- `activate_tab` 成为用户可见 tab 激活的统一入口：
  - `activate_tab_internal`：只负责 active index / focus / title。
  - `prepare_active_environment_after_visible_tab_activation`：负责 Environment runtime hydration。
- `prepare_active_environment_after_visible_tab_activation` 统一执行：
  - `queue_active_environment_runtime_placeholder_terminals_if_needed`
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

### Step 2.1 — #4 session 环境标签不对称（已关闭）

最终产品语义不是强行给 local 增加一条冗余 Environment runtime subtitle：local 继续以 cwd 表达上下文，remote 才在 cwd 之外显示 host。authority 的 label 语义已迁移到 `ParsedEnvironmentAuthority::display_label`；任何 consumer 禁止再自行剥离 SSH 前缀。

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

### Step 2.4 — File Browser FS metadata 抽象 + symlink 统一（已实施）

**结论**：已按同一套 UI 语义硬切，禁止再把 canonical target 当作文件树身份。不要把问题写成“远程服务器软链接识别不了”，也不要只在 remote daemon 上补一个 `is_symlink` 判断。用户面对的是同一个 File Browser；本地 terminal root 和 remote environment root 的差异应该只存在于 backend adapter。

已确认并修复的真实失败链路：

- 展开 `/root/link` 时，generation 以链接路径登记，但 daemon/local backend 返回 canonical target，回调按目标路径查 generation 后把结果误判为 stale。
- SFTP daemon backend 又用 canonical target 拼接子项，导致 UI identity 从链接命名空间跳到目标命名空间。
- `FileEntryType::Symlink` 原来没有 target type：一套实现把所有 symlink 当目录，另一套实现直接降格成 File/Directory，删除和导航语义互相污染。

当前契约：

- RPC `path` 是展示/操作身份，不跟随末级链接；`resolved_path` 仅用于 realpath。
- Server File Browser 与 SFTP 都把 entry kind 和 symlink target kind 分开保存。
- `is_directory_like` / `is_file_like` 只控制导航；mutation 只按 entry kind，symlink-to-dir 不能递归删除目标。
- Project Explorer 的 `expanded_folders` 是跨 tree snapshot 的用户意图，不能随 repo metadata snapshot 直接覆盖，也不能在 entry 被替换为 unloaded directory symlink 后只保留展开图标。所有 existing `RootDirectory` snapshot 替换统一走 `replace_root_entry`，本地与 remote event 使用同一个 reconciliation：
  - 同 lexical path 仍是 directory-like：保留展开意图；若 unloaded，立即走对应 backend load。
  - descendant 暂时缺失但最近可见 ancestor 尚未 loaded：先保留意图，ancestor materialize 后再判断。
  - snapshot 已加载且目标不存在，或 link retarget 为 file/missing/other：清理失效展开意图。
- remote Project Explorer lazy load 必须把词法路径身份贯穿到底：helper 只做 `StandardizedPath` 规范化，禁止 canonicalize 末级 symlink；transport success/failure 携带 `(host, repo, dir_path)`，先转成 `RepoMetadataModel::DirectoryLoadFinished`，UI 只消费统一模型事件。
- directory load completion 只释放精确 `(root, dir_path)` owner，禁止 `finish_root` 误释放同 root sibling；空目录和空 directory symlink 即使没有 child patch，也必须显式标记 `loaded=true`。
- Code Review 的文件系统边界也必须保留 link identity：New/Untracked symlink 先 `symlink_metadata`，再 `read_link`，按 Git mode `120000` blob 展示 link target；禁止交给 `git diff --no-index /dev/null <symlink>` 跟随 target。目录、文件和 dangling symlink 使用同一条路径；`FileDiff.is_symlink` 必须贯穿到视图层并选择 detached/selectable buffer，禁止 `GlobalBufferModel` 再次解引用工作树路径。
- File Search 不得在 tree model 之后另造 canonical repo identity：cache key、entry relative projection 和 invalidation 全部使用同一个 `RepositoryIdentifier::Local(StandardizedPath)`；`FileTreeUpdated` 只能表示 mutation 开始，最终 `FileTreeEntryUpdated/DirectoryLoadFinished` 也必须清缓存，否则异步 apply 或 symlink lazy-load 后会永久显示旧结果。

已复现并固定的 Project Explorer 回归链：

```text
expanded dir-link -> retarget A to B
  -> FileTreeEntryUpdated replaces entry with unloaded symlink
  -> old code rebuilds only
  -> row stays visually expanded but permanently empty

now:
  -> replace_root_entry
  -> reconcile expansion intent
  -> local load_directory / remote LoadRepoMetadataDirectory
  -> new target children appear without collapse/re-expand
```

```text
untracked dir-link/file-link/broken-link
  -> old: git diff --no-index /dev/null <symlink>
  -> directory/dangling link fails; file link displays target file contents

now:
  -> symlink_metadata preserves entry kind
  -> read_link returns the Git blob payload
  -> FileDiff.is_symlink selects a detached/selectable editor buffer
  -> Code Review displays one mode-120000 addition without dereferencing target
```

```text
repo opened through /path/repo-link
  -> old tree entries keep /path/repo-link/*
  -> old File Search canonicalizes root to /real/repo
  -> every lexical entry fails strip_prefix or stale cache survives lazy load

now:
  -> RepositoryIdentifier::Local(StandardizedPath) owns cache identity
  -> lexical root projects lexical children
  -> mutation completion events invalidate the same cache key
```

```text
create file/folder inside /visible/dir-link
  -> old local path canonicalizes parent to /real/target
  -> create succeeds but returns /real/target/New File
  -> reload produces /visible/dir-link/New File
  -> pending rename/selection cannot find the created row

now:
  -> local and remote share plan_new_entry
  -> IO may follow the symlink in the filesystem
  -> UI identity always remains /visible/dir-link/New File
```

目标抽象：

```text
FileBrowserFsBackend
  ├─ LocalTerminalFsBackend
  └─ EnvironmentRuntimeFsBackend

FileBrowserEntryMetadata
  path
  name
  entry_kind: File / Directory / Symlink / Other
  target_kind: None / File / Directory / Other / Missing
  canonical_path
  size
  modified_time
```

规则：

1. UI 渲染保留 symlink 身份：icon / label 可显示 symlink overlay。
2. UI 行为按 target kind 判断可导航性：
   - `Directory` 或 `Symlink -> Directory`：可展开、可进入、可作为 upload/cd/new-file 目标。
   - `File` 或 `Symlink -> File`：可打开文件。
   - `Symlink -> Missing/Other`：显示 symlink，但打开时给明确错误。
3. `list_directory` 和 `resolve_path` 必须通过同一转换函数产出 `FileBrowserEntryMetadata`，不要让 terminal/backend 各自散落判断。
4. remote proto 可以 hard-cut 扩字段：`DirEntry` / `ResolvePathSuccess` 增 `target_kind`（必要时再增 `link_target`）；仓库未发布，不为旧 daemon 保留兼容路径。
5. 先收口 list/resolve/symlink，再把 delete/rename/new/upload/download 的目标判断接到同一个 `is_directory_like()` / `is_file_like()` helper。

第一刀建议：

1. 在 File Browser 模块内新增统一 metadata helper，而不是直接改 UI 各处 `entry.kind == Directory`。
2. 本地 terminal backend 改用 symlink-aware metadata：先读 `symlink_metadata` 保留 link 身份，再按需 follow target metadata 得到 `target_kind`。
3. remote daemon list/resolve 同样返回 symlink 身份和 target kind；broken symlink 要能表示为 `target_kind=Missing/Other`。
4. 补 Unix symlink 测试：
   - symlink-to-dir 在本地 terminal root 和 remote environment root 都可展开。
   - symlink-to-file 在两种 root 下都可打开。
   - broken symlink 两种 root 下都不 crash，并显示明确错误。

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

### Step 3.3.1 — transport session 全局所有权（已落地）

`RemoteServerManager` 是 app-global singleton，所有 Workspace 都订阅它的事件流，因此 synthetic runtime session 不能由每个 `EnvironmentTable` 独立从 0 分配。旧模型在多窗口下会让两个环境同时拥有 `SessionId(0)`，使 binary check / install / connect / disconnect 事件跨 Workspace 串线。

现有硬约束：

1. synthetic Environment owner session 只由 `RemoteServerManager::allocate_environment_owned_session_id` 分配。
2. `EnvironmentTable` 只记录 session → authority ownership，不再拥有 allocator。
3. Workspace 收到 app-global transport event 后，先检查本表是否拥有该 session；non-owner 直接忽略，禁止进入生命周期 handler。
4. 多窗口测试必须同时证明 ID 不冲突，以及两个 Workspace 互不认可对方 session。

`EnvironmentTable` 自身还必须维持 capability 不变量：Current App/Local row 初始化和每次 upsert 后都恒为 `Connected`，且不能进入 `runtime_snapshots()`；只有 runtime-backed Environment 才允许 `Dormant → Connecting → Installing → Connected/Error` 状态机。禁止依赖 Environment Strip 的 dedupe 把错误 Local runtime row 隐藏掉。

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
  ├─ 2.3 本地 error lifecycle ── 先文档承认，阶段 3 再评估
  └─ 2.4 File Browser FS metadata ── 先统一 symlink/list/resolve，再审 delete/rename/new/upload/download

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
| 2.4 | 本地 + remote symlink-to-dir / symlink-to-file / broken symlink metadata 测试；Code Review mode `120000` blob 测试 | 同一个 File Browser 里 symlink-to-dir 可展开，symlink-to-file 可打开，broken symlink 不 crash；Code Review 三类链接只展示 link target，不读取 target 内容 |
| 3.x | 见设计文档 | 大规模 GUI 回归 |

## 风险与回退

- 阶段 1 每步都是小 handler 改动，出问题回退单个 commit 即可。
- 阶段 2.2 是纯重构，由现有测试保护，风险最低。
- 阶段 3 是大改，建议在单独分支进行，完整 CI + GUI 回归后再合入主干。

## #28 状态：File Browser mutation target 改为稳定路径身份

重命名编辑器过去持有 `pending_rename_index`。列表刷新、排序或 symlink retarget 会改变 index，提交时可能重命名另一行。现 hard-cut 为 `pending_rename_path`：

1. 开始编辑时捕获用户看到的词法 path。
2. render 和 commit 每次按 path 在当前 snapshot 中重新定位。
3. 若目标在刷新后已消失，提交直接终止，禁止 fallback 到原 index 或邻近行。
4. `rename_target_path_survives_listing_reorder` 固化重排不漂移。

## #29 状态：symlink target 生命周期改为状态刷新 + 双层 watcher

旧模型只给“已展开的目录链接”注册 canonical target recursive watch，并把 target 事件沿用为 lexical add/delete/move。这样 target 根被删除时，`deleted` 会直接删除 lexical link 行；broken external target 之后被创建，也没有 watcher 能触发恢复。

现收口为一个 mount 状态模型：

1. `SymlinkWatchMount.target_path` 只负责 target namespace → lexical namespace 投影。
2. watcher 注册独立记录 `WatchDepth::{Direct, Recursive}` 与 owner；多个 repo/link owner 共享同一路径时，有效深度取最强需求，释放 owner 后才允许降级或注销。
3. 已展开目录使用 target recursive content watch + parent direct lifecycle watch；未展开目录和文件使用 parent direct lifecycle watch；broken target 使用最近存在祖先的 direct watch。
4. 外部 mount 事件统一进入 `RepoUpdate.refreshed`，按当前 `symlink_metadata` 重建 lexical entry，而不是信任源事件的 add/delete 分类。
5. mount 根投影禁止 `join("")` 生成尾斜杠，否则 `lstat("link/")` 会解引用或把 broken link 误判为不存在。
6. 多层 missing target 每创建一级祖先，就把 lifecycle watch 前移一级，最终无需手动刷新恢复为可导航链接。

护栏测试：

- `external_target_delete_and_recreate_preserves_lexical_link_identity`
- `external_file_symlink_target_lifecycle_updates_kind_without_losing_link`
- `broken_external_symlink_target_creation_is_projected_without_manual_refresh`
- `exact_watch_path_stays_registered_until_every_owner_releases_it`

## #30 状态：daemon 删除协议显式编码 inode kind

旧 `DaemonSftpBackend::delete_dir_recursive` 清空子项后调用 `delete_file(path)`；helper 最终执行 `remove_file`，所以普通空目录无法删除。现新增硬切协议 `DeleteDirectory`：

1. `DeleteFile` 只删除文件或 symlink。
2. `DeleteDirectory` 只对一个空目录执行 `remove_dir`。
3. 递归遍历仍在 caller，确保 symlink 永远按 link 自身删除，不由 daemon 隐式跟随。
4. file/directory response 缺少 oneof result 都显式失败，禁止空响应假报成功。

护栏测试：`delete_directory_has_explicit_empty_directory_semantics`。

## #31 状态：symlink chain 的词法生命周期与规范内容 watcher 分离

`repo/link -> /external/alias -> /targets/a` 过去只监听最终 canonical target。修改 `/external/alias` 指向 `/targets/b` 时，事件不在 `/targets/a` 命名空间内，Project Explorer 会继续显示旧 target，直到手动刷新。

现将 mount 身份拆成两个不可混用的集合：

1. `target_path` 是 canonical content namespace，只负责展开目录内容的 relative projection。
2. `lifecycle_targets` 是 raw lexical target identity；其 inode 变化只刷新 link root，不把 alias 路径拼入树。
3. watcher 同时持有 canonical target parent 与 raw target parent 的 direct ownership；已展开目录额外持有 canonical target recursive ownership。
4. retarget 后统一重读 lexical link，并通过 owner registry 释放旧 mount、注册新 mount，禁止 watcher 泄漏或继续监听旧目标。

护栏测试：`external_symlink_chain_retarget_refreshes_mount_from_lexical_lifecycle_event`。

## #32 状态：File Browser transfer 禁止静默解引用 symlink

当前 upload/download RPC 只有普通 file bytes 与 directory traversal，不能表达 raw link target 或创建 link inode。旧实现却把 capability 缺失静默降级为解引用：file symlink 上传会复制 target 内容；递归下载遇到 directory symlink 会越出选择根，循环 link 还可能无限递归；WalkDir 错误会被 `filter_map(Result::ok)` 吞掉并产生不完整上传。

现执行明确的 capability guard：

1. upload source 根和所有 descendants 都先 `symlink_metadata`/`WalkDir::file_type`；发现 symlink 时在生成任何 transfer batch 前整体失败。
2. recursive download 与直接下载共用 `ensure_transferable_server_entry`；任何 symlink entry 都显式失败，禁止按 target kind 改走 file/directory transfer。
3. WalkDir I/O 错误向上返回，禁止静默跳文件。
4. 该限制只属于 transfer capability；symlink-to-directory 的导航、展开以及把它作为新建/上传目的目录仍然允许。
5. 若未来要支持 link-preserving transfer，必须新增 `ReadLink/CreateSymlink` 类型化协议，不得恢复“跟随 target 当普通文件/目录”的 fallback。

护栏测试：

- `remote_symlink_transfer_is_rejected_instead_of_dereferencing_target`
- `upload_source_symlink_is_rejected_instead_of_copying_target_contents`
- `upload_directory_with_nested_symlink_is_rejected_before_partial_planning`

## #33 状态：repo watcher mutation 使用 per-repo single-flight FIFO

`handle_watcher_event` 过去为每个 batch 直接 `ctx.spawn`。filesystem 读取在后台线程执行，main-thread callback 按完成时间返回；连续 delete/recreate、rename 或 symlink retarget 可以让旧 batch 晚于新 batch apply，重新写回旧 entry 和 watcher mount。

现增加 repository-scoped mutation pipeline：

1. 每个 repo 同时最多一个 background mutation batch。
2. 后续 batch 按接收顺序进入 `VecDeque`，前一个 apply 完成后才启动下一个。
3. active batch 持有单调 token；repository re-index/remove 会清空 queue 并失效 token。
4. 旧 callback 只有 token 仍匹配时才允许 apply，禁止同路径的新 repository incarnation 被旧任务污染。
5. 不依赖 debounce 时序或 executor 恰好 FIFO，正确性由模型状态机保证。

护栏测试：`repository_watcher_batches_are_single_flight_fifo_per_repo`。

## #34 状态：fallback Environment row 复用 authority capability parser

`EnvironmentTable::ensure_entry_for_authority` 过去用 `authority.starts_with("local")` 推断 Local，并自行拼 label/connection_ref。这重新制造了第二套 local/remote 抽象：任意以 `local` 开头的 runtime/custom authority 都会被错误初始化成 Connected Local。

现统一复用 `ParsedEnvironmentAuthority` 的 backend kind、display label 与 runtime connection ref capability。

禁止 EnvironmentTable 再解析 authority 字符串。护栏测试：`fallback_entry_classification_uses_authority_capability_not_string_prefix`。

## #35 状态：Environment authority 协议唯一类型化 home

`app_state`、`environment_runtime`、saved-SSH provider、EnvironmentTable、Workspace 与 Session Navigator 过去分别维护 authority 的前缀协议。即使常见输入碰巧一致，新增 custom provider 时仍会产生 backend、navigation key、connection ref 与 display label 的交叉误判。

现由 `app/src/environment_authority.rs::ParsedEnvironmentAuthority` 唯一解析：

1. `local` / `local:<root>` → `TerminalBootstrap`，统一 navigation key 为 `local`。
2. `ssh:<connection_ref>` / `ssh-config:<profile>` → `SavedSsh`，同时派生 connection ref 与 host label。
3. 其他 authority → `Runtime`，保持完整 identity；`locality:remote` 不得按文本前缀误判为 local。
4. `EnvironmentSnapshot.connection_ref` 显式字段优先，只有缺失时才从 `SavedSsh` authority 派生。
5. `script/check_local_remote_parity` 已接入 presubmit，禁止 shared home 外出现前缀 parser、旧 wrapper 或 transcript metadata parser 副本。

门禁：`SN-ENV-AUTHORITY-PARSER-01`、`environment_authority_parser_covers_local_saved_ssh_and_custom_runtime`、`environment_snapshot_runtime_connection_ref_uses_shared_authority_parser`、`fallback_entry_classification_uses_authority_capability_not_string_prefix`。

## #37 状态：SFTP 与 helper RPC 必须服从同一 symlink transfer deny contract

Workspace File Browser 已在 planner 拒绝 symlink，但旧 SFTP Browser 仍把 symlink-to-file 当普通文件下载，上传本地 symlink 时读取 target；上传覆盖远端 symlink-to-file 还会由 helper `OpenOptions` 跟随链接并改写 target。只修 UI 会被 backend direct call 或未来入口绕过。

目标架构由 `docs/LOCAL_REMOTE_PARITY_SPEC.yaml::LR-FILE-TRANSFER-SYMLINK-01` 定义：

1. UI planning 只允许普通 file 进入 transfer task；symlink target kind 只服务浏览/导航。
2. `SftpBackend` 的直接 upload/download 再次 lstat source/destination，防止调用方绕过 UI。
3. helper `ReadFileChunk/WriteFileChunk` 在协议边界拒绝末级 symlink；客户端是否正确不再决定安全语义。
4. 上传目标存在 symlink 时显式失败，禁止 truncate/write 跟随 target。
5. 真正支持 link transfer 前必须先增加 `ReadLink/CreateSymlink` 类型化 RPC。

门禁测试：`sftp_transfer_policy_rejects_symlink_entries`、`sftp_backend_rejects_symlink_upload_and_download`、`file_chunk_rpc_rejects_symlink_instead_of_reading_or_overwriting_target`。

## #38 状态：remote helper required result 必须在 transport boundary 一次性校验

现有 remote RPC 把 protobuf `oneof result` 暴露成 `Option` 后交给每个调用方自行解释，已经出现 `WriteFile`、`RenameFile`、`SaveBuffer`、`ResolvePath` 与 CLI-agent mutation 将 `None` 当作成功、未找到或已保存的分叉。局部修每个 match 无法阻止新增 RPC 再次回归。

目标架构由 `docs/LOCAL_REMOTE_PARITY_SPEC.yaml::LR-REMOTE-REQUIRED-RESULT-01` 定义：

1. `RemoteServerClient::send_request` 是 required-result 的唯一协议验证边界；任何声明 `oneof result` 的 response 缺失结果都返回 `ClientError::UnexpectedResponse`。
2. 业务适配层不再拥有“空响应语义”；保留 `None` 分支时只能返回显式错误，禁止与 `Success` 合并。
3. 静态契约从 `remote_server.proto` 枚举 required-result response，并与 transport validator 的覆盖 marker 对比；新增 RPC 漏接直接让 presubmit 失败。
4. contract test 通过 mock transport 注入 malformed response，证明错误在业务方法之前被截断。

门禁测试：`required_result_response_names_match_proto_contract`、`empty_required_result_is_rejected_at_transport_boundary`、`write_and_rename_empty_response_are_not_false_success`。

## #39 状态：目录枚举必须 complete-or-error，禁止 silent partial success

remote helper 的 `ListDirectory` 使用 `read_dir.flatten()` 跳过单条目错误，并用 `file_type().ok()` / `metadata().ok()` 把 metadata 错误降级成看似成功的 row；本地 SFTP backend 对同类错误则整体失败。用户看到的结果是远程目录偶发缺文件，但 UI 没有任何失败信号。

目标架构由 `docs/LOCAL_REMOTE_PARITY_SPEC.yaml::LR-FILE-LIST-COMPLETE-01` 定义：

1. helper 使用唯一 complete-listing collector，iterator、file type 与 lexical metadata 任一失败都产生 top-level Error。
2. Success 只在全部 entry 收集并排序完成后生成，不再表达 partial snapshot。
3. symlink inode metadata 与 target metadata 分离；broken/无权限 target 只改变 `target_kind`，lexical link row 仍保留。
4. static check 禁止 helper 重新出现 `read_dir.flatten()` / `filter_map(Result::ok)`。

门禁测试：`list_directory_entry_error_is_not_silently_dropped`、`list_directory_broken_symlink_preserves_lexical_row`、`list_directory_returns_sorted_metadata`。

## #40 状态：Session history scan 必须 error-aware，失败不得覆盖既有 rows

本地 scanner 的 `WalkDir::filter_map(Result::ok)` 与 remote helper 的 `read_dir.flatten()` 都会把 traversal failure 伪装成成功子集；Session Navigator Refresh 随后直接替换 indexed cache，用户看到的就是原会话无提示消失。两套独立 discovery 也使同类修复持续回归。

目标架构由 `docs/SESSION_NAVIGATOR_SPEC.yaml::SN-INTRA-SCAN-COMMIT-01` 定义：

1. local/remote scanner 共用 `cli_agent_jsonl` 中唯一 recent JSONL discovery。
2. discovery 与 scan 返回 typed `Result`；store 不存在是成功空集，存在但不可完整遍历是错误。
3. local Refresh 和 remote RPC 只在 Success 时替换 cache；Error 保留已有 rows、RowId、顺序与 selection。
4. static check 禁止 scanner 重新出现 `filter_map(Result::ok)` / `read_dir.flatten()`。

门禁测试：`recent_jsonl_scan_error_is_not_silently_dropped`、`session_refresh_scan_failure_preserves_cached_rows`、`remote_cli_agent_scan_failure_is_an_error_result`。

## #41 状态：CLI-agent home 是 required capability，unknown 不得降级为空集或 `/`

本地 scanner 在 `dirs::home_dir()` 失败时返回空列表，Refresh 会把“无法定位 store”提交成“store 确实为空”；remote helper 则把缺失 home 回退为 filesystem root `/`，使 scan/read/mutate 的 allow-list 错绑到 `/.claude` / `/.codex`。两种 fallback 都把未知状态伪装成有效状态。

目标架构由 `docs/SESSION_NAVIGATOR_SPEC.yaml::SN-INTRA-HOME-RESOLUTION-01` 定义：

1. `cli_agent_jsonl` 提供唯一 required-home resolver；local/remote 不再自行决定 fallback。
2. home unavailable 返回 typed Error，Refresh 保留既有 rows；remote RPC 返回 Error。
3. scan/read/mutate/expand-user/allow-list 共用同一 home，禁止任何 `PathBuf::from("/")` fallback。
4. static check 禁止 CLI-agent session 模块重新引入 root/empty home fallback。

门禁测试：`cli_agent_home_resolution_never_falls_back_to_filesystem_root`、`local_cli_agent_scan_requires_resolved_home`、`remote_cli_agent_paths_require_resolved_home`。

## #42 状态：File Browser complete-listing contract 必须覆盖 Terminal backend

remote helper 已在 `LR-041` 改成 complete-or-error，但同一 File Browser 的 Terminal backend 仍在 `symlink_metadata` 失败时 `continue`，本地列表会静默缺 row，且静态护栏只检查 helper，无法阻止这条双实现继续回归。

目标架构沿用 `docs/LOCAL_REMOTE_PARITY_SPEC.yaml::LR-FILE-LIST-COMPLETE-01`：

1. `list_terminal_directory` 的 read_dir/lexical metadata 任一错误都返回 listing Error。
2. broken symlink 的 target lookup 失败仍通过 `target_kind=Missing/Other` 表达，不删除 lexical link row。
3. static check 同时扫描 Terminal backend 与 remote helper，禁止 silent `continue` / best-effort drop。

门禁测试：`terminal_directory_listing_metadata_error_is_not_silently_dropped`、`terminal_symlink_directory_listing_keeps_link_namespace`、`terminal_broken_symlink_resolve_keeps_link_identity`。

## #43 状态：CLI-agent cwd metadata 必须由共享 host-scoped normalizer 投影

标题/id/cwd extraction 已集中，但本地 scanner 直接保存 raw cwd，remote scanner 仍独占 `clean_cwd`：相对路径、已删除目录、session store 内路径在本地可见，在远程却变成 None，Resume 启动目录因此继续分叉。

目标架构由 `docs/SESSION_NAVIGATOR_SPEC.yaml::SN-INTRA-CWD-PARSER-PARITY-01` 定义：

1. `cli_agent_jsonl` 同时拥有 cwd extraction 与 host-scoped normalization。
2. local/remote scanner 都传入同一 host home，统一 expand `~`、absolute/existing 校验与 session-store 排除。
3. 删除 remote-only `clean_cwd` / `cwd_from_item` 第二套实现，static check 禁止重建。

门禁测试：`cli_agent_session_cwd_normalization_is_shared`、`test_local_cli_agent_scan_uses_shared_cwd_normalization`、`test_remote_cli_agent_scan_uses_shared_cwd_normalization`。

## #44 状态：CLI-agent logical history quota 必须是共享的全局集合语义

`WORKSPACE_SESSION_NAVIGATOR_LOGICAL_LIMIT` 虽然被本地与远程共同引用，但本地 scanner 把它作为 `limit_per_agent` 分别传给 Claude/Codex，远程 scanner 则把两种 provider 聚合后只截一次。因此相同 store 在本地最多出现 `2 * limit` 条逻辑会话，在远程最多只有 `limit` 条，会制造稳定的会话缺失差异。

目标架构由 `docs/SESSION_NAVIGATOR_SPEC.yaml::SN-INTRA-LOGICAL-LIMIT-PARITY-01` 定义：

1. `cli_agent_jsonl` 提供唯一 logical source limiter，统一 physical source 去重、`(agent, provider session id)` 聚合、逻辑会话 recency 排序与全局 quota。
2. provider discovery 可以各自读取至多 `limit` 个候选，但 local/remote 都必须在所有 provider 聚合完成后只调用一次共享 limiter。
3. 入选逻辑会话保留全部 backing sources，Codex rollout 与 `session_index.jsonl` enrichment 不竞争 quota。
4. static check 禁止 local `limit_per_agent` 语义及 remote-only limiter 回归。

门禁测试：`cli_agent_logical_limit_is_shared_across_providers`、`test_local_cli_agent_scan_applies_global_logical_limit`、`test_remote_cli_agent_scan_applies_global_logical_limit`。

## #45 状态：Codex session index record 必须由唯一 parser 投影

transcript metadata 已共享，但 `session_index.jsonl` 仍有两套消费者：本地 `parse_codex_session_index_line` 只接受顶层 `id`，远程 scan loop 接受 `id/session_id`；title、cwd 与 RFC3339 `updated_at` 也分别解析。这会让相同 index 在本地缺 row、远程有 row，或后续格式演进时再次漂移。

目标架构由 `docs/SESSION_NAVIGATOR_SPEC.yaml::SN-INTRA-CODEX-INDEX-PARSER-PARITY-01` 定义：

1. `cli_agent_jsonl` 提供唯一 `CodexSessionIndexRecord` parser，集中 id fallback、title、cwd 与 updated_at epoch millis。
2. local/remote 都通过共享完整 JSONL reader 获取 values，再将 shared record 映射成各自输出类型。
3. 删除 `parse_codex_session_index_line` 与 remote index loop 的直接字段提取，static check 禁止重建。

门禁测试：`codex_session_index_record_parser_is_shared`、`test_local_codex_index_accepts_shared_session_id_fallback`、`test_remote_codex_index_accepts_shared_session_id_fallback`。

## #46 状态：Session Navigator initial scan 必须只有一个 typed commit boundary

Workspace 构造器目前调用 `scan_terminal_cli_agent_sessions`，该 wrapper 会把 typed scan error 记录后转换成 `Vec::new()`；`configure_new_workspace` 随后又调用 `refresh_indexed_cli_agent_sessions` 完成第二次扫描。启动日志因此稳定出现重复 local scan，而旧 wrapper 仍允许未来调用方把“扫描失败”重新解释为空 store。

目标架构由 `docs/SESSION_NAVIGATOR_SPEC.yaml::SN-INTRA-INITIAL-SCAN-BOUNDARY-01` 定义：

1. Workspace 构造阶段只建立空的 indexed cache，不执行 I/O。
2. 首次扫描与后续 Refresh 都只调用 `try_scan_terminal_cli_agent_sessions`，并通过 `commit_complete_cli_agent_session_scan` 提交完整 Success。
3. 删除 `scan_current_app_cli_agent_sessions` / `scan_terminal_cli_agent_sessions` 两层 error-to-empty wrapper，static check 禁止恢复第二入口。
4. 冷启动只扫描一次；失败保留构造时空 cache 或既有 cache，并显式记录错误，不伪装成成功空集。

门禁测试：`test_session_navigator_initial_cache_uses_typed_refresh_boundary`、`session_refresh_scan_failure_preserves_cached_rows`。

## #47 状态：dev remote helper install 必须 single-flight，timeout 必须分层

真实 GUI runtime 中，同一 `AshideDev` 进程先后派生了两组完全相同的 `cargo zigbuild --target x86_64-unknown-linux-musl --profile dev-remote`。两个 build 竞争 CPU/target lock，Workspace 在 900 秒触发 `environment runtime installing timed out`，底层 compile 也以同样 900 秒返回失败，但 cargo 输出随后分别显示 18m16s 与 27m53s 才结束。固定相同 timeout 既没有 single-flight，也没有给等待、上传和安装校验留预算。

目标架构由 `docs/SESSION_NAVIGATOR_SPEC.yaml::SN-CROSS-REMOTE-INSTALL-SINGLEFLIGHT-01` 定义：

1. DEBUG/source build 的 `dev_install_local_binary` 进入唯一 process-wide async gate；所有 session/Workspace/transport 共用。
2. gate 必须覆盖 detect → local freshness/build → remote stamp check/upload → verify，等待者进入后重新检查状态，禁止第二次 cargo build或第二次 daemon replacement。
3. `DEV_CROSS_COMPILE_TIMEOUT` 是底层 cargo 的单独预算；dev runtime installation watchdog 必须严格大于它，并额外覆盖 gate wait、上传和 verify。
4. release helper 路径不受 dev gate 影响；正式发布仍使用既有 release asset contract。

门禁测试：`dev_remote_install_is_process_single_flight`、`test_dev_environment_runtime_installation_watchdog_exceeds_cross_compile_budget`。

## #48 状态：watcher rename 必须按 lexical inode 存在性分类

真实 GUI 中，Project Explorer 对 `broken-link -> missing-target` 执行重命名后，文件系统上的新 symlink inode 已存在，但 row 立即消失。共享 watcher 在处理 macOS `RenameMode::Any` 时调用 `Path::exists()`；该 API 跟随 symlink target，因此 dangling symlink 永远被解释为“不存在”，新路径被错误归类为 delete。

目标架构由 `docs/LOCAL_REMOTE_PARITY_SPEC.yaml::LR-FILE-SYMLINK-RENAME-01` 定义：

1. watcher 边界建立唯一 lexical path state resolver，只使用 `symlink_metadata` 观察路径自身 inode。
2. `NotFound` 映射为 `Missing`；普通成功映射为 `Present`；其他 I/O 错误是 `Unknown`，整批归一化显式失败，禁止 false delete。
3. `RenameMode::Any` 的 destination dangling symlink 必须进入 `added`，已移除 source 必须进入 `deleted`。
4. repo metadata 增量 upsert 使用 lexical metadata 重建 `Entry::Symlink(SymlinkTargetKind::Missing)`，无需完整 rescan 即可保持 row。
5. static check 精确禁止 `RenameMode::Any` 分支重新出现 `Path::exists()`、`metadata()` 或 `canonicalize()` 分类逻辑。

门禁测试：`rename_any_classifies_broken_symlink_destination_as_added`、`rename_any_classifies_missing_source_as_deleted`、`broken_symlink_rename_remains_visible_without_rescan`。
