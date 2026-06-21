use crate::config::{load_config, profile_for, Config, ProfileConfig};
use crate::error::{
    bad_profile, invalid_request, provider_error, provider_failure, unsupported_method,
    ProviderResult,
};
use crate::exe_api::{find_vm, DirectSshDataPlane, SshExeControlPlane};
use crate::quoting::{quote_argv, resolve_remote_path, shell_quote};
use crate::ssh::SshRunner;
use crate::state::{
    load_group_states, load_persisted_state, persist_state, remove_state, ProviderState,
};
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use smol_workflow_sandbox::{
    Capabilities, CleanupSandboxGroupRequest, CreateTempDirRequest, JsonlResponseEnvelope,
    OpenSandboxRequest, SandboxExecEvent, SandboxExecRequest, SandboxSession, SandboxSpawnRequest,
    SessionPathRequest, WriteFileRequest,
};
use std::collections::BTreeMap;
use std::process::Stdio;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;

const VM_NAME_PREFIX: &str = "smol-workflows-exe-dev";
const MAX_VM_NAME_LEN: usize = 52;

#[derive(Debug, Clone)]
pub struct ExeDevProvider {
    config: Config,
    spawned_pids: BTreeMap<String, TrackedSpawn>,
}

#[derive(Debug, Clone)]
struct TrackedSpawn {
    ssh_dest: String,
    profile_name: Option<String>,
    pids: Vec<String>,
}

impl ExeDevProvider {
    pub fn from_environment() -> ProviderResult<Self> {
        Ok(Self {
            config: load_config()?,
            spawned_pids: BTreeMap::new(),
        })
    }

    pub fn new(config: Config) -> Self {
        Self {
            config,
            spawned_pids: BTreeMap::new(),
        }
    }

    pub async fn handle(
        &mut self,
        method: &str,
        params: Value,
    ) -> Result<ProviderAction, smol_workflow_sandbox::ProviderError> {
        let mut sink = tokio::io::sink();
        let mut events = EventWriter::new("", &mut sink);
        self.handle_with_events("", method, params, &mut events)
            .await
    }

    pub async fn handle_with_events<W>(
        &mut self,
        request_id: &str,
        method: &str,
        params: Value,
        events: &mut EventWriter<'_, W>,
    ) -> Result<ProviderAction, smol_workflow_sandbox::ProviderError>
    where
        W: AsyncWrite + Unpin,
    {
        match method {
            "capabilities" => Ok(ProviderAction::Respond(json!(Capabilities { exec: true }))),
            "open" => self.open(params).await.map(ProviderAction::Respond),
            "close" => self.close(params).await.map(ProviderAction::Respond),
            "cleanup_group" => self
                .cleanup_group(params)
                .await
                .map(ProviderAction::Respond),
            "create_temp_dir" => self
                .create_temp_dir(params)
                .await
                .map(ProviderAction::Respond),
            "create_dir_all" => self
                .create_dir_all(params)
                .await
                .map(ProviderAction::Respond),
            "write_file" => self.write_file(params).await.map(ProviderAction::Respond),
            "read_file" => self.read_file(params).await.map(ProviderAction::Respond),
            "remove" => self.remove(params).await.map(ProviderAction::Respond),
            "exec" => self
                .exec(request_id, params, events)
                .await
                .map(ProviderAction::Respond),
            "spawn" => self
                .spawn(request_id, params, events)
                .await
                .map(ProviderAction::Respond),
            "shutdown" => {
                self.shutdown_spawned().await;
                Ok(ProviderAction::Shutdown(json!({})))
            }
            other => Err(unsupported_method(other)),
        }
    }

