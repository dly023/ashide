//! Ashide-owned user state for CLI-agent sessions on the daemon host.
//!
//! Provider session sources (`~/.claude`, `~/.codex`) are mutated by
//! `cli_agent_sessions`. This module owns UI personalization state
//! (alias / pinned) and stores it in the environment
//! user's Ashide config directory on the same host as the daemon.

#![cfg(feature = "local_fs")]

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SessionUserState {
    #[serde(default)]
    pub aliases: HashMap<String, String>,
    #[serde(default)]
    pub pinned: HashSet<String>,
}

pub enum SessionUserStateMutation {
    SetAlias(String),
    ClearAlias,
    SetPinned,
    ClearPinned,
}

pub fn read_state() -> Result<SessionUserState, String> {
    read_state_from_path(&state_path()?)
}

pub fn mutate_state(
    keys: impl IntoIterator<Item = String>,
    mutation: SessionUserStateMutation,
) -> Result<SessionUserState, String> {
    let path = state_path()?;
    let mut state = read_state_from_path(&path)?;
    let keys = keys
        .into_iter()
        .map(|key| key.trim().to_owned())
        .filter(|key| !key.is_empty())
        .collect::<Vec<_>>();
    if keys.is_empty() {
        return Err("session user-state mutation has no keys".to_owned());
    }

    match mutation {
        SessionUserStateMutation::SetAlias(alias) => {
            let alias = alias.trim();
            if alias.is_empty() {
                for key in keys {
                    state.aliases.remove(&key);
                }
            } else {
                for key in keys {
                    state.aliases.insert(key, alias.to_owned());
                }
            }
        }
        SessionUserStateMutation::ClearAlias => {
            for key in keys {
                state.aliases.remove(&key);
            }
        }
        SessionUserStateMutation::SetPinned => {
            state.pinned.extend(keys);
        }
        SessionUserStateMutation::ClearPinned => {
            for key in keys {
                state.pinned.remove(&key);
            }
        }
    }

    write_state_to_path(&path, &state)?;
    Ok(state)
}

fn state_path() -> Result<PathBuf, String> {
    warp_core::paths::warp_home_config_dir()
        .map(|dir| dir.join("session_state.json"))
        .ok_or_else(|| "home directory is unavailable".to_owned())
}

fn read_state_from_path(path: &Path) -> Result<SessionUserState, String> {
    let Ok(contents) = fs::read_to_string(path) else {
        return Ok(SessionUserState::default());
    };
    serde_json::from_str::<SessionUserState>(&contents)
        .map(sanitize_state)
        .map_err(|error| format!("failed to decode {}: {error}", path.display()))
}

fn sanitize_state(mut state: SessionUserState) -> SessionUserState {
    state.aliases = state
        .aliases
        .into_iter()
        .filter_map(|(key, alias)| {
            let key = key.trim().to_owned();
            let alias = alias.trim().to_owned();
            (!key.is_empty() && !alias.is_empty()).then_some((key, alias))
        })
        .collect();
    state.pinned = state
        .pinned
        .into_iter()
        .map(|key| key.trim().to_owned())
        .filter(|key| !key.is_empty())
        .collect();
    state
}

fn write_state_to_path(path: &Path, state: &SessionUserState) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    }
    let contents = serde_json::to_string_pretty(&sanitize_state(state.clone()))
        .map_err(|error| format!("failed to encode session user state: {error}"))?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, contents)
        .map_err(|error| format!("failed to write {}: {error}", tmp.display()))?;
    fs::rename(&tmp, path).map_err(|error| format!("failed to replace {}: {error}", path.display()))
}
