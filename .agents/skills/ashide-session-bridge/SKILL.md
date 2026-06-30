---
name: ashide-session-bridge
description: 继续 Ashide SessionBridge、会话转换、会话编辑、agent restore、session persistence、Codex/Claude 会话导入导出相关开发。Use when the user mentions 会话转换、SessionBridge、agent restore、session persistence、会话编辑、Codex/Claude 会话导入。
user-invocable: true
---

# Ashide SessionBridge

## 核心原则

先定位当前 feature slice，再改代码。用户如果限定了范围，例如“只做会话转换相关”“不用管旁支”，必须严格收敛，不扩散到其他仓库或无关 agent 话题。

长时间编译、排锁或跑验证时，主动给短进度更新。

## Discovery

优先检查这些位置；不存在时再用 `rg` 查找：

- `app/src/session_bridge/`
- `app/src/ai/`
- `crates/ai/`
- `docs/AGENT_SESSIONS.md`
- `docs/REMOTE_SSH.md`
- 最近相关提交：`git log --oneline -- app/src/session_bridge app/src/ai crates/ai`

## Slice 分类

开始前先判断这次属于哪一类：

1. IR / normalize / sanitize / reader
2. Codex / Claude transcript import
3. CLI parser / export command
4. session bundle / persistence
5. 会话编辑 UI
6. agent restore / metadata
7. 测试和 fixture

## 实现检查清单

- 数据结构是否能表示来源、时间、role、tool call、tool result、附件/图片、错误事件？
- 是否明确 sanitize 边界，避免保存 secrets/cookies/token 原文？
- 是否有 fixture 覆盖至少一个真实历史格式的最小样本？
- 是否没有为单点使用引入过度抽象？
- 是否遵守硬切标准：无临时兼容层、无 no-op、无旧路径残留，除非用户明确要求迁移期。
- 如涉及持久化，是否走 migration，而不是手改生成 schema？

## 验证

- 小改动优先局部 `cargo check` / 单元测试。
- 提交前按项目约定至少跑 `cargo check`。
- 如果 workspace build lock 或磁盘不足，先报告真实状态，不要沉默等待。

## 汇报格式

```markdown
当前 slice：...
改动范围：...
已验证：...
未验证/阻塞：...
下一步：...
```
