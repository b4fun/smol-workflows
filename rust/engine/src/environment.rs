//! Execution environment abstraction for provider filesystem and process IO.
//!
//! This module contains the Rust-facing environment capability documented in
//! `docs/harness-capabilities/environment.md`. Local in-process execution lives
//! in [`local`].

mod local;
mod sandbox;

use anyhow::anyhow;
use std::collections::BTreeMap;
use std::path::Path;

pub use local::LocalExecutionEnvironment;
pub use sandbox::SandboxExecutionEnvironment;

/// UTF-8 path in the selected execution environment.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct EnvironmentPath(pub String);

impl EnvironmentPath {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for EnvironmentPath {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for EnvironmentPath {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

/// Request to run a foreground/background command inside an environment.
#[derive(Debug, Clone, Default)]
pub struct ExecRequest {
    /// Executable and arguments. `argv[0]` is the executable.
    pub argv: Vec<String>,
    /// Optional environment-local working directory override.
    pub cwd: Option<EnvironmentPath>,
    /// Per-call process environment overrides.
    pub env: BTreeMap<String, String>,
    /// Optional stdin bytes.
    pub stdin: Option<Vec<u8>>,
}

/// Foreground command result.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExecOutput {
    pub exit_code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Generic process event emitted by an execution environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecEvent {
    Started { process_id: Option<String> },
    Stdout { chunk: Vec<u8> },
    Stderr { chunk: Vec<u8> },
    Exited { exit_code: i32 },
}

#[async_trait::async_trait]
pub trait ExecEventSink: Send {
    async fn event(&mut self, event: ExecEvent) -> anyhow::Result<()>;
}

/// Background process start result.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpawnOutput {
    pub process_id: Option<String>,
}

#[async_trait::async_trait]
pub trait AgentExecutionEnvironment: Send + Sync {
    fn cwd(&self) -> Option<&EnvironmentPath>;

    async fn create_dir_all(&self, path: &EnvironmentPath) -> anyhow::Result<()>;

    async fn write_file(&self, path: &EnvironmentPath, content: &[u8]) -> anyhow::Result<()>;

    async fn read_file(&self, path: &EnvironmentPath) -> anyhow::Result<Vec<u8>>;

    async fn remove(&self, path: &EnvironmentPath) -> anyhow::Result<()>;

    async fn create_temp_dir(&self, prefix: &str) -> anyhow::Result<EnvironmentPath>;

    async fn exec(
        &self,
        request: ExecRequest,
        sink: &mut dyn ExecEventSink,
    ) -> anyhow::Result<ExecOutput>;

    async fn spawn(
        &self,
        request: ExecRequest,
        sink: Option<Box<dyn ExecEventSink>>,
    ) -> anyhow::Result<SpawnOutput>;
}

/// Event sink that ignores all events.
#[derive(Debug, Default)]
pub struct NullExecEventSink;

#[async_trait::async_trait]
impl ExecEventSink for NullExecEventSink {
    async fn event(&mut self, _event: ExecEvent) -> anyhow::Result<()> {
        Ok(())
    }
}

pub(crate) fn path_to_environment_path(path: impl AsRef<Path>) -> anyhow::Result<EnvironmentPath> {
    let path = path.as_ref();
    let value = path
        .to_str()
        .ok_or_else(|| anyhow!("environment paths must be valid UTF-8: {path:?}"))?;
    Ok(EnvironmentPath(value.to_string()))
}
