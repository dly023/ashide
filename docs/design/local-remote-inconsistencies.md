# 本地 / 远程行为不一致排查清单

对照 `docs/design/01-environment-first-class-runtime.md` 的核心原则——"本地和远程走同一上层语义，只有 backend/transport/capability 确实不同的地方才分支；分支只存在于 backend adapter，不泄漏到产品入口和用户心智"——系统性排查代码里因 `EnvironmentKind::Local/Ssh`、`uses_terminal_bootstrap/uses_runtime_transport`、`authority_uses_terminal_bootstrap` 等分支导致的本地 / 远程行为分叉。

跟踪清单见同目录 `local-remote-inconsistencies.csv`。本文档给出每项的代码位置、复现、修复方向。

当前实施状态以 `local-remote-inconsistencies.csv` 和 `local-remote-fix-plan.md` 顶部表格为准：#1/#2/#3/#7 已修，#4/#5/#6/#8 已关闭或确认合理 backend diff，#9 已修但待跑测，#10 为新确认的架构债务。

---

## 排查覆盖面

| 层 | 是否有 local/remote 分支 | 结论 |
|---|---|---|
| search / ai_context_menu | 无 | 干净，已收口 |
| notebooks / file 模型 | 无 | 干净（`BufferLocation` enum 统一 current-app / environment） |
| settings / command_palette / context_menu / block | 无 | 干净 |
| terminal 层 | 无 | 干净 |
| left_panel / persistence / autoupdate | 无 | 干净 |
| code editor / global_buffer_model | 有，走 `BufferLocation` 统一收口 | 合理 backend 分支 |
| code/file_tree | 有 `load_environment_directory`，feature gate + host_id 守卫 | 合理 backend 分支 |
| skill_manager | 有 scope 分流，接口统一 | 合理 backend 分支 |
| session_navigator | 有（删本地 session 焦点修复、存储结构差异） | 数据模型差异，P0 Environment table 待解决 |
| environment_runtime / view.rs | 多处 | **泄漏集中地** |

---

## P0 — 真实用户可见差异，建议优先修

### #1 `cd_to_directory` 在远程被静默忽略

**位置**：`app/src/workspace/view.rs:12387-12409`

```rust
fn cd_to_directory(&mut self, path: PathBuf, ctx: &mut ViewContext<Self>) {
    if !self.current_environment_uses_terminal_bootstrap(ctx) {
        log::warn!(
            "Ignoring current-app cd-to-directory while active environment is runtime-backed: {}",
            path.display()
        );
        return;
    }
    // 本地：把 `cd <path>` 填入输入框
    let cd_command = format!("cd {}", shell_words::quote(path_str));
    input_handle.update(ctx, |input_view, ctx| {
        input_view.replace_buffer_content(&cd_command, ctx);
    });
}
```

**问题**：file browser 右键 "cd to directory" 在本地有效（填入输入框），在远程只 `log::warn` 后 return，用户看不到任何反馈。旁边 `cd_to_environment_directory`（`view.rs:12414`）才是远程版本，且行为不同——**直接执行命令**而非填入输入框。

**结果**：同一个动作，本地填输入框让用户回车、远程直接执行；且本地这条路在远程时静默无效。

**修复方向**：
- 方案 A（推荐）：统一入口。`cd_to_directory` 按 capability 分流——本地走"填入输入框"、远程走 `cd_to_environment_directory`，不再静默 return。
- 方案 B：保持两个函数，但调用方按 environment capability 显式选其一，删掉 `cd_to_directory` 里的静默 ignore 分支。

---

### #2 `open_directory_in_new_tab` 在远程被静默忽略

**位置**：`app/src/workspace/view.rs:12426-12435`

```rust
fn open_directory_in_new_tab(&mut self, path: PathBuf, ctx: &mut ViewContext<Self>) {
    if !self.current_environment_uses_terminal_bootstrap(ctx) {
        log::warn!(
            "Ignoring current-app open-directory-new-tab while active environment is runtime-backed: {}",
            path.display()
        );
        return;
    }
    self.open_directory_tab_in_current_environment(path, false, ctx);
}
```

