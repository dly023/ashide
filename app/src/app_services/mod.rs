//! Functionality relating to services that the application provides
//! to the host system.
//!
//! For example, on macOS, this module sets up integrations with
//! Finder such that the user can open a new Ashide tab or window
//! in a given directory.

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
pub mod linux;
#[cfg(target_os = "macos")]
pub mod mac;
#[cfg(windows)]
pub mod windows;

use warpui::AppContext;

#[cfg(any(
    target_os = "macos",
    target_os = "linux",
    target_os = "freebsd",
    windows
))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesktopSingletonArbitration {
    ContinueAsOwner,
    ForwardedToOwner,
}

/// 在任何共享日志、持久化或后台服务初始化前完成桌面 GUI ownership 仲裁。
#[cfg(any(
    target_os = "macos",
    target_os = "linux",
    target_os = "freebsd",
    windows
))]
pub fn arbitrate_desktop_gui_singleton(
    args: &warp_cli::AppArgs,
) -> anyhow::Result<DesktopSingletonArbitration> {
    #[cfg(target_os = "macos")]
    return match mac::pass_startup_args_to_existing_instance(args) {
        Ok(()) => Ok(DesktopSingletonArbitration::ForwardedToOwner),
        Err(mac::StartupArgsForwardingError::NoExistingInstance) => {
            Ok(DesktopSingletonArbitration::ContinueAsOwner)
        }
        Err(error) => {
            Err(anyhow::Error::from(error).context("macOS GUI singleton startup failed closed"))
        }
    };

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    return match linux::pass_startup_args_to_existing_instance(args) {
        Ok(()) => Ok(DesktopSingletonArbitration::ForwardedToOwner),
        Err(
            linux::StartupArgsForwardingError::NoExistingInstance
            | linux::StartupArgsForwardingError::IgnoredAfterAutoUpdate,
        ) => Ok(DesktopSingletonArbitration::ContinueAsOwner),
        Err(error) => {
            Err(anyhow::Error::from(error).context("Linux GUI singleton startup failed closed"))
        }
    };

    #[cfg(windows)]
    return match windows::pass_startup_args_to_existing_instance(args) {
        Ok(()) => Ok(DesktopSingletonArbitration::ForwardedToOwner),
        Err(
            windows::StartupArgsForwardingError::NoExistingInstance
            | windows::StartupArgsForwardingError::IgnoredAfterAutoUpdate,
        ) => Ok(DesktopSingletonArbitration::ContinueAsOwner),
        Err(error) => {
            Err(anyhow::Error::from(error).context("Windows GUI singleton startup failed closed"))
        }
    };
}

pub fn init_desktop_gui_owner(_ctx: &mut AppContext) {
    log::info!("Initializing app services");

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    linux::init(_ctx);
    #[cfg(target_os = "macos")]
    mac::init(_ctx);
    #[cfg(windows)]
    windows::init(_ctx);
}

pub fn teardown_desktop_gui_owner(_ctx: &mut AppContext) {
    log::info!("Tearing down app services...");

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    linux::teardown(_ctx);
    #[cfg(target_os = "macos")]
    mac::teardown(_ctx);
}
