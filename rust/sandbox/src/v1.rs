//! Sandbox provider JSON protocol v1 types.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Current sandbox provider protocol version.
pub const PROTOCOL_VERSION: &str = "sandbox.v1";

/// Root schema holder used to generate the combined JSON Schema for protocol v1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SandboxProviderV1Schema {
    pub capabilities_request: CapabilitiesRequest,
    pub capabilities_response: CapabilitiesResponse,
    pub open_request: OpenSandboxRequest,
    pub open_response: OpenSandboxResponse,
    pub close_request: CloseSandboxRequest,
    pub close_response: CloseSandboxResponse,
    pub cleanup_group_request: CleanupSandboxGroupRequest,
    pub cleanup_group_response: CleanupSandboxGroupResponse,
    pub exec_request: ExecRequest,
    pub exec_response: ExecResponse,
}

/// Opaque metadata attached to each plugin request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Metadata {
    /// Protocol version, for example [`PROTOCOL_VERSION`].
    pub protocol_version: String,
    /// Opaque runtime-generated ID for correlating one plugin call with runtime logs.
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
    /// Local path on the machine running the workflow runner/provider plugin.
    pub host_path: PathBuf,
}

/// Request for `capabilities`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CapabilitiesRequest {
    pub metadata: Metadata,
}

/// Response from `capabilities`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CapabilitiesResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Capabilities>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ProviderError>,
}

/// Optional provider behavior. Lifecycle commands are required and are not reported here.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Capabilities {
    /// Whether the provider supports the future optional `exec` command.
    #[serde(default)]
    pub exec: bool,
}

/// Request for `open`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OpenSandboxRequest {
    pub metadata: Metadata,
    pub profile: ProfileRef,
    pub workspace_sync: WorkspaceSync,
    /// Optional sandbox-internal cwd override. This is not a host path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

/// Response from `open`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OpenSandboxResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<SandboxSession>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ProviderError>,
}

/// Provider session returned by `open` and passed back to later calls.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SandboxSession {
    /// Runtime/provider-facing session ID returned by the plugin.
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

/// Request for `close`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CloseSandboxRequest {
    pub metadata: Metadata,
    pub session: SandboxSession,
}

/// Response from `close`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CloseSandboxResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ProviderError>,
}

/// Request for `cleanup-group`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CleanupSandboxGroupRequest {
    pub metadata: Metadata,
    pub sandbox_group_id: String,
}

/// Response from `cleanup-group`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CleanupSandboxGroupResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleaned_count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ProviderError>,
}

/// Request for future optional `exec`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExecRequest {
    pub metadata: Metadata,
    pub session: SandboxSession,
    pub command: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdin_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

/// Successful output from future optional `exec`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExecOutput {
    pub exit_code: i32,
    #[serde(default)]
    pub stdout_text: String,
    #[serde(default)]
    pub stderr_text: String,
    #[serde(default)]
    pub duration_ms: u64,
}

/// Response from future optional `exec`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExecResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<ExecOutput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ProviderError>,
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
