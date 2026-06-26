use crate::ai::execution_profiles::{
    ActionPermission, ComputerUsePermission, WriteToPtyPermission,
};
use crate::ai::llms::LLMModelHost;
use crate::{
    auth::UserUid, object_store::ids::StableObjectId, settings::AgentModeCommandExecutionPredicate,
};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::{cmp::Ordering, path::PathBuf};

use super::team::{MembershipRole, Team};

#[derive(Clone, Copy, Hash, Debug, PartialEq, Eq)]
pub struct WorkspaceUid(StableObjectId);
impl From<String> for WorkspaceUid {
    fn from(uid: String) -> Self {
        WorkspaceUid(StableObjectId::from_string_lossy(uid))
    }
}
impl From<WorkspaceUid> for String {
    fn from(workspace_uid: WorkspaceUid) -> String {
        workspace_uid.0.to_string()
    }
}
impl From<StableObjectId> for WorkspaceUid {
    fn from(uid: StableObjectId) -> Self {
        WorkspaceUid(uid)
    }
}

#[derive(Clone, Debug)]
pub struct Workspace {
    pub uid: WorkspaceUid,
    pub name: String,
    pub teams: Vec<Team>,
    pub workspace_policy: WorkspacePolicyMetadata,
    pub settings: WorkspaceSettings,
    pub invite_code: Option<WorkspaceInviteCode>,
    pub invite_link_domain_restrictions: Vec<InviteLinkDomainRestriction>,
    pub pending_email_invites: Vec<EmailInvite>,
    // If local cached metadata marks the team as discoverable, show the discoverability toggle to the team admin
    pub is_eligible_for_discovery: bool,
    pub members: Vec<WorkspaceMember>,
}

impl Workspace {
    pub fn from_local_cache(uid: WorkspaceUid, name: String, teams: Option<Vec<Team>>) -> Self {
        // Derive the workspace policy from the first team's cached policy, if available.
        // This keeps workspace-level local policy consistent with team-level cache data.
        let workspace_policy = teams
            .as_ref()
            .and_then(|t| t.first())
            .map(|team| team.workspace_policy.clone())
            .unwrap_or_default();
        Self {
            uid,
            name,
            teams: teams.unwrap_or_default(),
            workspace_policy,
            settings: Default::default(), // TODO: persistence wrapper instead of default
            invite_code: Default::default(),
            invite_link_domain_restrictions: Default::default(),
            pending_email_invites: Default::default(),
            is_eligible_for_discovery: false,
            members: Default::default(),
        }
    }

    fn get_member_by_email(&self, email: &str) -> Option<&WorkspaceMember> {
        self.members.iter().find(|member| member.email == email)
    }

    pub fn is_workspace_admin(&self, user_email: &str) -> bool {
        self.get_member_by_email(user_email)
            .is_some_and(|member| member.role.is_admin_or_owner())
    }

    pub fn can_be_deleted(&self, current_user_email: &str) -> bool {
        // Current user needs to be an admin and be the only user remaining
        self.is_workspace_admin(current_user_email)
            && self.members.len() == 1
            && self
                .members
                .first()
                .is_some_and(|m| m.email == current_user_email)
    }

    pub fn is_custom_llm_enabled(&self) -> bool {
        self.settings.llm_settings.enabled
    }

    pub fn is_byo_api_key_enabled(&self) -> bool {
        self.workspace_policy.is_byo_api_key_enabled()
    }
}

#[derive(Clone, Debug)]
pub struct WorkspaceInviteCode {
    pub code: String,
}

#[derive(Clone, Eq, PartialEq, Debug)]
pub struct WorkspaceMember {
    pub uid: UserUid,
    pub email: String,
    pub role: MembershipRole,
    pub usage_info: WorkspaceMemberUsageInfo,
}

#[derive(Clone, Eq, PartialEq, Debug)]
pub struct WorkspaceMemberUsageInfo {
    pub is_unlimited: bool,
    pub request_limit: i32,
    pub requests_used_since_last_refresh: i32,
    pub is_request_limit_prorated: bool,
}

