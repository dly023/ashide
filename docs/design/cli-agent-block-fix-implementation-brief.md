# CLI Agent Block Fix 实现思路 Brief

> 目的：给后续实现者一份**不抄上游代码**的实现说明。本文只记录当前行为漏洞、上游 zap 的抽象修复思路、Ashide 侧推荐拆分方案和验收标准。
>
> 背景上游参考：zap `4f89c20` / `Cli agent block fix (#286)`。不要直接 cherry-pick；Ashide 当前 `cli.rs` / blocklist / BYOP fallback / LRC session 逻辑已经有本地差异，整包 patch 容易造成结构断裂。

---

## 1. 问题概览

CLI agent block 同时出现在两个用户可见路径：

1. 主对话区 / block list 中的 CLI agent block。
2. Terminal 右下角或 alt-screen 场景中的 CLI agent 浮窗。

当前风险集中在浮窗路径，但部分修复应复用到主 block list：

- 多轮对话气泡互相清空划词选择。
- 浮窗内容滚动体验不稳定。
- 鼠标滚轮事件可能穿透到背后的 terminal / block list。
- 历史 exchange 渲染和当前 exchange 渲染边界不清晰。
- `Hide responses` 只隐藏输出，不一致地保留用户 query/header。
- 多段历史输出参与 secret redaction 时，索引容易错位。

这些问题本质上不是单个 UI 细节，而是 **CLI agent block 把多个交互区域平铺在同一个容器里后，事件广播、selection state、scroll state 和历史模型没有被分区管理**。

---

## 2. 当前行为漏洞

### 2.1 多气泡划词互相清空

典型场景：

1. CLI agent 浮窗中有多轮内容：用户 query、agent output、tool/action block。
2. 每个气泡内部都有可选择文本区域。
3. 用户在第 N 个气泡中拖拽选中文字。
4. 父容器把同一个 mouse down / drag / up 事件广播给多个子元素。
5. 未命中的气泡也进入 selection 回调。
6. 未命中的气泡按“点击空白清选择”的逻辑清掉其它 selection handle。
7. 真正命中的第 N 个气泡刚选中的文本被其它气泡清掉。

用户可见表现：

- 划词时闪一下就消失。
- 鼠标松开后 selection 为空。
- 最后一个气泡相对容易选中，前面气泡容易被后续气泡清掉，或者反过来，取决于事件广播顺序。
- query/output/action 三类区域之间互相干扰。

根因：

- selection state 是全局/粗粒度的。
- 回调没有确认“本 SelectableArea 是否真的参与了这次选择”。
- 未命中区域和命中区域执行了同等清理逻辑。

### 2.2 浮窗滚动与滚轮穿透

典型场景：

1. CLI agent 浮窗内容高度超过浮窗高度。
2. 鼠标停在浮窗内容区域滚动。
3. 内部滚动到边界，或内部滚动容器没有消费事件。
4. 滚轮事件继续冒泡/穿透到背后的 terminal 或 block list。

用户可见表现：

- 浮窗没滚动，背后的终端滚了。
- 滚到顶部/底部后页面或 terminal 继续动。
- 浮窗边界滚动手感不一致。

根因：

- 浮窗内部滚动容器没有明确“消费滚轮事件”。
- 内部 scroll state 和外层 scroll state 边界不清。
- event dispatch result 没有表达“这个事件已经由浮窗处理”。

### 2.3 历史内容丢失或渲染边界混乱

CLI agent 浮窗可能需要同时显示：

- 当前 exchange。
- 当前 task 下此前 exchange。
- 后续 append 进来的 exchange。

当前风险：

- 只渲染当前 model，历史内容被替换或丢掉。
- append exchange 后重复渲染。
- Ashide BYOP / LRC fallback 场景中，初始时可能没有真实 subtask exchange，只能临时用 root task last exchange 占位；后续真实 exchange 到来后需要切换。

用户可见表现：

- 浮窗只剩最后一轮。
- 历史消息重复。
- follow-up 后上下文显示跳变。

根因：

- 缺少“历史 exchange id 列表”和“当前 live exchange id”的明确分层。
- 缺少去重逻辑。
- fallback 占位和真实 exchange 的生命周期边界没有集中处理。

### 2.4 Hide responses 语义不一致

当前语义问题：

