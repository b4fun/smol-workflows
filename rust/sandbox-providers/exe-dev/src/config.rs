use crate::error::{bad_profile, provider_failure, ProviderResult};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::PathBuf;

pub const CONFIG_ENV: &str = "SMOL_SANDBOX_EXE_DEV_CONFIG";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_profiles")]
    pub profiles: BTreeMap<String, ProfileConfig>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            profiles: default_profiles(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileConfig {
    #[serde(default = "default_image")]
    pub image: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    #[serde(default = "default_cwd")]
    pub cwd: String,
    #[serde(default = "default_true")]
    pub sync_workspace: bool,
    #[serde(default)]
    pub workspace_sync: WorkspaceSyncConfig,
    #[serde(default)]
    pub control_plane: ControlPlaneConfig,
    #[serde(default)]
    pub ssh: SshConfig,
    #[serde(default)]
    pub cleanup: CleanupConfig,
}

impl Default for ProfileConfig {
    fn default() -> Self {
        Self {
            image: default_image(),
            region: None,
            cwd: default_cwd(),
            sync_workspace: true,
            workspace_sync: WorkspaceSyncConfig::default(),
            control_plane: ControlPlaneConfig::default(),
            ssh: SshConfig::default(),
            cleanup: CleanupConfig::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceSyncConfig {
    #[serde(default = "default_workspace_sync_mode")]
    pub mode: String,
    #[serde(default = "default_workspace_excludes")]
    pub exclude: Vec<String>,
}

impl Default for WorkspaceSyncConfig {
    fn default() -> Self {
        Self {
            mode: default_workspace_sync_mode(),
            exclude: default_workspace_excludes(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlPlaneConfig {
    #[serde(default = "default_control_plane_mode")]
    pub mode: String,
}

impl Default for ControlPlaneConfig {
    fn default() -> Self {
        Self {
            mode: default_control_plane_mode(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SshConfig {
    #[serde(default = "default_ssh_program")]
    pub program: String,
    #[serde(default = "default_ssh_extra_args")]
    pub extra_args: Vec<String>,
}

impl Default for SshConfig {
    fn default() -> Self {
        Self {
            program: default_ssh_program(),
            extra_args: default_ssh_extra_args(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CleanupConfig {
    #[serde(default = "default_cleanup_delete")]
    pub on_close: String,
    #[serde(default = "default_cleanup_delete")]
    pub on_error: String,
    #[serde(default = "default_keep_env")]
    pub keep_env: String,
}

impl Default for CleanupConfig {
    fn default() -> Self {
        Self {
            on_close: default_cleanup_delete(),
            on_error: default_cleanup_delete(),
            keep_env: default_keep_env(),
        }
    }
}

pub fn load_config() -> ProviderResult<Config> {
    if let Some(path) = explicit_config_path() {
        return read_config(path);
    }

    for path in default_config_paths() {
        if path.exists() {
            return read_config(path);
        }
    }

    Ok(Config::default())
}

pub fn profile_for<'a>(
    config: &'a Config,
    provider: &str,
    name: &str,
) -> ProviderResult<&'a ProfileConfig> {
    if provider != "exe-dev" {
        return Err(bad_profile(format!(
            "profile provider `{provider}` is not supported by smol-sandbox-exe-dev"
        )));
    }
    config
        .profiles
        .get(name)
        .ok_or_else(|| bad_profile(format!("unknown exe.dev sandbox profile `{name}`")))
}

pub fn explicit_config_path() -> Option<PathBuf> {
    env::var_os(CONFIG_ENV).map(PathBuf::from)
}

pub fn default_config_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    if let Some(config_base) = env::var_os("CONFIG_BASE") {
        paths.push(
            PathBuf::from(config_base)
                .join("sandbox-providers")
                .join("exe-dev")
                .join("config.json"),
        );
    }

    if let Some(xdg_config_home) = env::var_os("XDG_CONFIG_HOME") {
        paths.push(
            PathBuf::from(xdg_config_home)
                .join("smol-workflows")
                .join("sandbox-providers")
                .join("exe-dev")
                .join("config.json"),
        );
    }

    if let Some(home) = env::var_os("HOME") {
        paths.push(
            PathBuf::from(home)
                .join(".config")
                .join("smol-workflows")
                .join("sandbox-providers")
                .join("exe-dev")
                .join("config.json"),
        );
    }

    paths
}

fn read_config(path: PathBuf) -> ProviderResult<Config> {
    let text = fs::read_to_string(&path).map_err(|source| {
        provider_failure(format!(
            "failed to read exe.dev sandbox config `{}`: {source}",
            path.display()
        ))
    })?;
    serde_json::from_str(&text).map_err(|source| {
        provider_failure(format!(
            "failed to parse exe.dev sandbox config `{}`: {source}",
            path.display()
        ))
    })
}

fn default_profiles() -> BTreeMap<String, ProfileConfig> {
    BTreeMap::from([("default".to_string(), ProfileConfig::default())])
}

fn default_image() -> String {
    "exeuntu".to_string()
}

fn default_cwd() -> String {
    "/home/exedev/workspace".to_string()
}

fn default_true() -> bool {
    true
}

fn default_workspace_sync_mode() -> String {
    "tar".to_string()
}

fn default_workspace_excludes() -> Vec<String> {
    vec![
        ".git".to_string(),
        "target".to_string(),
        "node_modules".to_string(),
    ]
}

fn default_control_plane_mode() -> String {
    "ssh".to_string()
}

fn default_ssh_program() -> String {
    "ssh".to_string()
}

fn default_ssh_extra_args() -> Vec<String> {
    vec![
        "-o".to_string(),
        "StrictHostKeyChecking=accept-new".to_string(),
        "-o".to_string(),
        "ServerAliveInterval=15".to_string(),
        "-o".to_string(),
        "ServerAliveCountMax=4".to_string(),
    ]
}

fn default_cleanup_delete() -> String {
    "delete".to_string()
}

fn default_keep_env() -> String {
    "SMOL_EXE_DEV_KEEP".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_profile_with_defaults() {
        let config: Config = serde_json::from_str(
            r#"{
              "profiles": {
                "fake": {
                  "image": "alpine:latest",
                  "ssh": { "program": "/tmp/fake-ssh" },
                  "cleanup": { "on_close": "keep" }
                }
              }
            }"#,
        )
        .unwrap();
        let profile = profile_for(&config, "exe-dev", "fake").unwrap();
        assert_eq!(profile.image, "alpine:latest");
        assert_eq!(profile.cwd, "/home/exedev/workspace");
        assert_eq!(profile.ssh.program, "/tmp/fake-ssh");
        assert_eq!(profile.cleanup.on_close, "keep");
        assert_eq!(profile.control_plane.mode, "ssh");
    }

    #[test]
    fn rejects_unknown_provider_and_profile() {
        let config = Config::default();
        assert_eq!(
            profile_for(&config, "local", "default").unwrap_err().code,
            "bad_profile"
        );
        assert_eq!(
            profile_for(&config, "exe-dev", "missing").unwrap_err().code,
            "bad_profile"
        );
    }
}
