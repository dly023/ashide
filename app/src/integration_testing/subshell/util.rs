use std::env;

pub const SSH_FIXTURE_USER_HOST_ENV: &str = "ASHIDE_INTEGRATION_SSH_USER_HOST";
pub const SSH_FIXTURE_PRIVATE_KEY_ENV: &str = "ASHIDE_INTEGRATION_SSH_PRIVATE_KEY";
pub const SSH_FIXTURE_KNOWN_HOSTS_ENV: &str = "ASHIDE_INTEGRATION_SSH_KNOWN_HOSTS";
pub const SSH_FIXTURE_PROXY_COMMAND_ENV: &str = "ASHIDE_INTEGRATION_SSH_PROXY_COMMAND";
pub const SSH_FIXTURE_READY_MARKER_ENV: &str = "ASHIDE_INTEGRATION_SSH_READY_MARKER";
pub const SSH_FIXTURE_READY_MARKER: &str = "ASHIDE_SSH_FIXTURE_READY";

fn fixture_value(key: &str, shell: &str) -> String {
    let value = env::var(key).unwrap_or_else(|_| {
        panic!("SSH fixture is not configured for remote shell {shell:?}: {key}")
    });
    assert!(
        !value.chars().any(char::is_whitespace),
        "SSH fixture value must not contain whitespace: {key}={value:?}"
    );
    value
}

/// 返回 hermetic fixture 的用户与主机标识。
pub fn user_host(shell: &str) -> String {
    fixture_value(SSH_FIXTURE_USER_HOST_ENV, shell)
}

pub fn ssh_fixture_ready_marker(shell: &str) -> String {
    fixture_value(SSH_FIXTURE_READY_MARKER_ENV, shell)
}

/// 构造完全隔离于开发者 SSH 配置的连接命令。
pub fn ssh_command(shell: &str, should_use_ssh_wrapper: bool) -> String {
    let private_key = fixture_value(SSH_FIXTURE_PRIVATE_KEY_ENV, shell);
    let known_hosts = fixture_value(SSH_FIXTURE_KNOWN_HOSTS_ENV, shell);
    let proxy_command = fixture_value(SSH_FIXTURE_PROXY_COMMAND_ENV, shell);

    [
        if should_use_ssh_wrapper {
            "ssh".to_owned()
        } else {
            "command ssh".to_owned()
        },
        "-F /dev/null".to_owned(),
        format!("-i {private_key}"),
        "-o IdentitiesOnly=yes".to_owned(),
        "-o BatchMode=yes".to_owned(),
        "-o PasswordAuthentication=no".to_owned(),
        "-o KbdInteractiveAuthentication=no".to_owned(),
        "-o StrictHostKeyChecking=yes".to_owned(),
        format!("-o UserKnownHostsFile={known_hosts}"),
        "-o GlobalKnownHostsFile=/dev/null".to_owned(),
        "-o HostKeyAlias=ashide-ssh-fixture".to_owned(),
        format!("-o ProxyCommand={proxy_command}"),
        user_host(shell),
    ]
    .join(" ")
}
