# Ashide Local/Remote Parity 与 Init Release 收口计划

> 更新日期：2026-07-26
> 状态来源：当前 runtime、代码、领域 SPEC、capability matrix 与 parity tracker。
> 原则：不新增平行抽象，不用 UI 补丁、重试、fallback 或第二套发布流程遮挡模型问题。
> 本轮首次公开发布身份：`v0.0.1`；清理后仅保留 `main`、根提交 `init` 与唯一公开 Release `v0.0.1`。

## 1. 背景

Ashide 的本地与远程环境长期存在行为层分叉、异步生命周期归属不一致、SessionId/容器身份混用，以及 release helper 安装身份与 wire protocol 不一致的问题。这些问题曾多次以局部补丁方式修复后回归，具体表现包括：

- Resume 时列表短暂减少、顶部临时行闪现或 source 暂失被误判为删除。
- 同一 runtime SessionId 内多个远程历史扫描乱序完成时，旧成功子集可覆盖较新 canonical collection，造成无关行瞬间消失。
- dev bundle 的 GUI owner path 曾错误复用 `WARP_DATA_PROFILE` 数据目录，使同一 `.app` 获得两个 lock/socket namespace。
- 本地与远程使用不同 action、projection、persistence 或 delivery 路径。
- terminal SessionId、Environment runtime owner SessionId、HostId 与布局 locator 被错误地互相替代。
- 多窗口连接同一远端 authority 时复用 authority-only SSH `ControlPath`，后创建的连接会通过 `ssh -O exit` 杀死前一个窗口的 ControlMaster。
- Fork/Resume 向原生 CLI 写入由 Ashide 猜测或硬编码的 provider/model/profile 等目标系统配置。
- tag release 只按应用版本命名 remote helper，导致 protocol revision 已变化时仍复用旧 helper。
- release tracker、capability matrix、测试矩阵与真实产物状态不同步，使 gate 不能准确反映发布条件。
- committed Session Navigator 曾允许 49 个 canonical sync 调用方选择只提交 membership、不提交当前 focus，使 Delete、close 与 Undo 后 committed model 可能短暂没有 active row。
- 首次公开 Release 的真实远程验证目标被写入测试 fixture、tracker evidence 与人工 notes，暴露开发者私人 SSH target；发布链此前没有 source/body/archive 隐私门禁。
- macOS Release 把已生成的 DMG 再封装为 `Ashide-macos.zip`，并允许 DMG 缺失时回退裸 `.app`，没有提供标准的直接 DMG 安装入口。

本轮目标不是继续“水多加面”，而是把差异收敛到最低 backend/delivery 边界：高层 action、identity、lifecycle、projection、persistence 与 user state 共用一套模型；发布只复用现有 bundle、seal、verify、package 流程。

## 2. 要删除或收敛的 legacy 点

### 2.1 Local/Remote 行为分叉

- 删除 Navigator、Resume、Fork、Pin、Alias、Refresh 等行为层的 `local/remote` 分支。
- 删除由 UI/render/snapshot 层重建 canonical collection 或冻结列表的补丁路径。
- 删除 source-missing 帧直接覆盖 canonical rows 的 destructive replace/clear/retain 语义。
- 删除无 generation 的异步 scan completion；scan token 与 indexed collection 统一归 `EnvironmentTable` navigation-key partition。
- 将本地同步 materialize 与远程异步 materialize 的差异限制在统一 backend 的 delivery 时机。
- GUI owner namespace 只读 `ChannelState::app_id`；`WARP_DATA_PROFILE` 继续隔离数据，但不得拆分进程 ownership。
- 删除 `reduce_session_navigator_refresh` 的可选 `inject_focused_live` / membership-only 分支；所有 canonical sync 调用方统一通过同一个 reducer transaction 提交 membership + `PaneFocused`。

### 2.2 Session 身份与路由

