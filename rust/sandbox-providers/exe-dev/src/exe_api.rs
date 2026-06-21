use crate::config::{ProfileConfig, SshConfig, WorkspaceSyncConfig};
use crate::error::{provider_error, ProviderResult};
use crate::ssh::{SshCommandOutput, SshRunner};
use serde::{Deserialize, Serialize};
use std::io;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::time::{sleep, Instant};

pub const CONTROL_DESTINATION: &str = "exe.dev";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExeVm {
    pub vm_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_dest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SshExeControlPlane {
    runner: SshRunner,
}

impl SshExeControlPlane {
    pub fn new(config: SshConfig) -> Self {
        Self {
            runner: SshRunner::new(config),
        }
    }

    pub async fn new_vm(&self, name: &str, profile: &ProfileConfig) -> ProviderResult<ExeVm> {
        let mut args = vec![
            "new".to_string(),
            "--name".to_string(),
            name.to_string(),
            "--image".to_string(),
            profile.image.clone(),
        ];
        if let Some(region) = &profile.region {
            args.push("--region".to_string());
            args.push(region.clone());
        }
        args.push("--json".to_string());

        let output = self.run_control(&args, "exe_new_failed").await?;
        parse_new_response(&String::from_utf8_lossy(&output.stdout)).map_err(|source| {
            provider_error(
                "exe_new_failed",
                format!("failed to parse exe.dev new JSON response: {source}"),
                false,
            )
        })
    }

    pub async fn list_vms(&self) -> ProviderResult<Vec<ExeVm>> {
        let args = vec!["ls".to_string(), "--json".to_string()];
        let output = self.run_control(&args, "exe_ls_failed").await?;
        parse_ls_response(&String::from_utf8_lossy(&output.stdout)).map_err(|source| {
            provider_error(
                "exe_ls_failed",
                format!("failed to parse exe.dev ls JSON response: {source}"),
                false,
            )
        })
    }

    pub async fn remove_vm(&self, name: &str) -> ProviderResult<()> {
        let args = vec!["rm".to_string(), name.to_string(), "--json".to_string()];
        self.run_control(&args, "exe_rm_failed").await?;
        Ok(())
    }

    async fn run_control(
        &self,
        args: &[String],
        code: &'static str,
    ) -> ProviderResult<SshCommandOutput> {
        let output = self
            .runner
            .run(CONTROL_DESTINATION, args)
            .await
            .map_err(|source| {
                provider_error(
                    code,
                    format!("failed to run exe.dev SSH control command: {source}"),
                    false,
                )
            })?;
        if output.success() {
            Ok(output)
        } else {
            let diagnostics = output_diagnostics(&output);
            Err(provider_error(
                code,
                format!(
                    "exe.dev SSH control command failed with status {:?}: {}",
                    output.status_code, diagnostics
                ),
                false,
            ))
        }
    }
}

#[derive(Debug, Clone)]
pub struct DirectSshDataPlane {
    runner: SshRunner,
}

impl DirectSshDataPlane {
    pub fn new(config: SshConfig) -> Self {
        Self {
            runner: SshRunner::new(config),
        }
    }

    pub async fn wait_until_ready(&self, ssh_dest: &str) -> ProviderResult<()> {
        let timeout = Duration::from_millis(env_u64("SMOL_EXE_DEV_READY_TIMEOUT_MS", 60_000));
        let mut delay = Duration::from_millis(env_u64("SMOL_EXE_DEV_READY_INITIAL_DELAY_MS", 250));
        let max_delay = Duration::from_millis(env_u64("SMOL_EXE_DEV_READY_MAX_DELAY_MS", 5_000));
        let deadline = Instant::now() + timeout;
        let args = vec!["true".to_string()];

        loop {
            let last_error = match self.runner.run(ssh_dest, &args).await {
                Ok(output) if output.success() => return Ok(()),
                Ok(output) => format!("status {:?}: {}", output.status_code, output.stderr_text()),
                Err(source) => source.to_string(),
            };

            if Instant::now() >= deadline {
                return Err(provider_error(
                    "ssh_not_ready",
                    format!("VM SSH destination `{ssh_dest}` did not become ready: {last_error}"),
                    true,
                ));
            }
            sleep(delay).await;
            delay = std::cmp::min(delay.saturating_mul(2), max_delay);
        }
    }

    pub async fn create_dir_all(&self, ssh_dest: &str, path: &str) -> ProviderResult<()> {
        let output = self
            .run_shell(
                ssh_dest,
                &format!("mkdir -p -- {}", shell_quote(path)),
                "file_io_failed",
            )
            .await?;
        ensure_success(output, "file_io_failed", "remote mkdir")
    }

    pub async fn sync_workspace_tar(
        &self,
        ssh_dest: &str,
        host_path: &Path,
        remote_cwd: &str,
        sync: &WorkspaceSyncConfig,
    ) -> ProviderResult<()> {
        let attempts = ssh_retry_attempts();
        let mut last_error = None;
        for attempt in 0..attempts {
            match self
                .sync_workspace_tar_once(ssh_dest, host_path, remote_cwd, sync)
                .await
            {
                Ok(()) => return Ok(()),
                Err(error) if attempt + 1 < attempts && is_retryable_ssh_provider_error(&error) => {
                    last_error = Some(error);
                    sleep(ssh_retry_delay(attempt)).await;
                }
                Err(error) => return Err(error),
            }
        }
        Err(last_error.unwrap_or_else(|| {
            provider_error(
                "workspace_sync_failed",
                "workspace upload failed without an error",
                false,
            )
        }))
    }

    async fn sync_workspace_tar_once(
        &self,
        ssh_dest: &str,
        host_path: &Path,
        remote_cwd: &str,
        sync: &WorkspaceSyncConfig,
    ) -> ProviderResult<()> {
        let mut tar_command = tokio::process::Command::new("tar");
        for exclude in &sync.exclude {
            tar_command.arg("--exclude").arg(exclude);
        }
        tar_command
            .arg("-C")
            .arg(host_path)
            .arg("-cf")
            .arg("-")
            .arg(".")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut tar_child = tar_command.spawn().map_err(|source| {
            provider_error(
                "workspace_sync_failed",
                format!("failed to start local tar for workspace upload: {source}"),
                false,
            )
        })?;
        let mut tar_stdout = tar_child.stdout.take().ok_or_else(|| {
            provider_error(
                "workspace_sync_failed",
                "failed to capture local tar stdout for workspace upload",
                false,
            )
        })?;

        let remote_command = format!("tar -C {} -xf -", shell_quote(remote_cwd));
        let remote_args = vec![remote_command];
        let mut ssh_command = self.runner.command(ssh_dest, &remote_args);
        ssh_command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut ssh_child = ssh_command.spawn().map_err(|source| {
            provider_error(
                "workspace_sync_failed",
                format!("failed to start SSH tar extraction: {source}"),
                false,
            )
        })?;
        let mut ssh_stdin = ssh_child.stdin.take().ok_or_else(|| {
            provider_error(
                "workspace_sync_failed",
                "failed to open SSH stdin for workspace upload",
                false,
            )
        })?;

        let copy_result = tokio::io::copy(&mut tar_stdout, &mut ssh_stdin).await;
        let _ = ssh_stdin.shutdown().await;
        drop(ssh_stdin);

        let tar_output = tar_child.wait_with_output().await.map_err(|source| {
            provider_error(
                "workspace_sync_failed",
                format!("failed to wait for local tar during workspace upload: {source}"),
                false,
            )
        })?;
        let ssh_output = ssh_child.wait_with_output().await.map_err(|source| {
            provider_error(
                "workspace_sync_failed",
                format!("failed to wait for SSH tar extraction: {source}"),
                false,
            )
        })?;

        if let Err(source) = copy_result {
            let retryable = is_retryable_ssh_io_error(&source);
            return Err(provider_error(
                "workspace_sync_failed",
                format!("failed to stream workspace tar over SSH: {source}"),
                retryable,
            ));
        }
        if !tar_output.status.success() {
            return Err(provider_error(
                "workspace_sync_failed",
                format!(
                    "local tar failed with status {:?}: {}",
                    tar_output.status.code(),
                    String::from_utf8_lossy(&tar_output.stderr).trim()
                ),
                false,
            ));
        }
        let ssh_output = SshCommandOutput {
            status_code: ssh_output.status.code(),
            stdout: ssh_output.stdout,
            stderr: ssh_output.stderr,
        };
        if is_retryable_ssh_output(&ssh_output) {
            return Err(provider_error(
                "workspace_sync_failed",
                format!(
                    "remote tar extraction failed with status {:?}: {}",
                    ssh_output.status_code,
                    ssh_output.stderr_text()
                ),
                true,
            ));
        }
        ensure_success(ssh_output, "workspace_sync_failed", "remote tar extraction")
    }

    pub async fn create_temp_dir(
        &self,
        ssh_dest: &str,
        cwd: &str,
        prefix: &str,
    ) -> ProviderResult<String> {
        let prefix = sanitize_temp_prefix(prefix);
        let template = format!("{}/{prefix}.XXXXXX", cwd.trim_end_matches('/'));
        let output = self
            .run_shell(
                ssh_dest,
                &format!("mktemp -d {}", shell_quote(&template)),
                "file_io_failed",
            )
            .await?;
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if path.is_empty() {
            Err(provider_error(
                "file_io_failed",
                "remote mktemp did not return a path",
                false,
            ))
        } else {
            Ok(path)
        }
    }

    pub async fn write_file(
        &self,
        ssh_dest: &str,
        path: &str,
        content: &[u8],
    ) -> ProviderResult<()> {
        let parent = remote_parent(path);
        let command = format!(
            "mkdir -p -- {} && cat > {}",
            shell_quote(&parent),
            shell_quote(path)
        );
        let attempts = ssh_retry_attempts();
        let mut last_error = None;
        for attempt in 0..attempts {
            let output = match self
                .runner
                .run_with_stdin(ssh_dest, std::slice::from_ref(&command), content)
                .await
            {
                Ok(output) => output,
                Err(source) if attempt + 1 < attempts && is_retryable_ssh_io_error(&source) => {
                    last_error = Some(provider_error(
                        "file_io_failed",
                        format!("failed to write remote file over SSH: {source}"),
                        true,
                    ));
                    sleep(ssh_retry_delay(attempt)).await;
                    continue;
                }
                Err(source) => {
                    return Err(provider_error(
                        "file_io_failed",
                        format!("failed to write remote file over SSH: {source}"),
                        false,
                    ));
                }
            };
            if output.success() {
                return Ok(());
            }
            if attempt + 1 < attempts && is_retryable_ssh_output(&output) {
                last_error = Some(provider_error(
                    "file_io_failed",
                    format!(
                        "remote file write failed with status {:?}: {}",
                        output.status_code,
                        output.stderr_text()
                    ),
                    true,
                ));
                sleep(ssh_retry_delay(attempt)).await;
                continue;
            }
            return ensure_success(output, "file_io_failed", "remote file write");
        }
        Err(last_error.unwrap_or_else(|| {
            provider_error(
                "file_io_failed",
                "remote file write failed without an error",
                false,
            )
        }))
    }

    pub async fn read_file(&self, ssh_dest: &str, path: &str) -> ProviderResult<Vec<u8>> {
        let output = self
            .run_shell(
                ssh_dest,
                &format!("cat -- {}", shell_quote(path)),
                "file_io_failed",
            )
            .await?;
        Ok(output.stdout)
    }

    pub async fn remove(&self, ssh_dest: &str, path: &str) -> ProviderResult<()> {
        let output = self
            .run_shell(
                ssh_dest,
                &format!("rm -rf -- {}", shell_quote(path)),
                "file_io_failed",
            )
            .await?;
        ensure_success(output, "file_io_failed", "remote remove")
    }

    pub async fn kill_pids(&self, ssh_dest: &str, pids: &[String]) -> ProviderResult<()> {
        let safe_pids = pids
            .iter()
            .filter(|pid| is_safe_remote_pid(pid))
            .map(|pid| shell_quote(pid))
            .collect::<Vec<_>>();
        if safe_pids.is_empty() {
            return Ok(());
        }
        let joined = safe_pids.join(" ");
        let command = format!(
            "kill -TERM -- {joined} 2>/dev/null || true; kill -KILL -- {joined} 2>/dev/null || true"
        );
        self.run_shell(ssh_dest, &command, "exec_failed").await?;
        Ok(())
    }

    async fn run_shell(
        &self,
        ssh_dest: &str,
        command: &str,
        code: &'static str,
    ) -> ProviderResult<SshCommandOutput> {
        let args = vec![command.to_string()];
        let attempts = ssh_retry_attempts();
        let mut last_error = None;
        for attempt in 0..attempts {
            let output = match self.runner.run(ssh_dest, &args).await {
                Ok(output) => output,
                Err(source) if attempt + 1 < attempts && is_retryable_ssh_io_error(&source) => {
                    last_error = Some(provider_error(
                        code,
                        format!("failed to run remote command over SSH: {source}"),
                        true,
                    ));
                    sleep(ssh_retry_delay(attempt)).await;
                    continue;
                }
                Err(source) => {
                    return Err(provider_error(
                        code,
                        format!("failed to run remote command over SSH: {source}"),
                        false,
                    ));
                }
            };
            if output.success() {
                return Ok(output);
            }
            if attempt + 1 < attempts && is_retryable_ssh_output(&output) {
                last_error = Some(provider_error(
                    code,
                    format!(
                        "remote command failed with status {:?}: {}",
                        output.status_code,
                        output.stderr_text()
                    ),
                    true,
                ));
                sleep(ssh_retry_delay(attempt)).await;
                continue;
            }
            return Err(provider_error(
                code,
                format!(
                    "remote command failed with status {:?}: {}",
                    output.status_code,
                    output.stderr_text()
                ),
                false,
            ));
        }
        Err(last_error.unwrap_or_else(|| {
            provider_error(code, "remote command failed without an error", false)
        }))
    }
}

pub fn parse_new_response(json: &str) -> serde_json::Result<ExeVm> {
    serde_json::from_str(json)
}

pub fn parse_ls_response(json: &str) -> serde_json::Result<Vec<ExeVm>> {
    let value: serde_json::Value = serde_json::from_str(json)?;
    if value.is_array() {
        serde_json::from_value(value)
    } else {
        serde_json::from_value(value.get("vms").cloned().unwrap_or(value))
    }
}

pub fn find_vm<'a>(vms: &'a [ExeVm], name: &str) -> Option<&'a ExeVm> {
    vms.iter().find(|vm| vm.vm_name == name)
}

