//! Local in-process execution environment implementation.

use super::{
    path_to_environment_path, AgentExecutionEnvironment, EnvironmentPath, ExecEvent, ExecEventSink,
    ExecOutput, ExecRequest, SpawnOutput,
};
use anyhow::{anyhow, Context};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use tempfile::TempDir;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// Local, in-process environment implementation.
#[derive(Debug, Clone)]
pub struct LocalExecutionEnvironment {
    cwd: Option<EnvironmentPath>,
    state: Arc<LocalEnvironmentState>,
}

#[derive(Debug, Default)]
struct LocalEnvironmentState {
    temp_dirs: Mutex<Vec<TempDir>>,
    spawned: Mutex<Vec<JoinHandle<()>>>,
}

impl Drop for LocalEnvironmentState {
    fn drop(&mut self) {
        if let Ok(mut tasks) = self.spawned.lock() {
            for task in tasks.drain(..) {
                task.abort();
            }
        }
        // TempDir cleanup happens through Drop after this state is dropped.
        // Aborting spawned-process tasks drops their kill_on_drop children.
    }
}

impl LocalExecutionEnvironment {
    pub fn new(cwd: Option<PathBuf>) -> anyhow::Result<Self> {
        let cwd = cwd.map(path_to_environment_path).transpose()?;
        Ok(Self {
            cwd,
            state: Arc::new(LocalEnvironmentState::default()),
        })
    }

    pub fn with_cwd(cwd: impl Into<PathBuf>) -> anyhow::Result<Self> {
        Self::new(Some(cwd.into()))
    }

    fn resolve_path(&self, path: &EnvironmentPath) -> PathBuf {
        let path = PathBuf::from(path.as_str());
        if path.is_absolute() {
            path
        } else if let Some(cwd) = &self.cwd {
            PathBuf::from(cwd.as_str()).join(path)
        } else {
            path
        }
    }

    fn request_cwd(&self, cwd: Option<&EnvironmentPath>) -> Option<PathBuf> {
        cwd.map(|path| self.resolve_path(path))
            .or_else(|| self.cwd.as_ref().map(|path| PathBuf::from(path.as_str())))
    }
}

#[async_trait::async_trait]
impl AgentExecutionEnvironment for LocalExecutionEnvironment {
    fn cwd(&self) -> Option<&EnvironmentPath> {
        self.cwd.as_ref()
    }

    async fn create_dir_all(&self, path: &EnvironmentPath) -> anyhow::Result<()> {
        tokio::fs::create_dir_all(self.resolve_path(path))
            .await
            .with_context(|| format!("failed to create directory `{}`", path.as_str()))
    }

    async fn write_file(&self, path: &EnvironmentPath, content: &[u8]) -> anyhow::Result<()> {
        tokio::fs::write(self.resolve_path(path), content)
            .await
            .with_context(|| format!("failed to write file `{}`", path.as_str()))
    }

    async fn read_file(&self, path: &EnvironmentPath) -> anyhow::Result<Vec<u8>> {
        tokio::fs::read(self.resolve_path(path))
            .await
            .with_context(|| format!("failed to read file `{}`", path.as_str()))
    }