    async fn open(&self, params: Value) -> ProviderResult<Value> {
        let request: OpenSandboxRequest = serde_json::from_value(params)
            .map_err(|source| invalid_request(format!("invalid open request: {source}")))?;
        let profile_name = request.profile.name.clone();
        let profile = profile_for(
            &self.config,
            &request.profile.provider,
            &request.profile.name,
        )?
        .clone();
        ensure_ssh_control_plane(&profile)?;
        ensure_workspace_sync_mode(&profile)?;

        let cwd = request.cwd.clone().unwrap_or_else(|| profile.cwd.clone());
        let session_id = format!("session_{}", unique_suffix());
        let vm_name = generate_vm_name(&request.metadata.sandbox_group_id);
        let control = SshExeControlPlane::new(profile.ssh.clone());
        let data_plane = DirectSshDataPlane::new(profile.ssh.clone());

        let created_vm = control.new_vm(&vm_name, &profile).await?;
        let created_ssh_dest = created_vm.ssh_dest.clone();
        let finish_open = async {
            let ssh_dest = match created_ssh_dest {
                Some(dest) if !dest.trim().is_empty() => dest,
                _ => resolve_ssh_dest(&control, &vm_name).await?,
            };

            data_plane.wait_until_ready(&ssh_dest).await?;
            data_plane.create_dir_all(&ssh_dest, &cwd).await?;
            if profile.sync_workspace {
                data_plane
                    .sync_workspace_tar(
                        &ssh_dest,
                        &request.workspace_sync.host_path,
                        &cwd,
                        &profile.workspace_sync,
                    )
                    .await?;
            }

            let cleanup_on_close = cleanup_on_close(&profile);
            let state = ProviderState::new(
                request.metadata.sandbox_group_id,
                session_id.clone(),
                vm_name.clone(),
                ssh_dest.clone(),
                cwd.clone(),
                cleanup_on_close,
            )
            .with_profile_name(profile_name);
            persist_state(&state)?;

            Ok(SandboxSession {
                id: session_id,
                provider_session_id: Some(vm_name.clone()),
                cwd: Some(cwd),
                capabilities: Capabilities { exec: true },
                provider_state_json: Some(state.to_provider_state_json()?),
            })
        }
        .await;

        match finish_open {
            Ok(session) => Ok(json!(session)),
            Err(error) => {
                cleanup_created_vm_after_open_error(&control, &profile, &vm_name, &error).await;
                Err(error)
            }
        }
    }

    async fn close(&mut self, params: Value) -> ProviderResult<Value> {
        let request: CloseRequest = serde_json::from_value(params)
            .map_err(|source| invalid_request(format!("invalid close request: {source}")))?;
        let Some(state_json) = request.session.provider_state_json.as_deref() else {
            return Err(invalid_request(
                "close request session is missing exe.dev provider_state_json",
            ));
        };
        let mut state = ProviderState::from_provider_state_json(state_json)?;
        if let Some(persisted) = load_persisted_state(&state)? {
            state = persisted;
        }
        let ssh_config = self.profile_for_state(&state)?.ssh.clone();
        let control = SshExeControlPlane::new(ssh_config.clone());
        let data_plane = DirectSshDataPlane::new(ssh_config);
        let pids = self.take_tracked_pids(&state);
        if let Err(error) = data_plane.kill_pids(&state.ssh_dest, &pids).await {
            eprintln!(
                "smol-sandbox-exe-dev: failed to terminate spawned processes for VM `{}`: {}",
                state.vm_name, error.code
            );
        }

        if state.cleanup_on_close {
            control.remove_vm(&state.vm_name).await?;
            remove_state(&state)?;
        } else {
            eprintln!(
                "smol-sandbox-exe-dev: keeping exe.dev VM `{}` at `{}` for debugging",
                state.vm_name, state.ssh_dest
            );
        }
        Ok(json!({}))
    }

    async fn cleanup_group(&mut self, params: Value) -> ProviderResult<Value> {
        let request: CleanupSandboxGroupRequest =
            serde_json::from_value(params).map_err(|source| {
                invalid_request(format!("invalid cleanup_group request: {source}"))
            })?;
        let mut cleaned_count = 0u32;

        for state in load_group_states(&request.sandbox_group_id)? {
            let ssh_config = self.profile_for_state(&state)?.ssh.clone();
            let data_plane = DirectSshDataPlane::new(ssh_config.clone());
            let pids = self.take_tracked_pids(&state);
            if let Err(error) = data_plane.kill_pids(&state.ssh_dest, &pids).await {
                eprintln!(
                    "smol-sandbox-exe-dev: failed to terminate spawned processes for VM `{}` during cleanup_group: {}",
                    state.vm_name, error.code
                );
            }
            let control = SshExeControlPlane::new(ssh_config);
            control.remove_vm(&state.vm_name).await?;
            remove_state(&state)?;
            cleaned_count += 1;
        }

        Ok(json!({ "cleaned_count": cleaned_count }))
    }