- 禁止把 `tab:X:leaf:Y`、tab index、EntityId 或 terminal runtime UUID 当作稳定容器身份。
- 禁止按 HostId 任取同 host 的任意 transport；具体 operation 必须携带 exact SessionId。
- terminal bootstrap SessionId 与 Environment owner SessionId 的等价判断统一复用 `RemoteServerManager` 的 alias canonicalization，禁止裸 `SessionId != SessionId` 比较。
- 删除 `client_for_host` 及任何下游重新推断 transport binding 的路径。
- 删除 authority-only SSH ControlMaster 路径；`ControlPath` 必须由 authority 与 Environment owner SessionId 共同派生，并由 transport、reconnect 与 deregister 全程携带同一个 owner path。

### 2.3 SessionBridge 原生 Fork/Resume

- 删除硬编码或猜测的 Codex/Claude provider、model、profile、version、branch、permission 等 target-owned 字段。
- 删除由 UI 参数覆盖目标 CLI installation 配置的路径。
- 删除直接修改 Codex 私有 SQLite registry 的实现。
- 本地与远程统一读取目标 execution environment 的系统配置与 canonical project identity，再交给同一个 native write plan。

### 2.4 Remote helper 与协议

- 删除仅靠 release version、build stamp、`--version` 或 help grep 判断兼容性的旧逻辑。
- source/dev helper 统一使用 `dev-pty-v<revision>` slot。
- tag release helper 统一使用 `<version>-pty-v<revision>` slot。
- 保留 Initialize 双向 protocol revision 作为最终 fail-closed 证明；client/server 均在业务或 auth 副作用前拒绝不匹配 revision。
- 删除 version-only helper 路径和不兼容 helper fallback。

### 2.5 Release 与 harness

- 删除平行的 bundle、签名、helper 打包或 checksum 生成流程。
- 只复用：`script/macos/bundle`、`script/make_release_artifacts`、`script/make_release_helper_artifacts` 与 `.github/workflows/release.yml`。
- 删除 feature-specific memory skill；稳定边界进入 `AGENTS.md`，动态事实进入 SPEC/matrix/tracker，可复用步骤进入 architecture workflow skill。
- tracker、capability matrix、init release matrix 必须同步；无真实证据不得把条目标记为 `verified`。
- 删除真实 SSH target 作为 fixture/evidence 的路径；统一使用 `remote-fixture-*`，本机 denylist 只保存在 `.git/ashide-private-release-tokens`，不得进入 source。
- 删除“已存在 Release body 不覆盖”的路径；workflow 必须用当前提交中经过扫描的 `docs/releases/<tag>.md` 覆盖 notes。
- 删除 macOS zip-wrapped DMG 与裸 App fallback；App 发布只允许直接、已验证的 `Ashide-macos.dmg`。

## 3. 目标文件

### 契约与跟踪

- `AGENTS.md`
- `.agents/skills/README.md`
- `.agents/skills/ashide-architecture-change/SKILL.md`
- `docs/LOCAL_REMOTE_PARITY_SPEC.yaml`
- `docs/SESSION_NAVIGATOR_SPEC.yaml`
- `docs/design/local-remote-parity-tracker.yaml`
- `docs/design/local-remote-capability-matrix.csv`
- `docs/design/init-release-test-matrix.yaml`
- `script/check_local_remote_parity`
- `script/check_agent_harness`

### Session / Environment / Navigator

- `app/src/workspace/environment_table.rs`
- `app/src/workspace/environment_provider.rs`
- `app/src/workspace/view.rs`
- `app/src/workspace/view_test.rs`
- `app/src/workspace/view/session_navigator.rs`
- `app/src/workspace/view/session_navigator_reducer.rs`
- `app/src/workspace/view/session_navigator_reducer_tests.rs`
- `app/src/pane_group/pane/pane_configuration.rs`
- `app/src/session_bridge/native_writer.rs`
- `app/src/session_bridge/tests.rs`

### Remote transport / file / buffer

- `crates/remote_server/src/manager.rs`
- `crates/remote_server/src/manager_tests.rs`
- `crates/remote_server/src/setup.rs`
- `crates/remote_server/src/setup_tests.rs`
- `app/src/remote_server/server_model.rs`
- `app/src/remote_server/server_model_tests.rs`
- `app/src/remote_server/server_buffer_tracker.rs`
- `app/src/remote_server/server_buffer_tracker_tests.rs`
- `app/src/workspace/environment_runtime.rs`
- `app/src/workspace/view/server_file_browser.rs`
- `app/src/sftp_manager/`
- `app/src/code/global_buffer_model.rs`
- `app/src/code/global_buffer_model_tests.rs`
- `crates/warp_files/src/lib.rs`
- `crates/warp_files/src/lib_test.rs`

