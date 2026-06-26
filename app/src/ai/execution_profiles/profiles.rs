use core::fmt;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde::{Deserialize, Serialize};
use uuid::Uuid;
use warp_core::channel::ChannelState;
use warp_core::user_preferences::GetUserPreferences;
use warpui::{AppContext, Entity, EntityId, ModelContext, SingletonEntity};

use crate::ai::llms::LLMId;
use crate::ai::mcp::templatable_manager::TemplatableMCPServerManagerEvent;
use crate::object_store::model::persistence::{ObjectStoreEvent, UpdateSource};
use crate::LaunchMode;

use crate::ai::mcp::TemplatableMCPServerManager;
use crate::drive::ObjectTypeAndId;
use crate::object_store::ids::ObjectStoreId;
use crate::object_store::update_manager::UpdateManager;
use crate::object_store::{GenericStringObjectFormat, JsonObjectType};
use crate::settings::AgentModeCommandExecutionPredicate;
use crate::workspaces::user_workspaces::UserWorkspaces;
use crate::ObjectStoreModel;
use crate::{
    object_store::ids::ClientId, object_store::model::generic_string_model::GenericStringObjectId,
};

use super::{
    AIExecutionProfile, AIExecutionProfileObjectModel, ActionPermission, WriteToPtyPermission,
};

/// ExecutionProfileId is the identifier that users of the AIExecutionProfilesModel use
/// to refer back to a specific profile. These are unique across the lifespan of the app.
#[derive(Copy, Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ClientProfileId(usize);

impl ClientProfileId {
    #[allow(clippy::new_without_default)]
    pub fn new() -> ClientProfileId {
        static NEXT_PROFILE_ID: AtomicUsize = AtomicUsize::new(0);
        let raw = NEXT_PROFILE_ID.fetch_add(1, Ordering::Relaxed);
        ClientProfileId(raw)
    }
}

impl fmt::Display for ClientProfileId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        std::fmt::Display::fmt(&self.0, f)
    }
}

#[derive(Clone, Debug)]
pub struct AIExecutionProfileInfo {
    id: ClientProfileId,
    #[cfg_attr(target_family = "wasm", allow(dead_code))]
    object_store_id: Option<ObjectStoreId>,
    data: AIExecutionProfile,
}

impl AIExecutionProfileInfo {
    pub fn id(&self) -> &ClientProfileId {
        &self.id
    }

    /// The local ObjectStore ID of this profile, if it has object-store backing.
    #[cfg_attr(target_family = "wasm", allow(dead_code))]
    pub fn object_store_id(&self) -> Option<ObjectStoreId> {
        self.object_store_id
    }

    pub fn data(&self) -> &AIExecutionProfile {
        &self.data
    }
}

#[derive(Clone, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum DefaultProfileState {
    InlineLocal {
        id: ClientProfileId,
        profile: AIExecutionProfile,
    },
    ObjectBacked {
        id: ClientProfileId,
    },
    /// Currently, the behavior of the CLI default is that it
    /// cannot be updated and will never be object-backed.
    Cli {
        id: ClientProfileId,
        profile: AIExecutionProfile,
    },
}

impl std::fmt::Display for DefaultProfileState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DefaultProfileState::InlineLocal { .. } => write!(f, "InlineLocal"),
            DefaultProfileState::ObjectBacked { .. } => write!(f, "ObjectBacked"),
            DefaultProfileState::Cli { .. } => write!(f, "CLI"),
        }
    }
}

impl DefaultProfileState {
    pub fn id(&self) -> ClientProfileId {
        match self {
            DefaultProfileState::InlineLocal { id, .. } => *id,
            DefaultProfileState::ObjectBacked { id } => *id,
            DefaultProfileState::Cli { id, .. } => *id,
        }
    }
}

pub struct AIExecutionProfilesModel {
    /// The default profile can be in one of three states:
    /// - InlineLocal: the default profile is held directly on this model before it has an ObjectStore backing object.
    /// - ObjectBacked: an ObjectStore object backs the profile, created by a local edit or restored from local object storage.
    /// - CLI: when running in CLI mode, a more permissive default profile that stays inline.
    ///
    /// Note that the default_profile_state becomes object-backed either (1) when an edit happens on
    /// this client or (2) when a saved default profile is restored from the object store. Once
    /// the profile is object-backed, it's never inline-local
    /// again. CLI profiles are currently never object-backed.
    default_profile_state: DefaultProfileState,
    profile_id_to_object_store_id: HashMap<ClientProfileId, ObjectStoreId>,
    /// Only contains entries for non-default profiles.
    active_profiles_per_session: HashMap<EntityId, ClientProfileId>,
}

