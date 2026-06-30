use super::{
    team::Team,
    workspace::{
        AdminEnablementSetting, EnterpriseSecretRegex, HostEnablementSetting,
        UgcCollectionEnablementSetting, Workspace, WorkspaceUid,
    },
};
use crate::{
    ai::llms::LLMModelHost,
    auth::{UserUid, TEST_USER_UID},
    object_store::{Owner, Space},
    settings::{AISettings, PrivacySettings},
    workspaces::workspace::{AiAutonomySettings, SandboxedAgentSettings},
};
use regex::Regex;
use warp_core::settings::{ChangeEventReason, Setting};
use warpui::{AppContext, Entity, ModelContext, SingletonEntity, Tracked};

#[cfg(test)]
use crate::object_store::ids::StableObjectId;
#[cfg(test)]
use crate::workspaces::workspace::{
    AIAutonomyPolicy, WorkspaceMember, WorkspacePolicyMetadata, WorkspaceSettings,
};

#[cfg(test)]
use super::team::MembershipRole;
#[cfg(test)]
use super::workspace::WorkspaceMemberUsageInfo;

#[derive(Debug)]
pub enum UserWorkspacesEvent {
    UpdateWorkspaceSettingsSuccess,
    UpdateWorkspaceSettingsRejected(anyhow::Error),
    /// Fired whenever the set of teams the user is on changes.
    TeamsChanged,
}

/// UserWorkspaces is a singleton model that holds workspace metadata (name, members, etc).
/// It should be used for getting information about the workspaces, teams, current teams,
/// and all other things related to operating on workspace and team data.
/// TODO: consolidate local SQLite refresh/update paths.
pub struct UserWorkspaces {
    current_workspace_uid: Tracked<Option<WorkspaceUid>>,
    workspaces: Tracked<Vec<Workspace>>,
}

impl UserWorkspaces {
    #[cfg(test)]
    pub fn mock(cached_workspaces: Vec<Workspace>, _ctx: &mut ModelContext<Self>) -> Self {
        Self {
            current_workspace_uid: cached_workspaces.first().map(|w| w.uid).into(),
            workspaces: cached_workspaces.into(),
        }
    }

    #[cfg(test)]
    pub fn default_mock(ctx: &mut ModelContext<Self>) -> Self {
        Self::mock(vec![], ctx)
    }

    pub fn new(
        cached_workspaces: Vec<Workspace>,
        current_workspace_uid: Option<WorkspaceUid>,
    ) -> Self {
        Self {
            current_workspace_uid: current_workspace_uid.into(),
            workspaces: cached_workspaces.into(),
        }
    }

    pub fn workspace_from_uid(&self, workspace_uid: WorkspaceUid) -> Option<&Workspace> {
        self.workspaces.iter().find(|w| w.uid == workspace_uid)
    }

    pub fn workspace_from_uid_mut(
        &mut self,
        workspace_uid: WorkspaceUid,
    ) -> Option<&mut Workspace> {
        self.workspaces.iter_mut().find(|w| w.uid == workspace_uid)
    }

    /// Returns local workspace team metadata when a local policy source provides it.
    pub fn current_team(&self) -> Option<&Team> {
        self.current_workspace()
            .and_then(|workspace| workspace.teams.first())
    }

    /// Returns the current local workspace metadata.
    pub fn current_workspace(&self) -> Option<&Workspace> {
        self.current_workspace_uid
            .and_then(|workspace_uid| self.workspace_from_uid(workspace_uid))
    }

    pub fn current_workspace_mut(&mut self) -> Option<&mut Workspace> {
        self.current_workspace_uid
            .and_then(|workspace_uid| self.workspace_from_uid_mut(workspace_uid))
    }

    pub fn workspaces(&self) -> &Vec<Workspace> {
        &self.workspaces
    }

