# 远程 file browser 上下文菜单结构修复

## 实施状态：✅ 已落地

- "终端"单元素子菜单 → 拍平为一级 `cd-to-terminal` 菜单项。
- "其他"子菜单（rename / copy-filename）→ 拍平为两个一级菜单项。
- "上传 / 新建"多项子菜单保留（多项聚合合理）。
- 子菜单标题文案 key `server-file-browser-menu-terminal` / `-other` 已确认无运行时引用，并从 i18n 源清理。

## 问题

远程 file browser（`app/src/workspace/view/server_file_browser.rs`）右键菜单里，"cd 到该目录"被包在一个**只有一个项的二级子菜单**里，体验很怪。

### 现状代码

`server_file_browser.rs:1367-1376`：

```rust
context_menu_submenu(
    crate::t!("server-file-browser-menu-terminal"),   // 一级："终端"（子菜单标题）
    Icon::Terminal,
    vec![
        MenuItemFields::new(crate::t!("server-file-browser-menu-cd-to-terminal"))
            .with_icon(Icon::Terminal)
            .with_on_select_action(ServerFileBrowserAction::CdToTerminal(cd_target))
            .into_item(),
    ],
),
```

"终端"是一级子菜单标题，点开只有一个子项"cd 到终端"。等价于点两下才能到一个本该一键完成的动作。

### 对比本地 file tree

本地 file tree（`app/src/code/file_tree/view.rs:2304`）里，"cd 到该目录"是**一级菜单项**，扁平，一键直达：

```rust
MenuItemFields::new(crate::t!("menu-filetree-cd-to-directory"))
    .with_on_select_action(FileTreeAction::CDToDirectory { id: id.clone() })
    .into_item(),
```

本地 / 远程菜单结构不一致——本地扁平、远程多了不必要的子菜单层。

### 同类的"其他"子菜单

`server_file_browser.rs:1377-1392` 的"其他"子菜单里有 Rename + Copy filename 两项。这个子菜单有 2 项，勉强算合理（虽然也可以扁平化），但"终端"子菜单只有 1 项，是明确的 UI 反模式。

---

## 修复计划

### Step 1 — "终端"子菜单拍平为一级菜单项

**改 `server_file_browser.rs:1367-1376`**：把 `context_menu_submenu(...)` 替换成直接的一级 `MenuItemFields`：

```rust
MenuItemFields::new(crate::t!("server-file-browser-menu-cd-to-terminal"))
    .with_icon(Icon::Terminal)
    .with_on_select_action(ServerFileBrowserAction::CdToTerminal(cd_target))
    .into_item(),
```

删掉外层 `context_menu_submenu` 包装。action / 事件链路不变（`CdToTerminal` → `ServerFileBrowserEvent::CdToDirectory` → `cd_to_environment_directory`）。

**i18n**：`server-file-browser-menu-terminal`（子菜单标题"终端"）不再使用，已在 orphan 清理中删除。一级项继续用 `server-file-browser-menu-cd-to-terminal`。如果觉得"cd 到终端"措辞不如本地"cd 到该目录"清晰，可统一文案——见 Step 3。

### Step 2 — 评估"其他"子菜单是否也拍平

`server_file_browser.rs:1377-1392`"其他"子菜单含 Rename + Copy filename 两项。

**建议**：也拍平成两个一级项，和本地 file tree 菜单结构对齐（本地 Rename 是一级项，`file_tree/view.rs:2337`）。远程菜单项不多（拍平后约 9 项一级），不需要分组。

```rust
MenuItemFields::new(crate::t!("server-file-browser-menu-rename"))
    .with_icon(Icon::Rename)
    .with_on_select_action(ServerFileBrowserAction::RenameEntry(index))
    .into_item(),
MenuItemFields::new(crate::t!("server-file-browser-menu-copy-filename"))
    .with_icon(Icon::Copy)
    .with_on_select_action(ServerFileBrowserAction::CopyName(target.name.clone()))
    .into_item(),
```

删掉"其他"子菜单包装。

### Step 3 — 文案与本地统一（可选）

远程用 `server-file-browser-menu-cd-to-terminal`（"cd 到终端"），本地用 `menu-filetree-cd-to-directory`（"cd 到该目录"）。措辞不一致。

**建议**：统一成"cd 到该目录"（更准确——动作是 cd 到这个目录，不是"到终端"）。改 i18n 文案即可，不动 key 结构（或同步两边的 key）。

### Step 4 — 保留的合理子菜单

`server_file_browser.rs:1323-1356` 的两个子菜单**保留**：
- "上传"含 Upload File + Upload Folder（2 项，分组合理）
- "新建"含 New File + New Folder（2 项，分组合理）

这两个是真正的分组子菜单，每项多于 1，符合子菜单用法。不动。

---

## 验收

| 步骤 | 验证 |
|---|---|
| Step 1 | 远程 file browser 右键目录，"cd 到终端"是一级项，一键直达；不再有只有一个项的子菜单 |
| Step 2 | 远程菜单结构与本地 file tree 对齐，Rename / Copy filename 是一级项 |
| Step 3 | 本地 / 远程 cd 菜单文案一致 |
| 回归 | CdToTerminal action 链路不变，cd 行为正常；upload / new 子菜单不受影响 |

## 风险

- Step 1/2 纯菜单结构改动，action 不变，风险极低。
- Step 3 涉及 i18n；`server-file-browser-menu-terminal` / `server-file-browser-menu-other` 已确认无运行时引用并清理。

## 与 local-remote 一致性计划的关系

这步属于 `local-remote-fix-plan.md` 阶段 1 的延伸——远程 file browser 菜单结构和本地 file tree 对齐，是"本地 / 远程行为一致"在 UI 层的体现。可与阶段 1.1（`cd_to_directory` 远程静默无效）一起做：1.1 修 cd 的**行为**，这里修 cd 的**菜单入口**，两者合在一个 PR 里自然。