    async fn create_temp_dir(&self, params: Value) -> ProviderResult<Value> {
        let request: CreateTempDirRequest = serde_json::from_value(params).map_err(|source| {
            invalid_request(format!("invalid create_temp_dir request: {source}"))
        })?;
        let state = state_from_session(&request.session)?;
        let cwd = request
            .session
            .cwd
            .as_deref()
            .map(|cwd| resolve_remote_path(&state.cwd, cwd))
            .unwrap_or_else(|| state.cwd.clone());
        let profile = self.profile_for_state(&state)?;
        let data_plane = DirectSshDataPlane::new(profile.ssh.clone());
        let path = data_plane
            .create_temp_dir(&state.ssh_dest, &cwd, &request.prefix)
            .await?;
        Ok(json!({ "path": path }))
    }

    async fn create_dir_all(&self, params: Value) -> ProviderResult<Value> {
        let request: SessionPathRequest = serde_json::from_value(params).map_err(|source| {
            invalid_request(format!("invalid create_dir_all request: {source}"))
        })?;
        let (state, path) = resolved_session_path(&request.session, &request.path)?;
        let profile = self.profile_for_state(&state)?;
        let data_plane = DirectSshDataPlane::new(profile.ssh.clone());
        data_plane.create_dir_all(&state.ssh_dest, &path).await?;
        Ok(json!({}))
    }

    async fn write_file(&self, params: Value) -> ProviderResult<Value> {
        let request: WriteFileRequest = serde_json::from_value(params)
            .map_err(|source| invalid_request(format!("invalid write_file request: {source}")))?;
        let content = BASE64_STANDARD
            .decode(request.content_base64.as_bytes())
            .map_err(|source| invalid_request(format!("invalid content_base64: {source}")))?;
        let (state, path) = resolved_session_path(&request.session, &request.path)?;
        let profile = self.profile_for_state(&state)?;
        let data_plane = DirectSshDataPlane::new(profile.ssh.clone());
        data_plane
            .write_file(&state.ssh_dest, &path, &content)
            .await?;
        Ok(json!({}))
    }

    async fn read_file(&self, params: Value) -> ProviderResult<Value> {
        let request: SessionPathRequest = serde_json::from_value(params)
            .map_err(|source| invalid_request(format!("invalid read_file request: {source}")))?;
        let (state, path) = resolved_session_path(&request.session, &request.path)?;
        let profile = self.profile_for_state(&state)?;
        let data_plane = DirectSshDataPlane::new(profile.ssh.clone());
        let content = data_plane.read_file(&state.ssh_dest, &path).await?;
        Ok(json!({ "content_base64": BASE64_STANDARD.encode(content) }))
    }

    async fn remove(&self, params: Value) -> ProviderResult<Value> {
        let request: SessionPathRequest = serde_json::from_value(params)
            .map_err(|source| invalid_request(format!("invalid remove request: {source}")))?;
        let (state, path) = resolved_session_path(&request.session, &request.path)?;
        let profile = self.profile_for_state(&state)?;
        let data_plane = DirectSshDataPlane::new(profile.ssh.clone());
        data_plane.remove(&state.ssh_dest, &path).await?;
        Ok(json!({}))
    }

