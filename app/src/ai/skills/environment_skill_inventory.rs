use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use ai::skills::{
    provider_rank, SkillProvider, SkillReference, SkillScope, SKILL_PROVIDER_DEFINITIONS,
};
use warp_core::SessionId;
use warpui::{AppContext, Entity, ModelContext, SingletonEntity};

use crate::ai::skills::{
    icon_override_for_skill_name, SkillDescriptor, SkillInventoryDuplicate, SkillInventoryItem,
};
use crate::terminal::model::session::active_session::ActiveSession as TerminalActiveSession;
use crate::workspace::environment_runtime::{self, EnvironmentRuntimeClient};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct EnvironmentSkillInventoryKey {
    session_id: SessionId,
    current_working_directory: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) enum EnvironmentSkillInventoryCacheEvent {
    InventoryChanged,
}

#[derive(Default)]
pub(crate) struct EnvironmentSkillInventoryCache {
    inventories: HashMap<EnvironmentSkillInventoryKey, Vec<SkillDescriptor>>,
    in_flight: HashSet<EnvironmentSkillInventoryKey>,
    failures: HashMap<EnvironmentSkillInventoryKey, String>,
}

impl EnvironmentSkillInventoryCache {
    pub(crate) fn new(_ctx: &mut ModelContext<Self>) -> Self {
        Self::default()
    }

    pub(crate) fn cached_for_session(
        &self,
        session_id: SessionId,
        current_working_directory: Option<&str>,
    ) -> Vec<SkillDescriptor> {
        let key = EnvironmentSkillInventoryKey {
            session_id,
            current_working_directory: current_working_directory.map(ToOwned::to_owned),
        };
        self.inventories.get(&key).cloned().unwrap_or_default()
    }

    pub(crate) fn refresh_for_session(
        &mut self,
        client: Arc<EnvironmentRuntimeClient>,
        session_id: SessionId,
        current_working_directory: Option<String>,
        ctx: &mut ModelContext<Self>,
    ) {
        let key = EnvironmentSkillInventoryKey {
            session_id,
            current_working_directory: current_working_directory.clone(),
        };
        if self.in_flight.contains(&key) {
            return;
        }
        self.in_flight.insert(key.clone());
        ctx.spawn(
            list_environment_skills(client, session_id, current_working_directory),
            move |cache, result, ctx| {
                cache.in_flight.remove(&key);
                match result {
                    Ok(skills) => {
                        cache.failures.remove(&key);
                        cache.inventories.insert(key, skills);
                    }
                    Err(error) => {
                        log::warn!("environment skill inventory refresh failed: {error}");
                        cache.failures.insert(key, error);
                    }
                }
                ctx.emit(EnvironmentSkillInventoryCacheEvent::InventoryChanged);
                ctx.notify();
            },
        );
    }

    pub(crate) fn refresh_for_session_if_missing(
        &mut self,
        client: Arc<EnvironmentRuntimeClient>,
        session_id: SessionId,
        current_working_directory: Option<String>,
        ctx: &mut ModelContext<Self>,
    ) {
        let key = EnvironmentSkillInventoryKey {
            session_id,
            current_working_directory: current_working_directory.clone(),
        };
        if self.inventories.contains_key(&key) || self.in_flight.contains(&key) {
            return;
        }

        self.refresh_for_session(client, session_id, current_working_directory, ctx);
    }

    pub(crate) fn is_refreshing_for_session(
        &self,
        session_id: SessionId,
        current_working_directory: Option<&str>,
    ) -> bool {
        let key = EnvironmentSkillInventoryKey {
            session_id,
            current_working_directory: current_working_directory.map(ToOwned::to_owned),
        };
        self.in_flight.contains(&key)
    }

    pub(crate) fn failure_for_session(
        &self,
        session_id: SessionId,
        current_working_directory: Option<&str>,
    ) -> Option<&str> {
        let key = EnvironmentSkillInventoryKey {
            session_id,
            current_working_directory: current_working_directory.map(ToOwned::to_owned),
        };
        self.failures.get(&key).map(String::as_str)
    }

    fn descriptor_for_reference(&self, reference: &SkillReference) -> Option<SkillDescriptor> {
        self.inventories
            .values()
            .flat_map(|skills| skills.iter())
            .find(|skill| &skill.reference == reference)
            .cloned()
    }
}

impl Entity for EnvironmentSkillInventoryCache {
    type Event = EnvironmentSkillInventoryCacheEvent;
}

impl SingletonEntity for EnvironmentSkillInventoryCache {}

pub(crate) fn current_app_skills_for_working_directory(
    working_directory: Option<&std::path::Path>,
    app: &AppContext,
) -> Vec<SkillDescriptor> {
    crate::ai::skills::SkillManager::as_ref(app)
        .get_skills_for_working_directory(working_directory, app)
}