- 在某些路径中，`Hide responses` 只隐藏 AI output。
- 用户 query/header 仍可见。
- CLI 浮窗和主 block list 行为不一致。

更合理的用户预期：

- 如果功能叫 Hide responses，至少对于“这一轮对话内容”应统一隐藏 query/header/output，或者明确区分 Hide output 与 Hide exchange。
- 同一个 block 在主列表和浮窗中不应行为不同。

### 2.5 Secret redaction 索引错位

当一个 CLI agent block 渲染多段历史输出时：

- redaction 状态如果仍按单一 output index 计算，容易把第 A 段的脱敏范围应用到第 B 段。
- 如果 query/output/action 混排，文本段序号也容易偏移。

用户可见风险：

- 应脱敏内容没脱敏。
- 不该脱敏内容被遮住。
- 复制选中文本时，脱敏/原文边界不一致。

---

## 3. 上游实现思路抽象

不要抄代码，只吸收以下设计思想。

### 3.1 Selection 必须按“区域 + 序号”分区

把 selection handle 从单个字段升级为分组列表：

- query selection handles：按 query 气泡 index 管理。
- output selection handles：按 output 气泡 index 管理。
- action selection handles：按 action/tool 气泡 index 管理。

每个 SelectableArea 绑定自己的 handle。

selection 回调里不要无条件清理其它区域，而是先判断：

- 当前区域是否正在 selecting。
- 或本次回调是否产生了非空 selection。

只有满足以上条件，才认为“本区域确实参与了本次选择”，然后：

- 清理其它 query/output/action 区域的 selection。
- 保留当前区域自己的 selection。
- 更新 selected text / copy-on-select 状态。

未命中区域收到广播事件时，不做清理。

### 3.2 清理选择要有统一入口

实现一个概念上的 helper：

- 输入：三组 selection handles、当前活跃组、当前活跃 index。
- 行为：
  - 清掉非活跃组全部 selection。
  - 清掉活跃组中除当前 index 外的 selection。
  - 不清当前 index。

这样 query/output/action 三类区域不会各写一套容易分叉的逻辑。

### 3.3 浮窗滚动必须显式消费事件

浮窗内部应该有明确的 conversation scroll state。

当鼠标滚轮发生在浮窗内部时：

- 优先滚动浮窗内容。
- 返回“事件已处理/已消费”的 dispatch result。
- 即使到达边界，也不要让事件穿透到背后的 terminal，除非产品明确要求边界透传。

滚动分层建议：

- 外层：控制浮窗整体尺寸和 conversation scroll。
- 内层：代码块、长文本、action block 如果有局部 scroll，需要避免和外层抢事件。
- 对于普通 CLI agent conversation，优先让外层 conversation 统一滚动，减少多层滚动冲突。

### 3.4 历史 exchange 渲染要和 live exchange 分层

浮窗 model 推荐拆成：

- `live_model`：当前最新 exchange 的模型。
- `history_models`：同 task 下需要一起显示的历史 exchange 模型列表。
- `history_exchange_ids`：去重集合/有序列表，防止 append 事件重复加入。

渲染时：

- 如果有 `history_models`，按历史顺序渲染它们。
- 如果没有历史列表，只渲染 `live_model`。
- 对最新 model 才显示某些 live-only UI，例如 running state、web search searching、失败 footer、pending action 等。

事件处理时：

- `AppendedExchange` 到来后，只在 exchange id 没出现过时加入 history。
- 对于 Ashide BYOP / LRC fallback：
  - 初始没有 subtask exchange 时，可以显示占位 exchange，但不要把占位误加入 subtask history。
  - 真实 subtask exchange 到来后，切换到真实 exchange 并去重。

### 3.5 Hide responses 判断集中化

不要在多个渲染点散写：

- query/header 是否显示。
- output 是否显示。
- pinned/top block 是否特殊显示。

推荐集中成一个语义函数：

- 输入：是否有 query、是否 first block query/header 应隐藏、是否 `should_hide_responses`。
- 输出：是否渲染 query/header。

主 block list 和 CLI 浮窗复用同一语义，避免一个路径藏 output、另一路径 query 还在。

### 3.6 Secret redaction 索引必须按真实渲染顺序推进

当渲染多个 model / 多个 output message 时：