### Release

- `script/macos/bundle`
- `script/make_release_artifacts`
- `script/make_release_helper_artifacts`
- `script/tests/macos_release_contract.rs`
- `script/check_release_privacy`
- `script/tests/release_privacy_contract.sh`
- `docs/releases/v0.0.1.md`
- `.github/workflows/release.yml`

## 4. 具体计划

1. **以 runtime 证明当前执行链**
   - 对每个问题记录 `action → owner model → lifecycle → projection/persistence → visible effect`。
   - runtime 与 source 冲突时，以真实运行行为为准。

2. **先更新契约，再修改实现**
   - 更新对应 SPEC 与 UX/negative-space matrix。
   - 将新发现的漏网追加或合并到正确 tracker 条目。
   - 更新 static checker，使错误路径能够 fail closed。

3. **先写失败测试**
   - 覆盖 source 暂失、乱序、失败、取消、0/1/many 无关实体。
   - 集合型异步流程至少包含目标实体和两个无关实体，并逐阶段检查 cardinality、identity、order。
   - transport 测试覆盖同 host 多 session、terminal alias、disconnect、collision 与 stale completion。
   - SSH ControlMaster 测试覆盖同 authority 多 owner session，证明 control path 彼此不同、单个 owner 中稳定且满足 Unix socket 长度约束。

4. **收敛到既有 owner**
   - identity 归 `PaneConfiguration` / `WorkspaceSessionSnapshot`。
   - Environment lifecycle、pending materialization 与 canonical projection 归 `EnvironmentTable`。
   - transport session alias 与 client resolution 归 `RemoteServerManager`。
   - SSH ControlMaster identity 归 Environment owner SessionId；同 authority 的不同窗口不得共享或互相回收 control socket。
   - native CLI 配置归目标 installation；Ashide 只负责 transcript/history 转换。
   - 删除被新 owner 取代的旧分支、wrapper、fallback 与重复 registry。

5. **逐项验证并维护 tracker**
   - focused test 通过后记录 test run。
   - `cargo check --locked` 通过后记录编译证据。
   - local/remote GUI/runtime 真实观察通过后记录 runtime 证据。
   - 只有 static + focused + cargo + runtime 全部成立，才能把 tracker 与 capability matrix 升级为 `verified`。

6. **清零发布隐私与安装产物门禁**
   - 先把现有 Release 转为 Draft，匿名化 body，并让真实 SSH target 在 tracked source 中零命中。
   - 用 `script/check_release_privacy` 扫描 source、版本化 notes、ZIP/TAR 解包内容和挂载后的 DMG；本机 denylist 不进入 Git。
   - 删除旧 App 与旧 macOS zip，只从当前 checkout 重建 sealed App 和直接 DMG。

7. **清零其余发布门禁**
   - 确认 tracker 无 release-blocking active row。
   - 按固定顺序运行完整 gates。
   - 所有 gates 通过后只保留唯一最终 App/DMG。

8. **生成并发布唯一正式产物**
   - 使用现有脚本直接生成 `Ashide-macos.dmg`、Linux x86_64/aarch64 helper、Windows installer 与 `SHA256SUMS`；macOS 不再发布 zip-wrapped DMG。
   - 验证 bundle seal、DMG image、identity、architecture、archive member、隐私扫描和 checksums。
   - 最后重写单根 `init`、唯一 `main`/`v0.0.1` refs，并用受检版本化 notes 覆盖公开 Release。

## 5. 验证方式

### Static / harness

```bash
bash script/check_agent_harness
bash script/check_local_remote_parity
```

预期：所有 contract marker、forbidden-path guard、tracker gate 与 capability matrix gate 全部通过。

### Rust 编译

```bash
cargo check --locked
```