impl PartialOrd for WorkspaceMember {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for WorkspaceMember {
    fn cmp(&self, other: &Self) -> Ordering {
        self.email.cmp(&other.email)
    }
}

#[derive(Clone, Eq, PartialEq, Debug)]
pub struct EmailInvite {
    pub invitee_email: String,
    pub expired: bool,
}

impl PartialOrd for EmailInvite {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for EmailInvite {
    fn cmp(&self, other: &Self) -> Ordering {
        self.invitee_email.cmp(&other.invitee_email)
    }
}

#[derive(Clone, Eq, PartialEq, Debug)]
pub struct InviteLinkDomainRestriction {
    pub uid: StableObjectId,
    pub domain: String,
}

impl PartialOrd for InviteLinkDomainRestriction {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for InviteLinkDomainRestriction {
    fn cmp(&self, other: &Self) -> Ordering {
        self.domain.cmp(&other.domain)
    }
}

/// Local representation of feature policy data restored from persisted workspace metadata.
#[derive(Clone, Debug, Copy, Serialize, Deserialize)]
pub struct AiAssistantPolicy {
    pub limit: i64,
    pub is_code_suggestions_toggleable: bool,
    pub is_prompt_suggestions_toggleable: bool,
    pub is_next_command_enabled: bool,
    pub is_voice_enabled: bool,
}
#[derive(Clone, Debug, Copy, Serialize, Deserialize)]
pub struct WorkspaceSizePolicy {
    pub is_unlimited: bool,
    pub limit: i64,
}
#[derive(Clone, Debug, Copy, Serialize, Deserialize)]
pub struct SharedNotebooksPolicy {
    pub is_unlimited: bool,
    pub limit: i64,
}
#[derive(Clone, Debug, Copy, Serialize, Deserialize)]
pub struct SharedWorkflowsPolicy {
    pub is_unlimited: bool,
    pub limit: i64,
}

#[derive(Clone, Debug, Copy, Serialize, Deserialize)]
pub struct SessionSharingPolicy {
    pub is_enabled: bool,
    pub max_session_size: u64,
}

#[derive(Clone, Debug, Copy, Serialize, Deserialize)]
pub struct AIAutonomyPolicy {
    pub is_enabled: bool,
    pub toggleable: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UgcDataCollectionPolicy {
    pub default_setting: UgcCollectionEnablementSetting,
    pub toggleable: bool,
}

#[derive(Clone, Debug, Copy, Serialize, Deserialize)]
pub struct ByoApiKeyPolicy {
    pub enabled: bool,
}

#[derive(Clone, Debug, Copy, Serialize, Deserialize)]
pub struct MultiAdminPolicy {
    pub enabled: bool,
}

#[derive(Clone, Debug, Copy, Serialize, Deserialize)]
pub struct AmbientAgentsPolicy {
    pub max_concurrent_agents: i32,
    pub instance_shape: Option<InstanceShape>,
}

#[derive(Clone, Debug, Copy, Serialize, Deserialize)]
pub struct InstanceShape {
    pub vcpus: i32,
    pub memory_gb: i32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub enum HostEnablementSetting {
    Enforce,
    #[default]
    RespectUserSetting,
}

/// Local representation of persisted local workspace policy.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkspacePolicySet {
    pub name: String,
    pub description: String,
    pub ai_assistant_policy: Option<AiAssistantPolicy>,
    pub workspace_size_policy: Option<WorkspaceSizePolicy>,
    pub shared_notebooks_policy: Option<SharedNotebooksPolicy>,
    pub shared_workflows_policy: Option<SharedWorkflowsPolicy>,
    pub session_sharing_policy: Option<SessionSharingPolicy>,
    pub ai_autonomy_policy: Option<AIAutonomyPolicy>,
    pub ugc_data_collection_policy: Option<UgcDataCollectionPolicy>,
    pub byo_api_key_policy: Option<ByoApiKeyPolicy>,
    pub multi_admin_policy: Option<MultiAdminPolicy>,
    pub ambient_agents_policy: Option<AmbientAgentsPolicy>,
}

/// Local representation of persisted policy metadata used by BYOP and local workspace checks.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkspacePolicyMetadata {
    pub policy: WorkspacePolicySet,
}

impl WorkspacePolicyMetadata {
    pub fn is_byo_api_key_enabled(&self) -> bool {
        self.policy
            .byo_api_key_policy
            .is_some_and(|policy| policy.enabled)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LlmHostSettings {
    pub enabled: bool,
    pub enablement_setting: HostEnablementSetting,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LlmSettings {
    pub enabled: bool,
    #[serde(default)]
    pub host_configs: std::collections::HashMap<LLMModelHost, LlmHostSettings>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub enum UgcCollectionEnablementSetting {
    Disable,
    Enable,
    #[default]
    RespectUserSetting,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct UgcCollectionSettings {
    pub setting: UgcCollectionEnablementSetting,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum AdminEnablementSetting {
    Disable,
    Enable,
    #[default]
    RespectUserSetting,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AiPermissionsSettings {
    pub allow_ai_in_non_current_app_environments: bool,
    #[serde(with = "serde_regex")]
    pub non_current_app_environment_regex_list: Vec<Regex>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AiAutonomySettings {
    pub apply_code_diffs_setting: Option<ActionPermission>,
    pub read_files_setting: Option<ActionPermission>,
    pub read_files_allowlist: Option<Vec<PathBuf>>,
    pub execute_commands_setting: Option<ActionPermission>,
    pub execute_commands_allowlist: Option<Vec<AgentModeCommandExecutionPredicate>>,
    pub execute_commands_denylist: Option<Vec<AgentModeCommandExecutionPredicate>>,
    pub write_to_pty_setting: Option<WriteToPtyPermission>,
    pub computer_use_setting: Option<ComputerUsePermission>,
}

impl AiAutonomySettings {
    pub fn has_any_overrides(&self) -> bool {
        self.apply_code_diffs_setting.is_some()
            || self.read_files_setting.is_some()
            || self.read_files_allowlist.is_some()
            || self.execute_commands_setting.is_some()
            || self.execute_commands_allowlist.is_some()
            || self.execute_commands_denylist.is_some()
            || self.write_to_pty_setting.is_some()
            || self.computer_use_setting.is_some()
    }

    pub fn has_override_for_code_diffs(&self) -> bool {
        self.apply_code_diffs_setting.is_some()
    }

    pub fn has_override_for_read_files(&self) -> bool {
        self.read_files_setting.is_some()
    }

    pub fn has_override_for_read_files_allowlist(&self) -> bool {
        self.read_files_allowlist.is_some()
    }

    pub fn has_override_for_execute_commands(&self) -> bool {
        self.execute_commands_setting.is_some()
    }

    pub fn has_override_for_execute_commands_allowlist(&self) -> bool {
        self.execute_commands_allowlist.is_some()
    }

    pub fn has_override_for_execute_commands_denylist(&self) -> bool {
        self.execute_commands_denylist.is_some()
    }

    pub fn has_override_for_write_to_pty(&self) -> bool {
        self.write_to_pty_setting.is_some()
    }

    pub fn has_override_for_computer_use(&self) -> bool {
        self.computer_use_setting.is_some()
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EnterpriseSecretRegex {
    pub pattern: String,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SecretRedactionSettings {
    pub enabled: bool,
    pub regexes: Vec<EnterpriseSecretRegex>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SandboxedAgentSettings {
    pub execute_commands_denylist: Option<Vec<AgentModeCommandExecutionPredicate>>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct WorkspaceSettings {
    pub llm_settings: LlmSettings,
    pub ugc_collection_settings: UgcCollectionSettings,
    pub secret_redaction_settings: SecretRedactionSettings,
    pub ai_permissions_settings: AiPermissionsSettings,
    pub ai_autonomy_settings: AiAutonomySettings,
    pub is_invite_link_enabled: bool,
    pub is_discoverable: bool,
    pub sandboxed_agent_settings: Option<SandboxedAgentSettings>,
    /// The team-level agent attribution setting. When `Enable` or `Disable`, the
    /// user toggle is locked. When `RespectUserSetting` (or absent), the user can choose.
    #[serde(default)]
    pub enable_agent_attribution: AdminEnablementSetting,
}