    async fn remove(&self, path: &EnvironmentPath) -> anyhow::Result<()> {
        let resolved = self.resolve_path(path);
        let metadata = match tokio::fs::symlink_metadata(&resolved).await {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("failed to inspect path `{}`", path.as_str()))
            }
        };

        if metadata.is_dir() {
            tokio::fs::remove_dir_all(&resolved)
                .await
                .with_context(|| format!("failed to remove directory `{}`", path.as_str()))
        } else {
            tokio::fs::remove_file(&resolved)
                .await
                .with_context(|| format!("failed to remove file `{}`", path.as_str()))
        }
    }

    async fn create_temp_dir(&self, prefix: &str) -> anyhow::Result<EnvironmentPath> {
        let temp_dir = tempfile::Builder::new()
            .prefix(prefix)
            .tempdir()
            .with_context(|| format!("failed to create temp directory with prefix `{prefix}`"))?;
        let path = path_to_environment_path(temp_dir.path())?;
        self.state
            .temp_dirs
            .lock()
            .map_err(|_| anyhow!("local environment temp-dir lock poisoned"))?
            .push(temp_dir);
        Ok(path)
    }

    async fn exec(
        &self,
        request: ExecRequest,
        sink: &mut dyn ExecEventSink,
    ) -> anyhow::Result<ExecOutput> {
        let (command, args) = split_argv(&request.argv)?;
        let mut command_builder = Command::new(command);
        command_builder.args(args);
        if let Some(cwd) = self.request_cwd(request.cwd.as_ref()) {
            command_builder.current_dir(cwd);
        }
        command_builder.envs(&request.env);
        command_builder
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if request.stdin.is_some() {
            command_builder.stdin(Stdio::piped());
        } else {
            command_builder.stdin(Stdio::null());
        }

        let mut child = command_builder
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("failed to spawn command `{}`", request.argv[0]))?;
        let process_id = child.id().map(|id| id.to_string());
        sink.event(ExecEvent::Started { process_id }).await?;

        let stdin_task = spawn_stdin_writer(child.stdin.take(), request.stdin);
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let (event_tx, mut event_rx) = mpsc::channel::<PipeEvent>(32);
        spawn_pipe_reader(stdout, PipeKind::Stdout, event_tx.clone());
        spawn_pipe_reader(stderr, PipeKind::Stderr, event_tx.clone());
        drop(event_tx);

        let wait = child.wait();
        tokio::pin!(wait);
        let mut stdout_acc = Vec::new();
        let mut stderr_acc = Vec::new();
        let mut exit_code = None;
        let mut pipes_open = true;

        while exit_code.is_none() || pipes_open {
            tokio::select! {
                status = &mut wait, if exit_code.is_none() => {
                    let status = status.context("failed to wait for command")?;
                    exit_code = Some(status.code().unwrap_or(-1));
                }
                event = event_rx.recv(), if pipes_open => {
                    match event {
                        Some(PipeEvent::Stdout(chunk)) => {
                            stdout_acc.extend_from_slice(&chunk);
                            sink.event(ExecEvent::Stdout { chunk }).await?;
                        }
                        Some(PipeEvent::Stderr(chunk)) => {
                            stderr_acc.extend_from_slice(&chunk);
                            sink.event(ExecEvent::Stderr { chunk }).await?;
                        }
                        None => pipes_open = false,
                    }
                }
            }
        }

        await_stdin_writer(stdin_task).await?;
        let exit_code = exit_code.unwrap_or(-1);
        sink.event(ExecEvent::Exited { exit_code }).await?;
        Ok(ExecOutput {
            exit_code,
            stdout: stdout_acc,
            stderr: stderr_acc,
        })
    }

    async fn spawn(
        &self,
        request: ExecRequest,
        sink: Option<Box<dyn ExecEventSink>>,
    ) -> anyhow::Result<SpawnOutput> {
        let (command, args) = split_argv(&request.argv)?;
        let mut command_builder = Command::new(command);
        command_builder.args(args);
        if let Some(cwd) = self.request_cwd(request.cwd.as_ref()) {
            command_builder.current_dir(cwd);
        }
        command_builder.envs(&request.env);
        command_builder.kill_on_drop(true);
        if request.stdin.is_some() {
            command_builder.stdin(Stdio::piped());
        } else {
            command_builder.stdin(Stdio::null());
        }
        command_builder
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = command_builder
            .spawn()
            .with_context(|| format!("failed to spawn command `{}`", request.argv[0]))?;
        let stdin_task = spawn_stdin_writer(child.stdin.take(), request.stdin);
        self.track_spawned_child(child, sink, stdin_task).await
    }
}

