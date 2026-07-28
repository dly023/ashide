---
name: ashide-architecture-change
description: 治理 Ashide 跨模块架构改动与反复回归。Use when fixing root causes instead of patches, local/remote divergence, async lifecycle or identity/persistence bugs, repeated regressions, or when maintaining SPEC/check/test/matrix/tracker and scanning similar abstraction leaks.
user-invocable: true
---

# Ashide Architecture Change

## 适用边界

本 skill 只提供可复用的架构变更工作流，不保存某个功能模块的当前设计。功能事实必须来自仓库代码、SPEC 和 tracker。

## Preflight

1. 读 `AGENTS.md` 的架构边界与任务路由。
2. 按路由加载该领域的 SPEC、tracker、matrix 和 static check；禁止依赖 skill 内的历史结论。
3. 从 runtime/当前代码证明一条窄链路：

```text
input/action → owner model → lifecycle transition → projection/persistence → visible effect
```

4. 对每个异步 lifecycle state 写出载体表：`canonical owner → identity → visible carrier → carrier missing semantics → success/failure/cancel cleanup`。任何一栏为空都不得开始实现。
5. 做负空间审查：除正常输入外，必须检查 source 暂失、永久删除、0/1/many 无关实体、结果乱序、失败和取消；禁止只推理完成态。
6. 写外部原生格式时，为每个输出字段标注 provenance：`protocol constant / source-derived / target-config-derived / generated identity`。无法归类或 owner 不明确的字段禁止写出；target-config-derived 禁止默认值、fallback 和本机代填远端。
7. 写出现有抽象复用表：owner、identity、intent、state machine、projection、persistence、verification。
8. 明确 bug 是哪个既有阶段漏网，以及修复后删除哪条平行/后补路径。
9. **若本次重写/旁路既有路径**：列出旧路径已执行的 ownership/static gates（identity、title、binding、collection、lifecycle），在同 PR 为新路径加上等价 fail-closed 检查 + 至少一条负向探针；否则不得标 verified。只锁 latency/snapshot 反模式而放开同域 ownership，视为体系漏网（见 `HARNESS-PARALLEL-PATH-OWNERSHIP-INHERITANCE-89` / LR-193）。

## 固定协议

```text
SPEC → UX Matrix → static CHECK → failing TEST → IMPLEMENTATION
→ focused verify → cargo check → GUI/runtime verify → tracker verified
```

- 扫描发现同类问题立即追加 tracker；实现一项清零一项。
- tracker 必须记录 `reused_abstractions` 和 `removed_parallel_paths`。
- local/remote 只能在最低 backend/delivery 边界分叉；高层 action、identity、projection、reducer 和 user state 共用一套语义。
- 异步流程必须逐阶段测试，不能只断言最终状态。
- lifecycle state 与 canonical projection 必须由同一 owner 原子持有；看到 `clear()`、`Vec::new()`、`state = incoming`、`items = new_items`、`retain()` 时，必须证明“本轮未观察到”不会被误解为删除。
- 单实体 fixture 不能证明集合稳定性；涉及列表/树/registry 的异步变更至少包含目标实体和两个无关实体，并逐阶段检查 cardinality、identity 与 order。
- 调用方审计必须扫描整个生产源码树，并从真正产生副作用的 primitive 反向枚举 wrapper 与高层入口；只检查当前文件或只锁调用次数不算闭环。static check 应提供只读 `path:line` inventory，同时锁定 enclosing function owner 集合，防止同一文件内以非法调用替换合法调用后靠相同数量蒙混过关；并用负向探针证明新文件/新包装调用会 fail closed。
- 热路径/平行路径重写必须继承旧路径 ownership gates，并用负向探针证明重新引入 forbidden pattern 会失败；性能 LR 不得在缺少 co-required ownership gate 时标 verified。

## 先复用、后扩展

提出新类型或 registry 前必须回答：

1. 现有 owner 类型为何不能承载？
2. 新类型是否会与既有 identity/state/persistence 双写？
3. 能否上移或补全既有抽象，而不是旁挂 adapter/overlay？
4. 编译器或静态检查如何阻止下一次漏实现？

无法给出明确证据时，不得新建平行抽象。

## 禁止方案

- 在 UI/render/snapshot 层按 backend 类型修数据。
- debounce、sleep、retry、延迟刷新、排序冻结或动画遮挡状态机错误。
- fallback 到 locator、EntityId、tab index、命令字符串等易失身份。
- 只新增兼容/后补路径而不删除旧路径。
- 用本地测试通过替代远程或逐阶段语义验证。
- lifecycle/state 被保存但其 visible carrier/canonical projection 由调用方临时重建。
- 用 source-missing 帧直接替换 canonical collection，或在 UI 层缓存/冻结集合掩盖该错误。
- 根据观察到的第三方输出样本自行补齐 provider/model/profile/权限等 target-owned 字段；writer/reader 自闭环不能证明外部消费者兼容。

## 进度纪律

- 开始时先用一句话说明正在读取/修改哪个层级。
- 长编译、锁等待或 GUI 验证前后主动通报真实状态。
- 不把思考时间描述成已完成工作；汇报必须包含已读文件、已改文件或已运行命令。
