use crate::{auth::UserUid, object_store::ids::StableObjectId};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;

use super::workspace::{
    EmailInvite, InviteLinkDomainRestriction, WorkspaceInviteCode, WorkspacePolicyMetadata,
    WorkspaceSettings,
};

#[derive(Clone, Copy, Eq, PartialEq, Debug, Serialize, Deserialize)]
pub enum MembershipRole {
    Owner,
    Admin,
    User,
}

impl MembershipRole {
    pub fn is_admin_or_owner(&self) -> bool {
        matches!(self, MembershipRole::Admin | MembershipRole::Owner)
    }

    pub fn is_owner(&self) -> bool {
        matches!(self, MembershipRole::Owner)
    }
}

#[derive(Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
pub struct TeamMember {
    pub uid: UserUid,
    pub email: String,
    pub role: MembershipRole,
}

impl PartialOrd for TeamMember {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for TeamMember {
    fn cmp(&self, other: &Self) -> Ordering {
        self.email.cmp(&other.email)
    }
}

#[derive(PartialEq, Eq, Clone)]
pub enum TeamDeleteDisabledReason {
    OtherMembers,
}

impl TeamDeleteDisabledReason {
    pub fn user_facing_message(&self) -> &str {
        match self {
            TeamDeleteDisabledReason::OtherMembers => {
                "Your team cannot be deleted with other team members."
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct Team {
    pub uid: StableObjectId,
    pub name: String,
    pub invite_code: Option<WorkspaceInviteCode>,
    pub members: Vec<TeamMember>,
    pub pending_email_invites: Vec<EmailInvite>,
    pub invite_link_domain_restrictions: Vec<InviteLinkDomainRestriction>,
    pub workspace_policy: WorkspacePolicyMetadata,
    pub organization_settings: WorkspaceSettings,
    /// If the team is eligible for discovery, then show toggle for setting discoverability to the team's admin
    pub is_eligible_for_discovery: bool,
}

impl Team {
    pub fn from_local_cache(
        uid: StableObjectId,
        name: String,
        workspace_settings: Option<WorkspaceSettings>,
        workspace_policy: Option<WorkspacePolicyMetadata>,
        members: Option<Vec<TeamMember>>,
    ) -> Self {
        Self {
            uid,
            name,
            invite_code: Default::default(),
            members: members.unwrap_or_default(),
            pending_email_invites: Default::default(),
            invite_link_domain_restrictions: Default::default(),
            workspace_policy: workspace_policy.unwrap_or_default(),
            organization_settings: workspace_settings.unwrap_or_default(),
            is_eligible_for_discovery: false,
        }
    }

    fn get_member_by_email(&self, email: &str) -> Option<&TeamMember> {
        self.members.iter().find(|member| member.email == email)
    }

    pub fn has_owner_permissions(&self, user_email: &str) -> bool {
        self.get_member_by_email(user_email)
            .is_some_and(|member| member.role.is_owner())
    }

    pub fn is_multi_admin_enabled(&self) -> bool {
        self.workspace_policy
            .policy
            .multi_admin_policy
            .is_some_and(|policy| policy.enabled)
    }

    pub fn has_admin_permissions(&self, user_email: &str) -> bool {
        self.get_member_by_email(user_email).is_some_and(|member| {
            member.role.is_owner()
                || (member.role == MembershipRole::Admin && self.is_multi_admin_enabled())
        })
    }

    pub fn get_delete_disabled_reason(
        &self,
        current_user_email: &str,
    ) -> Option<TeamDeleteDisabledReason> {
        if self.members.len() > 1
            || self
                .members
                .first()
                .is_none_or(|m| m.email != current_user_email)
        {
            return Some(TeamDeleteDisabledReason::OtherMembers);
        }
        None // No reason found, team can be deleted
    }

    pub fn is_custom_llm_enabled(&self) -> bool {
        self.organization_settings.llm_settings.enabled
    }
}
