# Development Guide

## Common commands

Check the Rust workspace:

```bash
MACOSX_DEPLOYMENT_TARGET=11.0 cargo check -q
```

Run the local macOS app build:

```bash
TERM=xterm-256color ./script/run
```

The app bundle build can be slow, so prefer `cargo check` for small code changes and run the GUI only when interaction needs verification.

根目录 `Cargo.lock` 必须随依赖变更一起提交。应用 bundle、CI Release 和 remote helper 构建统一使用 `--locked`，确保本地验证、tag 构建和发布产物解析到完全相同的依赖图；需要升级依赖时应显式更新 lockfile，而不是依赖 CI 临时解析最新版。

## Naming

- Product name: `Ashide`
- Binary/package id: `ashide`
- Bundle id namespace: `dev.ashide.*`

Avoid the misspelling `Aishide`.

## macOS 应用图标

图标契约以 [`APP_ICON_SPEC.yaml`](APP_ICON_SPEC.yaml) 为唯一权威定义。

- 规范品牌资源位于 `app/channels/oss/icon/`。
- `script/macos/bundle` 与 `script/compile_icon` 必须使用该资源树；其他 channel
  没有独立资源时必须显式回落到 `oss`。
- bundle 完成后，`Info.plist` 必须声明 `CFBundleIconFile=AppIcon`，并且
  `Contents/Resources/AppIcon.icns` 必须存在。
- Ashide 不提供运行时图标切换；图标设置、Dock 插件和 channel 图标变体都属于
  禁止重新引入的平行机制。
- DMG 背景固定使用 `app/assets/resources/mac/ashide_install_image.png`。

## Development principles

- Keep user-visible behavior local/offline-first where possible.
- Prefer existing OpenSSH config over custom SSH profile duplication.
- Keep local and remote environment state separate.
- Preserve upstream attribution and license notices.
- Make small, reversible, reviewable changes.
