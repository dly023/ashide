# 本地 / 远程能力矩阵

`local-remote-capability-matrix.csv` 是 `local-remote-inconsistencies.csv` 的系统化扩展：把 `local-remote-inconsistencies.md` 排查时用的"逐个 grep 同源模式"升级成**全量能力审计**——列本地所有用户可见能力，逐项核对远程是否有等价实现 + 等价参数 + 等价时机。

## 列说明

- `capability`：用户能力名。
- `entry_point`：入口函数 `file:line`。
- `local_path` / `remote_path`：本地 / 远程实际执行路径。
- `param_parity`：参数是否对齐（yes / no / na）。
- `timing_parity`：时机是否对齐（yes / deferred / n/a）。`deferred` 表示远程经 entry plan 异步 materialize，本地同步——这是合理的执行模型差异，不算 leak。
- `status`：in_progress / fixed / fixed_pending_validation / healthy / found / closed / pending_audit。
- `notes`：关联 inconsistencies CSV 编号 + 说明。

## 当前状态摘要

- Session restore 已收口为 `allocate container → deliver`，remote materialization 必须绑定显式 `PaneId`，禁止读取 ambient focus。
- File Browser symlink 契约已修：`path` 与 `resolved_path` 分离，entry kind 与 target kind 分离；导航按目标，mutation 按链接自身。
- Project Explorer 的 tree snapshot 替换已收口到 `replace_root_entry`：本地/远程更新都必须重新校验 expansion intent，并自动加载仍处于展开状态的 unloaded directory symlink。
- Code Review 把 symlink 视为 Git mode `120000` blob：New/Untracked 路径只读取 link target，禁止通过 `git diff --no-index` 解引用文件、目录或 dangling target。
- File Search 与 Project Explorer 共用词法 repo identity：禁止在结果投影阶段 canonicalize repo root；缓存以 `StandardizedPath` 为 key，并在 tree mutation 完成事件失效。
- File Browser 新建条目的路径由本地/远程共用 `plan_new_entry` 唯一生成；父目录可以由 OS 跟随 symlink 执行 IO，但返回给 selection/rename/tree 的身份必须保留用户看到的词法目录。
- File Browser rename editor 现绑定词法 path，而不是易变列表 index；目录刷新、排序或 symlink retarget 不得把编辑操作漂移到另一行。
- Project Explorer symlink watcher 现把 target 事件统一解释为“重新读取 lexical state”，禁止把 target 删除误判成 link inode 删除；断链 target 通过最近存在祖先的 direct lifecycle watch 自动推进到重建后的目标。
- daemon 文件协议显式区分 `DeleteFile` 与 `DeleteDirectory`；递归遍历仍由 caller 控制，空目录自身必须走 `remove_dir`，禁止让 `remove_file` 承担双重 inode 语义。
- Environment runtime 的 synthetic `SessionId` 改由 app-global transport manager 唯一分配；所有 Workspace 虽订阅同一事件流，但只允许 owner Workspace 消费自己的 session 事件。
- `EnvironmentTable` 由 capability 决定初始生命周期：Current App/Local row 必须恒为 `Connected` 且不得出现在 `runtime_snapshots()`，remote row 才能从 `Dormant` 开始。
- Environment authority 协议正在收敛到 `ParsedEnvironmentAuthority`：local/runtime 分类、navigation key、SSH connection ref 与 display label 必须来自同一个类型化 parse result，并由静态检查禁止 consumer 复制前缀解析。
- File Browser delete / rename / create 已通过统一 backend seam 核对；普通 upload/download 的完整错误与覆盖矩阵仍需独立审计。
- 详细状态与测试绑定以 `local-remote-capability-matrix.csv` 为权威，本文不再维护容易漂移的手工计数。

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
| #16 | File Browser symlink 语义被 local/remote backend 分裂 | `remote_server.proto` / `server_file_browser.rs` / `sftp_manager` | medium | fixed | RPC 硬切区分 `path` 与 `resolved_path`；两套文件浏览器都保留 symlink entry kind + target kind。树 identity 禁止使用 canonical target；导航按目标类型，删除/重命名按链接自身 |
| #17 | Project Explorer 替换 tree snapshot 时绕过 expansion/load 不变量 | `code/file_tree/view.rs` | medium | fixed | 已展开 directory symlink retarget 后曾显示“展开但为空”，必须手动折叠再展开。现有 root snapshot 只允许走 `replace_root_entry`：保留可物化的 expansion intent、清理失效 intent，并对本地/远程统一触发 lazy load；静态测试禁止恢复直接赋值 |
| #18 | pending materialization 生命周期只在 Workspace 显式关闭路径清理 | `pane_group/mod.rs` / `workspace/view.rs` / `environment_table.rs` | high | fixed | 非最后一个 pane 可在 PaneGroup 内部关闭并只发 `AppStateChanged`，旧测试直接调用 cancel helper，未覆盖真实关闭路径。改为每次 pane 状态持久化前按 authority + live pane ownership 全局对账，统一清除 queued/materializing orphan，禁止把失效 PaneId 留进后续恢复状态 |
| #19 | disconnect 先删 authority、后关同 authority tabs，期间激活回建已删除状态 | `workspace/view.rs disconnect_environment_authority` | high | fixed | 关闭多个远程 tab 时会暂时激活同 authority 邻居，`prepare_active_environment_after_visible_tab_activation` 可重新创建刚删除的 Environment row/pending intent。改为两阶段 teardown：先清 pending，关闭全部容器，最后销毁 runtime authority，并测试 table 不得残留 |
| #20 | expanded snapshot reload 缺少 single-flight，失败 snapshot 可同步递归重试 | `code/file_tree/view.rs` | high | fixed | `replace_root_entry → ensure_loaded_path → load_directory → replace_root_entry` 在本地 load 失败且 entry 仍 unloaded 时可重新进入自身；远程 snapshot 更新也可能重复发同一路径 load。增加 root/path 级 load tracker，直到成功 snapshot、失败事件或 root 移除才释放 |
| #21 | remote Project Explorer 把词法 symlink path canonicalize，且 lazy-load 完成身份只有 root 粒度 | `remote_server/server_model.rs` / `remote_server/manager.rs` / `repo_metadata` / `code/file_tree/view.rs` | high | fixed | 外部目标 directory symlink 曾在 helper 上解析为 target 后被 root-escape 校验拒绝；成功更新还会 `finish_root` 误释放 sibling load，网络失败则没有 UI 可观察的精确完成事件，空目录响应也无法把 entry 标记 loaded。现改为协议边界保留 `StandardizedPath` 词法身份，transport 完成/失败携带 `(host, repo, dir_path)`，先转换成统一 `RepoMetadataModel::DirectoryLoadFinished` 再由 UI 释放单个 owner；空目录/空 directory symlink 显式 commit loaded |
| #22 | Session restore 的 remote materialization 曾允许 ambient focus 决定目标 pane | `workspace/view.rs` / `environment_table.rs` | high | fixed | 本地/远程统一为 `allocate container → deliver`；所有 queued runtime intent 必须绑定显式 `PaneId`，materialize 时拒绝 missing/stale/cross-authority target，禁止左侧投影或当前焦点选择恢复目标 |
| #23 | Code Review 对 New/Untracked symlink 使用 `git diff --no-index /dev/null <path>` | `code_review/diff_state.rs` | high | fixed | Git 会解引用工作树 symlink：directory link 报 `<link>/null`，file link 泄露 target 内容，dangling link 失败。现以 `symlink_metadata + read_link` 生成 mode `120000` blob diff，并把 `is_symlink` 领域身份传入视图层，强制 detached/selectable buffer；directory/file/broken 三类统一只展示 link target，聚合行数和编辑器均禁止打开 target |
| #24 | File Search 在 repo tree 保留 symlink 词法路径后又 canonicalize repo root，且完成事件不清缓存 | `search/files/model.rs` | medium-high | fixed | 以 symlink 路径打开 repo 时，entry 与 root 进入不同命名空间而被过滤；异步 tree apply 或 lazy-load 完成后旧缓存还可永久存活。现缓存和投影统一使用 `RepositoryIdentifier::Local(StandardizedPath)`，并在 `FileTreeEntryUpdated/DirectoryLoadFinished` 清理 |
| #25 | 本地 File Browser 在 symlink 目录中新建条目时把 UI identity canonicalize 到 target | `workspace/view/server_file_browser.rs` | medium | fixed | IO 实际成功，但 create 返回 `/real/target/New File`，reload 生成 `/visible/link/New File`，导致 pending rename/selection 永远匹配不到；远程路径没有该问题。现本地/远程共用 `plan_new_entry`，条目 identity 只从 listing 返回的词法目录生成，测试覆盖 symlink parent 下创建和 reload round-trip |
| #26 | Environment runtime synthetic `SessionId` 由每个 Workspace 从 0 分配 | `workspace/environment_table.rs` / `workspace/view.rs` / `remote_server/manager.rs` | critical | fixed | transport manager 和事件流是 app-global，多窗口会复用 `SessionId(0)`，既产生 stale/untracked 噪音，也可能让另一个 Workspace 错收连接、安装或断线事件。allocator 已移动到 singleton transport manager 的独立高位命名空间；Workspace 订阅入口按 EnvironmentTable owner 过滤。测试覆盖两个 Workspace 分配不冲突且互不拥有对方事件 |
| #27 | `EnvironmentTable::upsert` 把 Local row 初始化为 Dormant 并混入 runtime snapshots | `workspace/environment_table.rs` | high | fixed | 模型注释规定 Local 永远 Connected，但构造器无条件 Dormant，状态错误仅被 strip 去重偶然掩盖。现 entry 初始状态由 backend capability 决定；Local upsert 强制 Connected，runtime snapshot 投影明确排除 Local。测试 `local_entry_is_connected_and_excluded_from_runtime_snapshots` |
| #28 | File Browser rename editor 用列表 index 绑定 mutation target | `workspace/view/server_file_browser.rs` | high | fixed | symlink retarget、异步 reload 或排序可在编辑器打开期间改变 index，使提交作用到另一行。现 active rename 只保存词法 path，提交和渲染均按 path 重新定位；测试 `rename_target_path_survives_listing_reorder` |
| #29 | Project Explorer 把 target 删除事件当作 lexical link 删除，且 broken target 无恢复 watcher | `repo_metadata/current_app_model.rs` | high | fixed | mount target 事件统一按当前 lexical state refresh；target 根投影禁止尾斜杠；已加载目录、文件与 missing target 分别持有内容/生命周期 watcher，missing 多级路径会逐级推进 |
| #30 | daemon 递归删除普通目录最终错误调用 `remove_file` | `daemon_backend.rs` / `remote_server.proto` | high | fixed | 新增显式 `DeleteDirectory` 协议；file/symlink 与 directory mutation 不再共用含混 endpoint，空 oneof response 也不再假报成功 |
| #31 | 二级 symlink retarget 不在最终 canonical target watcher 命名空间内 | `repo_metadata/current_app_model.rs` | high | fixed | mount 同时记录 canonical content target 与 raw lexical lifecycle target；`link -> alias -> target-a` 中 alias 改指 target-b 会投影为 link-root refresh，并原子替换 watcher ownership。测试 `external_symlink_chain_retarget_refreshes_mount_from_lexical_lifecycle_event` |
| #32 | File Browser transfer 静默解引用 symlink，递归下载可越界或循环 | `workspace/view/server_file_browser.rs` | high | fixed | 当前协议不能保留 link inode，因此 upload/download planner 明确拒绝 symlink；禁止 file link 泄露 target 内容、directory link 递归越出选择根、WalkDir 错误导致静默部分上传。浏览/导航和在 symlink 目录内新建仍按既有 capability 工作 |
| #33 | 同一 repo 的 watcher mutation 异步回调可乱序覆盖新状态 | `repo_metadata/current_app_model.rs` | high | fixed | 每个 repo 改为 single-flight FIFO pipeline；下一 batch 只能在前一 batch apply 后启动。re-index/remove 会失效 active token，旧 repository incarnation 的回调不能写入同路径的新 tree。测试 `repository_watcher_batches_are_single_flight_fifo_per_repo` |
| #34 | fallback Environment row 用 `starts_with("local")` 推断 backend | `workspace/environment_table.rs` | high | fixed | queue create-on-write 现在只消费 `ParsedEnvironmentAuthority` 的 backend、display label 与 connection ref capability；`locality:remote` 之类 authority 不再被误建为 Connected Local。测试 `fallback_entry_classification_uses_authority_capability_not_string_prefix` |
| #35 | Environment authority 在 app_state/runtime/provider/table/Navigator 各自解析 | `environment_authority.rs` + authority consumers | high | in_progress | `LR-037`：建立唯一 `ParsedEnvironmentAuthority` home；SPEC `SN-ENV-AUTHORITY-PARSER-01`、capability matrix、静态 guard 与 focused tests 先行，随后删除所有 consumer 的 `starts_with`/`strip_prefix` 和旧 wrapper。 |
| #36 | CLI-agent plugin scope 用 saved-SSH connection ref 是否存在判断 local/runtime | `workspace/view/session_navigator.rs` | high | in_progress | `LR-038`：custom/container/WSL runtime 缺 saved-SSH ref 时被错误注册成 current-app session，plugin UI 可能检查或自动安装到本机。改为 authority capability 决定 Some/None，transport descriptor 只 enrichment host label。 |
| #37 | SFTP Browser 与 helper chunk RPC 绕过 symlink transfer deny contract | `sftp_manager` + `remote_server/server_model.rs` | critical | in_progress | `LR-039`：Workspace planner 已拒绝 link，但 SFTP 下载/上传及 `ReadFileChunk/WriteFileChunk` 仍跟随 target；上传覆盖 symlink-to-file 会改写 target。修复必须覆盖 UI planning、backend direct call 和 helper protocol 三层。 |
| #38 | remote helper `oneof result` 缺失被解释为成功或领域默认值 | `remote_server/client` + `workspace/environment_runtime.rs` | critical | in_progress | `LR-040`：所有 required result 在 `send_request` transport boundary 统一校验；static contract 从 proto 核对 validator 覆盖，业务层禁止 `None => success/not-found/saved`。 |