fn shell_quote(value: &str) -> String {
    shlex::try_quote(value)
        .expect("remote shell arguments must not contain NUL bytes")
        .into_owned()
}

fn output_diagnostics(output: &SshCommandOutput) -> String {
    let stderr = output.stderr_text();
    if !stderr.is_empty() {
        return stderr;
    }
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn ensure_success(
    output: SshCommandOutput,
    code: &'static str,
    operation: &str,
) -> ProviderResult<()> {
    if output.success() {
        Ok(())
    } else {
        Err(provider_error(
            code,
            format!(
                "{operation} failed with status {:?}: {}",
                output.status_code,
                output.stderr_text()
            ),
            false,
        ))
    }
}

fn remote_parent(path: &str) -> String {
    let path = path.trim_end_matches('/');
    if path.is_empty() || path == "/" {
        return "/".to_string();
    }
    match path.rsplit_once('/') {
        Some(("", _)) => "/".to_string(),
        Some((parent, _)) => parent.to_string(),
        None => "/".to_string(),
    }
}

fn is_safe_remote_pid(pid: &str) -> bool {
    !pid.is_empty() && pid.bytes().all(|byte| byte.is_ascii_digit())
}

fn sanitize_temp_prefix(prefix: &str) -> String {
    let mut out = String::new();
    for ch in prefix.chars().take(64) {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' || ch == '.' {
            out.push(ch);
        } else {
            out.push('-');
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        "smol-wf".to_string()
    } else {
        out
    }
}

fn is_retryable_ssh_io_error(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::BrokenPipe
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::TimedOut
            | io::ErrorKind::UnexpectedEof
    )
}

