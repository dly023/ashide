<div align="center">

<img src="assets/brand/ashide-logo/ashide-logo.png" alt="Ashide" width="128" />

# Ashide

[English](./README.md)

**面向本地与 SSH 环境的 terminal-native CLI agent 工作区。**

</div>

Ashide 面向那些 AI 编码已经跑在真实 shell 里的开发者——Codex、Claude Code、
OpenCode、Gemini CLI、Google Antigravity (`agy`)，以及自定义的 shell agent。

它不替代这些 agent，不把它们包成聊天机器人，也不把工作搬进云端 IDE。终端依然是
运行现场，Ashide 只是补上长期终端 agent 缺少的那一层：环境、会话、项目、文件，
以及恢复。

## 为什么是 terminal-native

Agent 工作不只是一段对话，而是一个活的工作现场——PTY、SSH 连接、工作目录、环境
变量、本地与远程文件、机器凭证，都在其中。

多数工具想从终端外部管理这个现场：桌面 GUI、Web 页面、外挂协议层。Ashide 反过来
做——工作既然跑在终端里，终端就该理解周围的现场，并把它变成可恢复的东西：

- 环境保持显式；
- agent 会话靠发现，而不是靠人记；
- 恢复会话时带回它的 cwd、环境与项目上下文；
- 本地与远程状态彼此分离；
- 浏览文件和项目不必离开工作区。

## 功能

- **Agent-first 会话** —— CLI agent 始终是真实的终端进程。Ashide 发现、索引、组织
  它们的会话，并在底层 agent 支持时恢复原会话。
- **持久化环境** —— 本地与 SSH 环境都是一等上下文。切换环境，对应的终端、会话列表、
  项目根和文件视图会一起切过去。
- **Remote SSH 工作流** —— Ashide 直接读你已有的 OpenSSH 配置，不需要再维护一套主机
  档案。连上的主机就像一个工作区：终端在远端跑，远端会话在那台机器上被发现，远程
  文件视图读的是远程文件。
- **Session bridge** —— Agent 历史不该锁死在单个工具里。Ashide 在 CLI agent 之间
  （Codex、Claude Code、Ashide 自身）转换、编辑、fork 会话。
- **轻量 IDE** —— 项目浏览、文件浏览、垂直标签、会话导航，都是为支撑长期终端工作
  服务。
- **本地优先** —— 会话、环境、记忆状态默认留在本地。从上游继承来的云端、账号、计费、
  同步路径正在被移除。

## 一次典型流程

1. **启动。** Ashide 扫描已安装的 CLI agent，跨项目、跨目录索引它们的会话。
2. **恢复。** 会话导航列出发现到的会话；选一个，Ashide 带回它的 cwd、环境和项目
   上下文。
3. **跨环境。** 从本地切到某台 SSH 主机，看到的就是那台机器上的会话、终端和文件。
4. **跨 agent。** 在支持的路径上，把对话转换或 fork 到另一个 CLI agent，让工作不被
   锁在单个历史存储里。

## 跨 agent 会话转换

每个 CLI agent 都用各自的磁盘格式存历史——Codex 把 JSONL 放在 `~/.codex/sessions`，
Claude Code 放在 `~/.claude/projects`，等等。这些存储互不通用：在 Codex 里开始的
对话，光把 Claude Code 指向 Codex 的文件是打不开的。

Ashide 的 session bridge 在这些原生格式之间做真正的转换，而不是把 prompt 复制进一个
新聊天：

1. **读取**源 agent 的原生历史，由各 agent 专属的 reader 完成。
2. **归一化**成一个共享表示（SessionIR）：一组有序消息，带 role、文本、时间戳，以及
   agent 产出的产物（命令、文件编辑、tool 调用）。
3. **编辑** IR——裁剪轮次、修正路径、redact，或从选中的轮次切出一个聚焦的 fork。
4. **写回**目标 agent 的原生格式，写进它真实的会话存储，让它当作自己的会话来恢复。

它读写的是 agent 本就保存在磁盘上的历史文件，不是套在私有 API 外面的壳。支持按
agent 逐个提供，取决于各自格式是否稳定到可读可恢复——目前并非每个 agent、每种轮次
都能干净转换。此外还有便携 bundle 导出/导入，可在不暴露源机会话存储的前提下跨机器
迁移一段对话。

## 远程运行时交付

远程能力不该要求远端主机能访问 GitHub，所以 release 路径是本地优先的：

1. Ashide 先探测 SSH 目标的 OS 和架构。
2. 本地 app 从 GitHub Releases 拉取匹配的 helper（`ashide-<os>-<arch>.tar.gz`）到
   本地缓存。
3. 通过已有的 SSH 连接上传解包后的 helper——优先 `rsync`，不可用时回退到 `scp`
   加原子替换。
4. 远端运行上传上去的 helper，全程不需要访问 GitHub。

源码/调试构建则在本地编译匹配的 helper 并上传这个精确产物，让客户端和远端协议始终
一致。

