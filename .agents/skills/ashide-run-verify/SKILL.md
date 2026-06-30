---
name: ashide-run-verify
description: 启动并真实验证 Ashide/Warp GUI、macOS app bundle、UI 行为和截图。Use when the user says 跑一下 Ashide、启动 GUI、真的构建好了、截图验证、看改动是否生效、右键/标签/入口验证。
user-invocable: true
---

# Ashide Run Verify

## 核心原则

不要把“编译通过”等同于“功能已验证”。只有真实启动 app，并观察到用户点名的行为后，才能说已验证。

Ashide GUI 默认启动命令：

```bash
TERM=xterm-256color ./script/run
```

不要裸跑 `./script/run`，否则可能遇到终端颜色/渲染问题。

## Workflow

1. 确认当前在 `/Users/admin/ashide`，并快速看工作区状态。
2. 如果用户问“真的构建好了”或“改动是否生效”，先找最近构建/启动日志，再决定是否重跑。
3. 启动 GUI 时使用：
   ```bash
   TERM=xterm-256color ./script/run
   ```
4. 如果是 UI 行为验证，必须观察真实 app：窗口、截图、日志或可交互行为至少一种。
5. 如启动失败，回报失败阶段：依赖、编译、app 启动、运行时 panic、还是 UI 行为不符合。
6. 汇报时明确区分：
   - `cargo check` / 编译是否通过
   - app 是否成功启动
   - 指定行为是否真实观察到
   - 哪些验证因权限、环境或时间未执行

## 常见验证点

- 标签页是否重复激活。
- 右键菜单是否可用。
- 入口是否出现在用户自然寻找的位置。
- macOS app bundle 是否真的更新，而不是只编译了中间产物。

## 报告格式

```markdown
验证结果：通过 / 未通过 / 部分通过

- 编译：...
- 启动：...
- 观察到的行为：...
- 未验证项：...
- 下一步建议：...
```
