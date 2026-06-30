---
name: ashide-product-entry-review
description: 评审 Ashide/Warp 功能的产品入口、信息架构、UI/UX 和实现风险。Use when reviewing feature briefs, PRs, diffs, SSH/Environment/Agent entrypoints, or when the user says UI/UX 错误、自检检查不出来、入口合理吗、review 这个改动。
user-invocable: true
---

# Ashide Product Entry Review

## 核心原则

评审顺序固定：**先挑战产品入口，再验实现**。不要只检查代码是否符合 brief。

## Review Order

1. 产品入口 / 信息架构
   - 用户会在哪里自然寻找这个功能？
   - 入口是否贴近主概念？
   - 有没有把管理后台式页面当成创建/打开主流程？
   - 是否可以复用已有配置，而不是引入新的管理层？
2. 交互与行为边界
   - 空状态、失败态、重复点击、取消、撤销是否合理？
   - 是否有不可逆操作或外部副作用？
3. 持久化与安全副作用
   - 设置、token、路径、远端主机、文件写入是否有边界？
   - 是否需要 dry-run 或确认？
4. 代码 correctness
   - 状态流是否一致？
   - 是否有死分支、重复入口、过宽抽象或无关改动？

## Ashide 特别判断

Environment / SSH 相关默认判断：

- 主入口应靠近 Environment Strip、水平标签栏、`+`、quick picker 等自然入口。
- SSH 来源优先读取 `~/.ssh/config`。
- SSH Manager 若存在，更适合放在全局设置或高级管理里，不应挡在主流程前。

## 输出格式

```markdown
## 产品入口结论
- 结论：通过 / 不通过 / 有条件通过
- 主要问题：...

## 必改项
1. ...

## 建议项
1. ...

## 实现风险
- ...

## 验证建议
- ...
```

只提出能对应到用户目标的修改；不要顺手扩散到无关重构。