    async fn exec<W>(
        &self,
        _request_id: &str,
        params: Value,
        events: &mut EventWriter<'_, W>,
    ) -> ProviderResult<Value>
    where
        W: AsyncWrite + Unpin,
    {
        let request: SandboxExecRequest = serde_json::from_value(params)
            .map_err(|source| invalid_request(format!("invalid exec request: {source}")))?;
        validate_command_request("exec", &request.argv, request.cwd.as_deref(), &request.env)?;
        let stdin = decode_optional_base64("stdin_base64", request.stdin_base64)?;
        let state = state_from_session(&request.session)?;
        let cwd = effective_command_cwd(&request.session, &state, request.cwd.as_deref())?;
        let ssh_config = self.profile_for_state(&state)?.ssh.clone();
        let shell_command = build_exec_shell_command(&cwd, &request.env, &request.argv);
        let runner = SshRunner::new(ssh_config);
        let mut command = runner.command(&state.ssh_dest, &[shell_command]);
        command
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().map_err(|source| {
            provider_error(
                "exec_failed",
                format!("failed to start SSH exec process: {source}"),
                false,
            )
        })?;
        let process_id = child.id().map(|id| id.to_string());
        events
            .emit(SandboxExecEvent {
                r#type: "started".to_string(),
                process_id,
                data_base64: None,
                exit_code: None,
            })
            .await?;

        let stdout = child.stdout.take().ok_or_else(|| {
            provider_error(
                "exec_failed",
                "failed to capture SSH exec stdout pipe",
                false,
            )
        })?;
        let stderr = child.stderr.take().ok_or_else(|| {
            provider_error(
                "exec_failed",
                "failed to capture SSH exec stderr pipe",
                false,
            )
        })?;

        let stdin_task = if let Some(stdin) = stdin {
            let child_stdin = child.stdin.take().ok_or_else(|| {
                provider_error("exec_failed", "failed to open SSH exec stdin pipe", false)
            })?;
            Some(tokio::spawn(write_child_stdin(child_stdin, stdin)))
        } else {
            None
        };

        let (tx, mut rx) = mpsc::unbounded_channel();
        let stdout_task = tokio::spawn(read_child_stream(stdout, OutputKind::Stdout, tx.clone()));
        let stderr_task = tokio::spawn(read_child_stream(stderr, OutputKind::Stderr, tx.clone()));
        drop(tx);

        let mut stdout_chunks = Vec::new();
        let mut stderr_chunks = Vec::new();
        while let Some(chunk) = rx.recv().await {
            match chunk {
                OutputChunk::Stdout(data) => {
                    events
                        .emit(SandboxExecEvent {
                            r#type: "stdout".to_string(),
                            process_id: None,
                            data_base64: Some(BASE64_STANDARD.encode(&data)),
                            exit_code: None,
                        })
                        .await?;
                    stdout_chunks.extend_from_slice(&data);
                }
                OutputChunk::Stderr(data) => {
                    events
                        .emit(SandboxExecEvent {
                            r#type: "stderr".to_string(),
                            process_id: None,
                            data_base64: Some(BASE64_STANDARD.encode(&data)),
                            exit_code: None,
                        })
                        .await?;
                    stderr_chunks.extend_from_slice(&data);
                }
            }
        }

        join_io_task(stdout_task, "read SSH exec stdout").await?;
        join_io_task(stderr_task, "read SSH exec stderr").await?;
        if let Some(stdin_task) = stdin_task {
            join_io_task(stdin_task, "write SSH exec stdin").await?;
        }
        let status = child.wait().await.map_err(|source| {
            provider_error(
                "exec_failed",
                format!("failed to wait for SSH exec process: {source}"),
                false,
            )
        })?;
        let exit_code = status.code().unwrap_or(-1);
        events
            .emit(SandboxExecEvent {
                r#type: "exited".to_string(),
                process_id: None,
                data_base64: None,
                exit_code: Some(exit_code),
            })
            .await?;

        Ok(json!({
            "exit_code": exit_code,
            "stdout_base64": BASE64_STANDARD.encode(stdout_chunks),
            "stderr_base64": BASE64_STANDARD.encode(stderr_chunks),
        }))
    }

    async fn spawn<W>(
        &mut self,
        _request_id: &str,
        params: Value,
        events: &mut EventWriter<'_, W>,
    ) -> ProviderResult<Value>
    where
        W: AsyncWrite + Unpin,
    {
        let request: SandboxSpawnRequest = serde_json::from_value(params)
            .map_err(|source| invalid_request(format!("invalid spawn request: {source}")))?;
        validate_command_request("spawn", &request.argv, request.cwd.as_deref(), &request.env)?;
        let stdin = decode_optional_base64("stdin_base64", request.stdin_base64)?;
        let state = state_from_session(&request.session)?;
        let cwd = effective_command_cwd(&request.session, &state, request.cwd.as_deref())?;
        let ssh_config = self.profile_for_state(&state)?.ssh.clone();
        let data_plane = DirectSshDataPlane::new(ssh_config.clone());
        let spawn_id = unique_suffix();
        let stdin_path = if let Some(stdin) = stdin {
            let path = format!(
                "{}/.smol-spawn-stdin-{spawn_id}",
                state.cwd.trim_end_matches('/')
            );
            data_plane
                .write_file(&state.ssh_dest, &path, &stdin)
                .await?;
            Some(path)
        } else {
            None
        };
        let shell_command = build_spawn_shell_command(
            &cwd,
            &request.env,
            &request.argv,
            &spawn_id,
            stdin_path.as_deref(),
        );
        let runner = SshRunner::new(ssh_config);
        let output = runner
            .run(&state.ssh_dest, &[shell_command])
            .await
            .map_err(|source| {
                provider_error(
                    "exec_failed",
                    format!("failed to run SSH spawn command: {source}"),
                    false,
                )
            })?;
        if !output.success() {
            return Err(provider_error(
                "exec_failed",
                format!(
                    "remote spawn command failed with status {:?}: {}",
                    output.status_code,
                    output.stderr_text()
                ),
                false,
            ));
        }
        let process_id = String::from_utf8_lossy(&output.stdout)
            .lines()
            .next()
            .unwrap_or("")
            .trim()
            .to_string();
        if process_id.is_empty() {
            return Err(provider_error(
                "exec_failed",
                "remote spawn command did not return a PID",
                false,
            ));
        }
        self.record_spawned_pid(&state, &process_id)?;
        events
            .emit(SandboxExecEvent {
                r#type: "started".to_string(),
                process_id: Some(process_id.clone()),
                data_base64: None,
                exit_code: None,
            })
            .await?;
        Ok(json!({ "process_id": process_id }))
    }

