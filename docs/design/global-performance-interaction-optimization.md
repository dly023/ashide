# Ashide 全局性能与交互效率优化清单

> 目标：优化 Ashide 全局关键路径的性能、交互效率和用户体验
> 文件：`app/src/workspace/view.rs`、`app/src/workspace/view/session_navigator.rs`、`app/src/workspace/view/vertical_tabs.rs`、`app/src/search/command_palette/navigation/`
> 关联：`EnvironmentTable`、`SessionNavigatorModel`、`SshTargetCatalog`、`SearchMixer`、`WarpUI` 列表/滚动组件

---

## 审计基线

- 2026-07-29 当前 checkout 的隔离 AshideDev 已真实启动；SQLite 后台预热耗时 8 ms。
- 该启动时 Session Navigator 扫描 9 个 provider，一轮总耗时 342 ms（Claude 16 条 / 163 ms，Codex 82 条 / 342 ms），本地提交 89 条会话。此前 119 ms 是另一轮基线，不得与本轮运行混写。
- 后续交互中 Codex 单源扫描观测到 42–190 ms；关闭/删除会话与新建 split 后可再次触发扫描。
- 空闲采样中主线程仍持续执行 scene paint；样本落在 terminal grid、`ClippedScrollable` 和列表格式化路径。该采样只证明当前 paint 路径，不能单独量化各优化项收益。
- GUI 可启动并有真实交互日志；当前执行环境没有 macOS Accessibility/Screen Recording 权限，无法自动截图或驱动视觉点击，后续 UI 项必须用已有 integration saved-position 入口或用户人工验收补齐。

---

## 优化项列表

### 1. 收敛 Session Navigator 后台扫描触发
- **现状**：启动、terminal bootstrap、关闭/删除 agent pane、split/new terminal 等事件都可能发起被动历史扫描。现有同 authority in-flight 合并避免持续 token starvation，但 scan 完成后相邻无来源变化事件仍可重新扫描；运行日志出现并发扫描、stale generation，以及 Codex 单源 190 ms 的重复 I/O。
- **目标**：只让真正可能改变 provider 历史集合的语义事件触发 PassiveProjection：初始 authority discovery、history-enabled 设置变化、source mutation 成功、runtime HOME 首次就绪、agent-bound carrier 移除。generic terminal bootstrap/split 只更新 live projection；显式 Refresh 始终强制重扫。失败、SourceMissing、取消和 stale completion 继续保留 canonical rows。
- **状态**：✅ 已完成 — 删除 generic terminal bootstrap/split 的 provider 扫描；显式 Refresh、runtime HOME 首次发现、source mutation 与 agent close 历史接管保留。隔离 AshideLocal 冷启动只提交 1 次初始扫描，随后普通 shell `Bootstrapped` 未触发第二次扫描。

### 1.1 修复多窗口冷启动的重复 local provider 扫描
- **现状**：2026-07-29 的当前 checkout 冷启动恢复 3 个窗口时，日志显示同一组 9 个 local provider 完成了 3 次 discovery（577 ms、246 ms、246 ms）。原有 coalescing 只覆盖单个 Workspace 的 `EnvironmentTable`，无法覆盖共享的本机 provider store。
- **目标**：对 provider/observed-provider/scope 输入完全一致的 local Workspace，仅执行一个进程级 source worker；每个 Workspace 仍必须通过自己的 `EnvironmentTable` token 提交 canonical Navigator projection。不同 scope、显式 refresh 的 generation、stale completion 和关闭窗口都必须 fail closed。
- **状态**：🟡 Registry source-transaction coordinator、scope / observed-provider 隔离、stale-generation fail-closed 与 recipient token fanout 已实现；2026-07-29 隔离 `AshideDev.app` 单窗口启动实际完成 9-provider discovery（342 ms）并提交 89 条。仍待有 3 个等价 local Workspace 的 cold restore runtime 证明只出现一轮 source scan；不可用单窗口日志替代该多窗口性能 gate。

### 2. Session Navigator 大列表 viewport 化
- **现状**：`vertical_tabs.rs` 每次 render 都 clone/filter 全量会话，构建 reorder units、split group、状态查询、搜索字符串以及每行 `Hoverable + Draggable + SavePosition` 元素；`ClippedScrollable` 只裁剪 paint，不裁剪元素构建。当前运行态已有本地 89 条、远端 82 条。
- **目标**：只构建可见行和小范围 overscan；不可见行用稳定几何占位。保留 canonical RowId、selection/restoring、拖拽 unit 边界、inline rename、详情 hover、saved-position readiness、搜索结果顺序与滚动位置。
- **状态**：✅ 已完成 — committed `SessionNavigatorModel.revision` 驱动 render-only `ReorderUnit` `ListState` projection。只有可见 unit 与小范围 overscan 生成 row element；stable `RowId`/selection/drop boundary 保持 owner 不变，scroll 按 unit identity 恢复，离屏或缺失 anchor 的 detail sidecar fail closed。