**问题**：和 #1 同类。"在新 tab 打开目录"在本地有效，在远程静默无效，且**没有对应的 environment 版本**（不像 cd 至少有 `cd_to_environment_directory`）。

**结果**：远程 file browser 里这个动作完全没反应。

**修复方向**：新增远程版本（类似 `open_environment_runtime_terminal` 但带 cwd = path），或统一入口按 capability 分流到 `open_directory_tab_in_current_environment`（本地）/ 远程 spawn（远程）。删掉静默 ignore。

---

### #3 `open_environment_file_with_target` 能力弱于本地 `open_file_with_target`

**位置**：`app/src/workspace/view.rs:10427-10450`

```rust
pub fn open_environment_file_with_target(
    &mut self,
    environment_file_path: crate::code::buffer_location::EnvironmentFilePath,
    line_col: Option<LineAndColumnArg>,
    ctx: &mut ViewContext<Self>,
) {
    // ...
    self.open_code(
        CodeSource::EnvironmentFileTree { environment_file_path },
        layout,
        line_col,
        false, /* preview —— 写死 */
        &[],   /* additional_paths —— 写死 */
        ctx,
    );
}
```

**问题**：远程打开文件 `preview = false`、`additional_paths = &[]` 写死；本地 `open_file_with_target` 这两个是可配参数。

**结果**：远程不支持 preview 打开、不支持多文件一起打开。这是 UI 能力差异，不是 transport 本质限制（preview 是 UI 行为，跟文件在哪无关）。

**修复方向**：把 `open_environment_file_with_target` 的签名改成接收 `preview: bool` 和 `additional_paths: &[PathBuf]`，和 `open_file_with_target` 对齐，透传给 `open_code`。调用方（`ServerFileBrowserEvent::OpenEnvironmentFile` 在 `view.rs:7818`、Ctrl/Cmd 点击路径在 `view.rs:18531/19783`）按需传入。

---

### #7 welcome view 打开项目时，code review pane 本地有、远程无

**位置**：`app/src/workspace/view.rs:6663-6701` + `app/src/pane_group/pane/welcome_view.rs:167-189`

```rust
// welcome_view.rs:174
let terminal_ready = workspace.open_agent_directory_tab_in_current_environment(
    path_buf.clone(), false, ctx,
);
// welcome_view.rs:180-181
// Open code review pane only for immediately materialized current-app terminals.
if terminal_ready {
    workspace.active_tab_pane_group().update(ctx, |tab, ctx| {
        // ... toggle_code_review_pane
    });
}
```

`open_agent_directory_tab_in_current_environment`（`view.rs:6663`）：
- 本地路径：`open_terminal_bootstrap_directory_tab` + `start_agent_mode_in_new_pane`，返回 `true`（terminal 立即可用）。
- 远程路径：`try_route_current_runtime_environment_entry` → `open_environment_runtime_agent_entry`，只 queue intent，返回 `false`（terminal 还没 materialize）。

**问题**：welcome view 打开项目时，本地会自动开 code review pane，远程不会（因为 `terminal_ready == false`）。

**结果**：用户从欢迎页打开同一个项目，本地有 code review pane、远程没有。

**修复方向**：把 code review pane 的开启改为 deferred——不依赖 `open_agent_directory_tab_in_current_environment` 的同步返回值，而是在远程 terminal materialize 后（environment runtime terminal ready 事件）再触发开 code review pane。或者让 `open_agent_directory_tab_in_current_environment` 远程路径也返回一个"pending"信号，welcome view 据此注册一个回调。

---

### #9 tab 激活后 dormant runtime placeholder 未必 rehydrate

**位置**：`app/src/workspace/view.rs:5597-5610`、`app/src/workspace/view.rs:7361-7399`