预期：workspace locked dependency graph 编译零失败。

### Workspace 测试

```bash
cargo nextest run \
  --no-fail-fast \
  --workspace \
  --exclude command-signatures-v2
```

预期：零失败；不得用局部测试代替最终 full workspace gate。

2026-07-21 最终验证证据：`cargo nextest` Run ID `69d83d2e-6c89-47ca-978b-8294a9cc0560`，6329/6329 通过，87 skipped，零失败。LR-157 的 hermetic saved-provider fixture 覆盖原失败 22/22，并与 retained-disconnect 共享重连契约一致；release workflow contract 也已从固定调用次数改为枚举所有 `gh release` 调用并逐条校验 repository pin。

### macOS release contract

执行 `script/tests/macos_release_contract.rs` 对应测试，确认：

- 所有 release Bash 脚本先通过 `bash -n` native parse，禁止 marker 测试掩盖脚本本身无法执行；
- local/dev unsigned App 已完成 ad-hoc seal；
- `codesign --verify --deep --strict` 只作为 bundle 完整性门禁，不得冒充公开分发身份；
- tag/public upload 与签名模式正交：零 secrets 产出 ad-hoc sealed DMG，六项完整目标配置自动启用 Developer ID/notarization，部分配置失败；
- signed 模式的 App 必须具备 Developer ID Application identity 与 hardened runtime，DMG 必须 notarized/stapled；
- signed 模式下 App 与 DMG 必须同时通过 `spctl` Gatekeeper assessment；
- packaging 在 seal postcondition 缺失时 fail closed；
- App artifact 直接输出 `.dmg`，不再 zip 包裹，也不回退裸 `.app`；
- `hdiutil verify`、挂载后隐私扫描、bundle identity、版本和 architecture 与 release tag 一致。

### Remote helper contract

```bash
./script/make_release_helper_artifacts \
  --platform linux \
  --channel oss \
  --release-tag v0.0.1
```

确认：

- `ashide-linux-x86_64.tar.gz` 与 `ashide-linux-aarch64.tar.gz` 均存在；
- 每个 archive 只包含单一 `ashide` 成员；
- helper slot 含 release version 与 protocol revision；
- `SHA256SUMS` 全部校验通过；
- 真实远程 Initialize、PTY、文件浏览器及 operation routing 使用新 helper 成功。

### GUI/runtime

- 真实启动当前构建的 App，不以 `cargo check` 代替 GUI 观察。
- 2026-07-20 当前构建验证：本地 70 帧与 remote-fixture-secondary runtime 80 帧 Resume 序列均始终保持 31 个可见 Navigator OCR rows；本地 Delete 发出 `RequestDeleteWorkspaceSession` 且集合未瞬降；远程 `CloseCurrentSession → RemoveActive → focus → UndoCloseStack consumed` 的 110 帧标题集合保留全部无关会话。
- 分别验证本地与远程 Resume、Pin/Alias、Refresh、Fork、冷恢复与列表 cardinality/order。
- 验证 terminal alias 的目录导航不会再出现：

```text
Ignoring stale Environment navigation ... active host binding is ...
```

- 对多窗口/同 host 多 session、remote buffer、file transfer、symlink workspace 做对应 runtime 验证。
- 对同 authority 双窗口分别验证 Initialize、独立命令、`Ctrl-C`/Abort、PTY 当前目录与文件浏览器导航；关闭或中断一个 owner 不得影响另一个 owner 的 ControlMaster、proxy 或 terminal。

### 最终产物

```bash
script/make_release_artifacts \
  --source target/release-lto/bundle/osx \
  --name Ashide-macos \
  --artifact app \
  --channel oss \
  --distribution development \
  --release-tag v0.0.1
```

最终检查：

- App strict seal 通过；
- App 为 arm64，版本为 `0.0.1`，bundle id 为 `dev.ashide.Ashide`；
- 直接发布的 macOS DMG 与 Linux helper 均来自当前源码；
- 完整 `SHA256SUMS` 校验通过；
- tracker 全部 `verified`，全测试零失败后才允许发布；签名凭据不是直接 DMG 的前置条件。
