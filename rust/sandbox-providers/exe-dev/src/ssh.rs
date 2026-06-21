use crate::config::SshConfig;
use std::process::Stdio;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshCommandOutput {
    pub status_code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl SshCommandOutput {
    pub fn success(&self) -> bool {
        self.status_code == Some(0)
    }

    pub fn stderr_text(&self) -> String {
        String::from_utf8_lossy(&self.stderr).trim().to_string()
    }
}

#[derive(Debug, Clone)]
pub struct SshRunner {
    config: SshConfig,
}

impl SshRunner {
    pub fn new(config: SshConfig) -> Self {
        Self { config }
    }

    pub fn command(&self, destination: &str, args: &[String]) -> Command {
        let mut command = Command::new(&self.config.program);
        command
            .args(&self.config.extra_args)
            .arg(destination)
            .args(args);
        command
    }

    pub async fn run(
        &self,
        destination: &str,
        args: &[String],
    ) -> std::io::Result<SshCommandOutput> {
        let mut command = self.command(destination, args);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let output = command.output().await?;
        Ok(SshCommandOutput {
            status_code: output.status.code(),
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }

    pub async fn run_with_stdin(
        &self,
        destination: &str,
        args: &[String],
        stdin: &[u8],
    ) -> std::io::Result<SshCommandOutput> {
        let mut command = self.command(destination, args);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn()?;
        if let Some(mut child_stdin) = child.stdin.take() {
            child_stdin.write_all(stdin).await?;
            child_stdin.shutdown().await?;
        }
        let output = child.wait_with_output().await?;
        Ok(SshCommandOutput {
            status_code: output.status.code(),
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runner_keeps_config() {
        let runner = SshRunner::new(SshConfig {
            program: "/tmp/fake-ssh".to_string(),
            extra_args: vec!["-F".to_string(), "none".to_string()],
        });
        assert_eq!(runner.config.program, "/tmp/fake-ssh");
        assert_eq!(runner.config.extra_args, ["-F", "none"]);
    }

    #[test]
    fn output_reports_success_and_stderr() {
        let output = SshCommandOutput {
            status_code: Some(0),
            stdout: Vec::new(),
            stderr: b"warning\n".to_vec(),
        };
        assert!(output.success());
        assert_eq!(output.stderr_text(), "warning");
    }
}