fn is_retryable_ssh_output(output: &SshCommandOutput) -> bool {
    !output.success() && is_retryable_ssh_message(&output_diagnostics(output))
}

fn is_retryable_ssh_provider_error(error: &smol_workflow_sandbox::ProviderError) -> bool {
    error.retryable || is_retryable_ssh_message(&error.message)
}

fn is_retryable_ssh_message(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    [
        "broken pipe",
        "connection reset",
        "connection timed out",
        "connection closed",
        "connection refused",
        "operation timed out",
        "unexpected eof",
    ]
    .iter()
    .any(|needle| message.contains(needle))
}

fn ssh_retry_attempts() -> usize {
    3
}

fn ssh_retry_delay(attempt: usize) -> Duration {
    const BASE_MS: u64 = 250;
    const MAX_MS: u64 = 2_000;
    let shift = u32::try_from(attempt).unwrap_or(u32::MAX).min(16);
    Duration::from_millis(BASE_MS.saturating_mul(1u64 << shift).min(MAX_MS))
}

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_retryable_ssh_failures() {
        let broken_pipe = io::Error::new(io::ErrorKind::BrokenPipe, "broken pipe");
        assert!(is_retryable_ssh_io_error(&broken_pipe));
        assert!(is_retryable_ssh_message(
            "client_loop: send disconnect: Broken pipe"
        ));
        assert!(is_retryable_ssh_output(&SshCommandOutput {
            status_code: Some(255),
            stdout: Vec::new(),
            stderr: b"Connection reset by peer".to_vec(),
        }));
        assert!(!is_retryable_ssh_output(&SshCommandOutput {
            status_code: Some(1),
            stdout: Vec::new(),
            stderr: b"permission denied".to_vec(),
        }));
    }

    #[test]
    fn parses_new_and_ls_json() {
        let vm = parse_new_response(
            r#"{"vm_name":"smol-test","ssh_dest":"smol-test.exe.xyz","status":"running"}"#,
        )
        .unwrap();
        assert_eq!(vm.vm_name, "smol-test");
        assert_eq!(vm.ssh_dest.as_deref(), Some("smol-test.exe.xyz"));

        let vms = parse_ls_response(
            r#"[{"vm_name":"smol-test","ssh_dest":"smol-test.exe.xyz","status":"running"}]"#,
        )
        .unwrap();
        assert_eq!(vms.len(), 1);
        assert_eq!(find_vm(&vms, "smol-test").unwrap().vm_name, "smol-test");

        let wrapped_vms = parse_ls_response(
            r#"{"vms":[{"vm_name":"smol-test","ssh_dest":"smol-test.exe.xyz","status":"running"}]}"#,
        )
        .unwrap();
        assert_eq!(wrapped_vms.len(), 1);
        assert_eq!(
            find_vm(&wrapped_vms, "smol-test").unwrap().vm_name,
            "smol-test"
        );
    }
}