    fn record_spawned_pid(&mut self, state: &ProviderState, pid: &str) -> ProviderResult<()> {
        let tracked = self
            .spawned_pids
            .entry(state.session_id.clone())
            .or_insert_with(|| TrackedSpawn {
                ssh_dest: state.ssh_dest.clone(),
                profile_name: state.profile_name.clone(),
                pids: Vec::new(),
            });
        push_unique(&mut tracked.pids, pid.to_string());

        let mut persisted = load_persisted_state(state)?.unwrap_or_else(|| state.clone());
        push_unique(&mut persisted.spawned_pids, pid.to_string());
        persist_state(&persisted)?;
        Ok(())
    }

    fn take_tracked_pids(&mut self, state: &ProviderState) -> Vec<String> {
        let mut pids = state.spawned_pids.clone();
        if let Some(tracked) = self.spawned_pids.remove(&state.session_id) {
            for pid in tracked.pids {
                push_unique(&mut pids, pid);
            }
        }
        pids
    }

    async fn shutdown_spawned(&mut self) {
        let tracked = std::mem::take(&mut self.spawned_pids);
        for tracked in tracked.into_values() {
            let ssh_config = tracked
                .profile_name
                .as_deref()
                .and_then(|name| self.config.profiles.get(name))
                .or_else(|| self.config.profiles.get("default"))
                .or_else(|| self.config.profiles.values().next())
                .map(|profile| profile.ssh.clone());
            let Some(ssh_config) = ssh_config else {
                continue;
            };
            let data_plane = DirectSshDataPlane::new(ssh_config);
            if let Err(error) = data_plane.kill_pids(&tracked.ssh_dest, &tracked.pids).await {
                eprintln!(
                    "smol-sandbox-exe-dev: failed to terminate spawned processes during shutdown: {}",
                    error.code
                );
            }
        }
    }

    fn profile_for_state(&self, state: &ProviderState) -> ProviderResult<&ProfileConfig> {
        if let Some(profile_name) = state.profile_name.as_deref() {
            return self.config.profiles.get(profile_name).ok_or_else(|| {
                bad_profile(format!(
                    "exe.dev sandbox profile `{profile_name}` from provider state is not configured"
                ))
            });
        }
        self.default_cleanup_profile()
    }

    fn default_cleanup_profile(&self) -> ProviderResult<&ProfileConfig> {
        self.config
            .profiles
            .get("default")
            .or_else(|| self.config.profiles.values().next())
            .ok_or_else(|| bad_profile("exe.dev sandbox config has no profiles"))
    }
}

#[derive(Debug, Deserialize)]
struct CloseRequest {
    session: SandboxSession,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProviderAction {
    Respond(Value),
    Shutdown(Value),
}

pub struct EventWriter<'a, W> {
    request_id: &'a str,
    writer: &'a mut W,
}

impl<'a, W> EventWriter<'a, W>
where
    W: AsyncWrite + Unpin,
{
    pub fn new(request_id: &'a str, writer: &'a mut W) -> Self {
        Self { request_id, writer }
    }

    pub async fn emit<T>(&mut self, event: T) -> ProviderResult<()>
    where
        T: Serialize,
    {
        let event = serde_json::to_value(event).map_err(|source| {
            provider_failure(format!("failed to encode JSONL event: {source}"))
        })?;
        let response = JsonlResponseEnvelope {
            id: self.request_id.to_string(),
            result: None,
            error: None,
            event: Some(event),
        };
        let mut line = serde_json::to_vec(&response).map_err(|source| {
            provider_failure(format!("failed to serialize JSONL event: {source}"))
        })?;
        line.push(b'\n');
        self.writer
            .write_all(&line)
            .await
            .map_err(|source| provider_failure(format!("failed to write JSONL event: {source}")))?;
        self.writer
            .flush()
            .await
            .map_err(|source| provider_failure(format!("failed to flush JSONL event: {source}")))
    }
}

