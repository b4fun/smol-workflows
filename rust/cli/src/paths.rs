use anyhow::Context;
use std::env;
use std::path::PathBuf;

const APP_DIR: &str = "smol-workflows";
const DEFAULT_DATABASE_FILE: &str = "workflows.db";

pub fn default_database_path() -> anyhow::Result<PathBuf> {
    Ok(default_state_dir()?.join(DEFAULT_DATABASE_FILE))
}

fn default_state_dir() -> anyhow::Result<PathBuf> {
    platform_state_base_dir().map(|base| base.join(APP_DIR))
}

#[cfg(target_os = "windows")]
fn platform_state_base_dir() -> anyhow::Result<PathBuf> {
    if let Some(appdata) = non_empty_env("APPDATA") {
        return Ok(PathBuf::from(appdata));
    }
    if let Some(user_profile) = non_empty_env("USERPROFILE") {
        return Ok(PathBuf::from(user_profile).join("AppData").join("Roaming"));
    }
    anyhow::bail!("could not resolve default database path: APPDATA and USERPROFILE are unset")
}

#[cfg(target_os = "macos")]
fn platform_state_base_dir() -> anyhow::Result<PathBuf> {
    let home =
        non_empty_env("HOME").context("could not resolve default database path: HOME is unset")?;
    Ok(PathBuf::from(home)
        .join("Library")
        .join("Application Support"))
}

#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
fn platform_state_base_dir() -> anyhow::Result<PathBuf> {
    if let Some(xdg_state_home) = non_empty_env("XDG_STATE_HOME") {
        return Ok(PathBuf::from(xdg_state_home));
    }
    let home = non_empty_env("HOME")
        .context("could not resolve default database path: XDG_STATE_HOME and HOME are unset")?;
    Ok(PathBuf::from(home).join(".local").join("state"))
}

fn non_empty_env(name: &str) -> Option<String> {
    env::var(name).ok().filter(|value| !value.is_empty())
}