- 文本 section index、code section index、table/image section index 都应按真实渲染顺序递增。
- redaction state 使用的 index 要和最终渲染的 text section 对齐。
- hide responses 时，如果某段不渲染，应明确它是否仍占 index；建议“不渲染就不推进渲染 index”，避免不可见内容污染可见内容 index。

---

## 4. Ashide 推荐实现拆分

不要一次性搬完整上游 patch。建议拆为四个 PR / 四个提交。

### Step 1：只修 selection 互相清空

目标：最小闭环修复“划词一松手就没了”。

改动范围：

- CLI subagent view 的 state handles。
- query/output/action 三类 SelectableArea 构造。
- selection 清理 helper。

不做：

- 历史 exchange 重构。
- 滚动重构。
- hide responses 语义变化。
- secret redaction 大改。

验收：

- 多个 query/output/action 气泡同时存在时，每个气泡都能独立划词。
- 在一个气泡划词不会被其它未命中气泡清掉。
- 新选中另一个气泡时，旧气泡 selection 被正确清掉。
- copy-on-select 仍复制当前可见选中文本。

### Step 2：修浮窗滚动和滚轮穿透

目标：浮窗内部滚动稳定，不影响背后 terminal。

改动范围：

- CLI subagent floating container。
- conversation scroll state。
- wheel event handler / dispatch result。

验收：

- 鼠标在浮窗内滚动，只滚浮窗。
- 到顶部/底部后不穿透滚动 terminal。
- 长代码块、长文本、action block 不产生双层抢滚动。

### Step 3：历史 exchange 渲染

目标：浮窗保留多轮 CLI agent 历史内容。

改动范围：

- CLI subagent view model lifecycle。
- initial history exchange collection。
- appended exchange 去重。
- Ashide BYOP / LRC fallback 与真实 exchange 切换。

注意：这是和 Ashide 本地差异最大的一步，必须先画清楚现有 lifecycle。

验收：

- 新开浮窗显示当前 task 历史多轮内容。
- follow-up 后新增 exchange 只出现一次。
- BYOP/LRC 初始 fallback 不导致 root exchange 混入 subtask history。
- 切换到真实 exchange 后不丢当前输出。

### Step 4：Hide responses + secret redaction 一致化

目标：主 block list 和 CLI 浮窗语义一致，脱敏 index 正确。

改动范围：

- shared render helper。
- query/header render predicate。
- secret redaction index 推进逻辑。
- 单元测试。

验收：

- `Hide responses` 开启时，query/header/output 行为一致。
- 多段历史输出的 redaction 不错位。
- 隐藏内容不污染可见内容的 section index。

---

## 5. 不要做的事

- 不要直接 cherry-pick `4f89c20`。
- 不要把上游 `cli.rs` 大段结构照搬覆盖 Ashide 当前文件。
- 不要为省事在 match 里加 `_` 兜底；本仓库要求穷尽 match。
- 不要把 selection 清理写在每个回调里各自实现一遍。
- 不要让滚轮事件在浮窗边界默认穿透到 terminal。
- 不要在历史渲染里把 fallback root exchange 当成真实 subtask history。
- 不要把 hide responses 的判断散落在多个 view implementation 中。

---

## 6. 测试建议

### 6.1 Selection 测试

建议覆盖：

- query 气泡 A 划词，不被 output/action 气泡清掉。
- output 气泡 B 划词，不被 query/action 气泡清掉。
- action 气泡 C 划词，不被 query/output 气泡清掉。
- 从 A 改选 B，A 被清掉，B 保留。
- 未命中区域收到 mouse up，不清当前 selection。

### 6.2 滚动测试

建议覆盖：

- 浮窗内容超过高度时滚轮只改变 conversation scroll offset。
- 滚到顶部继续向上滚，不影响 terminal scrollback。
- 滚到底部继续向下滚，不影响 terminal scrollback。
- action/code block 存在时仍能正常滚。

### 6.3 历史渲染测试

建议覆盖：

- 初始多个 exchange 按顺序渲染。
- append 已存在 exchange id 不重复。
- append 新 exchange id 后出现一次。
- live-only UI 只显示在最新 exchange。
- fallback root exchange 不加入 subtask history。

### 6.4 Hide responses / redaction 测试

建议覆盖：