pub(crate) fn cached_environment_skills_for_session(
    session_id: SessionId,
    current_working_directory: Option<&str>,
    app: &AppContext,
) -> Vec<SkillDescriptor> {
    EnvironmentSkillInventoryCache::as_ref(app)
        .cached_for_session(session_id, current_working_directory)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ActiveSkillInventorySource {
    CurrentApp,
    EnvironmentRuntime,
}

pub(crate) struct ActiveSessionSkillInventory {
    skills: Vec<SkillDescriptor>,
    source: ActiveSkillInventorySource,
}

impl ActiveSessionSkillInventory {
    pub(crate) fn into_skills(self) -> Vec<SkillDescriptor> {
        self.skills
    }

    pub(crate) fn into_skills_for_cli_agent_providers(
        self,
        cli_agent_providers: Option<&[SkillProvider]>,
        include_bundled: bool,
        app: &AppContext,
    ) -> Vec<SkillDescriptor> {
        let skill_manager = crate::ai::skills::SkillManager::as_ref(app);
        self.skills
            .into_iter()
            .filter_map(|mut skill| {
                if !include_bundled && matches!(skill.reference, SkillReference::BundledSkillId(_))
                {
                    return None;
                }

                if let Some(providers) = cli_agent_providers {
                    match self.source {
                        ActiveSkillInventorySource::EnvironmentRuntime => {
                            if !providers.contains(&skill.provider) {
                                return None;
                            }
                        }
                        ActiveSkillInventorySource::CurrentApp => {
                            if !skill_manager.skill_exists_for_any_provider(&skill, providers) {
                                return None;
                            }
                            skill.provider =
                                skill_manager.best_supported_provider(&skill, providers);
                        }
                    }
                }

                Some(skill)
            })
            .collect()
    }
}

pub(crate) fn active_terminal_session_skill_inventory(
    active_session: &TerminalActiveSession,
    app: &AppContext,
) -> ActiveSessionSkillInventory {
    let cwd = active_session.current_working_directory().cloned();
    let cwd_path = cwd.as_deref().map(std::path::Path::new);
    let uses_environment_runtime = active_session
        .session_type(app)
        .as_ref()
        .is_some_and(|session_type| session_type.uses_environment_runtime());
    if uses_environment_runtime {
        if let Some(session) = active_session.session(app) {
            return ActiveSessionSkillInventory {
                skills: cached_environment_skills_for_session(session.id(), cwd.as_deref(), app),
                source: ActiveSkillInventorySource::EnvironmentRuntime,
            };
        }
    }

    ActiveSessionSkillInventory {
        skills: current_app_skills_for_working_directory(cwd_path, app),
        source: ActiveSkillInventorySource::CurrentApp,
    }
}

pub(crate) fn skill_descriptor_for_reference(
    reference: &SkillReference,
    app: &AppContext,
) -> Option<SkillDescriptor> {
    EnvironmentSkillInventoryCache::as_ref(app)
        .descriptor_for_reference(reference)
        .or_else(|| {
            crate::ai::skills::SkillManager::as_ref(app)
                .skill_by_reference(reference)
                .cloned()
                .map(SkillDescriptor::from)
        })
}

pub(crate) fn skill_descriptor_for_path(
    path: &std::path::Path,
    app: &AppContext,
) -> Option<SkillDescriptor> {
    skill_descriptor_for_reference(&SkillReference::Path(path.to_path_buf()), app)
}

pub(crate) fn skill_name_for_reference(
    reference: &SkillReference,
    app: &AppContext,
) -> Option<String> {
    skill_descriptor_for_reference(reference, app).map(|skill| skill.name)
}

pub(crate) fn inventory_items_from_skill_descriptors(
    skills: Vec<SkillDescriptor>,
) -> Vec<SkillInventoryItem> {
    let mut by_name: HashMap<String, Vec<SkillInventoryDuplicate>> = HashMap::new();

    for skill in skills {
        let SkillReference::Path(path) = skill.reference else {
            continue;
        };
        by_name
            .entry(skill.name.clone())
            .or_default()
            .push(SkillInventoryDuplicate {
                path,
                name: skill.name,
                description: skill.description,
                content: String::new(),
                provider: skill.provider,
                scope: skill.scope,
            });
    }

    let mut items = by_name
        .into_iter()
        .filter_map(|(name, mut duplicates)| {
            duplicates.sort_by(|left, right| {
                provider_rank(left.provider)
                    .cmp(&provider_rank(right.provider))
                    .then_with(|| format!("{:?}", left.scope).cmp(&format!("{:?}", right.scope)))
                    .then_with(|| left.path.cmp(&right.path))
            });
            let default_skill = duplicates.first()?.clone();
            Some(SkillInventoryItem {
                name,
                default_skill,
                duplicates,
            })
        })
        .collect::<Vec<_>>();

    items.sort_by(|left, right| left.name.cmp(&right.name));
    items
}

pub(crate) async fn list_environment_skills(
    client: Arc<EnvironmentRuntimeClient>,
    session_id: SessionId,
    current_working_directory: Option<String>,
) -> Result<Vec<SkillDescriptor>, String> {
    let output = environment_runtime::run_command_output(
        &client,
        session_id,
        environment_skill_inventory_command(),
        current_working_directory,
        HashMap::new(),
    )
    .await?;

    match output.exit_code {
        Some(0) => parse_environment_skill_inventory_stdout(&output.stdout),
        _ => {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            if stderr.is_empty() {
                Err("Remote skill inventory scan failed".to_string())
            } else {
                Err(format!("Remote skill inventory scan failed: {stderr}"))
            }
        }
    }
}

pub(crate) fn parse_environment_skill_inventory_stdout(
    stdout: &[u8],
) -> Result<Vec<SkillDescriptor>, String> {
    let stdout = String::from_utf8(stdout.to_vec())
        .map_err(|_| "Remote skill inventory returned non-UTF8 output".to_string())?;
    let mut skills = Vec::new();
    let mut seen = HashSet::new();
    for line in stdout.lines().filter(|line| !line.trim().is_empty()) {
        let mut parts = line.splitn(4, '\t');
        let Some(provider_name) = parts.next() else {
            continue;
        };
        let Some(path) = parts.next() else { continue };
        let Some(name) = parts.next() else { continue };
        let Some(description) = parts.next() else {
            continue;
        };
        if path.is_empty() || name.is_empty() {
            continue;
        }
        let provider = provider_name.parse::<SkillProvider>().map_err(|_| {
            format!("Remote skill inventory returned unknown provider: {provider_name}")
        })?;
        let path = path.to_owned();
        if !seen.insert((provider, path.clone())) {
            continue;
        }
        skills.push(SkillDescriptor {
            reference: SkillReference::Path(PathBuf::from(path)),
            name: name.to_string(),
            description: description.to_string(),
            scope: SkillScope::Project,
            provider,
            icon_override: icon_override_for_skill_name(name),
        });
    }
    Ok(skills)
}

pub(crate) fn environment_skill_inventory_command() -> String {
    let provider_scan_lines = SKILL_PROVIDER_DEFINITIONS
        .iter()
        .map(|definition| {
            let provider = shell_quote(&definition.provider.to_string());
            let path = shell_quote(&definition.skills_path.to_string_lossy().replace('\\', "/"));
            format!("  scan_provider \"$root\" {provider} {path}")
        })
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        r#"set -u
field_value() {{
  key="$1"
  file="$2"
  awk -v key="$key" '
    NR == 1 && $0 == "---" {{ in_fm = 1; next }}
    in_fm && $0 == "---" {{ exit }}
    in_fm {{
      prefix = key ":"
      if (index($0, prefix) == 1) {{
        sub("^[^:]*:[[:space:]]*", "", $0)
        print $0
        exit
      }}
    }}
  ' "$file"
}}
clean_field() {{
  printf '%s' "$1" | tr '\t\r\n' '   '
}}
emit_skill() {{
  provider="$1"
  skill_file="$2"
  name=$(field_value name "$skill_file")
  if [ -z "$name" ]; then
    name=$(basename "$(dirname "$skill_file")")
  fi
  description=$(field_value description "$skill_file")
  name=$(clean_field "$name")
  description=$(clean_field "$description")
  printf '%s\t%s\t%s\t%s\n' "$provider" "$skill_file" "$name" "$description"
}}
scan_provider() {{
  root="$1"
  provider="$2"
  provider_path="$3"
  provider_dir="$root/$provider_path"
  [ -d "$provider_dir" ] || return 0
  find "$provider_dir" -mindepth 2 -maxdepth 2 -type f -name SKILL.md 2>/dev/null | while IFS= read -r skill_file; do
    emit_skill "$provider" "$skill_file"
  done
}}
scan_root() {{
  root="$1"
  [ -n "$root" ] || return 0
{provider_scan_lines}
}}
scan_ancestor_roots() {{
  dir="$1"
  while [ -n "$dir" ]; do
    scan_root "$dir"
    [ "$dir" = "/" ] && break
    parent=$(dirname "$dir")
    [ "$parent" = "$dir" ] && break
    dir="$parent"
  done
}}
if [ -n "${{PWD:-}}" ]; then
  scan_ancestor_roots "$PWD"
fi
if [ -n "${{HOME:-}}" ]; then
  scan_root "$HOME"
fi
"#
    )
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inventory_command_scans_pwd_ancestors_and_home_provider_roots() {
        let command = environment_skill_inventory_command();

        assert!(command.contains("scan_ancestor_roots \"$PWD\""));
        assert!(command.contains("scan_root \"$HOME\""));
        assert!(command.contains("'.agents/skills'"));
        assert!(command.contains("'.claude/skills'"));
        assert!(command.contains("-mindepth 2 -maxdepth 2 -type f -name SKILL.md"));
        assert!(command.contains("printf '%s\\t%s\\t%s\\t%s\\n'"));
    }

    #[test]
    fn parses_remote_inventory_rows_into_skill_descriptors() {
        let stdout = b"Agents\t/work/.agents/skills/build/SKILL.md\tbuild\tBuild things\nClaude\t/home/me/.claude/skills/review/SKILL.md\treview\tReview code\n";

        let skills = parse_environment_skill_inventory_stdout(stdout).unwrap();

        assert_eq!(skills.len(), 2);
        assert_eq!(skills[0].name, "build");
        assert_eq!(skills[0].provider, SkillProvider::Agents);
        assert_eq!(
            skills[0].reference,
            SkillReference::Path(PathBuf::from("/work/.agents/skills/build/SKILL.md"))
        );
        assert_eq!(skills[1].name, "review");
        assert_eq!(skills[1].provider, SkillProvider::Claude);
    }

    #[test]
    fn rejects_unknown_remote_inventory_provider() {
        let err = parse_environment_skill_inventory_stdout(
            b"Unknown\t/tmp/.unknown/skills/a/SKILL.md\ta\tdesc\n",
        )
        .unwrap_err();

        assert!(err.contains("unknown provider"));
    }

    #[test]
    fn deduplicates_remote_inventory_rows_by_provider_and_path() {
        let stdout = b"Claude\t/root/.claude/skills/review/SKILL.md\treview\tReview\nClaude\t/root/.claude/skills/review/SKILL.md\treview\tReview\nAgents\t/root/.agents/skills/review/SKILL.md\treview\tReview\n";

        let skills = parse_environment_skill_inventory_stdout(stdout).unwrap();

        assert_eq!(skills.len(), 2);
        assert_eq!(skills[0].provider, SkillProvider::Claude);
        assert_eq!(skills[1].provider, SkillProvider::Agents);
    }

    #[test]
    fn converts_remote_descriptors_to_inventory_items() {
        let skills = parse_environment_skill_inventory_stdout(
            b"Claude\t/work/.claude/skills/review/SKILL.md\treview\tReview\nAgents\t/work/.agents/skills/review/SKILL.md\treview\tReview\n",
        )
        .unwrap();

        let items = inventory_items_from_skill_descriptors(skills);

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "review");
        assert_eq!(items[0].duplicates.len(), 2);
        assert_eq!(items[0].default_skill.name, "review");
        assert!(items[0].default_skill.content.is_empty());
    }

    #[test]
    fn cached_inventory_is_scoped_to_exact_remote_cwd() {
        let session_id = SessionId::from(42);
        let mut cache = EnvironmentSkillInventoryCache::default();
        let key = EnvironmentSkillInventoryKey {
            session_id,
            current_working_directory: Some("/work/old".to_owned()),
        };
        cache.inventories.insert(
            key,
            parse_environment_skill_inventory_stdout(
                b"Agents\t/work/old/.agents/skills/old/SKILL.md\told\tOld\n",
            )
            .unwrap(),
        );

        assert!(cache
            .cached_for_session(session_id, Some("/work/new"))
            .is_empty());
        assert!(cache.cached_for_session(session_id, None).is_empty());
        assert_eq!(
            cache.cached_for_session(session_id, Some("/work/old"))[0].name,
            "old"
        );
    }

    #[test]
    fn refresh_and_failure_state_are_scoped_to_exact_remote_session_and_cwd() {
        let session_id = SessionId::from(42);
        let other_session_id = SessionId::from(43);
        let mut cache = EnvironmentSkillInventoryCache::default();
        let key = EnvironmentSkillInventoryKey {
            session_id,
            current_working_directory: Some("/work/current".to_owned()),
        };
        cache.in_flight.insert(key.clone());
        cache.failures.insert(key, "permission denied".to_owned());

        assert!(cache.is_refreshing_for_session(session_id, Some("/work/current")));
        assert_eq!(
            cache.failure_for_session(session_id, Some("/work/current")),
            Some("permission denied")
        );

        assert!(!cache.is_refreshing_for_session(session_id, Some("/work/other")));
        assert_eq!(
            cache.failure_for_session(session_id, Some("/work/other")),
            None
        );
        assert!(!cache.is_refreshing_for_session(session_id, None));
        assert_eq!(cache.failure_for_session(session_id, None), None);
        assert!(!cache.is_refreshing_for_session(other_session_id, Some("/work/current")));
        assert_eq!(
            cache.failure_for_session(other_session_id, Some("/work/current")),
            None
        );
    }
}
