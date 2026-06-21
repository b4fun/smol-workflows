use crate::error::{provider_failure, ProviderResult};
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const STATE_DIR_ENV: &str = "SMOL_SANDBOX_EXE_DEV_STATE_DIR";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderState {
    pub sandbox_group_id: String,
    pub session_id: String,
    pub vm_name: String,
    pub ssh_dest: String,
    pub cwd: String,
    pub created_at_unix: u64,
    pub cleanup_on_close: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spawned_pids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_name: Option<String>,
}

impl ProviderState {
    pub fn new(
        sandbox_group_id: impl Into<String>,
        session_id: impl Into<String>,
        vm_name: impl Into<String>,
        ssh_dest: impl Into<String>,
        cwd: impl Into<String>,
        cleanup_on_close: bool,
    ) -> Self {
        Self {
            sandbox_group_id: sandbox_group_id.into(),
            session_id: session_id.into(),
            vm_name: vm_name.into(),
            ssh_dest: ssh_dest.into(),
            cwd: cwd.into(),
            created_at_unix: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_secs())
                .unwrap_or(0),
            cleanup_on_close,
            spawned_pids: Vec::new(),
            profile_name: None,
        }
    }

    pub fn with_profile_name(mut self, profile_name: impl Into<String>) -> Self {
        self.profile_name = Some(profile_name.into());
        self
    }

    pub fn to_provider_state_json(&self) -> ProviderResult<String> {
        serde_json::to_string(self).map_err(|source| {
            provider_failure(format!("failed to serialize provider state: {source}"))
        })
    }

    pub fn from_provider_state_json(value: &str) -> ProviderResult<Self> {
        serde_json::from_str(value)
            .map_err(|source| provider_failure(format!("failed to parse provider state: {source}")))
    }
}

pub fn persist_state(state: &ProviderState) -> ProviderResult<PathBuf> {
    let dir = default_state_dir();
    fs::create_dir_all(&dir).map_err(|source| {
        provider_failure(format!(
            "failed to create exe.dev sandbox state directory `{}`: {source}",
            dir.display()
        ))
    })?;
    let path = state_path(&dir, &state.session_id, &state.vm_name);
    let text = serde_json::to_string_pretty(state).map_err(|source| {
        provider_failure(format!("failed to serialize provider state: {source}"))
    })?;
    fs::write(&path, text).map_err(|source| {
        provider_failure(format!(
            "failed to write exe.dev sandbox state `{}`: {source}",
            path.display()
        ))
    })?;
    Ok(path)
}

pub fn remove_state(state: &ProviderState) -> ProviderResult<()> {
    let path = state_path(&default_state_dir(), &state.session_id, &state.vm_name);
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(provider_failure(format!(
            "failed to remove exe.dev sandbox state `{}`: {source}",
            path.display()
        ))),
    }
}

pub fn load_persisted_state(state: &ProviderState) -> ProviderResult<Option<ProviderState>> {
    let path = state_path(&default_state_dir(), &state.session_id, &state.vm_name);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(provider_failure(format!(
                "failed to read exe.dev sandbox state `{}`: {source}",
                path.display()
            )))
        }
    };
    serde_json::from_str(&text).map(Some).map_err(|source| {
        provider_failure(format!(
            "failed to parse exe.dev sandbox state `{}`: {source}",
            path.display()
        ))
    })
}

pub fn load_group_states(sandbox_group_id: &str) -> ProviderResult<Vec<ProviderState>> {
    let dir = default_state_dir();
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(provider_failure(format!(
                "failed to read exe.dev sandbox state directory `{}`: {source}",
                dir.display()
            )))
        }
    };

    let mut states = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(state) = serde_json::from_str::<ProviderState>(&text) else {
            continue;
        };
        if state.sandbox_group_id == sandbox_group_id {
            states.push(state);
        }
    }
    Ok(states)
}

pub fn default_state_dir() -> PathBuf {
    if let Some(path) = env::var_os(STATE_DIR_ENV) {
        return PathBuf::from(path);
    }
    if let Some(xdg_state_home) = env::var_os("XDG_STATE_HOME") {
        return PathBuf::from(xdg_state_home)
            .join("smol-workflows")
            .join("sandbox-providers")
            .join("exe-dev");
    }
    if let Some(home) = env::var_os("HOME") {
        return PathBuf::from(home)
            .join(".local")
            .join("state")
            .join("smol-workflows")
            .join("sandbox-providers")
            .join("exe-dev");
    }
    env::temp_dir()
        .join("smol-workflows")
        .join("sandbox-providers")
        .join("exe-dev")
}

fn state_path(dir: &Path, session_id: &str, vm_name: &str) -> PathBuf {
    dir.join(format!(
        "{}--{}.json",
        safe_file_component(session_id),
        safe_file_component(vm_name)
    ))
}

fn safe_file_component(value: &str) -> String {
    let safe: String = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect();
    if safe.is_empty() {
        "unknown".to_string()
    } else {
        safe
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_state_round_trips_json() {
        let state = ProviderState::new(
            "sbxgrp_1",
            "session_1",
            "smol-test",
            "smol-test.exe.xyz",
            "/workspace",
            true,
        );
        let encoded = state.to_provider_state_json().unwrap();
        let decoded = ProviderState::from_provider_state_json(&encoded).unwrap();
        assert_eq!(decoded.sandbox_group_id, "sbxgrp_1");
        assert_eq!(decoded.session_id, "session_1");
        assert!(decoded.cleanup_on_close);
    }

    #[test]
    fn safe_file_components_do_not_escape_state_dir() {
        assert_eq!(safe_file_component("../session:1"), "---session-1");
    }
}
