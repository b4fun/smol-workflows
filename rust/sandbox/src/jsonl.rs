//! Long-lived stdio JSONL client for sandbox provider `serve` processes.

use crate::v1::{
    Capabilities, CleanupSandboxGroupRequest, OpenSandboxRequest, ProviderError, SandboxSession,
};
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;

/// JSONL RPC request envelope.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct JsonlRequestEnvelope<P = Value> {
    pub id: String,
    pub method: String,
    pub params: P,
}

/// JSONL RPC response/event envelope emitted by providers.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct JsonlResponseEnvelope {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ProviderError>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<Value>,
}

/// Request params for methods that operate on an existing sandbox session.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct SessionPathRequest {
    pub session: SandboxSession,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct CreateTempDirRequest {
    pub session: SandboxSession,
    pub prefix: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct CreateTempDirResult {
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct WriteFileRequest {
    pub session: SandboxSession,
    pub path: String,
    pub content_base64: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ReadFileResult {
    pub content_base64: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct SandboxExecRequest {
    pub session: SandboxSession,
    pub argv: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdin_base64: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct SandboxExecResult {
    pub exit_code: i32,
    #[serde(default)]
    pub stdout_base64: String,
    #[serde(default)]
    pub stderr_base64: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct SandboxSpawnRequest {
    pub session: SandboxSession,
    pub argv: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdin_base64: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct SandboxSpawnResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct SandboxExecEvent {
    pub r#type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_base64: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
}

/// Long-lived client for `smol-sandbox-<provider> serve`.
///
/// The current client is intentionally serialized per provider process: it sends
/// one request, reads any events for that request, reads its final response, and
/// only then sends the next request on the same connection. Providers do not
/// need to support multiple concurrent in-flight requests.
#[derive(Debug)]
pub struct SandboxProviderJsonlClient {
    program: PathBuf,
    child: Mutex<Child>,
    inner: Mutex<ClientInner>,
}

#[derive(Debug)]
struct ClientInner {
    stdin: ChildStdin,
    stdout: Lines<BufReader<ChildStdout>>,
    next_id: u64,
}

impl SandboxProviderJsonlClient {
    /// Start a provider process with the `serve` subcommand.
    pub async fn start(program: impl Into<PathBuf>) -> Result<Self, JsonlClientError> {
        let program = program.into();
        let mut child = Command::new(&program)
            .arg("serve")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|source| JsonlClientError::Spawn {
                program: program.clone(),
                source,
            })?;
        let stdin = child.stdin.take().ok_or(JsonlClientError::MissingStdin)?;
        let stdout = child.stdout.take().ok_or(JsonlClientError::MissingStdout)?;
        if let Some(stderr) = child.stderr.take() {
            drain_provider_stderr(program.clone(), stderr);
        }
        Ok(Self {
            program,
            child: Mutex::new(child),
            inner: Mutex::new(ClientInner {
                stdin,
                stdout: BufReader::new(stdout).lines(),
                next_id: 1,
            }),
        })
    }

    pub fn program(&self) -> &Path {
        &self.program
    }

    pub async fn capabilities(&self) -> Result<Capabilities, JsonlClientError> {
        self.request("capabilities", serde_json::json!({})).await
    }

    pub async fn open(
        &self,
        request: OpenSandboxRequest,
    ) -> Result<SandboxSession, JsonlClientError> {
        self.request("open", request).await
    }

    pub async fn close(&self, session: SandboxSession) -> Result<(), JsonlClientError> {
        let _: Value = self
            .request("close", serde_json::json!({ "session": session }))
            .await?;
        Ok(())
    }

    pub async fn cleanup_group(
        &self,
        request: CleanupSandboxGroupRequest,
    ) -> Result<u32, JsonlClientError> {
        #[derive(Deserialize)]
        struct CleanupResult {
            cleaned_count: u32,
        }
        let result: CleanupResult = self.request("cleanup_group", request).await?;
        Ok(result.cleaned_count)
    }

    pub async fn create_temp_dir(
        &self,
        request: CreateTempDirRequest,
    ) -> Result<CreateTempDirResult, JsonlClientError> {
        self.request("create_temp_dir", request).await
    }

    pub async fn create_dir_all(
        &self,
        request: SessionPathRequest,
    ) -> Result<(), JsonlClientError> {
        let _: Value = self.request("create_dir_all", request).await?;
        Ok(())
    }

    pub async fn write_file(&self, request: WriteFileRequest) -> Result<(), JsonlClientError> {
        let _: Value = self.request("write_file", request).await?;
        Ok(())
    }

    pub async fn read_file(
        &self,
        request: SessionPathRequest,
    ) -> Result<ReadFileResult, JsonlClientError> {
        self.request("read_file", request).await
    }

    pub async fn remove(&self, request: SessionPathRequest) -> Result<(), JsonlClientError> {
        let _: Value = self.request("remove", request).await?;
        Ok(())
    }

    pub async fn exec<F>(
        &self,
        request: SandboxExecRequest,
        on_event: F,
    ) -> Result<SandboxExecResult, JsonlClientError>
    where
        F: FnMut(SandboxExecEvent) -> Result<(), JsonlClientError>,
    {
        self.request_with_events("exec", request, on_event).await
    }

    pub async fn spawn(
        &self,
        request: SandboxSpawnRequest,
    ) -> Result<SandboxSpawnResult, JsonlClientError> {
        self.request("spawn", request).await
    }

    pub async fn shutdown(&self) -> Result<(), JsonlClientError> {
        let _: Value = self.request("shutdown", serde_json::json!({})).await?;
        let _ = self.child.lock().await.wait().await?;
        Ok(())
    }

    pub async fn request<P, R>(&self, method: &str, params: P) -> Result<R, JsonlClientError>
    where
        P: Serialize,
        R: DeserializeOwned,
    {
        self.request_with_events(method, params, |_| Ok(())).await
    }

    pub async fn request_with_events<P, R, F>(
        &self,
        method: &str,
        params: P,
        mut on_event: F,
    ) -> Result<R, JsonlClientError>
    where
        P: Serialize,
        R: DeserializeOwned,
        F: FnMut(SandboxExecEvent) -> Result<(), JsonlClientError>,
    {
        let mut inner = self.inner.lock().await;
        let id = format!("req_{}", inner.next_id);
        inner.next_id += 1;
        let envelope = JsonlRequestEnvelope {
            id: id.clone(),
            method: method.to_string(),
            params,
        };
        let mut line = serde_json::to_vec(&envelope).map_err(JsonlClientError::EncodeRequest)?;
        line.push(b'\n');
        inner.stdin.write_all(&line).await?;
        inner.stdin.flush().await?;

        loop {
            let Some(line) = inner.stdout.next_line().await? else {
                return Err(JsonlClientError::TransportClosed);
            };
            if line.trim().is_empty() {
                continue;
            }
            let incoming: JsonlResponseEnvelope =
                serde_json::from_str(&line).map_err(JsonlClientError::DecodeResponse)?;
            if incoming.id != id {
                return Err(JsonlClientError::Protocol(format!(
                    "unexpected response id `{}` while waiting for `{id}`",
                    incoming.id
                )));
            }
            if let Some(event) = incoming.event {
                let event =
                    serde_json::from_value(event).map_err(JsonlClientError::DecodeResponse)?;
                on_event(event)?;
                continue;
            }
            if let Some(error) = incoming.error {
                return Err(JsonlClientError::Provider(error));
            }
            let result = incoming.result.unwrap_or(Value::Object(Default::default()));
            return serde_json::from_value(result).map_err(JsonlClientError::DecodeResponse);
        }
    }
}

fn drain_provider_stderr(program: PathBuf, stderr: ChildStderr) {
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    if !line.trim().is_empty() {
                        log::warn!("sandbox provider `{}` stderr: {line}", program.display());
                    }
                }
                Ok(None) => break,
                Err(error) => {
                    log::warn!(
                        "failed to read sandbox provider `{}` stderr: {error}",
                        program.display()
                    );
                    break;
                }
            }
        }
    });
}

impl Drop for SandboxProviderJsonlClient {
    fn drop(&mut self) {
        let child = self.child.get_mut();
        let _ = child.start_kill();
    }
}

/// Errors raised by the JSONL stdio client.
#[derive(Debug, thiserror::Error)]
pub enum JsonlClientError {
    #[error("failed to spawn sandbox provider `{}`: {source}", program.display())]
    Spawn {
        program: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("sandbox provider stdin was not available")]
    MissingStdin,
    #[error("sandbox provider stdout was not available")]
    MissingStdout,
    #[error("sandbox provider IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to encode sandbox provider request: {0}")]
    EncodeRequest(serde_json::Error),
    #[error("failed to decode sandbox provider response: {0}")]
    DecodeResponse(serde_json::Error),
    #[error("sandbox provider transport closed")]
    TransportClosed,
    #[error("sandbox provider protocol error: {0}")]
    Protocol(String),
    #[error(transparent)]
    Provider(#[from] ProviderError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v1::{Capabilities, SandboxSession};
    use std::fs;

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    #[cfg(unix)]
    fn write_provider(contents: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let provider_path = dir.path().join("provider.py");
        fs::write(&provider_path, contents).unwrap();
        let mut perms = fs::metadata(&provider_path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&provider_path, perms).unwrap();
        dir
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn jsonl_client_streams_exec_events_and_result() {
        let dir = write_provider(
            r#"#!/usr/bin/env python3
import json, sys
for line in sys.stdin:
    request = json.loads(line)
    request_id = request["id"]
    method = request["method"]
    if method == "exec":
        print(json.dumps({"id": request_id, "event": {"type": "started", "process_id": "p1"}}), flush=True)
        print(json.dumps({"id": request_id, "event": {"type": "stdout", "data_base64": "aGk="}}), flush=True)
        print(json.dumps({"id": request_id, "event": {"type": "exited", "exit_code": 0}}), flush=True)
        print(json.dumps({"id": request_id, "result": {"exit_code": 0, "stdout_base64": "aGk=", "stderr_base64": ""}}), flush=True)
    elif method == "shutdown":
        print(json.dumps({"id": request_id, "result": {}}), flush=True)
        break
    else:
        print(json.dumps({"id": request_id, "error": {"code": "unknown", "message": method, "retryable": False}}), flush=True)
"#,
        );
        let client = SandboxProviderJsonlClient::start(dir.path().join("provider.py"))
            .await
            .unwrap();
        let mut events = Vec::new();
        let result = client
            .exec(
                SandboxExecRequest {
                    session: SandboxSession {
                        id: "session_1".to_string(),
                        provider_session_id: None,
                        cwd: Some("/tmp".to_string()),
                        capabilities: Capabilities { exec: true },
                        provider_state_json: None,
                    },
                    argv: vec!["echo".to_string()],
                    cwd: None,
                    env: BTreeMap::new(),
                    stdin_base64: None,
                },
                |event| {
                    events.push(event);
                    Ok(())
                },
            )
            .await
            .unwrap();

        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout_base64, "aGk=");
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].r#type, "started");
        assert_eq!(events[1].data_base64.as_deref(), Some("aGk="));
        assert_eq!(events[2].exit_code, Some(0));
        client.shutdown().await.unwrap();
    }
}
