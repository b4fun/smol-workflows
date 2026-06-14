//! Sandbox-backed execution environment implementation.

use super::{
    AgentExecutionEnvironment, EnvironmentPath, ExecEvent, ExecEventSink, ExecOutput, ExecRequest,
    SpawnOutput,
};
use anyhow::Context;
use base64::prelude::*;
use smol_workflow_sandbox::{
    CreateTempDirRequest, OpenSandboxRequest, SandboxExecEvent, SandboxExecRequest,
    SandboxProviderJsonlClient, SandboxSession, SandboxSpawnRequest, SessionPathRequest,
    WriteFileRequest,
};
use std::sync::Arc;
use tokio::sync::mpsc;

/// Execution environment backed by a long-lived sandbox provider JSONL process.
#[derive(Debug, Clone)]
pub struct SandboxExecutionEnvironment {
    client: Arc<SandboxProviderJsonlClient>,
    session: SandboxSession,
    cwd: Option<EnvironmentPath>,
}

impl SandboxExecutionEnvironment {
    /// Start `program serve`, open a sandbox session, and wrap it as an execution environment.
    pub async fn open(
        program: impl Into<std::path::PathBuf>,
        request: OpenSandboxRequest,
    ) -> anyhow::Result<Self> {
        let client = Arc::new(SandboxProviderJsonlClient::start(program).await?);
        let session = client.open(request).await?;
        Ok(Self::from_session(client, session))
    }

    pub fn from_session(client: Arc<SandboxProviderJsonlClient>, session: SandboxSession) -> Self {
        let cwd = session.cwd.clone().map(EnvironmentPath);
        Self {
            client,
            session,
            cwd,
        }
    }

    pub fn client(&self) -> &Arc<SandboxProviderJsonlClient> {
        &self.client
    }

    pub fn session(&self) -> &SandboxSession {
        &self.session
    }

    /// Close the sandbox session and ask the provider to shut down.
    pub async fn close(self) -> anyhow::Result<()> {
        let close_result = self.client.close(self.session).await;
        let shutdown_result = self.client.shutdown().await;
        close_result?;
        shutdown_result?;
        Ok(())
    }

    fn session_path_request(&self, path: &EnvironmentPath) -> SessionPathRequest {
        SessionPathRequest {
            session: self.session.clone(),
            path: path.0.clone(),
        }
    }
}

#[async_trait::async_trait]
impl AgentExecutionEnvironment for SandboxExecutionEnvironment {
    fn cwd(&self) -> Option<&EnvironmentPath> {
        self.cwd.as_ref()
    }

    async fn create_dir_all(&self, path: &EnvironmentPath) -> anyhow::Result<()> {
        self.client
            .create_dir_all(self.session_path_request(path))
            .await
            .with_context(|| format!("failed to create sandbox directory `{}`", path.as_str()))
    }

    async fn write_file(&self, path: &EnvironmentPath, content: &[u8]) -> anyhow::Result<()> {
        self.client
            .write_file(WriteFileRequest {
                session: self.session.clone(),
                path: path.0.clone(),
                content_base64: BASE64_STANDARD.encode(content),
            })
            .await
            .with_context(|| format!("failed to write sandbox file `{}`", path.as_str()))
    }

    async fn read_file(&self, path: &EnvironmentPath) -> anyhow::Result<Vec<u8>> {
        let result = self
            .client
            .read_file(self.session_path_request(path))
            .await
            .with_context(|| format!("failed to read sandbox file `{}`", path.as_str()))?;
        BASE64_STANDARD
            .decode(result.content_base64)
            .context("sandbox read_file returned invalid base64 content")
    }

    async fn remove(&self, path: &EnvironmentPath) -> anyhow::Result<()> {
        self.client
            .remove(self.session_path_request(path))
            .await
            .with_context(|| format!("failed to remove sandbox path `{}`", path.as_str()))
    }

    async fn create_temp_dir(&self, prefix: &str) -> anyhow::Result<EnvironmentPath> {
        let result = self
            .client
            .create_temp_dir(CreateTempDirRequest {
                session: self.session.clone(),
                prefix: prefix.to_string(),
            })
            .await
            .context("failed to create sandbox temp directory")?;
        Ok(EnvironmentPath(result.path))
    }