impl LocalExecutionEnvironment {
    async fn track_spawned_child(
        &self,
        mut child: Child,
        mut sink: Option<Box<dyn ExecEventSink>>,
        stdin_task: Option<JoinHandle<anyhow::Result<()>>>,
    ) -> anyhow::Result<SpawnOutput> {
        let process_id = child.id().map(|id| id.to_string());
        if let Some(sink) = sink.as_mut() {
            sink.event(ExecEvent::Started {
                process_id: process_id.clone(),
            })
            .await?;
        }

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let task = tokio::spawn(async move {
            let (event_tx, mut event_rx) = mpsc::channel::<PipeEvent>(32);
            spawn_pipe_reader(stdout, PipeKind::Stdout, event_tx.clone());
            spawn_pipe_reader(stderr, PipeKind::Stderr, event_tx.clone());
            drop(event_tx);

            let wait = child.wait();
            tokio::pin!(wait);
            let mut exit_code = None;
            let mut pipes_open = true;

            while exit_code.is_none() || pipes_open {
                tokio::select! {
                    status = &mut wait, if exit_code.is_none() => {
                        exit_code = status.ok().map(|status| status.code().unwrap_or(-1));
                    }
                    event = event_rx.recv(), if pipes_open => {
                        match event {
                            Some(PipeEvent::Stdout(chunk)) => {
                                let failed = if let Some(sink_ref) = sink.as_mut() {
                                    sink_ref.event(ExecEvent::Stdout { chunk }).await.is_err()
                                } else {
                                    false
                                };
                                if failed {
                                    sink = None;
                                }
                            }
                            Some(PipeEvent::Stderr(chunk)) => {
                                let failed = if let Some(sink_ref) = sink.as_mut() {
                                    sink_ref.event(ExecEvent::Stderr { chunk }).await.is_err()
                                } else {
                                    false
                                };
                                if failed {
                                    sink = None;
                                }
                            }
                            None => pipes_open = false,
                        }
                    }
                }
            }

            let _ = await_stdin_writer(stdin_task).await;
            if let Some(sink) = sink.as_mut() {
                let _ = sink
                    .event(ExecEvent::Exited {
                        exit_code: exit_code.unwrap_or(-1),
                    })
                    .await;
            }
        });

        self.state
            .spawned
            .lock()
            .map_err(|_| anyhow!("local environment spawned-process lock poisoned"))?
            .push(task);
        Ok(SpawnOutput { process_id })
    }
}

#[derive(Debug)]
enum PipeEvent {
    Stdout(Vec<u8>),
    Stderr(Vec<u8>),
}

#[derive(Debug, Clone, Copy)]
enum PipeKind {
    Stdout,
    Stderr,
}

fn spawn_stdin_writer(
    child_stdin: Option<ChildStdin>,
    stdin: Option<Vec<u8>>,
) -> Option<JoinHandle<anyhow::Result<()>>> {
    match (child_stdin, stdin) {
        (Some(mut child_stdin), Some(stdin)) => Some(tokio::spawn(async move {
            child_stdin
                .write_all(&stdin)
                .await
                .context("failed to write command stdin")?;
            Ok(())
        })),
        (None, Some(_)) => Some(tokio::spawn(async {
            Err(anyhow!("failed to open command stdin"))
        })),
        _ => None,
    }
}

async fn await_stdin_writer(task: Option<JoinHandle<anyhow::Result<()>>>) -> anyhow::Result<()> {
    if let Some(task) = task {
        task.await
            .context("stdin writer task failed to complete")??;
    }
    Ok(())
}

fn spawn_pipe_reader<R>(reader: Option<R>, kind: PipeKind, event_tx: mpsc::Sender<PipeEvent>)
where
    R: AsyncRead + Unpin + Send + 'static,
{
    if let Some(reader) = reader {
        tokio::spawn(async move {
            read_pipe(reader, kind, event_tx).await;
        });
    }
}

async fn read_pipe<R>(mut reader: R, kind: PipeKind, event_tx: mpsc::Sender<PipeEvent>)
where
    R: AsyncRead + Unpin,
{
    let mut buffer = vec![0u8; 8192];
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let chunk = buffer[..n].to_vec();
                let event = match kind {
                    PipeKind::Stdout => PipeEvent::Stdout(chunk),
                    PipeKind::Stderr => PipeEvent::Stderr(chunk),
                };
                if event_tx.send(event).await.is_err() {
                    break;
                }
            }
        }
    }
}