impl AIExecutionProfilesModel {
    #[allow(unused_variables)]
    pub fn new(launch_mode: &LaunchMode, ctx: &mut ModelContext<Self>) -> Self {
        cfg_if::cfg_if! {
            if #[cfg(feature = "agent_mode_evals")] {
                let default_profile_state = DefaultProfileState::InlineLocal {
                    id: ClientProfileId::new(),
                    profile: AIExecutionProfile::create_agent_mode_eval_profile(),
                };
                let profile_id_to_object_store_id: HashMap<ClientProfileId, ObjectStoreId> = HashMap::new();
                let active_profiles_per_session: HashMap<EntityId, ClientProfileId> = HashMap::new();
            } else {
                let object_store_model = ObjectStoreModel::handle(ctx).as_ref(ctx);
                let all_profile_objects: Vec<&super::AIExecutionProfileObject> = object_store_model
                    .get_all_objects_of_type::<GenericStringObjectId, AIExecutionProfileObjectModel>()
                    .collect();

                let default_profile_object: Option<&super::AIExecutionProfileObject> = all_profile_objects
                    .iter()
                    .find(|obj| obj.model().string_model.is_default_profile)
                    .copied();

                let mut profile_id_to_object_store_id: HashMap<ClientProfileId, ObjectStoreId> = HashMap::new();
                let active_profiles_per_session: HashMap<EntityId, ClientProfileId> = HashMap::new();

                for profile_object in all_profile_objects
                    .iter()
                    .filter(|p| !p.model().string_model.is_default_profile)
                {
                    let profile_id = ClientProfileId::new();
                    profile_id_to_object_store_id.insert(profile_id, profile_object.id);
                }

                let default_profile_state = match launch_mode {
                    LaunchMode::App { .. } | LaunchMode::Test { .. } => match default_profile_object {
                        Some(p) => {
                            let execution_profile_id = ClientProfileId::new();
                            profile_id_to_object_store_id.insert(execution_profile_id, p.id);
                            DefaultProfileState::ObjectBacked {
                                id: execution_profile_id,
                            }
                        }
                        None => DefaultProfileState::InlineLocal {
                            id: ClientProfileId::new(),
                            profile: AIExecutionProfile::create_default_from_legacy_settings(ctx),
                        },
                    },
                    // When running as a CLI, we ignore the GUI default and use a more permissive default.
                    LaunchMode::CommandLine { is_sandboxed, computer_use_override, .. } => {
                        DefaultProfileState::Cli {
                            profile: AIExecutionProfile::create_default_cli_profile(*is_sandboxed, *computer_use_override),
                            id: ClientProfileId::new()
                        }
                    }
                    // EnvironmentRuntimeProxy and EnvironmentRuntimeDaemon don't use AI
                    // execution profiles. They never reach this code path
                    // since they don't go through initialize_app, but handle
                    // exhaustively.
                    LaunchMode::EnvironmentRuntimeProxy | LaunchMode::EnvironmentRuntimeDaemon => DefaultProfileState::InlineLocal {
                        id: ClientProfileId::new(),
                        profile: AIExecutionProfile::create_default_from_legacy_settings(ctx),
                    },
                };
            }
        }

        // We have to listen for changes to AIExecutionProfiles for a few reasons:
        // (1) In case the default profile is inline-local AND a default profile arrives from the object store
        // (2) Let views subscribed to us know whenever a backing profile changes.
        // (3) Keep profile_id_to_object_store_id map up to date when stored profiles are created/deleted
        if !cfg!(feature = "agent_mode_evals") {
            ctx.subscribe_to_model(&ObjectStoreModel::handle(ctx), |me, event, ctx| {
                me.handle_object_store_event(event, ctx);
            });
        }

        ctx.subscribe_to_model(
            &TemplatableMCPServerManager::handle(ctx),
            |me, event, ctx| {
                me.handle_templatable_mcp_server_manager_event(event, ctx);
            },
        );

        // In dev, it's possible the SQLite data read in for the default profile actually comes from a different environment
        // (say, we switch between local and staging servers). When that happens the default profile starts as object-backed but
        // then the profile is deleted when initial load returns. To fix that, we listen for the deletion of the default
        // profile and reset the model state when that happens.
        if ChannelState::channel().is_dogfood() {
            if let DefaultProfileState::ObjectBacked { id } = &default_profile_state {
                let object_store_id_of_default_profile = *profile_id_to_object_store_id
                    .get(id)
                    .expect("default profile is object-backed but no ObjectStore ID found");
                ctx.subscribe_to_model(&ObjectStoreModel::handle(ctx), move |me, event, _| {
                if let ObjectStoreEvent::ObjectDeleted {
                    type_and_id: ObjectTypeAndId::GenericStringObject {
                        id: deleted_object_store_id,
                        ..
                    },
                    ..
                } = event {
                    if *deleted_object_store_id == object_store_id_of_default_profile {
                        log::info!("Resetting execution profile model because default profile was deleted.");
                        me.reset();
                    }
                }
            });
            }
        }

        log::info!("Initialized execution profile model with state: {default_profile_state}",);

        let mut model = Self {
            default_profile_state,
            profile_id_to_object_store_id,
            active_profiles_per_session,
        };