async fn resolve_ssh_dest(control: &SshExeControlPlane, vm_name: &str) -> ProviderResult<String> {
    let vms = control.list_vms().await?;
    find_vm(&vms, vm_name)
        .and_then(|vm| vm.ssh_dest.clone())
        .filter(|dest| !dest.trim().is_empty())
        .ok_or_else(|| {
            provider_error(
                "exe_ls_failed",
                format!("exe.dev ls did not include ssh_dest for VM `{vm_name}`"),
                false,
            )
        })
}

fn ensure_ssh_control_plane(profile: &ProfileConfig) -> ProviderResult<()> {
    if profile.control_plane.mode == "ssh" {
        Ok(())
    } else {
        Err(bad_profile(format!(
            "exe.dev control plane mode `{}` is not implemented; only `ssh` is supported",
            profile.control_plane.mode
        )))
    }
}

fn ensure_workspace_sync_mode(profile: &ProfileConfig) -> ProviderResult<()> {
    if !profile.sync_workspace || profile.workspace_sync.mode == "tar" {
        Ok(())
    } else {
        Err(bad_profile(format!(
            "exe.dev workspace_sync mode `{}` is not implemented; only `tar` is supported",
            profile.workspace_sync.mode
        )))
    }
}

fn state_from_session(session: &SandboxSession) -> ProviderResult<ProviderState> {
    let Some(state_json) = session.provider_state_json.as_deref() else {
        return Err(invalid_request(
            "request session is missing exe.dev provider_state_json",
        ));
    };
    ProviderState::from_provider_state_json(state_json)
}

fn resolved_session_path(
    session: &SandboxSession,
    path: &str,
) -> ProviderResult<(ProviderState, String)> {
    let state = state_from_session(session)?;
    let cwd = session.cwd.clone().unwrap_or_else(|| state.cwd.clone());
    Ok((state, resolve_remote_path(&cwd, path)))
}

fn effective_command_cwd(
    session: &SandboxSession,
    state: &ProviderState,
    cwd_override: Option<&str>,
) -> ProviderResult<String> {
    if let Some(cwd) = cwd_override {
        ensure_no_nul(cwd, "cwd")?;
        let base = session.cwd.as_deref().unwrap_or(&state.cwd);
        return Ok(resolve_remote_path(base, cwd));
    }
    let cwd = session.cwd.as_deref().unwrap_or(&state.cwd);
    ensure_no_nul(cwd, "session cwd")?;
    Ok(resolve_remote_path(&state.cwd, cwd))
}

fn validate_command_request(
    method: &str,
    argv: &[String],
    cwd: Option<&str>,
    env: &BTreeMap<String, String>,
) -> ProviderResult<()> {
    if argv.is_empty() {
        return Err(invalid_request(format!("{method} argv must not be empty")));
    }
    for (index, arg) in argv.iter().enumerate() {
        ensure_no_nul(arg, &format!("argv[{index}]"))?;
    }
    if let Some(cwd) = cwd {
        ensure_no_nul(cwd, "cwd")?;
    }
    for (key, value) in env {
        if key.is_empty() {
            return Err(invalid_request(
                "environment variable names must not be empty",
            ));
        }
        if key.contains('=') {
            return Err(invalid_request(format!(
                "environment variable name `{key}` must not contain `=`"
            )));
        }
        ensure_no_nul(key, "environment variable name")?;
        ensure_no_nul(value, &format!("environment variable `{key}` value"))?;
    }
    Ok(())
}

fn ensure_no_nul(value: &str, field: &str) -> ProviderResult<()> {
    if value.contains('\0') {
        Err(invalid_request(format!(
            "{field} must not contain NUL bytes"
        )))
    } else {
        Ok(())
    }
}