    async fn exec(
        &self,
        request: ExecRequest,
        sink: &mut dyn ExecEventSink,
    ) -> anyhow::Result<ExecOutput> {
        let sandbox_request = SandboxExecRequest {
            session: self.session.clone(),
            argv: request.argv,
            cwd: request.cwd.map(|path| path.0),
            env: request.env,
            stdin_base64: request.stdin.map(|stdin| BASE64_STANDARD.encode(stdin)),
        };

        let (event_tx, mut event_rx) = mpsc::unbounded_channel::<ExecEvent>();
        let result = {
            let exec_future = self.client.exec(sandbox_request, move |event| {
                event_tx.send(convert_exec_event(event)?).map_err(|_| {
                    smol_workflow_sandbox::JsonlClientError::Protocol(
                        "sandbox exec event receiver closed".to_string(),
                    )
                })?;
                Ok(())
            });
            tokio::pin!(exec_future);

            loop {
                tokio::select! {
                    result = &mut exec_future => break result,
                    event = event_rx.recv() => {
                        if let Some(event) = event {
                            sink.event(event).await?;
                        }
                    }
                }
            }
        }?;

        while let Some(event) = event_rx.recv().await {
            sink.event(event).await?;
        }

        Ok(ExecOutput {
            exit_code: result.exit_code,
            stdout: BASE64_STANDARD
                .decode(result.stdout_base64)
                .context("sandbox exec returned invalid base64 stdout")?,
            stderr: BASE64_STANDARD
                .decode(result.stderr_base64)
                .context("sandbox exec returned invalid base64 stderr")?,
        })
    }

    async fn spawn(
        &self,
        request: ExecRequest,
        sink: Option<Box<dyn ExecEventSink>>,
    ) -> anyhow::Result<SpawnOutput> {
        let sandbox_request = SandboxSpawnRequest {
            session: self.session.clone(),
            argv: request.argv,
            cwd: request.cwd.map(|path| path.0),
            env: request.env,
            stdin_base64: request.stdin.map(|stdin| BASE64_STANDARD.encode(stdin)),
        };
        let Some(mut sink) = sink else {
            let result = self.client.spawn(sandbox_request).await?;
            return Ok(SpawnOutput {
                process_id: result.process_id,
            });
        };

        let (event_tx, mut event_rx) = mpsc::unbounded_channel::<ExecEvent>();
        let result: smol_workflow_sandbox::SandboxSpawnResult = {
            let spawn_future =
                self.client
                    .request_with_events("spawn", sandbox_request, move |event| {
                        event_tx.send(convert_exec_event(event)?).map_err(|_| {
                            smol_workflow_sandbox::JsonlClientError::Protocol(
                                "sandbox spawn event receiver closed".to_string(),
                            )
                        })?;
                        Ok(())
                    });
            tokio::pin!(spawn_future);

            loop {
                tokio::select! {
                    result = &mut spawn_future => break result,
                    event = event_rx.recv() => {
                        if let Some(event) = event {
                            sink.event(event).await?;
                        }
                    }
                }
            }
        }?;

        while let Some(event) = event_rx.recv().await {
            sink.event(event).await?;
        }
        Ok(SpawnOutput {
            process_id: result.process_id,
        })
    }
}

fn convert_exec_event(
    event: SandboxExecEvent,
) -> Result<ExecEvent, smol_workflow_sandbox::JsonlClientError> {
    match event.r#type.as_str() {
        "started" => Ok(ExecEvent::Started {
            process_id: event.process_id,
        }),
        "stdout" => Ok(ExecEvent::Stdout {
            chunk: decode_event_data(event.data_base64)?,
        }),
        "stderr" => Ok(ExecEvent::Stderr {
            chunk: decode_event_data(event.data_base64)?,
        }),
        "exited" => Ok(ExecEvent::Exited {
            exit_code: event.exit_code.unwrap_or(-1),
        }),
        other => Err(smol_workflow_sandbox::JsonlClientError::Protocol(format!(
            "unknown sandbox exec event type `{other}`"
        ))),
    }
}