## Ashide 不是什么

- 不是云端 IDE、聊天机器人 UI，也不是托管式 agent 运行时。
- 不是 ACP 式把 agent 从终端抽走、塞进外部协议或控制面的方案。
- 不是要取代你的 CLI agent——它组织的是这些 agent 已经用着的环境和会话。

## 状态与预期

Ashide 还很早、不完整。Remote SSH 体验在演进，会话索引/恢复仍是实验性，去云端还在
进行，UI 打磨和本地化都没做完。请预期会有 breaking changes。

**这主要是个自用项目。** 维护者首先是为自己的日常 agent 工作而做 Ashide；开源只是
副产品，不是一次产品发布。没有发布计划、没有 SLA，也不承诺在任何时间点交付某个功能。
开发可能长期安静，然后集中爆发。如果你需要可靠的更新、快速响应或稳定的 roadmap，
这个项目大概率会让你失望——直接 fork 是完全合理的选择。

欢迎贡献：PR、bug 报告、文档修复和讨论。如果 Ashide 的方向接近但不完全是你要的，
也明确欢迎 fork。

macOS 是维护者目前唯一会验证的桌面平台。Warp/zap 的底层是跨平台的，Ashide 保留这个
方向；某个平台暂时没有官方二进制，通常只是还没人去编译验证，而不是被放弃。每个 release
都会带一个经过验证的 macOS 构建，外加针对可安全支持平台的带版本 remote helper 归档。

## Roadmap

terminal-native 工作区是基础。在它之上：

- **跨 agent 共享记忆** —— 项目级、与 agent 无关的记忆层（`.agents/memory`），让
  上下文跨会话、跨 agent 存活。
- **Codegraph 索引** —— 可重建、感知版本的 codegraph，按需给 agent 提供聚焦的代码
  切片，而不是把整个仓库塞进上下文。
- **可复用 agent harness** —— 本地优先的运行时，负责 tool 执行、会话状态和 provider
  路由，终端是它的第一个客户端。

不做的：托管式云端运行时、独立的 web/desktop IDE 控制面，或让终端沦为薄视图的外挂
协议接管。

## 从源码运行

源码构建是试用未发布功能最稳妥的方式。macOS 是当前唯一验证过的桌面平台。

```bash
MACOSX_DEPLOYMENT_TARGET=10.14 cargo build --bin ashide
TERM=xterm-256color MACOSX_DEPLOYMENT_TARGET=10.14 ./script/run
```

更多说明见 [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md)。

## 文档

- [文档索引](docs/README.md) · [Roadmap](docs/roadmap.md)
- [Remote SSH 模型](docs/REMOTE_SSH.md) · [Agent 会话模型](docs/AGENT_SESSIONS.md)
- [开发指南](docs/DEVELOPMENT.md)

## 与上游的关系

Ashide 建立在两层上游工作之上：

- **Warp**（[warpdotdev/warp](https://github.com/warpdotdev/warp)）—— 原始终端代码库，
  Ashide 的终端、编辑器和 UI 基础大部分来自这里。
- **zap**（[zerx-lab/zap](https://github.com/zerx-lab/zap)）—— 在 Warp 之上的二次开发。
  Ashide 是在 zap 之上继续走的一条独立分支。感谢 zap 及其维护者。

Ashide 不是跟随上游主干的 fork。它继承底层基础，同时切掉不符合本地优先方向的云端和
账号依赖路径，对诸多底层行为进行了大幅修改。内部 crate 保留 `warp*` / `warpui*` 命名，作为对这层基础的致谢；只有用户可见的表面改名为 Ashide。

第三方库（比如本地 fork 的 `rust-genai`，带 DeepSeek/自定义 provider 支持，以及通过
`[patch.crates-io]` 锁定的若干 crate）各自保留上游许可，见 [NOTICE.md](./NOTICE.md)
和 `Cargo.lock`。

## 关于名字

Ashide（阿史德）是古突厥部族名。有学者认为 Ashide（*’âşitək）与 Ashina（阿史那
*’âşinâ）都可追溯到古突厥语词根 *aş-（“翻越[山岭]”）——这正合项目本身：在机器、环境、
agent 和会话之间穿行，同时把终端留作工作真正发生的地方。它也呼应 Warp 一词里穿线、
横越的意味，却走一条不同的路。

说白了，Ashide 是 **agent-first** 的：agent 是驱动力，但它的手脚是 **shell**——命令、
文件、进程都在真实终端里跑，而不是藏在一层抽象后面。而终端就是 **IDE**：编辑、查看、
检索、会话管理都在同一块工作区，不必在 agent 窗口和终端之间来回切。

## License

Ashide 保留上游版权和许可证声明。见 [NOTICE.md](NOTICE.md) 和 [LICENSE-AGPL](LICENSE-AGPL)。
除非另有说明，Ashide 新增改动按兼容的相同许可条款分发。