- hide responses 后 query/header/output 同步隐藏。
- 未 hide 时 query/header/output 正常显示。
- 多历史 output 的 redaction index 与渲染 section 对齐。
- 隐藏 output 不推进可见 redaction index。

---

## 7. 验收标准

实现完成后，至少满足：

1. `cargo check` 不新增错误。
2. 没有 `.rej` / 临时 patch 残留。
3. CLI agent 浮窗中多气泡划词稳定。
4. 浮窗滚轮不穿透到 terminal。
5. 历史 exchange 不重复、不丢失。
6. Hide responses 在主 block list 和浮窗语义一致。
7. Secret redaction 在多历史输出场景不发生错位。
8. Ashide BYOP / LRC fallback 行为不回退。

---

## 8. 推荐给实现模型的任务提示

可以把下面这段直接发给实现模型：

> 不要 cherry-pick zap `4f89c20`，也不要抄代码。按 `docs/design/cli-agent-block-fix-implementation-brief.md` 的抽象思路，在 Ashide 当前代码上裸实现。先做 Step 1 selection 分区修复，确保 query/output/action 多气泡划词不互相清空；通过后再做 Step 2 滚轮消费；Step 3 历史 exchange 和 Step 4 hide responses/redaction 分开提交。每一步都保持 `cargo check` 不新增错误，不使用 `_` match 通配，不破坏 Ashide BYOP/LRC fallback。

---

## 9. 同批 zap 更新里还应处理/确认的修复点

> 本节不是 `4f89c20` 的一部分，但来自同一轮 zap 最近更新。后续模型在修 CLI agent block 时，最好顺手核对这些点，避免只修浮窗而漏掉相邻体验/稳定性问题。

### 9.1 models.dev 拉取失败后 UI 永久 loading

来源：zap `08e4312`。

现象：

- Providers 设置页第一次拉取 `models.dev` catalog。
- 网络失败或接口异常时，后台只 log warning。
- UI 没有状态更新，也没有 repaint。
- `cached() == None` 一直被解释成 loading。
- 用户看到永久 `Loading models.dev catalog…`，不知道失败，也无法明确重试。

抽象修复思路：

- 给 models.dev 数据源增加“最近一次 fetch 是否失败”的进程级状态。
- 拉取成功时清掉失败状态。
- 拉取失败时设置失败状态并通知 UI 重绘。
- UI 在 `cached() == None` 时区分两种状态：
  - 未失败：显示 loading。
  - 已失败：显示“拉取失败，点击刷新重试”。
- refresh 按钮始终可见，失败后可手动重试。

验收：

- 断网打开 Providers 设置页，不会永久 loading。
- 失败后显示明确失败文案。
- 恢复网络点击 refresh 后能回到正常 catalog。

Ashide 当前状态：已手工吸收；后续如重构 Providers 设置页，需要保留这个状态机。

### 9.2 AI inline suggestions 语言跟随 UI / 系统语言

来源：zap `798d1d1` + `45efa89`。

现象：

- 终端命令结束后的 inline suggestion prompt 原先只要求“匹配用户语言”。
- prompt examples 大多是中文。
- 模型容易无视英文 UI，持续输出中文 follow-up。
- `Language::System` 如果硬编码 fallback 成 English，又会伤害中文/日文系统语言用户。

抽象修复思路：

- 渲染 prompt 前读取当前 UI language setting。
- 如果是显式 English / Simplified Chinese / Japanese，直接传对应 prompt language。
- 如果是 System，读取当前 i18n loader resolved locale：
  - `zh-*` → Simplified Chinese。
  - `ja-*` → Japanese。
  - 其它 → English。
- prompt 模板不要写“match user's language”这种模糊指令；改为“Respond in {language}”。
- examples 尽量使用中性/英文，避免 few-shot 把输出语言偏到中文。

验收：

- UI 语言 English 时 follow-up query 输出英文。
- UI 语言简中时输出简中。
- UI 语言 System 且系统 locale 是中文/日文时，输出对应语言。

Ashide 当前状态：已手工吸收；后续改 prompt 模板时不要退回模糊语言指令。

### 9.3 OpenAI Responses API reasoning 参数误发

来源：zap `eade8b9` 的一部分。

现象：