fn decode_optional_base64(field: &str, value: Option<String>) -> ProviderResult<Option<Vec<u8>>> {
    value
        .map(|value| {
            BASE64_STANDARD
                .decode(value.as_bytes())
                .map_err(|source| invalid_request(format!("invalid {field}: {source}")))
        })
        .transpose()
}

fn build_exec_shell_command(cwd: &str, env: &BTreeMap<String, String>, argv: &[String]) -> String {
    format!(
        "cd {} && exec {}",
        shell_quote(cwd),
        build_command_invocation(env, argv)
    )
}

fn build_spawn_shell_command(
    cwd: &str,
    env: &BTreeMap<String, String>,
    argv: &[String],
    spawn_id: &str,
    stdin_path: Option<&str>,
) -> String {
    let stdout_path = format!("/tmp/smol-spawn-{spawn_id}.out");
    let stderr_path = format!("/tmp/smol-spawn-{spawn_id}.err");
    let invocation = if let Some(stdin_path) = stdin_path {
        let inner = format!(
            "{} < {}; status=$?; rm -f -- {}; exit $status",
            build_command_invocation(env, argv),
            shell_quote(stdin_path),
            shell_quote(stdin_path)
        );
        format!("sh -c {}", shell_quote(&inner))
    } else {
        build_command_invocation(env, argv)
    };
    format!(
        "cd {} && {{ nohup {} > {} 2> {} < /dev/null & echo $!; }}",
        shell_quote(cwd),
        invocation,
        shell_quote(&stdout_path),
        shell_quote(&stderr_path)
    )
}

fn build_command_invocation(env: &BTreeMap<String, String>, argv: &[String]) -> String {
    let argv = quote_argv(argv);
    if env.is_empty() {
        return argv;
    }
    let assignments = env
        .iter()
        .map(|(key, value)| shell_quote(&format!("{key}={value}")))
        .collect::<Vec<_>>()
        .join(" ");
    format!("env {assignments} -- {argv}")
}

fn push_unique(values: &mut Vec<String>, value: String) {
    if !values.iter().any(|existing| existing == &value) {
        values.push(value);
    }
}

#[derive(Debug, Clone, Copy)]
enum OutputKind {
    Stdout,
    Stderr,
}

#[derive(Debug)]
enum OutputChunk {
    Stdout(Vec<u8>),
    Stderr(Vec<u8>),
}

async fn read_child_stream<R>(
    mut stream: R,
    kind: OutputKind,
    tx: mpsc::UnboundedSender<OutputChunk>,
) -> std::io::Result<()>
where
    R: AsyncRead + Unpin,
{
    let mut buffer = vec![0u8; 8192];
    loop {
        let len = stream.read(&mut buffer).await?;
        if len == 0 {
            return Ok(());
        }
        let chunk = buffer[..len].to_vec();
        let message = match kind {
            OutputKind::Stdout => OutputChunk::Stdout(chunk),
            OutputKind::Stderr => OutputChunk::Stderr(chunk),
        };
        if tx.send(message).is_err() {
            return Ok(());
        }
    }
}

async fn write_child_stdin(
    mut child_stdin: tokio::process::ChildStdin,
    stdin: Vec<u8>,
) -> std::io::Result<()> {
    match child_stdin.write_all(&stdin).await {
        Ok(()) => child_stdin.shutdown().await,
        Err(source) if source.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        Err(source) => Err(source),
    }
}

async fn join_io_task(
    task: tokio::task::JoinHandle<std::io::Result<()>>,
    operation: &str,
) -> ProviderResult<()> {
    task.await
        .map_err(|source| {
            provider_error(
                "exec_failed",
                format!("task failed while trying to {operation}: {source}"),
                false,
            )
        })?
        .map_err(|source| {
            provider_error(
                "exec_failed",
                format!("failed to {operation}: {source}"),
                false,
            )
        })
}

async fn cleanup_created_vm_after_open_error(
    control: &SshExeControlPlane,
    profile: &ProfileConfig,
    vm_name: &str,
    open_error: &smol_workflow_sandbox::ProviderError,
) {
    if cleanup_on_error(profile) {
        if let Err(cleanup_error) = control.remove_vm(vm_name).await {
            eprintln!(
                "smol-sandbox-exe-dev: failed to clean up exe.dev VM `{vm_name}` after open error `{}` (cleanup error `{}`)",
                open_error.code, cleanup_error.code
            );
        }
    } else {
        eprintln!(
            "smol-sandbox-exe-dev: keeping exe.dev VM `{vm_name}` after open error `{}` for debugging",
            open_error.code
        );
    }
}