    pub fn set_current_workspace_uid(
        &mut self,
        workspace_uid: WorkspaceUid,
        ctx: &mut ModelContext<Self>,
    ) {
        *self.current_workspace_uid = Some(workspace_uid);
        self.notify_and_emit_teams_changed(ctx);
    }

    /// Returns `true` if active AI is allowed for the current workspace, based on local policy.
    ///
    /// In the future, we should store active AI enablement on the policy directly. For now, we
    /// proxy whether active AI by checking if prompt suggestions, next command, or code suggestions are enabled.
    pub fn is_active_ai_allowed(&self) -> bool {
        self.current_team().is_none_or(|team| {
            team.workspace_policy
                .policy
                .ai_assistant_policy
                .is_none_or(|policy| {
                    policy.is_prompt_suggestions_toggleable
                        || policy.is_next_command_enabled
                        || policy.is_code_suggestions_toggleable
                })
        })
    }

    /// Local Ashide builds do not gate AI by remote plan/customer state.
    pub fn ai_allowed_for_current_team(&self) -> bool {
        true
    }

    /// Whether Prompt Suggestions should be toggleable for the current user, based on the active policies.
    /// Note that the value may be incorrect if called before the team's policy metadata has been loaded.
    pub fn is_prompt_suggestions_toggleable(&self) -> bool {
        self.current_team()
            // If the user has no team, they can toggle prompt suggestions (no restrictions).
            .is_none_or(|team| {
                team.workspace_policy
                    .policy
                    .ai_assistant_policy
                    .is_some_and(|policy| policy.is_prompt_suggestions_toggleable)
            })
    }

    /// Whether Code Suggestions should be toggleable for the current user, based on the active policies.
    /// Note that the value may be incorrect if called before the team's policy metadata has been loaded.
    pub fn is_code_suggestions_toggleable(&self) -> bool {
        self.current_team()
            // If the user has no team, they can toggle code suggestions (no restrictions).
            .is_none_or(|team| {
                team.workspace_policy
                    .policy
                    .ai_assistant_policy
                    .is_some_and(|policy| policy.is_code_suggestions_toggleable)
            })
    }

    /// Whether Next Command should be toggleable for the current user, based on the active policies.
    /// Note that the value may be incorrect if called before the team's policy metadata has been loaded.
    pub fn is_next_command_enabled(&self) -> bool {
        self.current_team()
            // If the user has no team, they can toggle Next Command (no restrictions).
            .is_none_or(|team| {
                team.workspace_policy
                    .policy
                    .ai_assistant_policy
                    .is_some_and(|policy| policy.is_next_command_enabled)
            })
    }

    /// Whether voice input should be toggleable for the current user, based on the active policies.
    /// Note that the value may be incorrect if called before the team's policy metadata has been loaded.
    /// If voice input support is not compiled into this build, always returns `false`.
    pub fn is_voice_enabled(&self) -> bool {
        cfg!(feature = "voice_input")
            && self
                .current_team()
                // If the user has no team, they can toggle Voice (no restrictions).
                .is_none_or(|team| {
                    team.workspace_policy
                        .policy
                        .ai_assistant_policy
                        .is_some_and(|policy| policy.is_voice_enabled)
                })
    }

    /// Whether BYO API key is enabled for the current user, based on the active policies.
    /// Local Ashide keeps BYOP enabled by default.
    pub fn is_byo_api_key_enabled(&self) -> bool {
        true
    }

    pub fn aws_bedrock_host_settings(&self) -> Option<&super::workspace::LlmHostSettings> {
        self.current_workspace().and_then(|workspace| {
            workspace
                .settings
                .llm_settings
                .host_configs
                .get(&LLMModelHost::AwsBedrock)
        })
    }

