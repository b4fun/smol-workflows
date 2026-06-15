//! Sandbox provider JSONL protocol v1 types.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Current sandbox provider protocol version.
pub const PROTOCOL_VERSION: &str = "sandbox.v1";

/// Root schema holder used to generate the combined JSON Schema for protocol v1.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SandboxProviderV1Schema {
    pub request_envelope: crate::jsonl::JsonlRequestEnvelope<serde_json::Value>,
    pub response_envelope: crate::jsonl::JsonlResponseEnvelope,
    pub open_request: OpenSandboxRequest,
    pub sandbox_session: SandboxSession,
    pub cleanup_group_request: CleanupSandboxGroupRequest,
    pub create_temp_dir_request: crate::jsonl::CreateTempDirRequest,
    pub create_temp_dir_result: crate::jsonl::CreateTempDirResult,
    pub session_path_request: crate::jsonl::SessionPathRequest,
    pub write_file_request: crate::jsonl::WriteFileRequest,
    pub read_file_result: crate::jsonl::ReadFileResult,
    pub exec_request: crate::jsonl::SandboxExecRequest,
    pub exec_result: crate::jsonl::SandboxExecResult,
    pub exec_event: crate::jsonl::SandboxExecEvent,
    pub spawn_request: crate::jsonl::SandboxSpawnRequest,
    pub spawn_result: crate::jsonl::SandboxSpawnResult,
    pub provider_error: ProviderError,
}

/// Opaque metadata attached to sandbox lifecycle requests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Metadata {
    /// Protocol version, for example [`PROTOCOL_VERSION`].
    pub protocol_version: String,
    /// Opaque runtime-generated ID for correlating a logical operation with logs.
    pub request_id: String,
    /// Opaque runtime-generated group ID for sandbox resources owned by one run.
    pub sandbox_group_id: String,
    /// Optional non-sensitive provider tags.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tags: BTreeMap<String, String>,
}

impl Metadata {
    /// Construct metadata using the current protocol version.
    pub fn new(request_id: impl Into<String>, sandbox_group_id: impl Into<String>) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION.to_string(),
            request_id: request_id.into(),
            sandbox_group_id: sandbox_group_id.into(),
            tags: BTreeMap::new(),
        }
    }
}

/// Provider-local profile reference selected by the workflow runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProfileRef {
    /// Provider name selected by the runtime.
    pub provider: String,
    /// Profile name local to the provider.
    pub name: String,
}

/// Local workspace path supplied by the workflow runner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WorkspaceSync {
    /// Local path on the machine running the workflow runner/provider process.
    pub host_path: PathBuf,
}

/// Request params for the JSONL `open` method.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OpenSandboxRequest {
    pub metadata: Metadata,
    pub profile: ProfileRef,
    pub workspace_sync: WorkspaceSync,
    /// Optional sandbox-internal cwd override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

/// Provider session returned by `open` and passed back to later calls.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SandboxSession {
    /// Runtime/provider-facing session ID returned by the provider.
    pub id: String,
    /// Provider-native session ID, if different.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_session_id: Option<String>,
    /// Effective sandbox cwd, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Capabilities for this session.
    #[serde(default)]
    pub capabilities: Capabilities,
    /// Opaque provider state for later calls. Treat as sensitive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_state_json: Option<String>,
}

/// Provider behavior flags.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Capabilities {
    /// Whether the provider supports `exec` for sessions it opens.
    #[serde(default)]
    pub exec: bool,
}

/// Request params for `cleanup_group`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CleanupSandboxGroupRequest {
    pub metadata: Metadata,
    pub sandbox_group_id: String,
}

/// Provider-declared operation error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, thiserror::Error)]
#[error("sandbox provider error {code}: {message}")]
pub struct ProviderError {
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub retryable: bool,
}