| #39 | remote ListDirectory 静默吞 entry/metadata 错误并返回 partial success | `remote_server/server_model.rs` | high | in_progress | `LR-041`：listing 必须 complete-or-error；统一 collector 禁止 `read_dir.flatten()`，lexical symlink metadata 与 target metadata 分离，broken link 仍保留 row。 |

| #40 | local/remote CLI-agent scan 吞 traversal error 后用 partial success 覆盖缓存 | `cli_agent_jsonl.rs` + local/remote scanners | critical | in_progress | `LR-042`：共享 error-aware recent JSONL discovery；scan 返回 typed Result；Refresh/cache replace 只在完整 Success 时发生，失败保留原 rows。 |


## 下一轮建议优先级

1. **#35 / LR-037 authority parser 集中化**：按 `SPEC → Matrix → CHECK → TEST → GUI` 门禁完成所有 consumer 迁移并验证 custom runtime authority。
2. **#37 / LR-039 symlink transfer 全入口门禁**：先封死 SFTP UI/backend/helper RPC 的解引用路径；若产品需要真正传输 symlink，再新增 `ReadLink/CreateSymlink` capability。
3. **#13 dormant_environment_from_server 甄别**：确认是否断路径或纯死代码。
4. **#14 i18n 孤儿 key**：726 个，含 `agent-management-*` 疑似整块死 UI，需人工甄别后批量清理。
5. **阶段 3 `EnvironmentBackend` trait**：从根上消除"两遍代码"，高风险大重构，单独 milestone。