    /// Did the admin enable AWS Bedrock for the current workspace?
    pub fn is_aws_bedrock_available_from_workspace(&self) -> bool {
        self.current_workspace().is_some_and(|workspace| {
            workspace.settings.llm_settings.enabled
                && self
                    .aws_bedrock_host_settings()
                    .is_some_and(|settings| settings.enabled)
        })
    }
    pub fn aws_bedrock_host_enablement_setting(&self) -> HostEnablementSetting {
        self.aws_bedrock_host_settings()
            .map(|settings| settings.enablement_setting.clone())
            .unwrap_or_default()
    }

    pub fn is_aws_bedrock_credentials_toggleable(&self) -> bool {
        matches!(
            self.aws_bedrock_host_enablement_setting(),
            HostEnablementSetting::RespectUserSetting
        )
    }

    pub fn is_aws_bedrock_credentials_enabled(&self, app: &AppContext) -> bool {
        // i.e. did the admin go and toggle on aws bedrock in the admin panel?
        if !self.is_aws_bedrock_available_from_workspace() {
            return false;
        }

        match self.aws_bedrock_host_enablement_setting() {
            HostEnablementSetting::Enforce => true,
            HostEnablementSetting::RespectUserSetting => *AISettings::as_ref(app)
                .aws_bedrock_credentials_enabled
                .value(),
        }
    }

    /// Returns the AI autonomy settings that are enforced by the workspace for all its members.
    /// If a setting is `None`, the workspace doesn't enforce a particular setting.
    pub fn ai_autonomy_settings(&self) -> AiAutonomySettings {
        self.current_team()
            .map(|team| team.organization_settings.ai_autonomy_settings.clone())
            .unwrap_or_default()
    }

    /// Returns the sandboxed agent settings enforced by the workspace, if any.
    pub fn sandboxed_agent_settings(&self) -> Option<SandboxedAgentSettings> {
        self.current_team()
            .and_then(|team| team.organization_settings.sandboxed_agent_settings.clone())
    }

    /// Returns true iff AI autonomy features are allowed for this client.
    /// TODO: This should be deleted soon. AI autonomy settings have been moved into organization
    /// settings (see `ai_autonomy_settings` above). If explicit org settings are absent,
    /// fall back to the local workspace policy snapshot.
    pub fn is_ai_autonomy_allowed(&self) -> bool {
        self.current_team().is_none_or(|team| {
            let settings = &team.organization_settings.ai_autonomy_settings;
            let all_settings_none = settings.apply_code_diffs_setting.is_none()
                && settings.read_files_setting.is_none()
                && settings.read_files_allowlist.is_none()
                && settings.execute_commands_setting.is_none()
                && settings.execute_commands_allowlist.is_none()
                && settings.execute_commands_denylist.is_none();

            if all_settings_none {
                team.workspace_policy
                    .policy
                    .ai_autonomy_policy
                    .is_some_and(|policy| policy.is_enabled)
            } else {
                true
            }
        })
    }

    // Team spaces are collaboration surfaces; local Ashide exposes only Personal space.
    pub fn team_spaces(&self) -> Vec<Space> {
        vec![]
    }

    // Local Drive keeps only Personal space. Team / Shared are collaboration surfaces,
    // so stale workspace metadata must not reopen them in Drive or Workflow UI.
    pub fn all_user_spaces(&self, ctx: &AppContext) -> Vec<Space> {
        let _ = ctx;
        vec![Space::Personal]
    }

    // Personal-space owner is pinned to the local placeholder user.
    // This must stay stable so old object owner fields remain visible after restart.
    fn effective_personal_user_uid() -> UserUid {
        UserUid::new(TEST_USER_UID)
    }

    // Returns the [`Owner`] for the user's personal drive.
    // Workflow / EnvVar / Folder / Notebook / Import create actions under Personal
    // space are owned by the local placeholder user and persisted only in local SQLite.
    pub fn personal_drive(&self, ctx: &AppContext) -> Option<Owner> {
        let _ = ctx;
        Some(Owner::User {
            user_uid: Self::effective_personal_user_uid(),
        })
    }

