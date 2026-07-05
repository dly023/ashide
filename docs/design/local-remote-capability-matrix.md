# 本地 / 远程能力矩阵

`local-remote-capability-matrix.csv` 是 `local-remote-inconsistencies.csv` 的系统化扩展：把 `local-remote-inconsistencies.md` 排查时用的"逐个 grep 同源模式"升级成**全量能力审计**——列本地所有用户可见能力，逐项核对远程是否有等价实现 + 等价参数 + 等价时机。

## 列说明

- `capability`：用户能力名。
- `entry_point`：入口函数 `file:line`。
- `local_path` / `remote_path`：本地 / 远程实际执行路径。
- `param_parity`：参数是否对齐（yes / no / na）。
- `timing_parity`：时机是否对齐（yes / deferred / n/a）。`deferred` 表示远程经 entry plan 异步 materialize，本地同步——这是合理的执行模型差异，不算 leak。
- `status`：fixed / fixed_pending_validation / healthy / found / closed / pending_audit。
- `notes`：关联 inconsistencies CSV 编号 + 说明。

## 状态统计（第一版，28 项）

- **fixed**：7（#1 cd、#2 open dir、#3 open file、#4 code review pane、#9 tab activation、#10 session navigator、#11 Linear title）。
- **fixed_pending_validation**：2（#9、#10，已合入 commit f21896f，未编译/未 GUI 验证）。
- **healthy**：11（12 个 `try_route` 调用点中 10 个对称健康 + cd-to-terminal stale guard + copy path）。
- **found**：1（#16 file browser symlink 语义未统一）。
- **closed**：4（#5 capability env、#4 session label、#6 error lifecycle、#8 session restore startup command——确认为合理 backend diff）。
- **pending_audit**：5（File Browser FS 操作：delete/rename/new/upload/download——当前 terminal/environment backend 已分流，但抽象未收口；本地 OS 操作与远程 RPC 的参数/语义待逐项核对）。

## 关键结论

1. **`try_route_current_runtime_environment_entry` 12 个调用点**：10 个对称健康，2 个已修（#4 welcome code review pane、#11 Linear title）。**模式 B 的风险已全部收口**。
2. **commit f21896f 编译错误已修**：该 commit 给 `DeferredEnvironmentAgentViewEntry` 加了 `fallback_display_title` 字段但没更新构造点，本轮完成该字段的全链路（构造点 + 消费点 + Linear 设值），编译通过。
3. **"两遍代码"根因**：`Environment` 抽象停在数据层（`EnvironmentSnapshot` / authority），没下沉到行为层。`try_route` 是分流器，把同一意图切成"远程 queue entry / 本地立即执行"两条路径，每个入口手写一遍分流，于是漏一边参数/副作用就成 bug。收敛方向 = fix-plan 阶段 3 `EnvironmentBackend` trait。
4. **`deferred` timing_parity 不是 leak**：远程异步 materialize 是执行模型本质差异，entry plan 是"把异步包装成看起来同步的入口"的合理中间态。审计时只把"远程 deferred 后丢失了本地同步路径的副作用"（如 #4 code review pane、#11 title）才算 bug。

## 其他扫描发现（非 local/remote 专项，归此处跟踪）

| id | 项 | 位置 | 严重度 | 状态 | 说明 |
|---|---|---|---|---|---|
| #12 | authority 字符串解析散落 3 处且逻辑不一致 | app_state.rs:134 / environment_runtime.rs:544 / source_saved_ssh.rs:198 | low-medium | found | `ssh:ssh-config:host` 在 display-label 路径得 `host`，在 connection_ref 路径得 `ssh-config:host`。集中化到单一 parser |
| #13 | `dormant_environment_from_server` 死代码 | source_saved_ssh.rs:325 | low | found | 仅 app_state_tests.rs:116 调用，生产代码未用。疑似"从 saved server 创 dormant env"入口走了别的方式，需甄别是否断路径 |
| #14 | i18n 孤儿 key 726 个 | app/i18n/*/warp.ftl | low | found | 含菜单改动确认孤儿 `server-file-browser-menu-terminal` / `-other`；大量 `agent-management-*` 疑似整块死 UI，误报率高需人工甄别 |
| #15 | build.rs panic on missing `MACOSX_DEPLOYMENT_TARGET` | app/build.rs:54 | low(DX) | fixed | 改为 fallback 10.14 + cargo:warning 提示 |
| #16 | File Browser symlink 语义被 local/remote backend 分裂 | server_file_browser.rs:4201 / server_model.rs:1883 | medium | found | 不应定位成“远程 symlink 识别”特例；需要统一文件浏览器 FS metadata 抽象，让本地 terminal root 和 remote environment root 都返回同一套 `entry_kind + target_kind` 语义。symlink-to-dir 应可展开/进入，symlink-to-file 应可打开，同时 UI 保留 symlink 身份 |

## 下一轮建议优先级

1. **#16 File Browser FS metadata 抽象 + symlink 统一**：先收口 list/resolve 的 entry 语义，再处理 UI 展开/打开行为；不要只 patch remote daemon。
2. **#12 authority parser 集中化**：抽单一 parser，3 处复用（纯重构，零行为变化）。
3. **file browser FS 操作 pending_audit 5 项**：在 #16 的 backend seam 上逐项核对 delete/rename/new/upload/download 参数/语义。
4. **#13 dormant_environment_from_server 甄别**：确认是否断路径或纯死代码。
5. **#14 i18n 孤儿 key**：726 个，含 `agent-management-*` 疑似整块死 UI，需人工甄别后批量清理。
6. **阶段 3 `EnvironmentBackend` trait**：从根上消除"两遍代码"，高风险大重构，单独 milestone。
