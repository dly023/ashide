use std::{
    env,
    ffi::OsString,
    fs::{self, File},
    net::{TcpListener, TcpStream},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Child,
    thread,
    time::{Duration, Instant},
};

use anyhow::{bail, Context, Result};
use command::{blocking::Command, Stdio};
use warp::integration_testing::subshell::util::{
    SSH_FIXTURE_KNOWN_HOSTS_ENV, SSH_FIXTURE_PRIVATE_KEY_ENV, SSH_FIXTURE_PROXY_COMMAND_ENV,
    SSH_FIXTURE_READY_MARKER, SSH_FIXTURE_READY_MARKER_ENV, SSH_FIXTURE_USER_HOST_ENV,
};

const SSHD_PATH: &str = "/usr/sbin/sshd";
const SSH_KEYGEN_PATH: &str = "/usr/bin/ssh-keygen";
const NC_PATH: &str = "/usr/bin/nc";
const ZSH_PATH: &str = "/bin/zsh";

pub struct SshFixture {
    root: PathBuf,
    child: Option<Child>,
    previous_env: Vec<(&'static str, Option<OsString>)>,
}

impl SshFixture {
    pub fn start() -> Result<Self> {
        ensure_executable(SSHD_PATH)?;
        ensure_executable(SSH_KEYGEN_PATH)?;
        ensure_executable(NC_PATH)?;
        ensure_executable(ZSH_PATH)?;

        let root = env::temp_dir().join(format!(
            "ashide-ssh-fixture-{}-{:016x}",
            std::process::id(),
            rand::random::<u64>()
        ));
        fs::create_dir(&root)
            .with_context(|| format!("failed to create SSH fixture root at {root:?}"))?;
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;

        let result = Self::start_in_root(root.clone());
        if result.is_err() {
            let _ = fs::remove_dir_all(&root);
        }
        result
    }

    fn start_in_root(root: PathBuf) -> Result<Self> {
        let home = root.join("home");
        fs::create_dir(&home)?;
        fs::set_permissions(&home, fs::Permissions::from_mode(0o700))?;
        fs::write(home.join(".zshenv"), "")?;
        fs::write(home.join(".zprofile"), "")?;
        fs::write(
            home.join(".zshrc"),
            format!("print -r -- {SSH_FIXTURE_READY_MARKER}\nPS1='ashide-ssh-fixture% '\n"),
        )?;
        fs::write(home.join(".hushlogin"), "")?;

        let host_key = root.join("host_key");
        let client_key = root.join("client_key");
        generate_key(&host_key)?;
        generate_key(&client_key)?;

        let authorized_keys = root.join("authorized_keys");
        fs::copy(client_key.with_extension("pub"), &authorized_keys)?;
        fs::set_permissions(&authorized_keys, fs::Permissions::from_mode(0o600))?;

        let known_hosts = root.join("known_hosts");
        let host_public_key = fs::read_to_string(host_key.with_extension("pub"))?;
        let mut host_public_key_parts = host_public_key.split_whitespace();
        let algorithm = host_public_key_parts
            .next()
            .context("generated host public key is missing its algorithm")?;
        let key = host_public_key_parts
            .next()
            .context("generated host public key is missing its key data")?;
        fs::write(
            &known_hosts,
            format!("ashide-ssh-fixture {algorithm} {key}\n"),
        )?;

        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let port = listener.local_addr()?.port();
        drop(listener);

        let proxy_command = root.join("proxy-command");
        fs::write(
            &proxy_command,
            format!("#!/bin/sh\nexec {NC_PATH} 127.0.0.1 {port}\n"),
        )?;
        fs::set_permissions(&proxy_command, fs::Permissions::from_mode(0o700))?;

        let username = whoami::username();
        let sshd_config = root.join("sshd_config");
        fs::write(
            &sshd_config,
            format!(
                "Port {port}\n\
                 ListenAddress 127.0.0.1\n\
                 AddressFamily inet\n\
                 HostKey {}\n\
                 PidFile {}\n\
                 AuthorizedKeysFile {}\n\
                 PubkeyAuthentication yes\n\
                 PasswordAuthentication no\n\
                 KbdInteractiveAuthentication no\n\
                 ChallengeResponseAuthentication no\n\
                 AuthenticationMethods publickey\n\
                 UsePAM no\n\
                 StrictModes no\n\
                 PermitRootLogin no\n\
                 AllowUsers {username}\n\
                 PrintMotd no\n\
                 PrintLastLog no\n\
                 UseDNS no\n\
                 AllowTcpForwarding no\n\
                 X11Forwarding no\n\
                 PermitTunnel no\n\
                 PermitUserEnvironment no\n\
                 SetEnv HOME={} ZDOTDIR={} HISTFILE=/dev/null SHELL={ZSH_PATH}\n\
                 LogLevel ERROR\n",
                host_key.display(),
                root.join("sshd.pid").display(),
                authorized_keys.display(),
                home.display(),
                home.display(),
            ),
        )?;

        let output = Command::new(SSHD_PATH)
            .args(["-t", "-f"])
            .arg(&sshd_config)
            .output()
            .context("failed to validate hermetic sshd configuration")?;
        if !output.status.success() {
            bail!(
                "hermetic sshd configuration is invalid: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        let log_path = root.join("sshd.log");
        let log = File::create(&log_path)?;
        let mut child = Command::new(SSHD_PATH)
            .args(["-D", "-e", "-f"])
            .arg(&sshd_config)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log.try_clone()?))
            .stderr(Stdio::from(log))
            .spawn()
            .context("failed to start hermetic sshd")?;

        wait_until_ready(&mut child, port, &log_path)?;

        let user_host = format!("{username}@ashide-ssh-fixture");
        let previous_env = set_fixture_env([
            (SSH_FIXTURE_USER_HOST_ENV, user_host.into()),
            (
                SSH_FIXTURE_PRIVATE_KEY_ENV,
                client_key.as_os_str().to_owned(),
            ),
            (
                SSH_FIXTURE_KNOWN_HOSTS_ENV,
                known_hosts.as_os_str().to_owned(),
            ),
            (
                SSH_FIXTURE_PROXY_COMMAND_ENV,
                proxy_command.as_os_str().to_owned(),
            ),
            (
                SSH_FIXTURE_READY_MARKER_ENV,
                SSH_FIXTURE_READY_MARKER.into(),
            ),
        ]);

        Ok(Self {
            root,
            child: Some(child),
            previous_env,
        })
    }