fn split_argv(argv: &[String]) -> anyhow::Result<(&str, &[String])> {
    let Some((command, args)) = argv.split_first() else {
        return Err(anyhow!("ExecRequest.argv must not be empty"));
    };
    Ok((command.as_str(), args))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;
    use tokio::time::{sleep, timeout, Duration, Instant};

    #[derive(Default)]
    struct RecordingSink {
        events: Vec<ExecEvent>,
    }

    #[async_trait::async_trait]
    impl ExecEventSink for RecordingSink {
        async fn event(&mut self, event: ExecEvent) -> anyhow::Result<()> {
            self.events.push(event);
            Ok(())
        }
    }

    #[derive(Clone, Default)]
    struct SharedRecordingSink {
        events: Arc<StdMutex<Vec<ExecEvent>>>,
    }

    #[async_trait::async_trait]
    impl ExecEventSink for SharedRecordingSink {
        async fn event(&mut self, event: ExecEvent) -> anyhow::Result<()> {
            self.events.lock().unwrap().push(event);
            Ok(())
        }
    }

    #[tokio::test]
    async fn local_environment_file_io_uses_relative_cwd() {
        let temp = tempfile::tempdir().unwrap();
        let env = LocalExecutionEnvironment::with_cwd(temp.path()).unwrap();
        let path = EnvironmentPath::from("nested/data.bin");

        env.create_dir_all(&EnvironmentPath::from("nested"))
            .await
            .unwrap();
        env.write_file(&path, b"hello\0world").await.unwrap();
        assert_eq!(env.read_file(&path).await.unwrap(), b"hello\0world");
        env.remove(&EnvironmentPath::from("nested")).await.unwrap();
        assert!(!temp.path().join("nested").exists());
        env.remove(&EnvironmentPath::from("missing")).await.unwrap();
    }

    #[tokio::test]
    async fn local_environment_exec_streams_and_accumulates_bytes() {
        let env = LocalExecutionEnvironment::new(None).unwrap();
        let mut sink = RecordingSink::default();
        let output = env
            .exec(
                ExecRequest {
                    argv: vec!["sh".into(), "-c".into(), "cat; printf err >&2".into()],
                    stdin: Some(b"out".to_vec()),
                    ..Default::default()
                },
                &mut sink,
            )
            .await
            .unwrap();

        assert_eq!(output.exit_code, 0);
        assert_eq!(output.stdout, b"out");
        assert_eq!(output.stderr, b"err");
        assert!(sink
            .events
            .iter()
            .any(|event| matches!(event, ExecEvent::Started { .. })));
        assert!(sink
            .events
            .iter()
            .any(|event| matches!(event, ExecEvent::Stdout { chunk } if chunk == b"out")));
        assert!(sink
            .events
            .iter()
            .any(|event| matches!(event, ExecEvent::Stderr { chunk } if chunk == b"err")));
        assert!(sink
            .events
            .iter()
            .any(|event| matches!(event, ExecEvent::Exited { exit_code: 0 })));
    }

    #[tokio::test]
    async fn local_environment_exec_reads_output_while_writing_large_stdin() {
        let env = LocalExecutionEnvironment::new(None).unwrap();
        let mut sink = RecordingSink::default();
        let stdin = vec![b'x'; 2 * 1024 * 1024];
        let output = timeout(
            Duration::from_secs(5),
            env.exec(
                ExecRequest {
                    argv: vec![
                        "sh".into(),
                        "-c".into(),
                        "printf ready; cat >/dev/null".into(),
                    ],
                    stdin: Some(stdin),
                    ..Default::default()
                },
                &mut sink,
            ),
        )
        .await
        .expect("exec should not deadlock")
        .unwrap();

        assert_eq!(output.exit_code, 0);
        assert_eq!(output.stdout, b"ready");
    }

    #[tokio::test]
    async fn local_environment_spawn_reaps_and_emits_exit() {
        let env = LocalExecutionEnvironment::new(None).unwrap();
        let sink = SharedRecordingSink::default();
        let events = Arc::clone(&sink.events);
        let output = env
            .spawn(
                ExecRequest {
                    argv: vec![
                        "sh".into(),
                        "-c".into(),
                        "printf spawned; printf err >&2".into(),
                    ],
                    ..Default::default()
                },
                Some(Box::new(sink)),
            )
            .await
            .unwrap();

        assert!(output.process_id.is_some());
        let started = Instant::now();
        loop {
            let snapshot = events.lock().unwrap().clone();
            if snapshot
                .iter()
                .any(|event| matches!(event, ExecEvent::Exited { exit_code: 0 }))
            {
                assert!(snapshot
                    .iter()
                    .any(|event| matches!(event, ExecEvent::Started { .. })));
                assert!(snapshot.iter().any(
                    |event| matches!(event, ExecEvent::Stdout { chunk } if chunk == b"spawned")
                ));
                assert!(snapshot
                    .iter()
                    .any(|event| matches!(event, ExecEvent::Stderr { chunk } if chunk == b"err")));
                break;
            }
            assert!(started.elapsed() < Duration::from_secs(5));
            sleep(Duration::from_millis(10)).await;
        }
    }

    #[tokio::test]
    async fn local_environment_create_temp_dir_cleans_up_on_drop() {
        let path = {
            let env = LocalExecutionEnvironment::new(None).unwrap();
            let path = env.create_temp_dir("smol-wf-test-").await.unwrap();
            let pathbuf = PathBuf::from(path.as_str());
            assert!(pathbuf.exists());
            pathbuf
        };
        assert!(!path.exists());
    }
}