- App 可能为了捕获 reasoning content，广泛设置 `capture_reasoning_content(true)`。
- OpenAI Responses adapter 如果只因 capture flag 就无条件插入 `reasoning` object，会让非 reasoning 模型也收到 `reasoning` 字段。
- 某些非 reasoning 模型会拒绝该字段，返回 400/502。

抽象修复思路：

- `reasoning` object 只能在调用方明确设置 reasoning effort 时插入。
- `capture_reasoning_content` 只能决定：当已有 reasoning object 时，是否附加 summary/detail capture。
- 不允许 capture flag 单独触发 reasoning object。

验收：

- 非 reasoning 模型 + capture flag 不发送 `reasoning` 字段。
- reasoning 模型 + explicit effort 发送 `reasoning.effort`。
- explicit effort + capture flag 同时存在时，才发送 effort + summary。

Ashide 当前状态：已手工吸收；后续调整 reasoning UI / model variants 时要保留这个 gate。

### 9.4 Linux ForceX11 默认值反了

来源：zap `0e72888`。

现象：

- 注释语义：WSL 默认 force X11，普通 Linux 默认不 force X11。
- 实现如果写成 `!is_wsl()`，实际变成普通 Linux force X11、WSL 不 force。
- 会导致 Wayland/X11 选择和用户平台预期相反。

抽象修复思路：

- 默认值应直接等于 `is_wsl()`。
- 不要同时改设置名、toml path 或迁移；这是纯默认值修复。

验收：

- WSL 下默认 `force_x11 = true`。
- 非 WSL Linux 下默认 `force_x11 = false`。

Ashide 当前状态：已手工吸收。

### 9.5 Dropdown action 重入崩溃

来源：zap `56ee083`。

现象：

- Dropdown 内选择某个 action 时，先同步 dispatch action，再关闭 dropdown。
- 被 dispatch 的 action 可能触发 view update / state mutation，导致 dropdown 仍在更新过程中被重入修改。
- 表现为某些 dropdown 点击后崩溃或 UI 状态错乱。

抽象修复思路：

- 用户选择 action 时，先关闭 dropdown。
- 再 deferred dispatch 用户选择的 action。
- action 类型如果需要 clone，就显式要求可 clone，而不是借用当前 stack 上的引用跨 deferred 边界。

验收：

- Dropdown 选择 action 不产生 reentrant update crash。
- action 仍能正常执行。
- 连续快速点击不会出现 dropdown 残留或重复执行。

Ashide 当前状态：已手工吸收。

### 9.6 BYOP / genai proxy_mode=Off 不能漏走系统代理

来源：zap `349bd85`。

现象：

- App 级网络设置选择 proxy mode Off。
- 普通 http client 尊重 Off，不走系统代理/环境变量代理。
- 但 BYOP chat stream 走 genai 自己的 reqwest client；如果没有显式 no_proxy，reqwest 仍可能读取系统代理或环境变量。
- 结果：用户以为关了代理，AI 请求仍从系统代理泄漏出去。

抽象修复思路：

- genai `WebConfig` 增加“禁用自动代理发现”的能力。
- `ProxyMode::Off` 时设置 no_proxy。
- `ProxyMode::System` 时不设置 explicit proxy，让 reqwest 按系统/环境变量走。
- `ProxyMode::Custom` 时只使用用户配置的 proxy URL / credentials / no_proxy list。

验收：

- Off：不走系统代理、不读 `HTTP_PROXY` / `HTTPS_PROXY`。
- System：沿用系统/环境代理。
- Custom：只走用户配置代理。

Ashide 当前状态：当前代码已经有等价修复；后续若替换 genai client builder，要重新验这个边界。

### 9.7 macOS 27 titlebar / traffic lights / resize hit-test

来源：zap `78567cc`。

现象：

- macOS 新版本下，自定义 titlebar window hit-test 行为变化。
- 左上角 traffic light 按钮可能被内容 view 抢事件。
- 窗口边缘 resize drag 可能被自定义 content mouse handling 吃掉。
- 表现为无法点 close/minimize/zoom，或边缘拖拽 resize 卡住。

抽象修复思路：

- 鼠标左键 down 时先判断是否命中 native traffic light button。
  - 如果命中，事件交给 AppKit/native button。
- 再判断是否命中 resize edge。
  - 如果命中，后续 drag/up 交给 AppKit 处理 resize。
- 只有普通内容区事件才交给自定义 content view。