**问题**：普通 tab switch 不会 teardown runtime，但历史上不同入口有的只调用 `activate_tab_internal`，只更新 UI focus/title，不会补走 runtime hydration。若切回的是 dormant runtime placeholder，就可能出现“标签还在，但 terminal 没 materialize / transport 没 ensure / 用户感觉盲”的状态。

**结果**：从用户视角看，就是“激活过的远程标签切回来不保活、容易被打断”。本质不是切走时杀 PTY，而是切回来时没恢复 runtime intent。

**修复方向**：

- 用户可见 tab 激活入口统一走 `activate_tab`。
- `activate_tab` 在 `activate_tab_internal` 后调用 `prepare_active_environment_after_visible_tab_activation`。
- `focus_pane` 默认也触发该 hook；只在明确“不应切当前 Environment”的路径使用 preserving 版本。
- shortcut / close-active-tab fallback / delete reselect / transfer 等入口都接统一路径或显式调用同一 hook。

---

### #10 远程会话 alias / pin 写在本机 sidecar，且 deleted tombstone 设计不应存在

**位置**：

- `app/src/workspace/view/session_navigator.rs`
- `app/src/terminal/cli_agent_session_index.rs`
- `app/src/remote_server/cli_agent_session_user_state.rs`
- `crates/remote_server/proto/remote_server.proto`

**问题**：Session Navigator 的用户态状态原先全部落本机 sidecar，远程 session scan 只返回 records；合并时仍使用本机 `pinned_session_ids` / `session_aliases`。这导致远程环境里的会话别名、置顶状态不是远端配置的一部分。换一台本地机器或重装本地 Ashide，远程别名会丢；多客户端打开同一远端也不会共享 Session Navigator 的用户意图。

同时，deleted tombstone 不符合“硬扫描 provider 实际会话文件/索引”的模型：删除应该修改 provider-owned source；若 source 删除失败，Session Navigator 不应通过 UI 状态把真实仍存在的会话隐藏起来。

**修复**：新增 Environment-owned user-state store，并删除 deleted tombstone：

- local/current-app：写本机 `~/.ashide/session_state.json`。
- remote runtime：通过 remote_server RPC 写远端 `~/.ashide/session_state.json`。
- user-state 只保存 `aliases` / `pinned`。
- 删除顺序保持“先删 provider source 成功，再清 alias / pin”；失败则不隐藏 session。
- scan result 是 source of truth，UI 不保存 deleted filter。

---

## P1 — 信息量 / 状态机不对称，建议后续修

### #4 session 级别环境标签不对称

**位置**：`app/src/workspace/environment_runtime.rs:537-549` + `app/src/workspace/view/vertical_tabs/session_display.rs:185-198`

```rust
pub(crate) fn session_environment_display_label(authority: &str) -> Option<String> {
    let authority = authority.trim();
    if authority.is_empty() || authority_uses_terminal_bootstrap(authority) {
        return None;   // 本地返 None
    }
    // 远程返主机名
    Some(authority.strip_prefix("ssh:ssh-config:")...)
}
```

**问题**：本地 authority 返回 `None`，远程返回主机名。`restored_session_environment_label` 用它生成 session navigator 的 session subtitle——本地 session fallback 到 cwd 或 "no root"、远程显示 "Environment runtime: <host>"。

**结果**：session 详情的信息量本地 / 远程不一致（chip label 层已统一，session 详情层没统一）。

**修复方向**：给本地也返回一个 label（如 "Local"），或统一 `restored_session_detail` 渲染逻辑，让本地 / 远程的 subtitle 结构一致。

---

### #5 terminal capability env 双份复制粘贴

**位置**：`app/src/workspace/environment_runtime.rs:1792-1813` + `app/src/terminal/local_tty/unix.rs:264-319`

