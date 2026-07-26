# Environment 一等 Runtime 抽象

本文件记录 Ashide 当前 Environment / Workspace / Tab / Pane 的分工、这次修复的行为边界，以及后续把本地和远程收口到同一抽象的推进路径。

## 目标

Ashide 的上层产品语义应该是：**Environment 是一等抽象**。

- Environment 拥有运行位置、连接、runtime lifecycle、root/home、能力集合和视觉身份。
- Workspace 拥有已打开/已激活 Environment 的 runtime lifecycle。
- Tab 只拥有 view/focus/order，不拥有 runtime 的生死。
- Pane 只拥有一个具体的 shell / agent / placeholder view。
- 本地与远程默认走同一上层语义；只有 backend / transport / capability 确实不同的地方才分支。

换句话说，用户切换标签不应该杀掉或降级一个已经激活过的远程 Environment；本地 Environment 也不应该在 UI 或 terminal capability 上表现成“没有身份的默认状态”。

## 当前项目现状

### 已有的好基础

- `Workspace` 已有 `current_environment`，每个 tab 也可以带 `EnvironmentSnapshot`。
- 远程 runtime 已有 `EnvironmentRuntimeRegistry`，用 authority 管理 synthetic session、host、control socket 和 lifecycle。
- 新建终端时已经能通过 `EnvironmentRuntimeSpawnPlan` 分流：
  - local/current-app：`TerminalBootstrap`
  - connected remote：`RuntimeTarget`
  - 未连接 remote：`RuntimeBootstrap`
- Environment Strip 已经按 `EnvironmentSnapshot` 渲染，并能展示 lifecycle dot、reconnect/disconnect 控制。

### 遗留耦合点

- `current_environment` 仍然是 window/tab 迁移期字段，不是完整 Environment table。
- tab 激活路径仍会同步 `current_environment` 和 active pane 的 terminal options，说明 view focus 与 runtime lifecycle 还没有完全解耦。
- 远程 runtime 的控制 session 和 visible terminal session 已经拆开，但 registry 仍主要面向远程；local 仍叫 `terminal_bootstrap`，不是完整 Runtime backend。
- 本地 Environment 过去没有 chip label，视觉上更像“默认空状态”；远程才有明显 label，造成 local/remote 心智不一致。
- 远程 native PTY 过去没有显式注入与本地 PTY 一致的 terminal capability env，导致颜色/terminal identity 容易出现本地黑白、远程彩色或反向不一致。
- Session Navigator 的 alias / pin 曾是 current-app sidecar：即使 session 来自远程 Environment，这些用户状态也会写本机 `~/.ashide/session_*.json`，不是远端 `~/.ashide`。本轮已改为 environment-owned store；deleted tombstone 设计删除，scan result 是 source of truth。

## 本次改动

### 1. Workspace 级 retained authority

新增 Workspace 持有的 runtime authority 集合：

- 激活/打开 runtime-backed Environment 时 retain authority。
- retain 时同步 `EnvironmentRuntimeRegistry` 里的 snapshot，确保 Environment 不因为当前 tab focus 变化而丢失 runtime 上下文。
- 普通 tab switch 只更新 view/focus，不 release authority。
- 显式 Disconnect 才 release authority，并清理 pending intent / restoring state / runtime registry。

这把生命周期语义从“哪个 tab 当前 active”推进到“Workspace 已经接受这个 Environment 进入 runtime 管理”。

### 2. retained runtime 断线后自动重连

当 transport 已连接后发生断开，或 bootstrap 后的 client request 报 `Disconnected`：

- 如果 authority 仍被 Workspace retained，就自动走 `reconnect_environment_runtime_authority`。
- reconnect 使用 `clear_user_intents = false`，保留 pending terminal / agent / restore intent。
- 显式 Disconnect 已经先 release authority，因此不会被自动重连反向拉起。

这对应用户体验上的“激活过的标签就一直想办法保活”。

### 3. 本地/远程视觉身份统一

`environment_display_info_for_environment` 现在给 local/current-app 也返回 chip label（例如 `Local`），不再只有 icon/dot。

含义是：本地不是“无 Environment”，而是一个具体 Environment backend。远程只是另一个 backend。

### 4. terminal capability builder 统一

新增 `terminal_capability_environment_variables()`，由本地 Unix PTY、Docker sandbox PTY 和远程 native runtime PTY 共用。所有 backend 统一声明关键 terminal capability env：

- `TERM=xterm-256color`
- `TERM_PROGRAM=WarpTerminal`
- `COLORTERM=truecolor`
- `TERM_PROGRAM_VERSION`（有 app version 时）
- `WARP_CLIENT_VERSION`
- `WARP_CLI_AGENT_PROTOCOL_VERSION`（HOA notifications 开启时）

这解决“本地黑白、远程彩色/带着色”的抽象不一致：颜色能力不再由 backend 偶然决定，而由 Ashide terminal capability builder 统一声明。

### 5. 回归测试

新增覆盖：

