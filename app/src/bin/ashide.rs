// On Windows, Ashide is a GUI app: double-clicking any build of the main executable must not
// open an extra console window. See https://doc.rust-lang.org/reference/runtime.html#the-windows_subsystem-attribute.
#![cfg_attr(windows, windows_subsystem = "windows")]

use anyhow::{bail, Result};
use warp_core::{
    channel::{Channel, ChannelConfig, ChannelState},
    features::DEBUG_FLAGS,
    AppId,
};

fn main() -> Result<()> {
    let channel = channel_from_env()?;
    let mut state = ChannelState::new(
        channel,
        ChannelConfig {
            app_id: app_id_for_channel(channel),
            logfile_name: logfile_for_channel(channel).into(),
            autoupdate_config: None,
            mcp_static_config: None,
        },
    );
    if cfg!(debug_assertions) {
        state = state.with_additional_features(DEBUG_FLAGS);
    }
    // 始终启用 IME marked-text 渲染:winit 的 IME 路径在 macOS / Windows 都支持,
    // 但若不在此处显式开启,Ashide 会把 preedit / 输入合成更新整体丢弃,只剩 OS 的候选窗
    // 可见 —— 在 Windows 上对日文 / 中文 / 韩文输入都属于实质性损坏。
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        use warp_core::features::FeatureFlag;
        state = state.with_additional_features(&[FeatureFlag::ImeMarkedText]);
    }
    ChannelState::set(state);

    warp::run()
}

fn channel_from_env() -> Result<Channel> {
    let channel = std::env::var("ASHIDE_CHANNEL").unwrap_or_else(|_| "oss".to_owned());
    match channel.as_str() {
        "oss" | "ashide" => Ok(Channel::Oss),
        "dev" => Ok(Channel::Dev),
        "local" => Ok(Channel::Local),
        "preview" => Ok(Channel::Preview),
        "stable" => Ok(Channel::Stable),
        other => bail!("Unsupported ASHIDE_CHANNEL={other}"),
    }
}

fn app_id_for_channel(channel: Channel) -> AppId {
    match channel {
        Channel::Oss | Channel::Stable => AppId::new("dev", "ashide", "Ashide"),
        Channel::Dev => AppId::new("dev", "ashide", "AshideDev"),
        Channel::Local => AppId::new("dev", "ashide", "AshideLocal"),
        Channel::Preview => AppId::new("dev", "ashide", "AshidePreview"),
        Channel::Integration => AppId::new("dev", "ashide", "AshideIntegration"),
    }
}

fn logfile_for_channel(channel: Channel) -> &'static str {
    match channel {
        Channel::Oss | Channel::Stable => "ashide.log",
        Channel::Dev => "ashide_dev.log",
        Channel::Local => "ashide_local.log",
        Channel::Preview => "ashide_preview.log",
        Channel::Integration => "ashide_integration.log",
    }
}

// If we're not using an external plist, embed the following as the Info.plist.
#[cfg(all(not(feature = "extern_plist"), target_os = "macos"))]
embed_plist::embed_info_plist_bytes!(r#"
    <?xml version="1.0" encoding="UTF-8"?>
    <!DOCTYPE plist PUBLIC "-//Apple Computer//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
    <plist version="1.0">
    <dict>
    <key>CFBundleDevelopmentRegion</key>
    <string>English</string>
    <key>CFBundleDisplayName</key>
    <string>Ashide</string>
    <key>CFBundleExecutable</key>
    <string>ashide</string>
    <key>CFBundleIdentifier</key>
    <string>dev.ashide.Ashide</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleLocalizations</key>
    <array>
    <string>en</string>
    <string>ja</string>
    <string>zh-CN</string>
    </array>
    <key>CFBundleName</key>
    <string>Ashide</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>0.1.0</string>
    <key>LSApplicationCategoryType</key>
    <string>public.app-category.developer-tools</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>UIDesignRequiresCompatibility</key>
    <true/>
    <key>CFBundleURLTypes</key>
    <array><dict><key>CFBundleURLName</key><string>Custom App</string><key>CFBundleURLSchemes</key><array><string>ashide</string></array></dict></array>
    <key>NSHumanReadableCopyright</key>
    <string>© 2026, Ashide</string>
    </dict>
    </plist>
"#.as_bytes());
