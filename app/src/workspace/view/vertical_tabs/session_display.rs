use crate::app_state::{CliAgentSessionOrigin, WorkspaceSessionKind, WorkspaceSessionSnapshot};
use crate::terminal::CLIAgent;
use warp_util::path::user_friendly_path;

pub(in crate::workspace::view) fn vtab_session_row_key(
    session_id: &str,
    environment_authority_key: Option<&str>,
) -> String {
    format!(
        "{}::{}",
        environment_authority_key
            .filter(|authority| !authority.trim().is_empty())
            .unwrap_or("local"),
        session_id
    )
}

pub(in crate::workspace::view) fn vtab_session_row_position_id(
    session_id: &str,
    environment_authority_key: Option<&str>,
) -> String {
    format!(
        "vertical_tabs:session_row:{}",
        vtab_session_row_key(session_id, environment_authority_key)
    )
}

fn restored_session_cli_agent(session: &WorkspaceSessionSnapshot) -> Option<CLIAgent> {
    session
        .cli_agent
        .as_deref()
        .map(CLIAgent::from_serialized_name)
        .filter(|agent| !matches!(agent, CLIAgent::Unknown))
}

fn short_identity_from_raw(raw: &str) -> Option<String> {
    let token = raw
        .trim()
        .rsplit(|ch| matches!(ch, '/' | '\\' | ':' | '#'))
        .find(|part| !part.trim().is_empty())?
        .trim()
        .trim_matches(|ch: char| !ch.is_ascii_alphanumeric() && ch != '-' && ch != '_');
    if token.is_empty() {
        return None;
    }

    if token.chars().count() <= 12 {
        return Some(token.to_owned());
    }

    let hyphen_tail = token.rsplit('-').find(|part| part.chars().count() >= 4);
    if let Some(tail) = hyphen_tail {
        return Some(tail_chars(tail, 8));
    }

    Some(middle_ellipsize(token, 4, 4))
}

fn tail_chars(value: &str, max_chars: usize) -> String {
    let chars: Vec<char> = value.chars().collect();
    if chars.len() <= max_chars {
        return value.to_owned();
    }
    chars[chars.len() - max_chars..].iter().collect()
}

fn middle_ellipsize(value: &str, prefix_chars: usize, suffix_chars: usize) -> String {
    let chars: Vec<char> = value.chars().collect();
    if chars.len() <= prefix_chars + suffix_chars + 1 {
        return value.to_owned();
    }
    let prefix: String = chars[..prefix_chars].iter().collect();
    let suffix: String = chars[chars.len() - suffix_chars..].iter().collect();
    format!("{prefix}…{suffix}")
}

fn label_suggests_session_bridge_fork(label: Option<&str>) -> bool {
    let Some(label) = label else {
        return false;
    };
    let label = label.trim().to_lowercase();
    !label.is_empty()
        && (label.contains("(fork)")
            || label.contains(" fork")
            || label.contains("fork ")
            || label.contains("已 fork")
            || label.contains("forked"))
}

pub(in crate::workspace::view) fn restored_session_short_identity(
    session: &WorkspaceSessionSnapshot,
) -> Option<String> {
    session
        .cli_agent_session_id
        .as_deref()
        .filter(|id| !id.trim().is_empty())
        .or(session.active_conversation_id.as_deref())
        .or_else(|| {
            session
                .conversation_ids
                .iter()
                .rev()
                .find(|id| !id.trim().is_empty())
                .map(String::as_str)
        })
        .or(Some(session.id.as_str()))
        .and_then(short_identity_from_raw)
}

fn restored_session_source_cue(session: &WorkspaceSessionSnapshot) -> String {
    if session.id.starts_with("tab:") || session.is_active {
        return crate::t!("workspace-session-navigator-cue-live");
    }
    if label_suggests_session_bridge_fork(session.label.as_deref()) {
        return crate::t!("workspace-session-navigator-cue-fork");
    }
    match session.cli_agent_origin.as_ref() {
        Some(CliAgentSessionOrigin::PluginObserved) => {
            crate::t!("workspace-session-navigator-cue-indexed")
        }
        Some(CliAgentSessionOrigin::CommandDetected) => {
            crate::t!("workspace-session-navigator-cue-detected")
        }
        None => crate::t!("workspace-session-navigator-cue-source"),
    }
}

fn restored_session_disambiguator(session: &WorkspaceSessionSnapshot) -> Option<String> {
    restored_session_short_identity(session)
        .map(|identity| format!("{} #{identity}", restored_session_source_cue(session)))
}

