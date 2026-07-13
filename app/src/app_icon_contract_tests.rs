use std::path::Path;

#[test]
fn test_app_icon_contract_is_complete_and_hard_cut() {
    let spec: serde_yaml::Value =
        serde_yaml::from_str(include_str!("../../docs/APP_ICON_SPEC.yaml"))
            .expect("APP_ICON_SPEC.yaml 必须是合法 YAML");
    assert_eq!(spec["spec_version"].as_u64(), Some(1));
    for field in [
        "classic_source",
        "adaptive_source",
        "development_runtime_source",
        "dmg_background",
    ] {
        assert!(
            spec["canonical_assets"][field].is_string(),
            "图标 SPEC 缺少 canonical_assets.{field}"
        );
    }
    assert_eq!(
        spec["change_contract"]["check_test"].as_str(),
        Some("test_app_icon_contract_is_complete_and_hard_cut")
    );

    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    for relative in [
        "channels/oss/icon/padded/512x512.png",
        "channels/oss/icon/AppIcon.icon",
        "assets/resources/mac/ashide_install_image.png",
    ] {
        assert!(
            manifest.join(relative).exists(),
            "规范图标资源缺失: {relative}"
        );
    }
    for relative in [
        "DockTilePlugin",
        "src/settings/app_icon.rs",
        "assets/bundled/png/local.png",
        "assets/bundled/png/dev.png",
        "assets/resources/mac/warp_install_image.png",
    ] {
        assert!(
            !manifest.join(relative).exists(),
            "已硬切的历史图标资源或机制被重新引入: {relative}"
        );
    }

    let production_sources = [
        ("src/appearance.rs", include_str!("appearance.rs")),
        ("src/settings/mod.rs", include_str!("settings/mod.rs")),
        (
            "src/settings_view/appearance_page.rs",
            include_str!("settings_view/appearance_page.rs"),
        ),
        ("build.rs", include_str!("../build.rs")),
        ("script/macos/run", include_str!("../../script/macos/run")),
        (
            "script/macos/bundle",
            include_str!("../../script/macos/bundle"),
        ),
    ];
    for (path, source) in production_sources {
        for forbidden in [
            "AppIconSettings",
            "DockTilePlugin",
            "dock_tile_plugin",
            "NSDockTilePlugIn",
            "setApplicationIconImage",
        ] {
            assert!(
                !source.contains(forbidden),
                "{path} 重新引入历史图标机制: {forbidden}"
            );
        }
    }

    let compile_icon = include_str!("../../script/compile_icon");
    assert!(compile_icon.contains("CFBundleIconFile"));
    assert!(compile_icon.contains("AppIcon.icns"));
    assert!(compile_icon.contains("rm -f \"$BUNDLED_RESOURCES_DIR/Ashide.icns\""));
    assert!(compile_icon.contains("plutil -remove NSDockTilePlugIn"));
    assert!(compile_icon.contains("AshideDockTilePlugin.docktileplugin"));
    assert!(compile_icon.contains("$APP_BUNDLE_PATH/Icon"));

    let bundle = include_str!("../../script/macos/bundle");
    assert!(bundle.contains("app/assets/resources/mac/ashide_install_image.png"));
    assert!(bundle.contains("ICON_ASSET_CHANNEL=\"oss\""));

    let app = include_str!("lib.rs");
    assert!(app.contains("channels/oss/icon/padded/512x512.png"));
}
