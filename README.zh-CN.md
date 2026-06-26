<div align="center">

<img src="assets/brand/ashide-logo/ashide-logo.png" alt="Ashide" width="128" />

# Ashide

[English](./README.md)

**面向本地与 SSH 环境的 terminal-native CLI agent 工作区。**

</div>

Ashide 面向那些已经把 AI 编码工作放在真实 shell 里的开发者：Codex、Claude
Code、OpenCode、Gemini CLI、Google Antigravity (`agy`)，以及自定义的
shell-based agent。

它不替代这些 agent，不把它们包成聊天机器人，也不把工作搬进云端 IDE。Ashide
保留终端作为原生运行现场，再补上长期终端 agent 缺少的 workspace 层：环境、会话、
项目、文件和恢复。

## 核心判断

Agent 工作不只是一段对话。它是一个活的工作现场：PTY、SSH 连接、工作目录、环境
变量、本地文件、远程文件、机器凭证都会参与其中。

很多 agent 工具试图从终端外部组织这些工作：桌面 GUI、Web 页面，或外挂协议层。
Ashide 反过来做：工作既然发生在终端里，终端就应该理解这个工作现场。

Ashide 把本地与远程终端活动组织成一个可恢复的 workspace：

- 环境保持显式；
- agent 会话可以被发现，而不是靠人记住；
- 恢复会话时带回 cwd、环境和项目上下文；
- 本地与远程状态分离；
- 文件和项目导航不离开终端工作区。

> Agent 工作发生在终端里。现代 agent 工作会跨越多台机器。Ashide 让这些环境和
> 会话变得可恢复、可延续。

## Ashide 提供什么

**Agent-first 会话** —— CLI agent 保持真实终端进程形态，而不是隐藏在聊天后端。
Ashide 发现、索引并组织它们的会话；底层 agent 支持恢复时，Ashide 恢复原会话，
而不是假装所有 agent 都是同一种聊天协议。

**持久化环境** —— 本地与 SSH 环境都是一等 workspace context。切换环境时，终端、
session list、项目根和文件视图一起切换。

**Remote SSH 工作流** —— Ashide 读取已有 OpenSSH config，不要求维护另一套 host
profile。连接后的 SSH host 是一个 workspace context：终端在远端运行，远端 agent
会话在那台机器上发现，远程项目/文件视图读取远程文件。

**Session bridge** —— Agent 历史不应该困在单个工具里。Ashide 正在建设跨 CLI
agent 的转换、编辑、fork/resume 流程，例如 Codex、Claude Code 与 Ashide 之间的
会话迁移。

**轻量 IDE 能力** —— Project explorer、file browser、垂直标签和 session
navigator 都是为了支持长期终端工作，而不是把 Ashide 做成另一个 IDE 控制面。

**Local/offline-first** —— 核心 session、environment、memory 状态默认留在本地。从
上游继承来的云端、账号、计费、同步和 paywall 路径会被移除，或替换成本地优先方案。

## 一次典型流程

1. **启动 Ashide。** 它扫描已安装的 CLI agent，并跨项目、跨工作目录索引会话。
2. **恢复正确的工作现场。** Session navigator 显示发现到的 agent 会话；选择一个，
   Ashide 带回继续工作需要的 cwd、环境和项目上下文。
3. **跨环境移动。** 从本地切到 SSH 环境时，看到的是那台机器上的会话、终端和文件。
4. **跨 agent 移动。** 在支持的路径上，把对话转换或 fork 到另一个 CLI agent，避免
   工作被锁在单个历史存储里。

## 跨 agent 会话转换

每个 CLI agent 都用各自的磁盘格式保存对话历史：Codex 把 JSONL 存在
`~/.codex/sessions`，Claude Code 把 JSONL 存在 `~/.claude/projects`，以此类推。这些
存储彼此不通用。在 Codex 里开始的对话，不能仅仅把 Claude Code 指向 Codex 的文件就打开。