- 切离 runtime Environment tab 后，runtime session / lifecycle / retained authority 仍保留。
- 显式 disconnect 会 release retained authority 并移除 runtime registry。
- retained runtime transport disconnect 后会重新注册 session 并进入 Connecting。
- local Environment display 有和 runtime Environment 一样的 chip label 形态。
- runtime PTY 会声明与 local PTY 对齐的 terminal capability env。

### 6. Tab 激活路径补洞

用户可见的 tab 激活入口已统一到 `activate_tab`，并在 `activate_tab_internal` 之后执行 Environment runtime hydration：

- queue active runtime placeholder terminal intent。
- ensure 当前 Environment runtime transport。
- materialize 当前 Environment 的 pending terminal。

`focus_pane` 默认也走同一 hook；只有 Settings / Notebook / inspector 这类“聚焦 pane 但不切当前 Environment”的路径使用 preserving 版本。

这解决的不是“切走时别杀 runtime”（代码里普通 tab switch 本来就没有 teardown），而是“切回来时 dormant placeholder 必须恢复成可用 runtime”。

## 行为原则

### Tab 不拥有 runtime

Tab 是 UI 容器：focus、order、title、pane layout。普通切 tab 不应该产生 runtime teardown 副作用。

### Environment 拥有 runtime lifecycle

Environment 的 authority 是 lifecycle 边界。只要 Workspace retained 了 authority，runtime 就应该尽力保持 active / reconnecting / recoverable。

### 显式动作才 teardown

只有用户明确 Disconnect、关闭/清理 Environment、或应用级 teardown，才 release authority 并 deregister runtime session。

### 本地也是 Environment

本地/current-app 不应该被当作“没有环境”。它应该拥有同样的 visual identity、tool capability 判断入口和 terminal capability 声明。差异应藏在 backend adapter：local 用 terminal bootstrap，remote 用 runtime transport。

## 后续推进路径

### P0：把 retained authority 提升为 Environment table

当前 retained authority 是 Workspace 内的集合，解决了 lifecycle ownership，但还不是完整 source of truth。下一步应收口成 Environment table：

- key：authority
- snapshot：label、kind、root/home、connection ref、capabilities
- lifecycle：dormant / connecting / connected / reconnecting / error
- runtime handle：session id、host id、control path、heartbeat generation
- user intent：open terminal / open agent / restore / startup command

让 Environment Strip、Session Navigator、File Browser、new terminal 全部读这张表，而不是散落读 tab/current_environment/registry。

### P0.5：Session Navigator user-state 按 Environment 归属

alias / pin 应该归属于 session 所在 Environment；deleted 不应作为 UI state 保存：

- local/current-app session：写本机 `~/.ashide/session_state.json`。
- remote runtime session：通过 remote_server 写远端 `~/.ashide/session_state.json`。
- delete：先删除 provider source，成功后清 alias / pin；失败不隐藏真实 scan 到的 session。

已落地方式：

1. Workspace 层新增 environment-aware user-state 读写入口。
2. remote_server 新增专门 user-state RPC，和 provider source 的 `MutateCliAgentSession` 分离。
3. remote scan 后读取远端 user-state，再生成 Session Navigator rows。
4. 本地/远端 `session_state.json` 只保存 `aliases` / `pinned`。

### P1：抽象 RuntimeBackend，绞杀 local/remote 分支

引入 backend trait 或 enum，明确两个后端：

- `LocalBackend`：spawn local PTY，文件/搜索/skill 走 current app/local fs。
- `RemoteBackend`：spawn runtime PTY，文件/搜索/skill 走 remote server RPC。

上层只问 Environment capability，不直接问 `EnvironmentKind::Local` / `EnvironmentKind::Ssh`。

### P1：明确 teardown policy

补一个明确状态机：

- `Retained + Connected`：保持 heartbeat。
- `Retained + Disconnected`：自动 reconnect。
- `Released + Connected`：显式 teardown。
- `Released + Disconnected`：只清状态，不重连。
- `Error`：区分 transient error 和 user-action-required error，避免 auth/config 错误无限重试。

### P2：把 local root/home 也纳入 Environment roots

远程已有 runtime home roots；本地应走同样接口，只是 provider 是 local fs。这样 Project Explorer / File Browser / Skill Manager 不需要知道 backend。

### P2：GUI 运行时验真

Rust 测试只能证明模型和状态机。涉及真实体验时还需要 GUI 验证：

- 启动命令：`TERM=xterm-256color ./script/run`
- 观察 Environment Strip local/remote chip 是否一致。
- 打开远程 Environment，切到本地 tab，再切回远程 tab，确认远程 session 未被打断。
- 在 retained runtime 断线后确认 UI 进入 Connecting/Reconnecting，而不是静默死掉。

## 判断标准

做到“本地和远程没有任何区别”不是指实现没有 backend 分支，而是：

- 用户看到的是同一套 Environment 概念。
- tab/pane/session 的行为不因 local/remote 改变生命周期语义。
- 新 terminal / agent / file browser / skill manager 都从 Environment capability 出发。
- 必须分支时，分支只存在于 backend adapter，不泄漏到产品入口和用户心智。