| #41 | CLI-agent home unavailable 被解释为空 store 或 filesystem root | `cli_agent_jsonl.rs` + local/remote session operations | critical | in_progress | `LR-043`：建立唯一 required-home resolver；unknown 必须显式失败并保留缓存，scan/read/mutate 禁止回退 `/` 或访问错误用户 store。 |

| #42 | Terminal File Browser 静默跳过 lexical metadata 失败的 entry | `workspace/view/server_file_browser.rs` + remote helper | high | in_progress | `LR-044`：把 `LR-FILE-LIST-COMPLETE-01` 扩展到 Terminal backend；本地/远程 Success 都只能表达完整 lexical snapshot。 |

| #43 | CLI-agent cwd 本地原样保留、远程单独清洗 | `cli_agent_jsonl.rs` + local/remote scanners | high | in_progress | `LR-045`：cwd extraction 与 host-path normalization 一并集中到共享 home；相同 transcript 必须投影相同 cwd。 |

| #44 | CLI-agent logical limit 本地按 provider、远程按全局截断 | `cli_agent_jsonl.rs` + local/remote scanners | critical | in_progress | `LR-046`：共享 physical-source dedup、logical-session 聚合、全局 recency 排序与 quota；本地/远程相同 store 必须得到相同逻辑会话集合。 |