Ashide 的 session bridge 在这些原生格式之间做真正的转换，而不是把 prompt 复制进一个新
聊天。路径是：

1. **读取**源 agent 的原生历史（它在本地或远程磁盘上的真实 session 文件），由各 agent
   专属 reader 完成。
2. **归一化**为一个共享的中间表示（SessionIR）：一组有序消息，带 role、text、timestamp，
   以及 agent 产出的 artifacts（命令、文件编辑、tool 调用等）。
3. 在写入前**编辑 / 裁剪 / 清洗** IR——删掉某些轮次、修正路径、redact，或从选中的轮次切出
   一个聚焦 fork。
4. 把 IR **写回**目标 agent 的原生格式，写入该 agent 真实的 session 存储，使目标 agent 能
  把它当作自己的一个 session 恢复，而不是收到一坨粘贴。

它不是套在 agent 私有 API 上的壳——它读写的是 agent
本来就在磁盘上保存的历史文件。支持按 agent 逐个提供，取决于各 agent 历史格式是否稳定到可
读可恢复；并非每个 agent、每种轮次类型目前都能干净转换。

另有一个便携 bundle 导出/导入，用于在不暴露源机 session 存储的前提下跨机器迁移一段对话。

## 远程运行时交付

远程能力不应该依赖远端主机能访问 GitHub。Ashide 的 release 路径按 local-first
设计：

1. Ashide 先探测 SSH 目标的 OS 和架构。
2. 本地 app 从 GitHub Releases 拉取匹配的远程 helper，例如
   `ashide-<os>-<arch>.tar.gz`，并放进本地缓存。
3. Ashide 通过既有 SSH 连接上传解包后的 helper：优先 `rsync`，不可用时回退到
   `scp` + 原子替换。
4. 远端运行上传后的 helper；远端不需要 GitHub 访问能力，也不需要 GitHub 凭证。

源码/调试构建则在本地编译匹配的 helper，并上传这个精确产物，而不是回退到可能过期
的公开 release。这样客户端和远端协议保持一致，同时仍然是 local-first 交付。

## 平台与发布策略

Ashide 目前主要在 macOS 上开发和验证，但它不是 macOS-only 项目。Warp/zap 的底层
基础是跨平台的；只要产品架构仍然支持，Ashide 会保留这个方向。

现实的发布基线是：

- 至少发布经过验证的 macOS 桌面构建；
- 发布带版本的 remote helper 归档，覆盖 Ashide 能安全探测并上传的远程平台；
- 在有 CI、硬件或维护者能验证时，接受 Linux 和 Windows 桌面支持。

如果某个平台暂时没有官方二进制，通常表示当前维护者没有机器和时间去编译、验证它，
而不是项目决定放弃该平台。欢迎社区维护对应构建和修复。

## 当前状态

Ashide 还很早期、不完整。Remote SSH UX 在演进，agent session 索引/恢复仍是实验性，
去云端仍在进行，UI 打磨和本地化也没完成。请预期 breaking changes。

**这主要是一个自用项目。** 维护者首先为自己的日常 agent 工作而构建 Ashide；开源是它的
副产品，不是一次产品发布。没有发布计划、没有 SLA、也不承诺在任何时间线内交付某个功能或
修复。开发节奏可能长期安静后突然集中推进；release cadence 不是项目健康度的可靠信号。如果
你需要可靠的更新、快速响应或稳定的 roadmap，这个项目大概率会让你失望——fork 它或自己做
二次开发，都是完全合理的回应。

欢迎贡献：PR、bug 报告、文档修复和功能讨论。如果 Ashide 的方向接近但不完全符合你的
需求，也明确欢迎 fork 和二次开发。

## Ashide 不是什么

- 不是云端 IDE。
- 不是聊天机器人 UI。
- 不是托管式 agent 运行时。
- 不是 ACP 式把 agent 从终端里抽走、放进外部协议/控制面的方案。
- 不试图取代你的 CLI agent；它组织的是这些 agent 已经运行其中的环境和会话。