验收：

- macOS 27+ 上 traffic light 正常。
- 边缘 resize 正常。
- 普通 pane/tab drag 仍正常。
- 不回退旧 macOS 的交互。

Ashide 当前状态：当前 `window.m` 已有同等/更细处理；后续如重构 mac window event routing，要保留 native chrome / resize edge 分流。

### 9.8 Home directory file tree 应可显示，但不能递归 watch 整个 Home

来源：zap `811d580`。

现象：

- 终端 cwd 是 `~` 时，文件树可能尝试把 home directory 注册成 repository root。
- 为避免递归监听整个 home，旧逻辑直接拒绝 home 或 home ancestors。
- 结果：文件树连 home 第一层 children 都显示不出来。

正确语义：

- Home directory 可以作为 lazy-loaded file tree entry 显示。
- 但不要对 home directory 及其 ancestors 注册 recursive watcher。

抽象修复思路：

- “是否允许建立 file tree entry”和“是否允许注册 watcher”分开。
- 对 home / home ancestors：
  - 允许创建 repository/file-tree entry。
  - 禁止 recursive watcher registration。
- remove/unregister 时也要镜像这个 guard：没注册过 watcher 的路径不要 unregister。

验收：

- cwd 为 `~` 时，文件树能显示 home 第一层内容。
- 不会递归 watch 整个 home。
- remove repository 不对未注册 watcher 的 home path 做多余 unregister。

Ashide 当前状态：repo_metadata 结构已和 zap 对应文件不同，不能原样 patch；建议后续单独按当前 `current_app_model` / `repositories` / `watcher` 结构裸实现。

### 9.9 OMP / oh-my-pi CLI agent 识别

来源：zap `3250483`。

现象：

- 用户运行 `omp` CLI agent 时，Ashide 如果不认识它，就不会显示对应 CLI agent affordance / icon / session handling 分支。

抽象修复思路：

- 在 CLI agent enum 中新增 OMP。
- command prefix 为 `omp`。
- 添加 display name、brand color、icon。
- listener/plugin-manager 中把它按“暂无专用 handler/plugin”的普通 detected agent 处理。
- 不要默认把它提升为 Ashide first-class adapter，除非已有 resume/session contract。

验收：

- `omp` 命令能被识别为 OMP agent。
- UI 有图标/颜色。
- 不暴露尚未实现的 first-class resume/promote 能力。
- match 必须穷尽，不用 `_` 兜底。

Ashide 当前状态：已手工吸收，并补了 Ashide 当前额外 match arms。

---

## 10. 暂不建议本轮吸收的 zap 改动

### 10.1 Per-window theme

来源：zap `e8bbe33`。

原因：

- 属于产品功能，不是明确 bugfix。
- 涉及持久化 schema、theme chooser、workspace/window 状态。
- Ashide local-first 方向下可以做，但需要产品入口和信息架构先评审。

建议：另开产品 brief，不和 CLI agent block 修复混在一起。

### 10.2 Fallback font 设置

来源：zap `77bd970`。

原因：

- 属于终端渲染/字体配置功能。
- 影响 settings UI、grid renderer、glyph cache 和测试。
- 有价值，但和 CLI agent block 修复无直接依赖。

建议：如果用户反馈 CJK/emoji fallback 字体问题，再单独迁移。

### 10.3 Website redesign

来源：zap `5c145cc`。

原因：

- 只影响 zap 网站，不是 Ashide app runtime。
- 不应吸收。

---

## 11. 给后续模型的扩展任务提示

如果希望后续模型不只实现 `4f89c20` 思路，而是把这轮 zap 值得吸收的修复都扫完，可以追加下面这段：

> 除了按本文 Step 1-4 裸实现 CLI agent block fix，还要核对第 9 节的相邻修复点。已在 Ashide 当前工作区吸收的点不要重复实现，但重构时不能回退：models.dev 失败态、inline suggestion 语言、OpenAI Responses reasoning gate、Linux ForceX11 默认值、dropdown deferred dispatch、OMP 识别。仍需单独按当前 Ashide 结构实现/确认的是 home directory file tree 可显示但不 recursive watch。proxy Off、macOS titlebar hit-test 当前已有等价实现，重构时保留语义。不要吸收 website redesign；per-window theme 和 fallback font 另开产品评审。