fn decode_event_data(
    data_base64: Option<String>,
) -> Result<Vec<u8>, smol_workflow_sandbox::JsonlClientError> {
    let Some(data_base64) = data_base64 else {
        return Ok(Vec::new());
    };
    BASE64_STANDARD.decode(data_base64).map_err(|error| {
        smol_workflow_sandbox::JsonlClientError::Protocol(format!(
            "sandbox exec event contained invalid base64 data: {error}"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use smol_workflow_sandbox::{Metadata, ProfileRef, WorkspaceSync};
    use std::fs;

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    #[derive(Default)]
    struct RecordingSink {
        events: Vec<ExecEvent>,
    }

    #[derive(Clone, Default)]
    struct SharedRecordingSink {
        events: Arc<std::sync::Mutex<Vec<ExecEvent>>>,
    }

    #[async_trait::async_trait]
    impl ExecEventSink for RecordingSink {
        async fn event(&mut self, event: ExecEvent) -> anyhow::Result<()> {
            self.events.push(event);
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl ExecEventSink for SharedRecordingSink {
        async fn event(&mut self, event: ExecEvent) -> anyhow::Result<()> {
            self.events.lock().unwrap().push(event);
            Ok(())
        }
    }

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
    async fn sandbox_environment_adapts_file_io_and_exec() {
        let dir = write_provider(
            r#"#!/usr/bin/env python3
import json, sys
files = {}
session = {"id": "session_1", "cwd": "/sandbox", "capabilities": {"exec": True}}
for line in sys.stdin:
    request = json.loads(line)
    request_id = request["id"]
    method = request["method"]
    params = request.get("params", {})
    if method == "open":
        print(json.dumps({"id": request_id, "result": session}), flush=True)
    elif method == "write_file":
        files[params["path"]] = params["content_base64"]
        print(json.dumps({"id": request_id, "result": {}}), flush=True)
    elif method == "read_file":
        print(json.dumps({"id": request_id, "result": {"content_base64": files[params["path"]]}}), flush=True)
    elif method == "exec":
        print(json.dumps({"id": request_id, "event": {"type": "started", "process_id": "p1"}}), flush=True)
        print(json.dumps({"id": request_id, "event": {"type": "stdout", "data_base64": "AAH/"}}), flush=True)
        print(json.dumps({"id": request_id, "event": {"type": "exited", "exit_code": 0}}), flush=True)
        print(json.dumps({"id": request_id, "result": {"exit_code": 0, "stdout_base64": "AAH/", "stderr_base64": "/gA="}}), flush=True)
    elif method == "spawn":
        print(json.dumps({"id": request_id, "event": {"type": "started", "process_id": "spawn_1"}}), flush=True)
        print(json.dumps({"id": request_id, "result": {"process_id": "spawn_1"}}), flush=True)
    elif method == "close" or method == "shutdown":
        print(json.dumps({"id": request_id, "result": {}}), flush=True)
        if method == "shutdown":
            break
    else:
        print(json.dumps({"id": request_id, "result": {}}), flush=True)
"#,
        );

        let env = SandboxExecutionEnvironment::open(
            dir.path().join("provider.py"),
            OpenSandboxRequest {
                metadata: Metadata::new("req_open", "sbxgrp_test"),
                profile: ProfileRef {
                    provider: "fake".to_string(),
                    name: "test".to_string(),
                },
                workspace_sync: WorkspaceSync {
                    host_path: dir.path().to_path_buf(),
                },
                cwd: None,
            },
        )
        .await
        .unwrap();

        let path = EnvironmentPath("binary.bin".to_string());
        env.write_file(&path, &[0, 1, 255]).await.unwrap();
        assert_eq!(env.read_file(&path).await.unwrap(), vec![0, 1, 255]);

        let mut sink = RecordingSink::default();
        let output = env
            .exec(
                ExecRequest {
                    argv: vec!["ignored".to_string()],
                    ..ExecRequest::default()
                },
                &mut sink,
            )
            .await
            .unwrap();
        assert_eq!(output.stdout, vec![0, 1, 255]);
        assert_eq!(output.stderr, vec![254, 0]);
        assert!(sink.events.iter().any(
            |event| matches!(event, ExecEvent::Stdout { chunk } if chunk == &vec![0, 1, 255])
        ));

        let spawn_sink = SharedRecordingSink::default();
        let spawn_events = Arc::clone(&spawn_sink.events);
        let spawned = env
            .spawn(
                ExecRequest {
                    argv: vec!["ignored".to_string()],
                    ..ExecRequest::default()
                },
                Some(Box::new(spawn_sink)),
            )
            .await
            .unwrap();
        assert_eq!(spawned.process_id.as_deref(), Some("spawn_1"));
        assert!(spawn_events.lock().unwrap().iter().any(
            |event| matches!(event, ExecEvent::Started { process_id } if process_id.as_deref() == Some("spawn_1"))
        ));
        env.close().await.unwrap();
    }
}