fn cleanup_on_close(profile: &ProfileConfig) -> bool {
    cleanup_policy_allows_delete(&profile.cleanup.keep_env, &profile.cleanup.on_close)
}

fn cleanup_on_error(profile: &ProfileConfig) -> bool {
    cleanup_policy_allows_delete(&profile.cleanup.keep_env, &profile.cleanup.on_error)
}

fn cleanup_policy_allows_delete(keep_env: &str, policy: &str) -> bool {
    if env_truthy(keep_env) {
        return false;
    }
    !policy.eq_ignore_ascii_case("keep")
}

fn env_truthy(name: &str) -> bool {
    std::env::var(name)
        .map(|value| {
            matches!(
                value.to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "y" | "on" | "keep"
            )
        })
        .unwrap_or(false)
}

pub fn generate_vm_name(sandbox_group_id: &str) -> String {
    let group = sanitize_name_part(sandbox_group_id);
    let suffix = unique_suffix();
    let reserved = VM_NAME_PREFIX.len() + suffix.len() + 2;
    let max_group_len = MAX_VM_NAME_LEN.saturating_sub(reserved).max(1);
    let group = truncate_chars(&group, max_group_len);
    trim_name(&format!("{VM_NAME_PREFIX}-{group}-{suffix}"))
}

fn sanitize_name_part(value: &str) -> String {
    let mut out = String::new();
    let mut previous_hyphen = false;
    for ch in value.chars().flat_map(|ch| ch.to_lowercase()) {
        let next = if ch.is_ascii_alphanumeric() { ch } else { '-' };
        if next == '-' {
            if previous_hyphen {
                continue;
            }
            previous_hyphen = true;
        } else {
            previous_hyphen = false;
        }
        out.push(next);
    }
    trim_name(&out)
}

fn trim_name(value: &str) -> String {
    let trimmed = value.trim_matches('-');
    if trimmed.is_empty() {
        "x".to_string()
    } else {
        trimmed.to_string()
    }
}

fn truncate_chars(value: &str, max_len: usize) -> String {
    value.chars().take(max_len).collect()
}

fn unique_suffix() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let mixed = nanos ^ ((std::process::id() as u128) << 64);
    base36(mixed)
        .chars()
        .rev()
        .take(10)
        .collect::<String>()
        .chars()
        .rev()
        .collect()
}

fn base36(mut value: u128) -> String {
    if value == 0 {
        return "0".to_string();
    }
    let mut out = Vec::new();
    while value > 0 {
        let digit = (value % 36) as u8;
        out.push(match digit {
            0..=9 => (b'0' + digit) as char,
            _ => (b'a' + digit - 10) as char,
        });
        value /= 36;
    }
    out.iter().rev().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CleanupConfig;

    #[tokio::test]
    async fn handles_capabilities_cleanup_and_shutdown() {
        let mut provider = ExeDevProvider::new(Config::default());
        assert_eq!(
            provider.handle("capabilities", json!({})).await.unwrap(),
            ProviderAction::Respond(json!({ "exec": true }))
        );
        assert_eq!(
            provider
                .handle(
                    "cleanup_group",
                    json!({
                        "metadata": {"protocol_version":"sandbox.v1","request_id":"req","sandbox_group_id":"sbxgrp_missing"},
                        "sandbox_group_id": "sbxgrp_missing"
                    })
                )
                .await
                .unwrap(),
            ProviderAction::Respond(json!({ "cleaned_count": 0 }))
        );
        assert_eq!(
            provider.handle("shutdown", json!({})).await.unwrap(),
            ProviderAction::Shutdown(json!({}))
        );
    }

    #[test]
    fn vm_names_are_safe_and_prefixed() {
        let name = generate_vm_name("SbxGrp_../hello world with a very very very long suffix");
        assert!(name.starts_with("smol-workflows-exe-dev-"));
        assert!(name.len() <= MAX_VM_NAME_LEN);
        assert!(name
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-'));
        assert!(!name.ends_with('-'));
    }

    #[test]
    fn cleanup_policy_honors_profile_keep() {
        let mut profile = ProfileConfig::default();
        profile.cleanup = CleanupConfig {
            on_close: "keep".to_string(),
            ..CleanupConfig::default()
        };
        assert!(!cleanup_on_close(&profile));
    }
}
