---
name: ashide-rust-check
description: 为 Ashide/Warp Rust workspace 选择并执行正确验证方式，包括 cargo check、局部测试、nextest、编译失败诊断、build lock 和磁盘不足处理。Use when the user says 跑测试、cargo check、验证一下、编译失败、nextest、workspace test。
user-invocable: true
---

# Ashide Rust Check

## 核心原则

这个 workspace 很大。验证前先按改动范围选择最小有效命令；不要把长时间无输出当成正常，遇到长编译、锁等待或磁盘问题要汇报真实状态。

项目约定：提 PR / 推新 commit 前，至少通过 `cargo check`。

## 命令选择

按优先级选择：

1. 只改单个 crate 或模块：优先局部 `cargo check -p <package>` 或相关单测。
2. 普通实现完成后：
   ```bash
   cargo check
   ```
3. 需要全量测试时：
   ```bash
   cargo nextest run --no-fail-fast --workspace --exclude command-signatures-v2
   ```
4. GUI 行为验证不要只跑 Rust 检查；转用 `ashide-run-verify` 的真实启动流程。

## 预检

- 如报 ENOSPC 或编译异常，先看磁盘空间。
- 如怀疑有活跃构建，先区分 `cargo`/`rustc` 是否还在跑。
- 不要在活跃构建期间粗暴删除 `target/`。
- macOS 环境不要默认有 GNU `timeout`；优先用命令自身超时参数或后台任务监控。

## 失败诊断

回报时按类别归因：

- Rust 类型/借用/feature 错误
- 缺失依赖或 workspace 配置错误
- build script / native dependency 错误
- LFS/smudge 或文件缺失问题
- build lock / 并发 cargo 问题
- 磁盘不足

不要把环境问题包装成代码已验证失败。

## 汇报格式

```markdown
验证命令：...
结果：通过 / 失败 / 未完成
失败类别：...
关键错误：...
下一步：...
```

如果跳过了全量测试，要说明原因和已执行的替代验证。
