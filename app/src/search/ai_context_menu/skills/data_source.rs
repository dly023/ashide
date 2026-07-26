use super::search_item::SkillSearchItem;
use crate::ai::skills::environment_skill_inventory::{
    cached_environment_skills_for_session, current_app_skills_for_working_directory,
};
use crate::search::ai_context_menu::mixer::AIContextMenuSearchableAction;
use crate::search::data_source::{Query, QueryResult};
use crate::search::mixer::{DataSourceRunErrorWrapper, SyncDataSource};
use fuzzy_match::FuzzyMatchResult;
use std::path::PathBuf;
use warpui::{AppContext, Entity, SingletonEntity};

#[cfg(not(target_family = "wasm"))]
use crate::workspace::ActiveSession;

const MAX_RESULTS: usize = 50;

pub struct SkillsDataSource;

impl SkillsDataSource {
    pub fn new() -> Self {
        Self
    }
}

impl SyncDataSource for SkillsDataSource {
    type Action = AIContextMenuSearchableAction;

    fn run_query(
        &self,
        query: &Query,
        app: &AppContext,
    ) -> Result<Vec<QueryResult<Self::Action>>, DataSourceRunErrorWrapper> {
        let query_text = &query.text;

        // Resolve the current working directory from the active window's session.
        let cwd: Option<PathBuf> = {
            #[cfg(not(target_family = "wasm"))]
            {
                let active_session = ActiveSession::as_ref(app);
                if let Some((window_id, session)) = app
                    .windows()
                    .state()
                    .active_window
                    .and_then(|window_id| {
                        active_session
                            .session(window_id)
                            .map(|session| (window_id, session))
                    })
                    .filter(|(_, session)| session.session_type().uses_environment_runtime())
                {
                    let current_working_directory = active_session
                        .current_working_directory(window_id)
                        .map(str::to_owned);
                    let skills = cached_environment_skills_for_session(
                        session.id(),
                        current_working_directory.as_deref(),
                        app,
                    );
                    return Ok(skill_results(skills, query_text));
                }

                if !crate::workspace::active_window_environment_allows_current_app_skill_manager(
                    app,
                ) {
                    return Ok(Vec::new());
                }

                app.windows()
                    .state()
                    .active_window
                    .and_then(|window_id| active_session.current_app_path(window_id))
                    .map(PathBuf::from)
            }
            #[cfg(target_family = "wasm")]
            {
                None
            }
        };

        let skills = current_app_skills_for_working_directory(cwd.as_deref(), app);

        Ok(skill_results(skills, query_text))
    }
}

fn skill_results(
    skills: Vec<crate::ai::skills::SkillDescriptor>,
    query_text: &str,
) -> Vec<QueryResult<AIContextMenuSearchableAction>> {
    let mut results: Vec<QueryResult<AIContextMenuSearchableAction>> = if query_text.is_empty() {
        // Zero state: show all skills with a uniform high score.
        skills
            .into_iter()
            .map(|skill| {
                QueryResult::from(SkillSearchItem {
                    name: skill.name,
                    description: skill.description,
                    provider: skill.provider,
                    icon_override: skill.icon_override,
                    match_result: FuzzyMatchResult {
                        score: 1000,
                        matched_indices: vec![],
                    },
                })
            })
            .collect()
    } else {
        // Fuzzy match against skill name.
        skills
            .into_iter()
            .filter_map(|skill| {
                let match_result =
                    fuzzy_match::match_indices_case_insensitive(&skill.name, query_text)?;
                // Skip very weak matches once the user has typed more than one character.
                if query_text.len() > 1 && match_result.score < 10 {
                    return None;
                }
                Some(QueryResult::from(SkillSearchItem {
                    name: skill.name,
                    description: skill.description,
                    provider: skill.provider,
                    icon_override: skill.icon_override,
                    match_result,
                }))
            })
            .collect()
    };

    results.sort_by_key(|r| std::cmp::Reverse(r.score()));
    results.truncate(MAX_RESULTS);
    results
}

impl Entity for SkillsDataSource {
    type Event = ();
}