    pub fn shutdown(&mut self) {
        restore_fixture_env(std::mem::take(&mut self.previous_env));
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

impl Drop for SshFixture {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn ensure_executable(path: &str) -> Result<()> {
    let metadata =
        fs::metadata(path).with_context(|| format!("required executable is missing: {path}"))?;
    if metadata.permissions().mode() & 0o111 == 0 {
        bail!("required path is not executable: {path}");
    }
    Ok(())
}

fn generate_key(path: &Path) -> Result<()> {
    let output = Command::new(SSH_KEYGEN_PATH)
        .args(["-q", "-t", "ed25519", "-N", "", "-f"])
        .arg(path)
        .output()
        .with_context(|| format!("failed to generate SSH key at {path:?}"))?;
    if !output.status.success() {
        bail!(
            "ssh-keygen failed for {path:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

fn wait_until_ready(child: &mut Child, port: u16, log_path: &Path) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return Ok(());
        }
        if let Some(status) = child.try_wait()? {
            let log = fs::read_to_string(log_path).unwrap_or_default();
            bail!("hermetic sshd exited with {status}: {log}");
        }
        if Instant::now() >= deadline {
            let log = fs::read_to_string(log_path).unwrap_or_default();
            bail!("timed out waiting for hermetic sshd: {log}");
        }
        thread::sleep(Duration::from_millis(25));
    }
}

fn set_fixture_env<const N: usize>(
    values: [(&'static str, OsString); N],
) -> Vec<(&'static str, Option<OsString>)> {
    values
        .into_iter()
        .map(|(key, value)| {
            let previous = env::var_os(key);
            env::set_var(key, value);
            (key, previous)
        })
        .collect()
}

fn restore_fixture_env(previous: Vec<(&'static str, Option<OsString>)>) {
    for (key, value) in previous {
        match value {
            Some(value) => env::set_var(key, value),
            None => env::remove_var(key),
        }
    }
}