## 文档

- [文档索引](docs/README.md) · [Roadmap](docs/roadmap.md)
- [Remote SSH 模型](docs/REMOTE_SSH.md) · [Agent 会话模型](docs/AGENT_SESSIONS.md)
- [开发指南](docs/DEVELOPMENT.md)

## Roadmap

Terminal-native workspace 是基础。在此之上的方向：

- **跨 agent 共享记忆** —— 项目级、agent 无关的记忆层（`.agents/memory`），让上下文
  跨会话、跨 agent 存活。
- **Codegraph 索引** —— 可重建、revision-aware 的 codegraph，按需给 agent 提供聚焦
  的代码切片，而不是把整个仓库塞进上下文。
- **可复用 agent harness** —— local-first 的 agent runtime，负责 tool execution、
  session state 和 provider routing，终端是第一个客户端。

明确不做：托管式云端 agent 运行时、独立 web/desktop IDE 控制面，或让终端沦为薄视图
的外挂协议接管。

## 与上游的关系

Ashide 建立在两层上游工作之上，并使用了一批第三方库。

- **Warp**（[warpdotdev/warp](https://github.com/warpdotdev/warp)）—— 原始终端代码库。
  Ashide 的终端、编辑器和 UI 基础大部分源自此处。
- **zap**（[zerx-lab/zap](https://github.com/zerx-lab/zap)）—— 基于 Warp 的二次开发。
  Ashide 是在 zap 基础上继续前进的一条独立分支。感谢 zap 及其维护者。

Ashide 不是跟随上游主干维护的 fork。它继承底层基础，同时切掉不符合
local/offline-first 方向的云端和账号依赖路径。

内部 Rust crate 保留上游 `warp*` / `warpui*` 命名，作为对底层基础的致谢；用户可见
表面改名为 Ashide。

**第三方库：** `rust-genai`（本地 fork，位于 `lib/rust-genai`，含 DeepSeek / 自定义
provider 支持），以及通过 `[patch.crates-io]` 锁定的 `core-foundation`、`objc`、
`tink`、`jemalloc` 等。每个库保留自己的上游许可，见 [NOTICE.md](./NOTICE.md) 和
`Cargo.lock`。

## 从源码运行

源码构建是试用未发布工作的最稳妥方式。macOS 是当前维护者唯一实际验证的桌面平台。

```bash
MACOSX_DEPLOYMENT_TARGET=10.14 cargo build --bin ashide
TERM=xterm-256color MACOSX_DEPLOYMENT_TARGET=10.14 ./script/run
```

更多说明见 [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md)。

## 关于名字

Ashide（阿史德）是古突厥部族名。有学者认为 Ashide（阿史德 *’âşitək）与 Ashina
（阿史那 *’âşinâ）都可追溯至古突厥语词根 *aş-（“翻越[山岭]”）。

这个名字贴合项目本身：Ashide 关注跨机器、跨环境、跨 agent、跨工作会话的连续性，同时
保留终端作为工作真正发生的地方。它也回应了 Warp 一词里“穿行、横越、抛过”的意味：
致敬上游血缘，但走一条不同的路。

换句话说，Ashide 是 **agent-first** 的：AI agent 是驱动力，但它的手脚是 **shell**——
命令、文件、进程都落在真实的终端里跑，而不是被一层抽象吞掉。终端本身又是完整的 **IDE**：
编辑、查看、检索、会话管理都在同一块工作区，不需要在 agent 窗口和终端窗口之间来回切换。
agent 想到的事，在终端里发生；终端里发生的事，agent 接得上。

## License

Ashide 保留上游版权和许可证声明。见 [NOTICE.md](NOTICE.md) 和 [LICENSE-AGPL](LICENSE-AGPL)。
除非另有说明，Ashide 新增改动按兼容的相同许可条款分发。