        model.maybe_inherit_from_legacy_settings(ctx);
        model
    }

    /// This function performs one-time migrations from legacy settings into the default profile.
    /// The issue this solves is that, whenever we migrate an existing setting into the profile object,
    /// users will initialize the new field to its default value. We need to manually check to see if
    /// the legacy setting hasn't been migrated and, if it hasn't, do a one-time overwrite on the new profile
    /// field.
    fn maybe_inherit_from_legacy_settings(&mut self, ctx: &mut ModelContext<Self>) {
        let DefaultProfileState::ObjectBacked {
            id: default_profile_id,
        } = self.default_profile_state
        else {
            return;
        };

        if let Some(base_llm_id) = ctx
            .private_user_preferences()
            .read_value("PreferredAgentModeLLMId")
            .ok()
            .flatten()
            .map(|s| serde_json::from_str::<Option<LLMId>>(&s))
            .and_then(|res| res.ok())
            .flatten()
        {
            if let Err(e) = ctx
                .private_user_preferences()
                .remove_value("PreferredAgentModeLLMId")
            {
                log::error!("Failed to remove old PreferredAgentModeLLMId user pref: {e}");
            }
            self.set_base_model(default_profile_id, Some(base_llm_id.clone()), ctx);
            log::info!("Overwrote default profile with legacy setting for base llm: {base_llm_id}");
        }
    }

    pub fn create_profile(&mut self, ctx: &mut ModelContext<Self>) -> Option<ClientProfileId> {
        let profile_id = ClientProfileId::new();

        let Some(owner) = UserWorkspaces::as_ref(ctx).personal_drive(ctx) else {
            log::error!("Failed to create AI execution profile: personal drive not available");
            return None;
        };

        let mut new_profile = self.default_profile(ctx).data().clone();
        new_profile.name = "".to_string();
        new_profile.is_default_profile = false;
        new_profile.auto_save_plans_to_local_drive = true;

        let update_manager = UpdateManager::handle(ctx);
        let client_id = ClientId::default();
        update_manager.update(ctx, |update_manager, ctx| {
            update_manager.create_ai_execution_profile(new_profile, client_id, owner, ctx);
        });

        self.profile_id_to_object_store_id
            .insert(profile_id, ObjectStoreId::ClientId(client_id));

        ctx.emit(AIExecutionProfilesModelEvent::ProfileCreated);

        Some(profile_id)
    }

    pub fn delete_profile(&mut self, profile_id: ClientProfileId, ctx: &mut ModelContext<Self>) {
        let id = self.default_profile_state.id();
        if id == profile_id {
            log::warn!("Attempted to delete default profile (id: {profile_id})");
            return;
        }

        let Some(object_store_id) = self.profile_id_to_object_store_id.get(&profile_id).cloned()
        else {
            return;
        };

        self.active_profiles_per_session
            .retain(|_, active_profile_id| *active_profile_id != profile_id);

        self.profile_id_to_object_store_id.remove(&profile_id);

        let update_manager = UpdateManager::handle(ctx);
        update_manager.update(ctx, |update_manager, ctx| {
            update_manager.delete_ai_execution_profile(object_store_id, ctx);
        });

        ctx.emit(AIExecutionProfilesModelEvent::ProfileDeleted);
    }

    // On logout, we need to clear any existing profile state.
    pub fn reset(&mut self) {
        self.default_profile_state = DefaultProfileState::InlineLocal {
            id: ClientProfileId::new(),
            profile: AIExecutionProfile {
                is_default_profile: true,
                ..Default::default()
            },
        };
        self.profile_id_to_object_store_id.clear();
        self.active_profiles_per_session.clear();
    }

    /// Returns the active permissions profile for a specific terminal view.
    /// If no terminal_view is provided, returns the default profile.
    ///
    /// If you need to account for enterprise overrides, call `BlocklistAIPermissions::active_permissions_profile` instead.
    pub fn active_profile(
        &self,
        terminal_view_id: Option<EntityId>,
        ctx: &AppContext,
    ) -> AIExecutionProfileInfo {
        terminal_view_id
            .and_then(|id| self.active_profiles_per_session.get(&id))
            .and_then(|profile_id| self.get_profile_by_id(*profile_id, ctx))
            .unwrap_or_else(|| self.default_profile(ctx))
    }

    pub fn default_profile_id(&self) -> ClientProfileId {
        self.default_profile_state.id()
    }

    pub fn default_profile(&self, ctx: &AppContext) -> AIExecutionProfileInfo {
        match &self.default_profile_state {
            DefaultProfileState::InlineLocal { id, profile } => AIExecutionProfileInfo {
                id: *id,
                object_store_id: None,
                data: profile.clone(),
            },
            DefaultProfileState::ObjectBacked { id } => {
                let Some(object_store_id) = self.profile_id_to_object_store_id.get(id) else {
                    log::error!(
                        "Default profile is object-backed but no object_store_id found in profile_id_to_object_store_id map."
                    );
                    return AIExecutionProfileInfo {
                        id: *id,
                        object_store_id: None,
                        data: AIExecutionProfile::default(),
                    };
                };
                let object_store_model = ObjectStoreModel::as_ref(ctx);
                let data = object_store_model
                    .get_object_of_type::<GenericStringObjectId, AIExecutionProfileObjectModel>(
                        object_store_id,
                    )
                    .map(|o| o.model().string_model.clone())
                    .unwrap_or_default();

                AIExecutionProfileInfo {
                    id: *id,
                    object_store_id: Some(*object_store_id),
                    data,
                }
            }
            DefaultProfileState::Cli { id, profile } => AIExecutionProfileInfo {
                id: *id,
                object_store_id: None,
                data: profile.clone(),
            },
        }
    }

    /// Sets the active profile for a specific terminal view.
    pub fn set_active_profile(
        &mut self,
        terminal_view_id: EntityId,
        profile_id: ClientProfileId,
        ctx: &mut ModelContext<Self>,
    ) {
        self.active_profiles_per_session
            .insert(terminal_view_id, profile_id);
        ctx.emit(AIExecutionProfilesModelEvent::UpdatedActiveProfile { terminal_view_id });
    }

    /// Returns a profile by its client ID.
    /// Returns None if the profile is not found.
    pub fn get_profile_by_id(
        &self,
        profile_id: ClientProfileId,
        ctx: &AppContext,
    ) -> Option<AIExecutionProfileInfo> {
        // Handle an inline-local default profile (including CLI)
        match &self.default_profile_state {
            DefaultProfileState::InlineLocal { id, profile }
            | DefaultProfileState::Cli { id, profile } => {
                if profile_id == *id {
                    return Some(AIExecutionProfileInfo {
                        id: *id,
                        object_store_id: None,
                        data: profile.clone(),
                    });
                }
            }
            DefaultProfileState::ObjectBacked { .. } => {}
        }

        // Handle all object-backed profiles (default and non-default)
        let object_store_id = self.profile_id_to_object_store_id.get(&profile_id)?;
        let object_store_model = ObjectStoreModel::as_ref(ctx);
        let data = object_store_model
            .get_object_of_type::<GenericStringObjectId, AIExecutionProfileObjectModel>(
                object_store_id,
            )
            .map(|o| o.model().string_model.clone())
            .unwrap_or_default();

        Some(AIExecutionProfileInfo {
            id: profile_id,
            object_store_id: Some(*object_store_id),
            data,
        })
    }

    pub fn get_all_profile_ids(&self) -> Vec<ClientProfileId> {
        let default_profile_id = self.default_profile_state.id();

        // Default profile is always first in the list
        std::iter::once(default_profile_id)
            .chain(
                self.profile_id_to_object_store_id
                    .keys()
                    .filter(|&&id| id != default_profile_id)
                    .cloned(),
            )
            .collect()
    }

    /// Look up a local client profile ID from its ObjectStore ID.
    #[cfg_attr(target_family = "wasm", allow(dead_code))]
    pub fn get_profile_id_by_object_store_id(
        &self,
        object_store_id: &ObjectStoreId,
    ) -> Option<ClientProfileId> {
        self.profile_id_to_object_store_id
            .iter()
            .find_map(|(client_id, id)| {
                if id == object_store_id {
                    Some(*client_id)
                } else {
                    None
                }
            })
    }

    pub fn has_multiple_profiles(&self) -> bool {
        let default_profile_id = self.default_profile_state.id();

        self.profile_id_to_object_store_id
            .keys()
            .any(|&id| id != default_profile_id)
    }

    pub fn set_base_model(
        &mut self,
        profile_id: ClientProfileId,
        llm_id: Option<LLMId>,
        ctx: &mut ModelContext<Self>,
    ) {
        self.edit_profile_internal(
            profile_id,
            |profile| {
                if profile.base_model != llm_id {
                    profile.base_model = llm_id.clone();
                    return true;
                }
                false
            },
            ctx,
        );
    }

    pub fn set_coding_model(
        &mut self,
        profile_id: ClientProfileId,
        model_id: Option<LLMId>,
        ctx: &mut ModelContext<Self>,
    ) {
        self.edit_profile_internal(
            profile_id,
            |profile| {
                if profile.coding_model != model_id {
                    profile.coding_model = model_id.clone();
                    return true;
                }
                false
            },
            ctx,
        );
    }

    pub fn set_cli_agent_model(
        &mut self,
        profile_id: ClientProfileId,
        model_id: Option<LLMId>,
        ctx: &mut ModelContext<Self>,
    ) {
        self.edit_profile_internal(
            profile_id,
            |profile| {
                if profile.cli_agent_model != model_id {
                    profile.cli_agent_model = model_id.clone();
                    return true;
                }
                false
            },
            ctx,
        );
    }

    pub fn set_title_model(
        &mut self,
        profile_id: ClientProfileId,
        model_id: Option<LLMId>,
        ctx: &mut ModelContext<Self>,
    ) {
        self.edit_profile_internal(
            profile_id,
            |profile| {
                if profile.title_model != model_id {
                    profile.title_model = model_id.clone();
                    return true;
                }
                false
            },
            ctx,
        );
    }

    pub fn set_active_ai_model(
        &mut self,
        profile_id: ClientProfileId,
        model_id: Option<LLMId>,
        ctx: &mut ModelContext<Self>,
    ) {
        self.edit_profile_internal(
            profile_id,
            |profile| {
                if profile.active_ai_model != model_id {
                    profile.active_ai_model = model_id.clone();
                    return true;
                }
                false
            },
            ctx,
        );
    }

    pub fn set_next_command_model(
        &mut self,
        profile_id: ClientProfileId,
        model_id: Option<LLMId>,
        ctx: &mut ModelContext<Self>,
    ) {
        self.edit_profile_internal(
            profile_id,
            |profile| {
                if profile.next_command_model != model_id {
                    profile.next_command_model = model_id.clone();
                    return true;
                }
                false
            },
            ctx,
        );
    }

    pub fn set_computer_use_model(
        &mut self,
        profile_id: ClientProfileId,
        model_id: Option<LLMId>,
        ctx: &mut ModelContext<Self>,
    ) {
        self.edit_profile_internal(
            profile_id,
            |profile| {
                if profile.computer_use_model != model_id {
                    profile.computer_use_model = model_id.clone();
                    return true;
                }
                false
            },
            ctx,
        );
    }

    pub fn set_context_window_limit(
        &mut self,
        profile_id: ClientProfileId,
        limit: Option<u32>,
        ctx: &mut ModelContext<Self>,
    ) {
        let changed = self.edit_profile_internal(
            profile_id,
            |profile| {
                if profile.context_window_limit != limit {
                    profile.context_window_limit = limit;
                    return true;
                }
                false
            },
            ctx,
        );

        if changed {}
    }

    pub fn set_apply_code_diffs(
        &mut self,
        profile_id: ClientProfileId,
        apply_code_diffs: &ActionPermission,
        ctx: &mut ModelContext<Self>,
    ) {
        self.edit_profile_internal(
            profile_id,
            |profile| {
                if profile.apply_code_diffs != *apply_code_diffs {
                    profile.apply_code_diffs = *apply_code_diffs;
                    return true;
                }
                false
            },
            ctx,
        );
    }

    pub fn set_read_files(
        &mut self,
        profile_id: ClientProfileId,
        read_files: &ActionPermission,
        ctx: &mut ModelContext<Self>,
    ) {
        self.edit_profile_internal(
            profile_id,
            |profile| {
                if profile.read_files != *read_files {
                    profile.read_files = *read_files;
                    return true;
                }
                false
            },
            ctx,
        );
    }

    pub fn set_execute_commands(
        &mut self,
        profile_id: ClientProfileId,
        execute_commands: &ActionPermission,
        ctx: &mut ModelContext<Self>,
    ) {
        self.edit_profile_internal(
            profile_id,
            |profile| {
                if profile.execute_commands != *execute_commands {
                    profile.execute_commands = *execute_commands;
                    return true;
                }
                false
            },
            ctx,
        );
    }

    pub fn set_write_to_pty(
        &mut self,
        profile_id: ClientProfileId,
        write_to_pty: &WriteToPtyPermission,
        ctx: &mut ModelContext<Self>,
    ) {
        self.edit_profile_internal(
            profile_id,
            |profile| {
                if profile.write_to_pty != *write_to_pty {
                    profile.write_to_pty = *write_to_pty;
                    return true;
                }
                false
            },
            ctx,
        );
    }

    pub fn set_mcp_permissions(
        &mut self,
        profile_id: ClientProfileId,
        mcp_permissions: &ActionPermission,
        ctx: &mut ModelContext<Self>,
    ) {
        self.edit_profile_internal(
            profile_id,
            |profile| {
                if profile.mcp_permissions == *mcp_permissions {
                    return false;
                }

                if mcp_permissions == &ActionPermission::AlwaysAllow {
                    profile.mcp_allowlist.clear();
                } else if mcp_permissions == &ActionPermission::AlwaysAsk {
                    profile.mcp_denylist.clear();
                }
                profile.mcp_permissions = *mcp_permissions;
                true
            },
            ctx,
        );
    }

    pub fn set_computer_use(
        &mut self,
        profile_id: ClientProfileId,
        permission: &super::ComputerUsePermission,
        ctx: &mut ModelContext<Self>,
    ) {
        let current_value = self
            .get_profile_by_id(profile_id, ctx)
            .map(|p| p.data().computer_use);

        self.edit_profile_internal(
            profile_id,
            |profile| {
                if profile.computer_use != *permission {
                    profile.computer_use = *permission;
                    return true;
                }
                false
            },
            ctx,
        );

        if current_value != Some(*permission) {}
    }

    pub fn set_ask_user_question(
        &mut self,
        profile_id: ClientProfileId,
        permission: super::AskUserQuestionPermission,
        ctx: &mut ModelContext<Self>,
    ) {
        let current_value = self
            .get_profile_by_id(profile_id, ctx)
            .map(|p| p.data().ask_user_question);

        self.edit_profile_internal(
            profile_id,
            |profile| {
                if profile.ask_user_question != permission {
                    profile.ask_user_question = permission;
                    return true;
                }
                false
            },
            ctx,
        );

        if current_value != Some(permission) {}
    }

    pub fn set_web_search_enabled(
        &mut self,
        profile_id: ClientProfileId,
        enabled: bool,
        ctx: &mut ModelContext<Self>,
    ) {
        self.edit_profile_internal(
            profile_id,
            |profile| {
                if profile.web_search_enabled != enabled {
                    profile.web_search_enabled = enabled;
                    return true;
                }
                false
            },
            ctx,
        );
    }

    pub fn set_auto_save_plans_to_local_drive(
        &mut self,
        profile_id: ClientProfileId,
        enabled: bool,
        ctx: &mut ModelContext<Self>,
    ) {
        self.edit_profile_internal(
            profile_id,
            |profile| {
                if profile.auto_save_plans_to_local_drive != enabled {
                    profile.auto_save_plans_to_local_drive = enabled;
                    return true;
                }
                false
            },
            ctx,
        );
    }

    pub fn set_profile_name(
        &mut self,
        profile_id: ClientProfileId,
        name: &str,
        ctx: &mut ModelContext<Self>,
    ) {
        self.edit_profile_internal(
            profile_id,
            |profile| {
                if profile.name != name {
                    profile.name = name.to_string();
                    return true;
                }
                false
            },
            ctx,
        );
    }

    pub fn add_to_command_allowlist(
        &mut self,
        profile_id: ClientProfileId,
        predicate: &AgentModeCommandExecutionPredicate,
        ctx: &mut ModelContext<Self>,
    ) {
        self.edit_profile_internal(
            profile_id,
            |profile| {
                if !profile.command_allowlist.contains(predicate) {
                    profile.command_allowlist.push(predicate.clone());
                    return true;
                }
                false
            },
            ctx,
        );
    }

    pub fn remove_from_command_allowlist(
        &mut self,
        profile_id: ClientProfileId,
        predicate: &AgentModeCommandExecutionPredicate,
        ctx: &mut ModelContext<Self>,
    ) {
        self.edit_profile_internal(
            profile_id,
            |profile| {
                let original_len = profile.command_allowlist.len();
                profile.command_allowlist.retain(|p| p != predicate);
                profile.command_allowlist.len() != original_len
            },
            ctx,
        );
    }

    pub fn add_to_directory_allowlist(
        &mut self,
        profile_id: ClientProfileId,
        path: &PathBuf,
        ctx: &mut ModelContext<Self>,
    ) {
        self.edit_profile_internal(
            profile_id,
            |profile| {
                if !profile.directory_allowlist.contains(path) {
                    profile.directory_allowlist.push(path.clone());
                    return true;
                }
                false
            },
            ctx,
        );
    }

    pub fn remove_from_directory_allowlist(
        &mut self,
        profile_id: ClientProfileId,
        path: &PathBuf,
        ctx: &mut ModelContext<Self>,
    ) {
        self.edit_profile_internal(
            profile_id,
            |profile| {
                let original_len = profile.directory_allowlist.len();
                profile.directory_allowlist.retain(|p| p != path);
                profile.directory_allowlist.len() != original_len
            },
            ctx,
        );
    }

    pub fn add_to_command_denylist(
        &mut self,
        profile_id: ClientProfileId,
        predicate: &AgentModeCommandExecutionPredicate,
        ctx: &mut ModelContext<Self>,
    ) {
        self.edit_profile_internal(
            profile_id,
            |profile| {
                if !profile.command_denylist.contains(predicate) {
                    profile.command_denylist.push(predicate.clone());
                    return true;
                }
                false
            },
            ctx,
        );
    }

    pub fn remove_from_command_denylist(
        &mut self,
        profile_id: ClientProfileId,
        predicate: &AgentModeCommandExecutionPredicate,
        ctx: &mut ModelContext<Self>,
    ) {
        self.edit_profile_internal(
            profile_id,
            |profile| {
                let original_len = profile.command_denylist.len();
                profile.command_denylist.retain(|p| p != predicate);
                profile.command_denylist.len() != original_len
            },
            ctx,
        );
    }

    pub fn add_to_mcp_allowlist(
        &mut self,
        profile_id: ClientProfileId,
        id: &Uuid,
        ctx: &mut ModelContext<Self>,
    ) {
        self.edit_profile_internal(
            profile_id,
            |profile| {
                if !profile.mcp_allowlist.contains(id) {
                    profile.mcp_allowlist.push(*id);
                    return true;
                }
                false
            },
            ctx,
        );
    }

    pub fn remove_from_mcp_allowlist(
        &mut self,
        profile_id: ClientProfileId,
        id: &Uuid,
        ctx: &mut ModelContext<Self>,
    ) {
        self.edit_profile_internal(
            profile_id,
            |profile| {
                let original_len = profile.mcp_allowlist.len();
                profile.mcp_allowlist.retain(|p| p != id);
                profile.mcp_allowlist.len() != original_len
            },
            ctx,
        );
    }

    pub fn add_to_mcp_denylist(
        &mut self,
        profile_id: ClientProfileId,
        id: &Uuid,
        ctx: &mut ModelContext<Self>,
    ) {
        self.edit_profile_internal(
            profile_id,
            |profile| {
                if !profile.mcp_denylist.contains(id) {
                    profile.mcp_denylist.push(*id);
                    return true;
                }
                false
            },
            ctx,
        );
    }

    pub fn remove_from_mcp_denylist(
        &mut self,
        profile_id: ClientProfileId,
        id: &Uuid,
        ctx: &mut ModelContext<Self>,
    ) {
        self.edit_profile_internal(
            profile_id,
            |profile| {
                let original_len = profile.mcp_denylist.len();
                profile.mcp_denylist.retain(|p| p != id);
                profile.mcp_denylist.len() != original_len
            },
            ctx,
        );
    }

    /// `edit_profile_internal` edits an AIExecutionProfile and persists the changed profile locally
    /// Parameters:
    /// * `profile_id`: The id of the profile to edit
    /// * `edit_fn`: a closure that safely modifies the AIExecutionProfile. It should return `true` if the profile was changed, `false` otherwise. When `true`, it persists the changes locally, and otherwise exits early to prevent excessive local persistence operations if no changes occurred.
    /// * `ctx`: The model context
    ///
    /// Returns `true` if the profile was actually changed,
    /// `false` otherwise. Callers can use this to gate side effects on real
    /// changes.
    fn edit_profile_internal(
        &mut self,
        profile_id: ClientProfileId,
        edit_fn: impl FnOnce(&mut AIExecutionProfile) -> bool,
        ctx: &mut ModelContext<Self>,
    ) -> bool {
        // We don't yet support editing the default profile for the CLI.
        if let DefaultProfileState::Cli { id, .. } = &self.default_profile_state {
            if *id == profile_id {
                log::warn!("Attempted to edit CLI default profile, which is not yet supported.");
                return false;
            }
        }

        // Case: this might be an edit to a not-yet-created default profile object. If so, we need to create
        // an ObjectStore object to back the default profile.
        if let DefaultProfileState::InlineLocal { id, profile } = &self.default_profile_state {
            if *id == profile_id {
                let mut new_profile = profile.clone();
                // If the edit function didn't make any changes to the profile, keep the inline default as-is.
                let value_changed = edit_fn(&mut new_profile);
                if !value_changed {
                    return false;
                }

                if let Some(owner) = UserWorkspaces::as_ref(ctx).personal_drive(ctx) {
                    let update_manager = UpdateManager::handle(ctx);
                    let client_id = ClientId::default();
                    update_manager.update(ctx, |update_manager, ctx| {
                        update_manager.create_ai_execution_profile(
                            new_profile,
                            client_id,
                            owner,
                            ctx,
                        );
                    });

                    // For forever on, the default profile state is object-backed.
                    let object_store_id = ObjectStoreId::ClientId(client_id);
                    self.default_profile_state =
                        DefaultProfileState::ObjectBacked { id: profile_id };
                    self.profile_id_to_object_store_id
                        .insert(profile_id, object_store_id);

                    log::info!(
                        "Creating an ObjectStore object for the default execution profile: {profile_id:?}"
                    );
                } else {
                    // The user isn't logged in yet (or personal drive isn't available),
                    // so we can't create an ObjectStore object. Persist the edit locally on the
                    // InlineLocal profile so it isn't silently dropped; it will be promoted
                    // to an ObjectBacked ObjectStore object the next time an edit can create backing storage.
                    // Without this, onboarding-driven edits (e.g. autonomy permissions
                    // written by `apply_agent_settings`) disappear when onboarding is
                    // completed before login.
                    self.default_profile_state = DefaultProfileState::InlineLocal {
                        id: profile_id,
                        profile: new_profile,
                    };

                    log::info!(
                        "Updated local inline-local default execution profile (no personal drive yet): {profile_id:?}"
                    );
                }
                ctx.emit(AIExecutionProfilesModelEvent::ProfileUpdated(profile_id));
                return true;
            }
        }

        let mut value_changed = false;
        if let Some(object_store_id) = self.profile_id_to_object_store_id.get(&profile_id) {
            let object_store_model = ObjectStoreModel::as_ref(ctx);
            if let Some(object) = object_store_model
                .get_object_of_type::<GenericStringObjectId, AIExecutionProfileObjectModel>(
                    object_store_id,
                )
            {
                let mut data = object.model().string_model.clone();
                // If the edit function didn't make any changes to the profile, we should exit early
                value_changed = edit_fn(&mut data);
                if !value_changed {
                    return false;
                }
                let update_manager = UpdateManager::handle(ctx);
                update_manager.update(ctx, |update_manager, ctx| {
                    update_manager.update_ai_execution_profile(data, *object_store_id, None, ctx);
                });

                log::info!("Edited execution profile with id: {profile_id:?}");
            } else {
                log::error!("Profile id is mapped but no object found: {profile_id:?}");
            }
        }
        ctx.emit(AIExecutionProfilesModelEvent::ProfileUpdated(profile_id));
        value_changed
    }

    /// Handle ObjectStoreModel events to keep the profile_id_to_object_store_id map and default profile state up to date.
    fn handle_object_store_event(
        &mut self,
        event: &ObjectStoreEvent,
        ctx: &mut ModelContext<Self>,
    ) {
        match event {
            ObjectStoreEvent::ObjectCreated {
                type_and_id:
                    ObjectTypeAndId::GenericStringObject {
                        object_type:
                            GenericStringObjectFormat::Json(JsonObjectType::AIExecutionProfile),
                        id,
                    },
            } => {
                self.handle_ai_execution_profile_created(*id, ctx);
            }
            ObjectStoreEvent::ObjectDeleted {
                type_and_id:
                    ObjectTypeAndId::GenericStringObject {
                        object_type:
                            GenericStringObjectFormat::Json(JsonObjectType::AIExecutionProfile),
                        id,
                    },
                folder_id: _,
            } => {
                self.handle_ai_execution_profile_deleted(*id, ctx);
            }
            ObjectStoreEvent::ObjectUpdated {
                type_and_id:
                    ObjectTypeAndId::GenericStringObject {
                        object_type:
                            GenericStringObjectFormat::Json(JsonObjectType::AIExecutionProfile),
                        id,
                    },
                source,
            } => {
                self.handle_ai_execution_profile_updated(*id, *source, ctx);
            }
            ObjectStoreEvent::InitialLoadCompleted => {
                self.reconcile_with_object_store_after_initial_load(ctx);
            }
            _ => {}
        }
    }

    /// Reconcile model state with `ObjectStoreModel` once local object restore completes.
    ///
    /// Stored objects can be present in `ObjectStoreModel` before this model observes
    /// per-object `ObjectCreated` events. That means the normal
    /// `handle_ai_execution_profile_created` handler may never fire for the
    /// restored default profile, and the model stays in `InlineLocal` even though
    /// the user already has a saved default profile.
    ///
    /// Without this reconciliation, a subsequent edit from `apply_agent_settings`
    /// (onboarding) would hit the `InlineLocal` branch of `edit_profile_internal`
    /// and *create a duplicate* object-store default profile rather than editing the
    /// existing one. That manifests as the default profile showing neither
    /// the user's prior stored values nor the onboarding choices — because the
    /// UI ends up reading a fresh client-side default with only a few fields
    /// touched.
    fn reconcile_with_object_store_after_initial_load(&mut self, ctx: &mut ModelContext<Self>) {
        let object_store_model = ObjectStoreModel::as_ref(ctx);
        let all_profiles: Vec<(ObjectStoreId, bool)> = object_store_model
            .get_all_objects_of_type::<GenericStringObjectId, AIExecutionProfileObjectModel>()
            .map(|o| (o.id, o.model().string_model.is_default_profile))
            .collect();

        // Transition InlineLocal -> ObjectBacked if object store has a default profile.
        if let DefaultProfileState::InlineLocal { id, .. } = self.default_profile_state {
            if let Some((object_store_id, _)) =
                all_profiles.iter().find(|(_, is_default)| *is_default)
            {
                self.default_profile_state = DefaultProfileState::ObjectBacked { id };
                self.profile_id_to_object_store_id
                    .insert(id, *object_store_id);
                log::info!(
                    "Reconciled default execution profile with object store after initial load: \
                     profile_id={id:?}, object_store_id={object_store_id:?}"
                );
                ctx.emit(AIExecutionProfilesModelEvent::ProfileUpdated(id));
            }
        }

        // Register any non-default profiles from object store that we aren't
        // already tracking so later edits find their backing object_store_id.
        let mut added_non_default = false;
        for (object_store_id, is_default) in all_profiles {
            if is_default {
                continue;
            }
            if !self
                .profile_id_to_object_store_id
                .values()
                .any(|s| *s == object_store_id)
            {
                let profile_id = ClientProfileId::new();
                self.profile_id_to_object_store_id
                    .insert(profile_id, object_store_id);
                log::info!(
                    "Registered existing execution profile object after initial load: {object_store_id:?}"
                );
                added_non_default = true;
            }
        }
        if added_non_default {
            ctx.emit(AIExecutionProfilesModelEvent::ProfileCreated);
        }
    }

    fn handle_templatable_mcp_server_manager_event(
        &mut self,
        event: &TemplatableMCPServerManagerEvent,
        ctx: &mut ModelContext<Self>,
    ) {
        match event {
            TemplatableMCPServerManagerEvent::TemplatableMCPServersUpdated => {
                self.remove_deleted_mcp_servers(ctx);
            }
            TemplatableMCPServerManagerEvent::StateChanged { uuid: _, state: _ }
            | TemplatableMCPServerManagerEvent::ServerInstallationAdded(_)
            | TemplatableMCPServerManagerEvent::ServerInstallationDeleted(_) => {}
        }
    }

    /// Handle a newly created AI execution profile from the object store.
    fn handle_ai_execution_profile_created(
        &mut self,
        object_store_id: ObjectStoreId,
        ctx: &mut ModelContext<Self>,
    ) {
        let object_store_model = ObjectStoreModel::as_ref(ctx);
        let Some(object) = object_store_model
            .get_object_of_type::<GenericStringObjectId, AIExecutionProfileObjectModel>(
                &object_store_id,
            )
        else {
            log::warn!("Received ObjectCreated event for AI execution profile but object not found in ObjectStoreModel: {object_store_id:?}");
            return;
        };

        // Check if this is the default profile
        if object.model().string_model.is_default_profile {
            // Don't add the object-store default profile if we're in CLI mode
            if matches!(self.default_profile_state, DefaultProfileState::Cli { .. }) {
                log::info!(
                    "Ignoring object-store default profile in CLI mode: {object_store_id:?}"
                );
                return;
            }

            // If we're in an inline-local state, transition to object-backed
            if let DefaultProfileState::InlineLocal { id, .. } = self.default_profile_state {
                self.default_profile_state = DefaultProfileState::ObjectBacked { id };
                self.profile_id_to_object_store_id
                    .insert(id, object_store_id);
                log::info!(
                    "Received default execution profile from object store. Marking profile as object-backed: {object_store_id:?}"
                );
                ctx.emit(AIExecutionProfilesModelEvent::ProfileUpdated(id));
            }

            return;
        }

        // For non-default profiles, add to the map if not already present
        let profile_exists = self
            .profile_id_to_object_store_id
            .values()
            .any(|id| *id == object_store_id);
        if !profile_exists {
            let profile_id = ClientProfileId::new();
            self.profile_id_to_object_store_id
                .insert(profile_id, object_store_id);
            log::info!("Added new execution profile to map: {object_store_id:?}");
            ctx.emit(AIExecutionProfilesModelEvent::ProfileCreated);
        }
    }

    /// Handle a deleted AI execution profile from the object store.
    fn handle_ai_execution_profile_deleted(
        &mut self,
        object_store_id: ObjectStoreId,
        ctx: &mut ModelContext<Self>,
    ) {
        // Find and remove the profile from our map
        let profile_id = self
            .profile_id_to_object_store_id
            .iter()
            .find_map(|(client_id, id)| {
                if *id == object_store_id {
                    Some(*client_id)
                } else {
                    None
                }
            });

        if let Some(profile_id) = profile_id {
            self.profile_id_to_object_store_id.remove(&profile_id);

            // Also remove from active profiles per session
            self.active_profiles_per_session
                .retain(|_, active_id| *active_id != profile_id);

            // If the default profile was deleted, transition back to inline-local state
            let is_default = matches!(&self.default_profile_state, DefaultProfileState::ObjectBacked { id } if *id == profile_id);
            if is_default {
                log::warn!("Default execution profile was deleted from object store. Transitioning to inline-local state: {object_store_id:?}");
                self.default_profile_state = DefaultProfileState::InlineLocal {
                    id: profile_id,
                    profile: AIExecutionProfile {
                        is_default_profile: true,
                        ..Default::default()
                    },
                };
            }

            log::info!("Removed execution profile from map: {object_store_id:?}");
            ctx.emit(AIExecutionProfilesModelEvent::ProfileDeleted);
        }
    }

    /// Handle an updated AI execution profile from the object store.
    fn handle_ai_execution_profile_updated(
        &mut self,
        object_store_id: ObjectStoreId,
        source: UpdateSource,
        ctx: &mut ModelContext<Self>,
    ) {
        // Only notify about external updates (not local updates, which we already handle).
        if source != UpdateSource::External {
            return;
        }

        // Find the client profile ID for this object-store ID
        let profile_id = self.get_profile_id_by_object_store_id(&object_store_id);

        if let Some(profile_id) = profile_id {
            log::info!("Execution profile updated externally: {object_store_id:?}");
            ctx.emit(AIExecutionProfilesModelEvent::ProfileUpdated(profile_id));
        }
    }

    /// Handle deleted MCP servers by deleting its uuid from all profiles.
    fn remove_deleted_mcp_servers(&mut self, ctx: &mut ModelContext<Self>) {
        let all_valid_uuids =
            TemplatableMCPServerManager::get_all_templatable_mcp_server_names(ctx);
        for profile_id in self.get_all_profile_ids() {
            self.edit_profile_internal(
                profile_id,
                |profile| {
                    let original_allowlist_len = profile.mcp_allowlist.len();
                    let original_denylist_len = profile.mcp_denylist.len();
                    profile
                        .mcp_allowlist
                        .retain(|uuid| all_valid_uuids.contains_key(uuid));
                    profile
                        .mcp_denylist
                        .retain(|uuid| all_valid_uuids.contains_key(uuid));
                    profile.mcp_allowlist.len() != original_allowlist_len
                        || profile.mcp_denylist.len() != original_denylist_len
                },
                ctx,
            );
        }
    }

    // We don't want stale client ids in our map. We won't be able to find the backing object-store object when
    // an edit occurs.
    pub fn replace_client_id_with_server_id(
        &mut self,
        server_id: ObjectStoreId,
        client_id: ObjectStoreId,
    ) {
        for (_, object_store_id) in self.profile_id_to_object_store_id.iter_mut() {
            if *object_store_id == client_id {
                *object_store_id = server_id;
                log::info!("Updated profile id mapping after creating a new execution profile");
            }
        }
    }

    /// Replaces the given profile's data with CLI defaults for the given sandboxed state.
    /// Use in tests to simulate the profile configuration used by the sandboxed CLI agent.
    #[cfg(test)]
    pub fn apply_cli_profile_defaults_for_test(
        &mut self,
        profile_id: ClientProfileId,
        is_sandboxed: bool,
        ctx: &mut ModelContext<Self>,
    ) {
        let cli_profile = AIExecutionProfile::create_default_cli_profile(is_sandboxed, None);
        self.edit_profile_internal(
            profile_id,
            move |profile| {
                *profile = cli_profile;
                true
            },
            ctx,
        );
    }
}

#[allow(clippy::enum_variant_names)]
pub enum AIExecutionProfilesModelEvent {
    ProfileUpdated(ClientProfileId),
    ProfileCreated,
    ProfileDeleted,
    UpdatedActiveProfile { terminal_view_id: EntityId },
}

impl Entity for AIExecutionProfilesModel {
    type Event = AIExecutionProfilesModelEvent;
}

impl SingletonEntity for AIExecutionProfilesModel {}

#[cfg(test)]
#[path = "profiles_tests.rs"]
mod tests;