### 3. 会话全局搜索维护增量索引
- **现状**：会话 query 每次跨所有 window 收集 live sessions，并 clone 各 Workspace 已提交的 Navigator snapshots；Tantivy 路径每次 query 都创建 searcher、重建完整索引。输入每个字符会重复窗口遍历、字符串拼装和索引构建。
- **目标**：复用 `EnvironmentTable` / committed `SessionNavigatorModel` 的 canonical source，在 session projection 提交时增量更新共享 searchable documents；按键 query 只执行匹配。live prompt enrichments 保留，但不得创建第二套 identity、membership 或 local/remote overlay。
- **状态**：🟡 代码与静态/单测验证已完成，等待当前构建的独立 GUI 进程验收 — canonical reducer commit 现在只发布本窗口 committed documents 到 `WorkspaceRegistry`，并通过 generation event 刷新 DataSource 的派生 Tantivy index。query 不再枚举 window/workspace、调用 `all_sessions` 或 rebuild index；generation 不一致 fail closed。live terminal 仅可 enrich 已存在 live row，window close 会清除 partition。已覆盖 partition replace/close、event delivery、query-time rebuild 禁止和原 navigation search；`cargo check` 通过。`./script/run` 于 2026-07-29 重新打包并 codesign 了当前 bundle，但 macOS 将启动请求转给已有旧进程，且本环境没有 Accessibility/Screen Recording 权限，故不声称当前代码的视觉交互已验收。

### 4. Environment 快速选择器改为明确且键盘优先
- **现状**：Environment Strip 的 `+` 在 hover 时打开，打开即使用 `ExplicitRefresh`；popover 只有鼠标点击，没有 query、selected row、方向键、Enter/Escape。用户移动指针经过入口会触发弹层和潜在磁盘读取，目标多时定位效率低。
- **目标**：点击或明确快捷键打开；立即展示 `SshTargetCatalog` 已提交候选，必要时后台 refresh，不因 hover 制造副作用。提供搜索、上下选择、Enter 打开、Escape 关闭；保留显式 Refresh 与配置入口，并清楚区分 current/open/dormant/loading/error。
- **状态**：🟡 代码、静态门禁和特性编译已完成，等待隔离 GUI 验收 — `+` 现在仅在显式点击时打开，已删除 hover 打开与 open-time `ExplicitRefresh`；popover 直接显示 `SshTargetCatalog` 当前 committed snapshot，并提供搜索、stable alias 选中、Up/Down、Enter 和 Escape。Refresh 仍是唯一触发 catalog refresh 的显式入口；打开、关闭与其他 popover/环境导航的 transient query/selection 清理由 Workspace 统一拥有。已覆盖筛选/别名稳定性和源码负向探针，`cargo check -p warp --features gui,local_fs` 于 2026-07-29 通过；因当前 shell 无 Accessibility/Screen Recording 且已有旧进程可能接收 launch forwarding，暂不声明视觉交互验收。

### 5. Session Navigator 补齐键盘操作面
- **现状**：搜索框只过滤文本；会话行主要依赖单击激活和右键菜单执行重命名、置顶、删除等动作。未发现列表 selection navigation 或针对 Navigator 的 Enter/快捷动作，键盘用户需要频繁切换到指针。
- **目标**：搜索结果拥有稳定选中行；上下键移动、Enter 激活/聚焦、显式刷新快捷键、重命名/置顶/删除动作均走既有 typed `WorkspaceAction` 和 reducer，操作完成或取消后焦点返回合理位置。不得让键盘 selection 覆盖 Environment-owned Navigator selection 语义。
- **状态**：🟡 代码、静态门禁、focused test 与 `cargo check` 已完成，等待隔离 GUI 验收 — `VerticalTabsPanelState` 现在持有不可持久化的 `SessionNavigatorRowIdentity` cursor；它在 committed/query-filtered viewport 中按 stable identity 归一化，永不写入 `SessionNavigatorState.selected_row_id` 或 list index。搜索框支持 Up/Down、Enter（复用 `ActivateRestoredWorkspaceSession` typed action）与 Escape（同时清 query 文本和 cursor）；cursor 变化会通过 `ListState` 将所属 split-aware reorder unit 滚动到可见区域。

---

## 依赖关系与建议顺序

```text
1 扫描触发收敛
├─ 为 2 的真实性能基线降噪
└─ 为 3 提供稳定、低频的 canonical source commit

2 Navigator viewport 化 ──┐
                          ├─ 5 键盘 selection/scroll-to-visible 依赖列表几何与 viewport API
3 增量搜索索引 ────────────┘

4 Environment 快速选择器：与 1 共用“committed snapshot 先显、refresh 后台更新”原则，但 UI 可独立实施
```

建议执行顺序：**1 → 2 → 3 → 4 → 5**。

- **强耦合**：2 与 5。先确定 viewport/scroll/row identity，再做键盘 selection，避免实现两套滚动定位。
- **前置依赖**：1 先于 3。增量索引应由低噪声 canonical commit 驱动，不能把重复扫描事件原样放大成重复 index rebuild。
- **独立项**：4 可独立，但放在 3 后能复用搜索/selection 设计经验。
- **提交边界**：每项独立 commit；若 2 的 WarpUI viewport primitive 与 Navigator 接入不可分割，则同一项内提交，不和 5 混合。

## 验收门禁

- 行为/架构项固定按：SPEC → UX matrix → static check → failing test → implementation → focused verify → `cargo check` → GUI/runtime verify。
- 性能项必须记录可复现基线与改后证据；不能只用“代码更少”或编译通过声称更快。
- Session Navigator 相关改动必须保持 container identity、RowId、display order、selection、restoring、local/remote timing parity 和 title/binding ownership。
- UI 项必须验证键盘、鼠标、空状态、错误状态、加载状态、重复触发和取消路径。