pub(crate) fn restored_session_label(session: &WorkspaceSessionSnapshot) -> String {
    // UIREQ-014:agent 身份由行图标(+ aria/tooltip)标识,标题不再冗余地加
    // "Claude Code ·" / "Codex ·" 前缀。fallback 链:
    //   1. 会话标题(session.label;构造期无标题时已并入「首句截取」,见
    //      historical_ashide_conversation_sessions / cli_agent_session_index);
    //   2. agent 名(Codex / Claude Code)兜底;
    //   3. 会话 kind 兜底。
    if let Some(label) = session.label.as_deref().filter(|label| !label.is_empty()) {
        return label.to_string();
    }

    if let Some(agent) = restored_session_cli_agent(session) {
        return agent.display_name().to_string();
    }

    match session.kind {
        WorkspaceSessionKind::Terminal => crate::t!("workspace-session-navigator-terminal"),
        WorkspaceSessionKind::AgentTerminal => session
            .cli_agent
            .clone()
            .or_else(|| session.cli_command.clone())
            .unwrap_or_else(|| crate::t!("workspace-session-navigator-terminal")),
        WorkspaceSessionKind::Welcome => crate::t!("workspace-session-navigator-welcome"),
        WorkspaceSessionKind::Other => crate::t!("workspace-session-navigator-session"),
    }
}

fn restored_session_uses_environment_runtime(session: &WorkspaceSessionSnapshot) -> bool {
    crate::workspace::environment_runtime::session_authority_uses_runtime_environment(
        session.environment_authority_key.as_deref(),
    )
}

pub(in crate::workspace::view) fn restored_session_root_label(
    session: &WorkspaceSessionSnapshot,
) -> Option<String> {
    session
        .cwd
        .as_deref()
        .or(session.startup_directory.as_deref())
        .filter(|root| !root.trim().is_empty())
        .map(|root| {
            if restored_session_uses_environment_runtime(session) {
                root.to_owned()
            } else {
                user_friendly_path(root, None).into_owned()
            }
        })
}

pub(in crate::workspace::view) fn restored_session_environment_label(
    session: &WorkspaceSessionSnapshot,
) -> Option<String> {
    let authority = session
        .environment_authority_key
        .as_deref()
        .filter(|authority| !authority.trim().is_empty())?;
    let label =
        crate::workspace::environment_runtime::session_environment_display_label(authority)?;
    Some(crate::t!(
        "workspace-session-preview-environment-runtime",
        environment = label
    ))
}

pub(in crate::workspace::view) fn restored_session_detail(
    session: &WorkspaceSessionSnapshot,
) -> String {
    let root = restored_session_root_label(session)
        .or_else(|| restored_session_environment_label(session))
        .unwrap_or_else(|| crate::t!("workspace-session-navigator-no-root"));
    // The agent identity is already conveyed by the row icon and the title
    // (restored_session_label); the subtitle leads with the working directory
    // and never repeats the agent name. Only an optional disambiguator follows.
    if let Some(disambiguator) = restored_session_disambiguator(session) {
        format!("{root} · {disambiguator}")
    } else {
        root
    }
}

pub(in crate::workspace::view) fn restored_session_state_label(
    session: &WorkspaceSessionSnapshot,
) -> &'static str {
    if session.is_active {
        crate::t_static!("workspace-session-navigator-state-active")
    } else {
        crate::t_static!("workspace-session-navigator-state-restored")
    }
}

pub(crate) fn restored_session_search_fragments(session: &WorkspaceSessionSnapshot) -> Vec<String> {
    let mut fragments = vec![
        restored_session_label(session),
        restored_session_detail(session),
        restored_session_state_label(session).to_string(),
    ];
    if let Some(agent) = &session.cli_agent {
        fragments.push(agent.clone());
    }
    if let Some(command) = &session.cli_command {
        fragments.push(command.clone());
    }
    if let Some(identity) = restored_session_short_identity(session) {
        fragments.push(identity);
    }
    if let Some(root) = restored_session_root_label(session) {
        fragments.push(root);
    }
    if let Some(authority) = &session.environment_authority_key {
        fragments.push(authority.clone());
    }
    if let Some(session_id) = &session.cli_agent_session_id {
        fragments.push(session_id.clone());
    }
    if let Some(conversation_id) = &session.active_conversation_id {
        fragments.push(conversation_id.clone());
    }
    fragments.extend(session.conversation_ids.iter().cloned());
    fragments
}

#[cfg(test)]
mod tests {
    use crate::app_state::{CliAgentSessionOrigin, WorkspaceSessionKind, WorkspaceSessionSnapshot};
    use crate::terminal::CLIAgent;

    use super::{
        restored_session_detail, restored_session_label, restored_session_search_fragments,
        restored_session_short_identity, vtab_session_row_key,
    };

    fn agent_session(agent: CLIAgent, label: Option<&str>) -> WorkspaceSessionSnapshot {
        WorkspaceSessionSnapshot {
            id: "external:test-session.jsonl".to_string(),
            kind: WorkspaceSessionKind::AgentTerminal,
            label: label.map(str::to_owned),
            environment_authority_key: Some("local".to_string()),
            cwd: Some("/Users/admin/project".to_string()),
            startup_directory: None,
            cli_agent: Some(agent.to_serialized_name()),
            cli_command: Some(agent.command_prefix().to_string()),
            cli_agent_origin: Some(CliAgentSessionOrigin::PluginObserved),
            conversation_ids: Vec::new(),
            active_conversation_id: None,
            cli_agent_session_id: Some("provider-session-1".to_string()),
            is_active: false,
            is_pinned: false,
            updated_at_unix_ms: None,
        }
    }

