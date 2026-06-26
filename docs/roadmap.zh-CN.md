# Roadmap

Ashide 围绕一个假设构建：**agent 工作发生在终端里**，现代 agent 工作会跨越多个环境。
下面这份 roadmap 反映我们真正要做的事——context engineering、可复用的 agent
harness、跨 agent 共享记忆、codegraph 索引。它**不是**一个做独立 GUI 控制面、
托管式云端运行时或 Kubernetes 控制面的计划。

## Phase 1 — Terminal-native workspace（进行中）

已经存在并正在加固的基础：

- 持久化本地与 SSH 远程环境作为一等 workspace context。
- Agent 会话发现：检测、索引、组织每个环境内的 Codex / Claude Code / OpenCode / Gemini CLI / `agy` / 自定义 shell agent 会话。
- 工作现场恢复：恢复一个 agent 会话时自动还原其 cwd、环境和项目上下文。
- 本地/远程状态分离；默认 local-first、offline-first。
- 内建于终端的轻量项目与文件导航。
- Session bridge：跨 CLI agent 转换、编辑、fork 会话（Codex ↔ Claude ↔ Ashide），让工作不被困在单个 agent 的历史里。

## Phase 2 — 跨 agent 共享记忆

Agent 上下文不应该随单个会话消亡，也不该被锁在某个 agent 的私有存储里。目标是做一个共享的、可重建的记忆层，项目里每个 CLI agent 都能读写。

- `.agents/memory` 作为项目级、agent 无关的记忆存储：证据、决策、开放问题、恢复线索，跨会话、跨 agent 存活。
- `.agents/evidence` 采集：记录 agent 实际做了什么（命令、diff、结果），让下一个 agent——或同一 agent 的下一个会话——从 ground truth 出发，而不是重新摸索。
- 统一记忆词汇表，让 Codex、Claude Code、自定义 agent 都能贡献，不必各自发明 schema。
- 记忆作用域：随仓库走的项目级记忆，加上机器本地的凭证/环境特定记忆。

## Phase 3 — Codegraph 索引

一个可重建、revision-aware 的 codegraph，按需给 agent 提供*聚焦*的代码上下文——"codegraph slice"——而不是把整个仓库塞进上下文窗口。

- 混合 parser 策略：Rust 精确解析，其余语言用 tree-sitter fallback（复用现有 editor 解析栈，不引入第二个 tree-sitter）。
- 增量索引，冷启动/部分构建时优雅降级，不阻塞 agent。
- Agent 接口：command-first 的 `codegraph slice`、go-to-def、find-callers、MCP tool、可选 `--json`。默认低认知负担。
- Editor 集成复用现有 editor pane——不外挂陌生面板。

> 实现（CG-04..）在内部设计文档 review 通过后才开始。该设计文档不放在本仓库。

## Phase 4 — 可复用 agent harness

把 agent loop、tool runtime、session state、prompt templating、provider routing 抽成一个独立的、local-first 的运行时，终端作为第一个客户端驱动它。harness 是**本地**服务，不是托管的：它跑在开发者自己的机器上（或通过 SSH 跑在自己的远程机器上），凭证和历史留在磁盘，完全可自托管，无 SaaS 依赖。

- 终端表面与 harness 之间的稳定 IPC / JSON-RPC 协议。
- 可插拔 tool registry：内建 shell / read / edit / search tool，加上用户提供的 tool，统一 RPC 接口。
- 版本化协议 + 能力协商。
- harness 有意保持 local-first；托管/多租户运行时**不在本项目范围内**。

## 不在 roadmap 上

为了让方向明确：

- **不做独立 IDE 控制面。** Ashide 是终端工作区，不是包着 web IDE 的
  Electron/Tauri 壳。目标不是把终端工作搬进另一个桌面或 web 控制面。
- **不做托管式云端 agent 运行时。** 无多租户 SaaS、无托管 sandbox、无强制云依赖。远程工作通过 SSH 到开发者自己的机器。
- **不做 ACP 式外挂协议接管。** 终端是原生运行现场，我们不把 agent 从中抽象进独立的 GUI/协议层。

> Roadmap 条目是探索性的，会随真实使用反馈调整。

---

[English](./roadmap.md)
