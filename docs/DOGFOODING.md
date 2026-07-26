# Ashide Dogfooding 排查计划

> 当前批次：`v0.0.1`（macOS / Linux / Windows）。Windows installer 由 CI 构建并发布；本轮暂不以 Windows 真机安装或 WSL 手工运行验证阻断 Release。
>
> 目标：在不上传遥测、不读取无关用户文件的前提下，把每个可复现问题收敛为
> `版本 → 复现步骤 → 时间点 → 本地日志证据 → 根因 / 修复验证` 的闭环。

## 1. 运行前基线

开始使用一个新包前，记录下面四项；每个问题都必须带上它们：

```text
Ashide 版本 / GitHub Release tag：
macOS 或 Linux 版本与架构：
安装包文件名：
首次启动的本地时间：
```

发布包只从对应 GitHub Release 下载，并用该 Release 的 `SHA256SUMS` 校验。不要将
开发构建、旧 DMG 或不同架构的归档与当前 dogfooding 结果混用。

## 2. 本地证据位置与保留策略

### macOS GUI 包

正式 macOS 包的主日志目录是：

```text
~/Library/Logs/
```

本轮 OSS / stable 包使用：

```text
~/Library/Logs/ashide.log        # 当前这次运行
~/Library/Logs/ashide.log.old.0  # 上一次运行（数字越大越旧）
...
~/Library/Logs/ashide.log.old.4
```

程序在下一次启动时轮转日志，因此**遇到问题后先导出，再反复重启**。主日志最多保留当前
运行加 5 份历史运行，不能把它当作长期归档。

### Linux

Linux 主日志落在 Ashide 的 state 目录（遵循 `XDG_STATE_HOME`；未设置时通常为
`~/.local/state/dev.ashide.Ashide/`）。排查时优先使用应用内日志导出，而不是猜测目录。

### 内置导出路径（首选）

不需要手动翻目录：

1. **Help → View Ashide Logs**：生成一个 zip 并在文件管理器中显示；
2. **Settings → About → Export logs**：选择一个明确的输出路径保存 zip。

日志包包含当前与轮转主日志、`manifest.txt`（版本、channel、OS、架构、执行模式、日志
目录）以及存在时的 MCP stderr / 平台辅助日志。它**不主动打包**终端 transcript、工作区
文件、SQLite 状态、minidump 或 profiling 产物。

日志本身仍可能含项目路径、命令片段、Agent / MCP 错误文本或环境名称；分享前先自行检查
zip 内容，移除不应共享的敏感段落。Dogfooding 不启用自动上传或云端遥测。

## 3. 反馈的最小证据包

每条反馈请按下面模板提供；信息不足时优先补时间点与可复现步骤，避免只发截图。

```markdown
### 标题
一句话描述：例如“恢复 Jcode 会话后停在空白终端”。

- Release / 版本：v0.0.1
- 平台：macOS 版本、芯片架构
- 首次出现时间：YYYY-MM-DD HH:MM，时区
- 是否可稳定复现：每次 / 偶发（出现次数与总尝试数）
- 前置条件：环境类型、Agent、是否从旧会话恢复、是否刚切换 SSH target 等
- 复现步骤：最少步骤，按顺序编号
- 期望行为：
- 实际行为：
- 影响：无法继续 / 有替代方案 / 仅视觉问题
- 附件：日志 zip、截图或录屏（如有）
```

出现崩溃、会话 / 配置丢失、错误连到远端环境、或存在数据写入风险时，先停止继续尝试可能
破坏状态的操作，立即保存日志包和精确时间点。

## 4. 本轮优先覆盖的使用路径

按真实日常工作流使用，不要求一次性穷举。每次有行为变化再记录结果。

| 优先级 | 路径 | 要验证的结果 |
|---|---|---|
| P0 | 冷启动、退出、重启、异常退出后重开 | 不崩溃；窗口 / pane / 会话恢复符合预期 |
| P0 | 新建终端、执行命令、长任务完成 | 输入和输出连续；完成 / 失败状态可辨认 |
| P0 | 发现并恢复 Claude、Jcode、Omp 会话 | 仅发现已启用 provider；resume 使用原生会话，不改变 Navigator 顺序 |
| P1 | Session Navigator：切换、pin、重排、关闭、重启 | 选择 / pin / 别名不因 tab / pane 布局变化错绑 |
| P1 | Agent running / idle / blocked 图标 | 状态变化及时且不覆盖真实会话身份 |
| P1 | SSH target catalog：新增 / 修改 config 后刷新、连接与重连 | 列表刷新可见；不会错误使用历史 target 或本机替代远端配置 |
| P1 | Explorer：隐藏文件开关、切换本地 / 远端环境 | 显示结果与当前环境一致，切换不串数据 |
| P2 | Dracula 默认主题、标签栏、新建 Agent 菜单 | 默认视觉可用；菜单随已启用 Agent 收敛，无空隙 / 死入口 |
| P2 | MCP / Agent 报错后查看并导出日志 | 导出成功、日志包可解压、有正确 manifest |

## 5. 每日分诊节奏

### 即时处理

- **P0**：崩溃循环、数据 / 会话丢失、跨环境误操作、无法启动或无法打开任何终端。
  当天建立最小复现，保留原始日志；修复前不清理证据。
- **P1**：核心日常路径受阻但可绕过。下一个开发批次处理；先复现并确认影响范围。
- **P2 / P3**：功能局部退化或视觉问题。按频率与影响合并处理，不打断 P0 / P1。

### 每日一次整理

1. 将新反馈按版本、平台、模块与严重度去重；
2. 以日志时间点为锚，关联操作步骤和当前 `ashide.log`；
3. 先验证当前 Release 是否仍可复现，再在源码构建复现；
4. 每个确认 bug 建立一条 tracker：`症状 → 决定性日志 → 窄执行链 → 修复 → 回归用例`；
5. 修复只能在新候选包中验证，旧包的日志不得被当作修复证据。

## 6. 修复与回归门槛

所有确认问题按以下顺序推进：

```text
最小复现 → 日志 / runtime 证据 → 对应 SPEC 或 UX matrix → 失败测试
→ 实现 → focused test → cargo check → 新包人工复测 → 记录结果
```

涉及会话、持久化、本地 / 远端或异步 lifecycle 的问题，还必须遵循仓库的
`SPEC → UX Matrix → static CHECK → failing TEST → IMPLEMENTATION → verify` 流程；
不能用一次成功的 GUI 操作替代身份、顺序、失败与重启场景的验证。

## 7. 批次结束标准

一个 dogfooding 批次可收口，当且仅当：

- P0 为零，P1 都已修复或明确记录了可接受的短期绕过；
- 每个修复都有代码级回归检查与新包的真实人工复测；
- 新 Release 的 checksum、构建工作流、产物与本地日志导出均已核验；
- 未解决项明确带有影响、复现条件、owner 和下一步，不以“暂未复现”关闭。