    #[test]
    fn restored_session_label_uses_title_without_agent_prefix() {
        // UIREQ-014: 标题直接用 session.label,不再加 "Claude Code ·" 前缀;
        // agent(这里是 Claude)由行图标标识。即使标题里提到别的 agent 名,
        // 也按用户给的标题原样显示。
        let session = agent_session(CLIAgent::Claude, Some("Codex fork from planning"));

        assert_eq!(restored_session_label(&session), "Codex fork from planning");
        let detail = restored_session_detail(&session);
        assert!(
            !detail.starts_with("Claude Code"),
            "detail must not repeat the agent name shown by the icon + title: {detail}"
        );
        assert!(
            detail.contains("project"),
            "detail must lead with cwd: {detail}"
        );
    }

    #[test]
    fn restored_session_label_does_not_duplicate_same_agent_prefix() {
        let session = agent_session(CLIAgent::Codex, Some("Codex fork from planning"));

        assert_eq!(restored_session_label(&session), "Codex fork from planning");
        let detail = restored_session_detail(&session);
        assert!(
            !detail.starts_with("Codex"),
            "detail must not repeat the agent name (icon + title already show it): {detail}"
        );
    }

    #[test]
    fn restored_session_label_uses_target_agent_when_no_source_title_exists() {
        let session = agent_session(CLIAgent::Claude, None);

        assert_eq!(restored_session_label(&session), "Claude Code");
    }

    #[test]
    fn restored_session_detail_disambiguates_duplicate_agent_sessions_with_same_cwd() {
        let mut first = agent_session(CLIAgent::Codex, Some("Planning"));
        first.cli_agent_session_id = Some("11111111-2222-3333-4444-aaaaaaaaaaaa".to_string());
        let mut second = agent_session(CLIAgent::Codex, Some("Planning"));
        second.cli_agent_session_id = Some("11111111-2222-3333-4444-bbbbbbbbbbbb".to_string());

        let first_detail = restored_session_detail(&first);
        let second_detail = restored_session_detail(&second);

        assert_ne!(first_detail, second_detail);
        assert!(!first_detail.starts_with("Codex"), "{first_detail}");
        assert!(!second_detail.starts_with("Codex"), "{second_detail}");
        assert!(first_detail.contains("#aaaaaaaa"), "{first_detail}");
        assert!(second_detail.contains("#bbbbbbbb"), "{second_detail}");
    }

    #[test]
    fn restored_session_detail_keeps_target_agent_primary_for_cross_agent_fork() {
        let mut session = agent_session(CLIAgent::Claude, Some("Codex planning (fork)"));
        session.cli_agent_session_id = Some("claude-derived-session-abc123".to_string());

        let label = restored_session_label(&session);
        let detail = restored_session_detail(&session);

        // UIREQ-014: 标题不再加 agent 前缀(图标已标识 Claude)。
        assert_eq!(label, "Codex planning (fork)");
        assert!(!detail.starts_with("Claude Code"), "{detail}");
        assert!(detail.contains("fork"), "{detail}");
        assert!(detail.contains("#abc123"), "{detail}");
    }

    #[test]
    fn restored_session_short_identity_falls_back_to_source_id_tail() {
        let mut session = agent_session(CLIAgent::Codex, Some("No provider id"));
        session.id = "external-index:codex:abcdef1234567890".to_string();
        session.cli_agent_session_id = None;

        assert_eq!(
            restored_session_short_identity(&session).as_deref(),
            Some("34567890")
        );
        assert!(restored_session_detail(&session).contains("#34567890"));
    }

    #[test]
    fn restored_session_search_fragments_include_stable_identity_and_authority() {
        let mut session = agent_session(CLIAgent::Claude, Some("Remote plan"));
        session.environment_authority_key = Some("ssh:ssh-config:dnyx216".to_string());
        session.cli_agent_session_id = Some("remote-claude-session-fedcba".to_string());
        session.active_conversation_id = Some("conversation-needle".to_string());

        let fragments = restored_session_search_fragments(&session);

        assert!(fragments.iter().any(|fragment| fragment == "fedcba"));
        assert!(fragments
            .iter()
            .any(|fragment| fragment == "ssh:ssh-config:dnyx216"));
        assert!(fragments
            .iter()
            .any(|fragment| fragment == "conversation-needle"));
    }

    #[test]
    fn session_row_key_includes_environment_authority() {
        assert_ne!(
            vtab_session_row_key("tab:0:leaf:0", Some("local")),
            vtab_session_row_key("tab:0:leaf:0", Some("ssh:ssh-config:dnyx216"))
        );
        assert_eq!(
            vtab_session_row_key("tab:0:leaf:0", None),
            "local::tab:0:leaf:0"
        );
    }
}
