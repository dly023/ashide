---
name: ashide-shared-memory
description: 处理 Ashide 共享记忆、.agents/memory、agent memory runtime、记忆开关、所有 agent 共享记忆、AGENTS.md 是否承载动态记忆等设计和实现。Use when the user mentions 共享记忆、memory 开关、.agents/memory、agent memory runtime。
user-invocable: true
---

# Ashide Shared Memory

## 当前设计口径

Ashide 共享记忆主线默认是：

- `.agents/memory/` 作为项目内动态记忆 source of truth。
- runtime 使用 event-sourced 思路维护状态。
- `AGENTS.md` / `CLAUDE.md` / `GEMINI.md` 只做 stable pointer，不承载动态 memory 内容。

如果代码或文档显示该口径已变化，以当前仓库为准，并在汇报中指出变化。

## Workflow

1. 先读取项目导航：`AGENTS.md`。
2. 再查设计文档和实现入口：
   - `.agents/memory/`
   - `app/src/ai/`
   - `crates/ai/`
   - persistence / migrations 相关路径
3. 明确本次是设计、实现、评审还是修 bug。
4. 涉及写入策略时，先确认：谁能写、何时写、是否 ask-before-write、是否需要审计记录。
5. 涉及多 agent 并发时，检查冲突处理和事件顺序。

## 检查清单

- 是否避免把动态记忆写进 `AGENTS.md`？
- 是否区分用户记忆、项目记忆、运行时短期状态？
- 是否有 secret/cookie/token 过滤边界？
- 是否能解释 memory source of truth 和缓存/索引的关系？
- 是否考虑 FTS/search、graph-lite、WAL/并发读写等运行时需求？
- 是否提供可恢复、可审计的事件记录，而不是只保存最终状态？

## 输出要求

设计类输出要给：

```markdown
## 当前口径
## 数据模型
## 写入流程
## 读取/检索流程
## 并发与安全边界
## 最小实现切片
## 验证方式
```

实现类输出要明确改动文件、验证命令和未覆盖风险。
