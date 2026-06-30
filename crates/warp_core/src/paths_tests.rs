use dirs::home_dir;

use super::*;

fn data_profile_suffix() -> String {
    ChannelState::data_profile()
        .map(|profile| format!("-{profile}"))
        .unwrap_or_default()
}

#[test]
fn test_data_dir_path() {
    let home_dir = home_dir().expect("Should be able to compute home directory");
    // ChannelState, by default, is configured for Channel::Oss.
    cfg_if::cfg_if! {
        if #[cfg(target_os = "macos")] {
            assert_eq!(data_dir(), home_dir.join(".ashide"));
        } else if #[cfg(any(target_os = "linux", target_os = "freebsd"))] {
            let suffix = data_profile_suffix();
            assert_eq!(data_dir(), home_dir.join(format!(".local/share/ashide{suffix}")));
        } else if #[cfg(windows)] {
            let suffix = data_profile_suffix();
            assert_eq!(data_dir(), home_dir.join(format!("AppData\\Roaming\\ashide\\Ashide{suffix}\\data")));
        } else {
            unimplemented!("Need to update tests for current platform!");
        }
    }
}

#[test]
fn test_config_local_dir_path() {
    let home_dir = home_dir().expect("Should be able to compute home directory");
    // ChannelState, by default, is configured for Channel::Oss.
    cfg_if::cfg_if! {
        if #[cfg(target_os = "macos")] {
            assert_eq!(config_local_dir(), home_dir.join(".ashide"));
        } else if #[cfg(any(target_os = "linux", target_os = "freebsd"))] {
            let suffix = data_profile_suffix();
            assert_eq!(config_local_dir(), home_dir.join(format!(".config/ashide{suffix}")));
        } else if #[cfg(windows)] {
            let suffix = data_profile_suffix();
            assert_eq!(config_local_dir(), home_dir.join(format!("AppData\\Local\\ashide\\Ashide{suffix}\\config")));
        } else {
            unimplemented!("Need to update tests for current platform!");
        }
    }
}

#[test]
fn test_warp_home_config_dir_path() {
    let home_dir = home_dir().expect("Should be able to compute home directory");
    let expected_dir_name = match ChannelState::data_profile() {
        Some(data_profile) => format!(".ashide-{data_profile}"),
        None => ".ashide".to_string(),
    };

    assert_eq!(
        warp_home_config_dir(),
        Some(home_dir.join(expected_dir_name))
    );
}

#[test]
fn test_warp_home_skills_and_mcp_paths() {
    let Some(config_dir) = warp_home_config_dir() else {
        panic!("Should be able to compute Ashide home config directory");
    };

    assert_eq!(warp_home_skills_dir(), Some(config_dir.join("skills")));
    assert_eq!(
        warp_home_mcp_config_file_path(),
        Some(config_dir.join(".mcp.json"))
    );
}
#[test]
fn test_cache_dir_path() {
    let home_dir = home_dir().expect("Should be able to compute home directory");
    let suffix = data_profile_suffix();
    // ChannelState, by default, is configured for Channel::Oss.
    cfg_if::cfg_if! {
        if #[cfg(target_os = "macos")] {
            assert_eq!(cache_dir(), home_dir.join(format!("Library/Application Support/dev.ashide.Ashide{suffix}")));
        } else if #[cfg(any(target_os = "linux", target_os = "freebsd"))] {
            assert_eq!(cache_dir(), home_dir.join(format!(".cache/ashide{suffix}")));
        } else if #[cfg(windows)] {
            assert_eq!(cache_dir(), home_dir.join(format!("AppData\\Local\\ashide\\Ashide{suffix}\\cache")));
        } else {
            unimplemented!("Need to update tests for current platform!");
        }
    }
}

#[test]
fn test_state_dir_path() {
    let home_dir = home_dir().expect("Should be able to compute home directory");
    let suffix = data_profile_suffix();
    cfg_if::cfg_if! {
        // ChannelState, by default, is configured for Channel::Oss.
        if #[cfg(target_os = "macos")] {
            assert_eq!(state_dir(), home_dir.join(format!("Library/Application Support/dev.ashide.Ashide{suffix}")));
        } else if #[cfg(any(target_os = "linux", target_os = "freebsd"))] {
            assert_eq!(state_dir(), home_dir.join(format!(".local/state/ashide{suffix}")));
        } else if #[cfg(windows)] {
            assert_eq!(state_dir(), home_dir.join(format!("AppData\\Local\\ashide\\Ashide{suffix}\\data")));
        } else {
            unimplemented!("Need to update tests for current platform!");
        }
    }
}

#[test]
fn test_oss_secure_state_dir_is_disabled() {
    // ChannelState 默认是 Channel::Oss。Ashide 不应该探测官方 App Group,
    // 否则 macOS 会把它识别成访问其他 App 数据并在每次启动时弹权限窗。
    assert_eq!(secure_state_dir(), None);
}

#[test]
fn test_project_path_for_ashide_dev_app_id() {
    // Covers the `starts_with("Ashide")` branch in `project_dirs_for_app_id` on Linux,
    // which maps suffixed application names like `AshideDev` to a dashed lowercase
    // directory matching the Linux package name (e.g. `ashide-dev`).
    let project_dirs = project_dirs_for_app_id(AppId::new("dev", "ashide", "AshideDev"), None)
        .expect("should be able to compute project dirs");
    cfg_if::cfg_if! {
        if #[cfg(target_os = "macos")] {
            assert_eq!(project_dirs.project_path(), "dev.ashide.AshideDev");
        } else if #[cfg(any(target_os = "linux", target_os = "freebsd"))] {
            assert_eq!(project_dirs.project_path(), "ashide-dev");
        } else if #[cfg(windows)] {
            assert_eq!(project_dirs.project_path(), "ashide\\AshideDev");
        } else {
            unimplemented!("Need to update tests for current platform!");
        }
    }
}

#[test]
fn test_project_path_for_oss_app_id() {
    let project_dirs = project_dirs_for_app_id(AppId::new("dev", "ashide", "Ashide"), None)
        .expect("should be able to compute project dirs");
    cfg_if::cfg_if! {
        if #[cfg(target_os = "macos")] {
            assert_eq!(project_dirs.project_path(), "dev.ashide.Ashide");
        } else if #[cfg(any(target_os = "linux", target_os = "freebsd"))] {
            assert_eq!(project_dirs.project_path(), "ashide");
        } else if #[cfg(windows)] {
            assert_eq!(project_dirs.project_path(), "ashide\\Ashide");
        } else {
            unimplemented!("Need to update tests for current platform!");
        }
    }
}
