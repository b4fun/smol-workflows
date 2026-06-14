//! Local binary plugin client for sandbox provider protocol v1.

use crate::v1::{
    Capabilities, CapabilitiesRequest, CapabilitiesResponse, CleanupSandboxGroupRequest,
    CleanupSandboxGroupResponse, CloseSandboxRequest, CloseSandboxResponse, ExecOutput,
    ExecRequest, ExecResponse, OpenSandboxRequest, OpenSandboxResponse, ProviderError,
    SandboxSession,
};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

/// Local sandbox provider plugin client.
#[derive(Debug, Clone)]
pub struct SandboxProviderPlugin {
    program: PathBuf,
}

impl SandboxProviderPlugin {
    /// Create a plugin client for a local provider executable.
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
        }
    }

    /// Return the configured local executable path.
    pub fn program(&self) -> &Path {
        &self.program
    }

    /// Invoke `capabilities` and return the extracted success payload.
    pub async fn capabilities(
        &self,
        request: &CapabilitiesRequest,
    ) -> Result<Capabilities, PluginClientError> {
        let response: CapabilitiesResponse = self.invoke("capabilities", request).await?;
        fail_if_provider_error(response.error)?;
        response.capabilities.ok_or_else(|| {
            PluginClientError::Protocol(
                "capabilities response missing `capabilities` success payload".to_string(),
            )
        })
    }

    /// Invoke `open` and return the extracted sandbox session.
    pub async fn open(
        &self,
        request: &OpenSandboxRequest,
    ) -> Result<SandboxSession, PluginClientError> {
        let response: OpenSandboxResponse = self.invoke("open", request).await?;
        fail_if_provider_error(response.error)?;
        response.session.ok_or_else(|| {
            PluginClientError::Protocol(
                "open response missing `session` success payload".to_string(),
            )
        })
    }

    /// Invoke `close`.
    pub async fn close(&self, request: &CloseSandboxRequest) -> Result<(), PluginClientError> {
        let response: CloseSandboxResponse = self.invoke("close", request).await?;
        fail_if_provider_error(response.error)
    }

    /// Invoke `cleanup-group` and return the number of cleaned resources.
    pub async fn cleanup_group(
        &self,
        request: &CleanupSandboxGroupRequest,
    ) -> Result<u32, PluginClientError> {
        let response: CleanupSandboxGroupResponse = self.invoke("cleanup-group", request).await?;
        fail_if_provider_error(response.error)?;
        response.cleaned_count.ok_or_else(|| {
            PluginClientError::Protocol(
                "cleanup-group response missing `cleaned_count` success payload".to_string(),
            )
        })
    }

    /// Invoke future optional `exec` and return the extracted command output.
    pub async fn exec(&self, request: &ExecRequest) -> Result<ExecOutput, PluginClientError> {
        let response: ExecResponse = self.invoke("exec", request).await?;
        fail_if_provider_error(response.error)?;
        response.output.ok_or_else(|| {
            PluginClientError::Protocol(
                "exec response missing `output` success payload".to_string(),
            )
        })
    }

    /// Invoke an arbitrary plugin subcommand with a JSON request/response pair.
    pub async fn invoke<Req, Res>(
        &self,
        subcommand: &str,
        request: &Req,
    ) -> Result<Res, PluginClientError>
    where
        Req: Serialize + ?Sized,
        Res: DeserializeOwned,
    {
        let request_json = serde_json::to_vec(request).map_err(PluginClientError::EncodeRequest)?;
        let mut child = Command::new(&self.program)
            .arg(subcommand)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|source| PluginClientError::Spawn {
                program: self.program.clone(),
                source,
            })?;

        let mut stdin = child.stdin.take().ok_or(PluginClientError::MissingStdin)?;
        stdin.write_all(&request_json).await?;
        stdin.shutdown().await?;
        drop(stdin);

        let output = child.wait_with_output().await?;
        if !output.status.success() {
            return Err(PluginClientError::Exit {
                status: output.status,
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
                stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            });
        }

        serde_json::from_slice(&output.stdout).map_err(PluginClientError::DecodeResponse)
    }
}

fn fail_if_provider_error(error: Option<ProviderError>) -> Result<(), PluginClientError> {
    match error {
        Some(error) => Err(PluginClientError::Provider(error)),
        None => Ok(()),
    }
}

/// Errors raised by the local plugin client.
#[derive(Debug, thiserror::Error)]
pub enum PluginClientError {
    #[error("failed to spawn sandbox provider plugin `{}`: {source}", program.display())]
    Spawn {
        program: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("sandbox provider plugin stdin was not available")]
    MissingStdin,
    #[error("sandbox provider plugin IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to encode sandbox provider request: {0}")]
    EncodeRequest(serde_json::Error),
    #[error("sandbox provider plugin exited with {status}; stderr: {stderr}")]
    Exit {
        status: ExitStatus,
        stderr: String,
        stdout: String,
    },
    #[error("failed to decode sandbox provider response: {0}")]
    DecodeResponse(serde_json::Error),
    #[error("sandbox provider protocol error: {0}")]
    Protocol(String),
    #[error(transparent)]
    Provider(#[from] ProviderError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v1::Metadata;
    use std::fs;

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    fn metadata() -> Metadata {
        Metadata::new("req_1", "sbxgrp_1")
    }

    #[cfg(unix)]
    fn write_plugin(contents: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let plugin_path = dir.path().join("plugin.sh");
        fs::write(&plugin_path, contents).unwrap();
        let mut perms = fs::metadata(&plugin_path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&plugin_path, perms).unwrap();
        dir
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn invokes_capabilities_plugin() {
        let dir = write_plugin(
            r#"#!/bin/sh
cat >/dev/null
case "$1" in
  capabilities) printf '{"capabilities":{"exec":true}}' ;;
  *) printf '{"error":{"code":"unknown","message":"unknown command","retryable":false}}' ;;
esac
"#,
        );
        let plugin_path = dir.path().join("plugin.sh");

        let plugin = SandboxProviderPlugin::new(&plugin_path);
        let capabilities = plugin
            .capabilities(&CapabilitiesRequest {
                metadata: metadata(),
            })
            .await
            .unwrap();

        assert!(capabilities.exec);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn surfaces_provider_error_without_success_payload() {
        let dir = write_plugin(
            r#"#!/bin/sh
cat >/dev/null
printf '{"error":{"code":"bad_profile","message":"bad profile","retryable":false}}'
"#,
        );
        let plugin_path = dir.path().join("plugin.sh");

        let plugin = SandboxProviderPlugin::new(&plugin_path);
        let error = plugin
            .capabilities(&CapabilitiesRequest {
                metadata: metadata(),
            })
            .await
            .unwrap_err();

        assert!(
            matches!(error, PluginClientError::Provider(provider) if provider.code == "bad_profile")
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn reports_missing_success_payload() {
        let dir = write_plugin(
            r#"#!/bin/sh
cat >/dev/null
printf '{}'
"#,
        );
        let plugin_path = dir.path().join("plugin.sh");

        let plugin = SandboxProviderPlugin::new(&plugin_path);
        let error = plugin
            .capabilities(&CapabilitiesRequest {
                metadata: metadata(),
            })
            .await
            .unwrap_err();

        assert!(
            matches!(error, PluginClientError::Protocol(message) if message.contains("capabilities"))
        );
    }
}