    // Maps any historical Drive space into the single local Personal owner.
    pub fn space_to_owner(&self, space: Space, ctx: &AppContext) -> Option<Owner> {
        let _ = space;
        self.personal_drive(ctx)
    }

    // Maps any historical owner into the single local Personal space.
    pub fn owner_to_space(&self, owner: Owner, ctx: &AppContext) -> Space {
        let _ = (owner, ctx);
        Space::Personal
    }

    pub fn has_teams(&self) -> bool {
        false
    }

    pub fn has_workspaces(&self) -> bool {
        !self.workspaces.is_empty()
    }

    pub fn update_workspaces(&mut self, workspaces: Vec<Workspace>, ctx: &mut ModelContext<Self>) {
        *self.workspaces = workspaces;
        self.notify_and_emit_teams_changed(ctx);
    }

    fn notify_and_emit_teams_changed(&self, ctx: &mut ModelContext<Self>) {
        // PrivacySettings can't observe UserWorkspaces for updates, as it's initialized too early in
        // the app initialization flow. So, we update it manually whenever teams data changes.
        PrivacySettings::handle(ctx).update(ctx, |settings, ctx| {
            settings.set_enterprise_secret_redaction_settings(
                self.is_enterprise_secret_redaction_enabled(),
                self.get_enterprise_secret_redaction_regex_list(),
                ChangeEventReason::ExternalUpdate,
                ctx,
            );
        });

        ctx.emit(UserWorkspacesEvent::TeamsChanged);
        ctx.notify();
    }

    pub fn is_enterprise_secret_redaction_enabled(&self) -> bool {
        self.current_team()
            .map(|team| team.organization_settings.secret_redaction_settings.enabled)
            .unwrap_or(false)
    }

    pub fn get_enterprise_secret_redaction_regex_list(&self) -> Vec<EnterpriseSecretRegex> {
        self.current_team()
            .map(|team| {
                team.organization_settings
                    .secret_redaction_settings
                    .regexes
                    .clone()
            })
            .unwrap_or_default()
    }

    pub fn get_ugc_collection_enablement_setting(&self) -> UgcCollectionEnablementSetting {
        self.current_team()
            .map(|team| {
                team.organization_settings
                    .ugc_collection_settings
                    .setting
                    .clone()
            })
            .unwrap_or_default()
    }

    pub fn is_ai_allowed_in_non_current_app_environments(&self) -> bool {
        // Local Ashide has no hosted organization policy; non-current-app environments
        // can always use the local Agent capability.
        true
    }

    pub fn get_non_current_app_environment_regex_list(&self) -> Vec<Regex> {
        self.current_team()
            .map(|team| {
                team.organization_settings
                    .ai_permissions_settings
                    .non_current_app_environment_regex_list
                    .clone()
            })
            .unwrap_or_default()
    }

    /// Returns the team-level agent attribution setting.
    ///
    /// Use this to decide whether the user's attribution toggle should be locked
    /// (`Enable`/`Disable`) or editable (`RespectUserSetting`).
    pub fn get_agent_attribution_setting(&self) -> AdminEnablementSetting {
        self.current_team()
            .map(|team| team.organization_settings.enable_agent_attribution.clone())
            .unwrap_or_default()
    }
}

