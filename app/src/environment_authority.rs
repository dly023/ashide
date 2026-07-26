//! Environment authority 的唯一解析边界。
//!
//! `authority_key` 是 Environment 的稳定身份，同时携带 backend capability 与
//! provider connection reference。所有 local/runtime 分类、导航去重、SSH
//! connection ref 和显示名都必须从本模块的类型化结果派生，禁止 consumer
//! 再次用 `starts_with` / `strip_prefix` 维护第二套字符串协议。

pub(crate) const TERMINAL_BOOTSTRAP_AUTHORITY: &str = "local";
const TERMINAL_BOOTSTRAP_AUTHORITY_PREFIX: &str = "local:";
const SAVED_SSH_AUTHORITY_PREFIX: &str = "ssh:";
const SSH_CONFIG_CONNECTION_PREFIX: &str = "ssh-config:";

pub(crate) fn saved_ssh_authority(connection_ref: &str) -> String {
    format!("{SAVED_SSH_AUTHORITY_PREFIX}{connection_ref}")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ParsedEnvironmentAuthority<'a> {
    TerminalBootstrap {
        authority: &'a str,
        root: Option<&'a str>,
    },
    SavedSsh {
        authority: &'a str,
        connection_ref: &'a str,
        display_label: &'a str,
    },
    Runtime {
        authority: &'a str,
    },
}

impl<'a> ParsedEnvironmentAuthority<'a> {
    pub(crate) fn parse(authority: &'a str) -> Self {
        if authority == TERMINAL_BOOTSTRAP_AUTHORITY {
            return Self::TerminalBootstrap {
                authority,
                root: None,
            };
        }
        if let Some(root) = authority.strip_prefix(TERMINAL_BOOTSTRAP_AUTHORITY_PREFIX) {
            return Self::TerminalBootstrap {
                authority,
                root: Some(root),
            };
        }
        if let Some(connection_ref) = authority.strip_prefix(SAVED_SSH_AUTHORITY_PREFIX) {
            return Self::SavedSsh {
                authority,
                connection_ref,
                display_label: connection_ref
                    .strip_prefix(SSH_CONFIG_CONNECTION_PREFIX)
                    .unwrap_or(connection_ref),
            };
        }
        if authority.starts_with(SSH_CONFIG_CONNECTION_PREFIX) {
            return Self::SavedSsh {
                authority,
                connection_ref: authority,
                display_label: authority
                    .strip_prefix(SSH_CONFIG_CONNECTION_PREFIX)
                    .expect("checked ssh-config authority prefix"),
            };
        }
        Self::Runtime { authority }
    }

    pub(crate) fn authority(self) -> &'a str {
        match self {
            Self::TerminalBootstrap { authority, .. }
            | Self::SavedSsh { authority, .. }
            | Self::Runtime { authority } => authority,
        }
    }

    pub(crate) fn uses_terminal_bootstrap(self) -> bool {
        matches!(self, Self::TerminalBootstrap { .. })
    }

    pub(crate) fn uses_runtime_environment(self) -> bool {
        !self.uses_terminal_bootstrap()
    }

    pub(crate) fn navigation_key(self) -> &'a str {
        if self.uses_terminal_bootstrap() {
            TERMINAL_BOOTSTRAP_AUTHORITY
        } else {
            self.authority()
        }
    }

    pub(crate) fn runtime_connection_ref(self) -> Option<&'a str> {
        match self {
            Self::SavedSsh { connection_ref, .. } => Some(connection_ref),
            Self::TerminalBootstrap { .. } | Self::Runtime { .. } => None,
        }
    }

    pub(crate) fn display_label(self) -> Option<&'a str> {
        let label = match self {
            Self::TerminalBootstrap { .. } => return None,
            Self::SavedSsh { display_label, .. } => display_label,
            Self::Runtime { authority } => authority.trim(),
        };
        (!label.is_empty()).then_some(label)
    }

    pub(crate) fn matches(self, other: Self) -> bool {
        self.navigation_key() == other.navigation_key()
    }
}

pub(crate) fn session_authority_or_terminal_bootstrap(session_authority: Option<&str>) -> &str {
    session_authority.unwrap_or(TERMINAL_BOOTSTRAP_AUTHORITY)
}

pub(crate) fn session_authority_matches(
    session_authority: Option<&str>,
    current_authority: &str,
) -> bool {
    ParsedEnvironmentAuthority::parse(session_authority_or_terminal_bootstrap(session_authority))
        .matches(ParsedEnvironmentAuthority::parse(current_authority))
}

pub(crate) fn session_authority_uses_runtime_environment(session_authority: Option<&str>) -> bool {
    session_authority.is_some_and(|authority| {
        ParsedEnvironmentAuthority::parse(authority).uses_runtime_environment()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn environment_authority_parser_covers_local_saved_ssh_and_custom_runtime() {
        let local = ParsedEnvironmentAuthority::parse("local");
        assert_eq!(local.navigation_key(), "local");
        assert!(local.uses_terminal_bootstrap());
        assert_eq!(local.runtime_connection_ref(), None);
        assert_eq!(local.display_label(), None);

        let local_root = ParsedEnvironmentAuthority::parse("local:/repo");
        assert_eq!(local_root.navigation_key(), "local");
        assert!(local.matches(local_root));
        assert_eq!(
            local_root,
            ParsedEnvironmentAuthority::TerminalBootstrap {
                authority: "local:/repo",
                root: Some("/repo"),
            }
        );

        let saved_ssh = ParsedEnvironmentAuthority::parse("ssh:ssh-config:remote-fixture-dev");
        assert!(saved_ssh.uses_runtime_environment());
        assert_eq!(
            saved_ssh.navigation_key(),
            "ssh:ssh-config:remote-fixture-dev"
        );
        assert_eq!(
            saved_ssh.runtime_connection_ref(),
            Some("ssh-config:remote-fixture-dev")
        );
        assert_eq!(saved_ssh.display_label(), Some("remote-fixture-dev"));

        let bare_saved_ssh = ParsedEnvironmentAuthority::parse("ssh-config:remote-fixture-dev");
        assert_eq!(
            bare_saved_ssh.runtime_connection_ref(),
            Some("ssh-config:remote-fixture-dev")
        );
        assert_eq!(bare_saved_ssh.display_label(), Some("remote-fixture-dev"));

        let custom_runtime = ParsedEnvironmentAuthority::parse("locality:remote");
        assert!(custom_runtime.uses_runtime_environment());
        assert_eq!(custom_runtime.runtime_connection_ref(), None);
        assert_eq!(custom_runtime.display_label(), Some("locality:remote"));
    }

    #[test]
    fn session_authority_matching_uses_typed_navigation_identity() {
        assert!(session_authority_matches(None, "local:/repo"));
        assert!(session_authority_matches(
            Some("local:/first"),
            "local:/second"
        ));
        assert!(!session_authority_matches(
            Some("ssh:ssh-config:first"),
            "ssh:ssh-config:second"
        ));
    }
}
