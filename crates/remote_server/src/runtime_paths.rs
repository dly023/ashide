use warp_core::channel::{Channel, ChannelState};

/// 返回远端二进制安装目录,按 channel 隔离。
///
/// - stable:      `~/.ashide/remote-server`
/// - preview:     `~/.ashide-preview/remote-server`
/// - dev:         `~/.ashide-dev/remote-server`
/// - local:       `~/.ashide-local/remote-server`
/// - integration: `~/.ashide-dev/remote-server`
/// - oss:         `~/.ashide/remote-server`
pub fn remote_server_dir() -> String {
    let warp_dir = match ChannelState::channel() {
        Channel::Stable => ".ashide",
        Channel::Preview => ".ashide-preview",
        Channel::Dev | Channel::Integration => ".ashide-dev",
        Channel::Local => ".ashide-local",
        Channel::Oss => ".ashide",
    };
    format!("~/{warp_dir}/remote-server")
}

/// 返回可安全放入路径的 remote-server identity key 目录名。
///
/// identity key 不是密钥,但可能包含路径中不安全或有歧义的字节。
/// 保留 ASCII 字母数字以及 `-` / `_`,其他 UTF-8 字节做百分号编码。
pub fn remote_server_identity_dir_name(identity_key: &str) -> String {
    if identity_key.is_empty() {
        return "empty".to_string();
    }

    let mut encoded = String::with_capacity(identity_key.len());
    for byte in identity_key.bytes() {
        match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' => {
                encoded.push(byte as char);
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

/// 返回按 identity 隔离的远端目录,用于 daemon socket 和 PID 文件。
pub fn remote_server_daemon_dir(identity_key: &str) -> String {
    format!(
        "{}/{}",
        remote_server_dir(),
        remote_server_identity_dir_name(identity_key)
    )
}

/// 返回远端 remote-server 二进制文件名。
pub fn binary_name() -> &'static str {
    ChannelState::channel().cli_command_name()
}