#[cfg(test)]
impl UserWorkspaces {
    /// Creates a test workspace with a team and sets it as the current workspace.
    /// Returns the workspace UID and admin UID for use in tests.
    pub fn setup_test_workspace(&mut self, ctx: &mut ModelContext<Self>) {
        let workspace_uid = WorkspaceUid::from(StableObjectId::from(1));
        let owner_uid = UserUid::new("test_owner");

        let workspace_settings = WorkspaceSettings::default();

        let workspace = Workspace {
            uid: workspace_uid,
            name: "Test Workspace".to_string(),
            teams: vec![Team {
                uid: StableObjectId::from(2),
                name: "Test Team".to_string(),
                organization_settings: workspace_settings.clone(),
                workspace_policy: WorkspacePolicyMetadata::default(),
                members: vec![],
                invite_code: None,
                pending_email_invites: vec![],
                invite_link_domain_restrictions: vec![],
                is_eligible_for_discovery: false,
            }],
            members: vec![WorkspaceMember {
                uid: owner_uid,
                email: "test@example.com".to_string(),
                role: MembershipRole::Owner,
                usage_info: WorkspaceMemberUsageInfo {
                    requests_used_since_last_refresh: 0,
                    request_limit: 1000,
                    is_unlimited: false,
                    is_request_limit_prorated: false,
                },
            }],
            workspace_policy: WorkspacePolicyMetadata::default(),
            settings: workspace_settings,
            invite_code: None,
            invite_link_domain_restrictions: vec![],
            pending_email_invites: vec![],
            is_eligible_for_discovery: false,
        };

        self.update_workspaces(vec![workspace], ctx);
        self.set_current_workspace_uid(workspace_uid, ctx);
    }

    /// Updates the current workspace by applying a mutation function.
    pub fn update_current_workspace<F>(&mut self, f: F, ctx: &mut ModelContext<Self>)
    where
        F: FnOnce(&mut Workspace),
    {
        if let Some(workspace) = self.current_workspace() {
            if workspace.teams.is_empty() {
                panic!("No team found in current workspace. Did you call setup_test_workspace()?");
            }

            let mut new_workspace = workspace.clone();
            f(&mut new_workspace);

            self.update_workspaces(vec![new_workspace], ctx);
        } else {
            panic!("No workspace found. Did you call setup_test_workspace()?");
        }
    }

    pub fn update_sandboxed_agent_settings<F>(&mut self, f: F, ctx: &mut ModelContext<Self>)
    where
        F: FnOnce(&mut Option<SandboxedAgentSettings>),
    {
        self.update_current_workspace(
            |workspace| {
                if let Some(team) = workspace.teams.first_mut() {
                    f(&mut team.organization_settings.sandboxed_agent_settings);
                } else {
                    panic!(
                        "No team found in current workspace. Did you call setup_test_workspace()?"
                    );
                }
            },
            ctx,
        );
    }

    pub fn update_ai_autonomy_settings<F>(&mut self, f: F, ctx: &mut ModelContext<Self>)
    where
        F: FnOnce(&mut AiAutonomySettings),
    {
        self.update_current_workspace(
            |workspace| {
                if let Some(team) = workspace.teams.first_mut() {
                    f(&mut team.organization_settings.ai_autonomy_settings);
                } else {
                    panic!(
                        "No team found in current workspace. Did you call setup_test_workspace()?"
                    );
                }
            },
            ctx,
        );
    }

    pub fn update_ai_autonomy_policy_flag(&mut self, enabled: bool, ctx: &mut ModelContext<Self>) {
        self.update_current_workspace(
            |workspace| {
                if let Some(team) = workspace.teams.first_mut() {
                    team.workspace_policy.policy.ai_autonomy_policy = Some(AIAutonomyPolicy {
                        is_enabled: enabled,
                        toggleable: true,
                    });
                } else {
                    panic!(
                        "No team found in current workspace. Did you call setup_test_workspace()?"
                    );
                }
            },
            ctx,
        );
    }
}

impl Entity for UserWorkspaces {
    type Event = UserWorkspacesEvent;
}

/// Mark UserWorkspaces as global application state.
impl SingletonEntity for UserWorkspaces {}

// The old `user_workspaces_tests.rs` covered hosted team RPC paths
// (`MockTeamClient` / `mockall::Sequence`); those paths are deleted in local Ashide.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_team_allows_ai_in_non_current_app_environments() {
        let workspaces = UserWorkspaces::new(vec![], None);

        assert!(workspaces.is_ai_allowed_in_non_current_app_environments());
    }
}