| #45 | Codex session index 本地/远程字段 fallback 与 timestamp parser 分叉 | `cli_agent_jsonl.rs` + local/remote scanners | high | in_progress | `LR-047`：建立唯一 index record parser；本地/远程只映射共享 record，不再直接读取 `id/session_id/title/cwd/updated_at`。 |

| #46 | Workspace 冷启动拥有 lossy constructor scan 与 typed refresh 两条边界 | `workspace/view.rs` + `workspace/view/session_navigator.rs` + local scanner | high | fixed | `LR-048`：构造器只初始化空 cache；首次/后续扫描统一由 typed scan + transactional commit；最新 App 冷启动 runtime 已确认 single scan。 |

| #47 | dev remote helper 可重复 zigbuild，且安装 watchdog 与 compile timeout 同为 900s | `remote_server/dev_remote_install.rs` + `workspace/view.rs` + `remote_server/setup.rs` | critical | fixed | `LR-049`：process-wide install single-flight 与分层 watchdog 已通过 remote → local → remote runtime 验证，重复激活未出现第二组 build pipeline。 |

| #48 | broken symlink rename 后 watcher 把新路径误判为删除 | `crates/watcher/src/lib.rs` + `repo_metadata/current_app_model.rs` | high | in_progress | `LR-050`：`RenameMode::Any` 必须基于 lexical inode tri-state 分类；禁止 `Path::exists()` 跟随 target。new dangling path 归类 added，old missing path 归类 deleted，Unknown 显式失败。 |