**问题**：`TERM` / `TERM_PROGRAM` / `COLORTERM` / `TERM_PROGRAM_VERSION` / `WARP_CLIENT_VERSION` / `WARP_CLI_AGENT_PROTOCOL_VERSION` 这套 env 在本地 PTY 和远程 runtime PTY 各有一份生成逻辑，复制粘贴。两边目前一致，但未来易漂移。

**结果**：潜在漂移风险（设计文档 P1 已承认）。

**修复方向**：抽成单一 builder（如 `terminal_capability_environment_variables()`），本地 PTY 和 runtime PTY 都调用它。设计文档 P1 "统一 terminal capability builder" 即此项。

---

### #6 Environment Error lifecycle 只对远程

**位置**：`app/src/workspace/view.rs:10111` `handle_environment_runtime_failed` + `app/src/workspace/environment_runtime.rs:6464` `mark_environment_runtime_error_for_authority`

**问题**：远程有完整的 error lifecycle（`handle_environment_runtime_failed` → mark error → 更新 lifecycle → notify → reconnect）；本地 terminal 出错走 terminal view 自己的渲染，不进 Environment lifecycle 状态机。

**结果**：远程连不上有明确 Environment Error 状态 + reconnect UI，本地终端挂了是另一套 UI。违反"本地也是 Environment"原则。

**修复方向**：给本地 Environment 也加一个 error lifecycle 状态（哪怕只是把 terminal view 的错误映射到 EnvironmentLifecycleState::Error），或在设计文档明确承认这是 backend 差异、不收口。设计文档 P1 teardown policy 状态机目前只针对远程，可一并补本地。

---

## P2 — 疑似合理 backend 分支，需确认

### #8 session restore 命令构造不对称

**位置**：`app/src/workspace/view.rs:8364-8388`

```rust
// 远程：只返回 pending_command
fn restored_environment_runtime_startup_command(pending_command: Option<String>) -> Option<String> {
    pending_command.filter(|command| !command.trim().is_empty())
}

// 本地：拼 `cd <cwd> && pending_command`
fn restored_terminal_bootstrap_startup_command(session, pending_command) -> Option<String> {
    let cd_command = session.cwd.as_deref()...map(|cwd| format!("cd {}", shell_words::quote(cwd)));
    match (cd_command, pending_command) {
        (Some(cd), Some(cmd)) => Some(format!("{cd} && {cmd}")),
        ...
    }
}
```

**判断**：疑似合理 backend 分支——远程 runtime spawn PTY 时应该直接用 cwd 启动（所以不需要再 cd），本地只能在 terminal 启动后 cd。测试 `test_restored_environment_runtime_startup_command_does_not_duplicate_cd`（`view_test.rs:4620`）也暗示这个意图。

**待确认**：远程 spawn PTY 时是否真的用 session.cwd 作为启动 cwd？如果是，这项不是问题；如果没传，远程恢复的 session 不会到原工作目录，是 bug。

**修复方向**：核实 `open_environment_runtime_terminal` / spawn plan 是否把 cwd 传给 runtime PTY 的 spawn cwd。若已传，本项关闭；若没传，补上。

---

## Top 3 修复优先级

1. **#9**（tab 激活 lifecycle）—— 用户当前直接遇到的“切回来盲/不保活”，已补代码，优先验证。
2. **#10**（Session Navigator user-state 归属）—— 远程会话别名/置顶/删除状态写本机，和 Environment 一等抽象冲突，下一阶段修。
3. **阶段 3 Environment table / RuntimeBackend** —— 继续绞杀散落的 local/remote 分支，把前面修复沉淀成数据模型。

#1/#2/#3/#7 已修；#4/#5/#6/#8 已关闭或确认合理 backend diff。

---

## 附：跟踪 CSV

同目录 `local-remote-inconsistencies.csv`，字段：`id,file,line,area,symptom,local_behavior,remote_behavior,is_leak,severity,status,fix_direction`。开发改完一项把 `status` 从 `found` 改成 `fixed` 即可。
