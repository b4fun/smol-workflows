use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine as _;
use smol_sandbox_exe_dev::exe_api::parse_ls_response;
use smol_sandbox_exe_dev::state::ProviderState;
use smol_workflow_sandbox::{
    CleanupSandboxGroupRequest, CreateTempDirRequest, JsonlClientError, Metadata,
    OpenSandboxRequest, ProfileRef, SandboxExecRequest, SandboxProviderJsonlClient,
    SandboxSpawnRequest, SessionPathRequest, WorkspaceSync, WriteFileRequest,
};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use tokio::process::Command;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn lock_env() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
}

#[tokio::test(flavor = "current_thread")]
async fn provider_process_handles_capabilities_cleanup_and_shutdown() {
    let _guard = lock_env();
    let temp = tempfile::tempdir().unwrap();
    std::env::remove_var("SMOL_SANDBOX_EXE_DEV_CONFIG");
    std::env::set_var(
        "SMOL_SANDBOX_EXE_DEV_STATE_DIR",
        temp.path().join("provider-state"),
    );
    let provider = env!("CARGO_BIN_EXE_smol-sandbox-exe-dev");
    let client = SandboxProviderJsonlClient::start(provider).await.unwrap();

    let capabilities = client.capabilities().await.unwrap();
    assert!(capabilities.exec);

    let cleaned = client
        .cleanup_group(CleanupSandboxGroupRequest {
            metadata: Metadata::new("req_cleanup", "sbxgrp_test"),
            sandbox_group_id: "sbxgrp_test".to_string(),
        })
        .await
        .unwrap();
    assert_eq!(cleaned, 0);

    client.shutdown().await.unwrap();
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn provider_lifecycle_uses_fake_ssh_and_deletes_on_close() {
    let _guard = lock_env();
    let temp = tempfile::tempdir().unwrap();
    let fake_ssh_bin = write_fake_ssh_on_path(temp.path());
    let _path_guard = prepend_path(&fake_ssh_bin);
    let config_path = temp.path().join("config.json");
    let state_dir = temp.path().join("provider-state");
    let fake_state = temp.path().join("fake-ssh-state.json");
    let log_path = temp.path().join("fake-ssh.log");

    fs::write(
        &config_path,
        serde_json::json!({
            "profiles": {
                "default": {
                    "image": "exeuntu",
                    "sync_workspace": false,
                    "ssh": { "extra_args": [] },
                    "cleanup": { "on_close": "delete" }
                }
            }
        })
        .to_string(),
    )
    .unwrap();

    std::env::set_var("SMOL_SANDBOX_EXE_DEV_CONFIG", &config_path);
    std::env::set_var("SMOL_SANDBOX_EXE_DEV_STATE_DIR", &state_dir);
    std::env::set_var("FAKE_EXE_DEV_SSH_STATE", &fake_state);
    std::env::set_var("FAKE_EXE_DEV_SSH_LOG", &log_path);
    std::env::set_var("FAKE_EXE_DEV_READY_FAILS", "1");
    std::env::set_var("SMOL_EXE_DEV_READY_INITIAL_DELAY_MS", "1");
    std::env::set_var("SMOL_EXE_DEV_READY_MAX_DELAY_MS", "2");
    std::env::set_var("SMOL_EXE_DEV_READY_TIMEOUT_MS", "2000");

    let provider = env!("CARGO_BIN_EXE_smol-sandbox-exe-dev");
    let client = SandboxProviderJsonlClient::start(provider).await.unwrap();
    let workspace = tempfile::tempdir().unwrap();

    let session = client
        .open(OpenSandboxRequest {
            metadata: Metadata::new("req_open", "sbxgrp_lifecycle"),
            profile: ProfileRef {
                provider: "exe-dev".to_string(),
                name: "default".to_string(),
            },
            workspace_sync: WorkspaceSync {
                host_path: workspace.path().to_path_buf(),
            },
            cwd: None,
        })
        .await
        .unwrap();

    let vm_name = session.provider_session_id.clone().unwrap();
    assert!(vm_name.starts_with("smol-workflows-exe-dev-sbxgrp-lifecycle-"));
    assert_eq!(session.cwd.as_deref(), Some("/home/exedev/workspace"));
    assert!(state_dir.read_dir().unwrap().next().is_some());

    client.close(session).await.unwrap();
    assert!(state_dir.read_dir().unwrap().next().is_none());
    client.shutdown().await.unwrap();

    let log = fs::read_to_string(log_path).unwrap();
    assert!(log.contains("exe.dev new --name "));
    assert!(log.contains(" --image exeuntu --json"));
    assert!(log.contains("exe.dev ls --json"));
    assert!(log.contains(".exe.xyz true"));
    assert!(log.contains(".exe.xyz mkdir -p -- /home/exedev/workspace"));
    assert!(log.contains(&format!("exe.dev rm {vm_name} --json")));
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn open_failure_deletes_created_vm_when_readiness_times_out() {
    let _guard = lock_env();
    let temp = tempfile::tempdir().unwrap();
    let fake_ssh_bin = write_fake_ssh_on_path(temp.path());
    let _path_guard = prepend_path(&fake_ssh_bin);
    let config_path = temp.path().join("config.json");
    let state_dir = temp.path().join("provider-state");
    let fake_state = temp.path().join("fake-ssh-state.json");
    let log_path = temp.path().join("fake-ssh.log");

    fs::write(
        &config_path,
        serde_json::json!({
            "profiles": {
                "default": {
                    "image": "exeuntu",
                    "sync_workspace": false,
                    "ssh": { "extra_args": [] },
                    "cleanup": { "on_error": "delete" }
                }
            }
        })
        .to_string(),
    )
    .unwrap();

    std::env::set_var("SMOL_SANDBOX_EXE_DEV_CONFIG", &config_path);
    std::env::set_var("SMOL_SANDBOX_EXE_DEV_STATE_DIR", &state_dir);
    std::env::set_var("FAKE_EXE_DEV_SSH_STATE", &fake_state);
    std::env::set_var("FAKE_EXE_DEV_SSH_LOG", &log_path);
    std::env::set_var("FAKE_EXE_DEV_READY_FAILS", "1000");
    std::env::set_var("SMOL_EXE_DEV_READY_INITIAL_DELAY_MS", "1");
    std::env::set_var("SMOL_EXE_DEV_READY_MAX_DELAY_MS", "1");
    std::env::set_var("SMOL_EXE_DEV_READY_TIMEOUT_MS", "5");

    let provider = env!("CARGO_BIN_EXE_smol-sandbox-exe-dev");
    let client = SandboxProviderJsonlClient::start(provider).await.unwrap();
    let workspace = tempfile::tempdir().unwrap();

    let error = client
        .open(OpenSandboxRequest {
            metadata: Metadata::new("req_open", "sbxgrp_ready_fail"),
            profile: ProfileRef {
                provider: "exe-dev".to_string(),
                name: "default".to_string(),
            },
            workspace_sync: WorkspaceSync {
                host_path: workspace.path().to_path_buf(),
            },
            cwd: None,
        })
        .await
        .unwrap_err();
    match error {
        JsonlClientError::Provider(error) => assert_eq!(error.code, "ssh_not_ready"),
        other => panic!("expected provider error, got {other}"),
    }

    client.shutdown().await.unwrap();

    let log = fs::read_to_string(log_path).unwrap();
    assert!(log.contains("exe.dev new --name "));
    assert!(log.contains("exe.dev ls --json"));
    assert!(log.contains(".exe.xyz true"));
    assert!(log.contains("exe.dev rm smol-workflows-exe-dev-sbxgrp-ready-fail-"));
    assert!(!state_dir.exists() || state_dir.read_dir().unwrap().next().is_none());
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn control_plane_new_failure_includes_stdout_json_error_body() {
    let _guard = lock_env();
    let temp = tempfile::tempdir().unwrap();
    let fake_ssh_bin = write_fake_ssh_on_path(temp.path());
    let _path_guard = prepend_path(&fake_ssh_bin);
    let config_path = temp.path().join("config.json");
    let state_dir = temp.path().join("provider-state");
    let fake_state = temp.path().join("fake-ssh-state.json");
    let log_path = temp.path().join("fake-ssh.log");

    fs::write(
        &config_path,
        serde_json::json!({
            "profiles": {
                "default": {
                    "image": "exeuntu",
                    "sync_workspace": false,
                    "ssh": { "extra_args": [] },
                    "cleanup": { "on_error": "delete" }
                }
            }
        })
        .to_string(),
    )
    .unwrap();

    std::env::set_var("SMOL_SANDBOX_EXE_DEV_CONFIG", &config_path);
    std::env::set_var("SMOL_SANDBOX_EXE_DEV_STATE_DIR", &state_dir);
    std::env::set_var("FAKE_EXE_DEV_SSH_STATE", &fake_state);
    std::env::set_var("FAKE_EXE_DEV_SSH_LOG", &log_path);
    std::env::set_var("FAKE_EXE_DEV_REJECT_NEW", "1");

    let provider = env!("CARGO_BIN_EXE_smol-sandbox-exe-dev");
    let client = SandboxProviderJsonlClient::start(provider).await.unwrap();
    let workspace = tempfile::tempdir().unwrap();

    let error = client
        .open(OpenSandboxRequest {
            metadata: Metadata::new("req_open", "sbxgrp_new_rejected"),
            profile: ProfileRef {
                provider: "exe-dev".to_string(),
                name: "default".to_string(),
            },
            workspace_sync: WorkspaceSync {
                host_path: workspace.path().to_path_buf(),
            },
            cwd: None,
        })
        .await
        .unwrap_err();
    match error {
        JsonlClientError::Provider(error) => {
            assert_eq!(error.code, "exe_new_failed");
            assert!(error.message.contains("create_failed"));
            assert!(error
                .message
                .contains("fake exe.dev rejected new VM request"));
        }
        other => panic!("expected provider error, got {other}"),
    }

    client.shutdown().await.unwrap();
    std::env::remove_var("FAKE_EXE_DEV_REJECT_NEW");

    let log = fs::read_to_string(log_path).unwrap();
    assert!(log.contains("exe.dev new --name "));
    assert!(!log.contains("exe.dev rm "));
    assert!(!state_dir.exists() || state_dir.read_dir().unwrap().next().is_none());
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn fake_ssh_fixture_enforces_control_plane_shapes_and_name_constraints() {
    let _guard = lock_env();
    let temp = tempfile::tempdir().unwrap();
    let fake_ssh_bin = write_fake_ssh_on_path(temp.path());
    let ssh = fake_ssh_bin.join("ssh");
    let fake_state = temp.path().join("fake-ssh-state.json");
    let log_path = temp.path().join("fake-ssh.log");

    std::env::set_var("FAKE_EXE_DEV_SSH_STATE", &fake_state);
    std::env::set_var("FAKE_EXE_DEV_SSH_LOG", &log_path);
    std::env::set_var("FAKE_EXE_DEV_READY_FAILS", "0");

    let valid_name = "a".repeat(52);
    let new_output = Command::new(&ssh)
        .args([
            "exe.dev",
            "new",
            "--name",
            &valid_name,
            "--image",
            "exeuntu",
            "--json",
        ])
        .output()
        .await
        .unwrap();
    assert!(
        new_output.status.success(),
        "new failed: stdout={} stderr={}",
        String::from_utf8_lossy(&new_output.stdout),
        String::from_utf8_lossy(&new_output.stderr)
    );
    let new_json: serde_json::Value = serde_json::from_slice(&new_output.stdout).unwrap();
    assert_eq!(new_json["vm_name"], valid_name);

    let ls_output = Command::new(&ssh)
        .args(["exe.dev", "ls", "--json"])
        .output()
        .await
        .unwrap();
    assert!(ls_output.status.success());
    let listed = parse_ls_response(&String::from_utf8_lossy(&ls_output.stdout)).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].vm_name, valid_name);
    assert_eq!(
        listed[0].ssh_dest.as_deref(),
        Some(format!("{valid_name}.exe.xyz").as_str())
    );

    let ready_output = Command::new(&ssh)
        .args([format!("{valid_name}.exe.xyz"), "true".to_string()])
        .output()
        .await
        .unwrap();
    assert!(ready_output.status.success());

    let invalid_name_output = Command::new(&ssh)
        .args([
            "exe.dev", "new", "--name", "Bad_Name", "--image", "exeuntu", "--json",
        ])
        .output()
        .await
        .unwrap();
    assert!(!invalid_name_output.status.success());
    let invalid_name_json: serde_json::Value =
        serde_json::from_slice(&invalid_name_output.stdout).unwrap();
    assert_eq!(invalid_name_json["error"]["code"], "invalid_name");
    assert!(invalid_name_json["error"]["message"]
        .as_str()
        .unwrap()
        .contains("lowercase letters"));

    let overlong_name = "b".repeat(53);
    let overlong_output = Command::new(&ssh)
        .args([
            "exe.dev",
            "new",
            "--name",
            &overlong_name,
            "--image",
            "exeuntu",
            "--json",
        ])
        .output()
        .await
        .unwrap();
    assert!(!overlong_output.status.success());
    let overlong_json: serde_json::Value = serde_json::from_slice(&overlong_output.stdout).unwrap();
    assert_eq!(overlong_json["error"]["code"], "invalid_name");
    assert!(overlong_json["error"]["message"]
        .as_str()
        .unwrap()
        .contains("between 1 and 52"));

    let rm_invalid_output = Command::new(&ssh)
        .args(["exe.dev", "rm", "Bad_Name", "--json"])
        .output()
        .await
        .unwrap();
    assert!(!rm_invalid_output.status.success());
    let rm_invalid_json: serde_json::Value =
        serde_json::from_slice(&rm_invalid_output.stdout).unwrap();
    assert_eq!(rm_invalid_json["error"]["code"], "invalid_name");

    let rm_output = Command::new(&ssh)
        .args(["exe.dev", "rm", &valid_name, "--json"])
        .output()
        .await
        .unwrap();
    assert!(rm_output.status.success());
    let after_rm_ls = Command::new(&ssh)
        .args(["exe.dev", "ls", "--json"])
        .output()
        .await
        .unwrap();
    assert!(after_rm_ls.status.success());
    assert!(
        parse_ls_response(&String::from_utf8_lossy(&after_rm_ls.stdout))
            .unwrap()
            .is_empty()
    );

    let log = fs::read_to_string(log_path).unwrap();
    assert!(log.contains(&format!(
        "exe.dev new --name {valid_name} --image exeuntu --json"
    )));
    assert!(log.contains("exe.dev ls --json"));
    assert!(log.contains(&format!("{valid_name}.exe.xyz true")));
    assert!(log.contains(&format!("exe.dev rm {valid_name} --json")));
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn cleanup_group_removes_persisted_fake_ssh_vm() {
    let _guard = lock_env();
    let temp = tempfile::tempdir().unwrap();
    let fake_ssh_bin = write_fake_ssh_on_path(temp.path());
    let _path_guard = prepend_path(&fake_ssh_bin);
    let config_path = temp.path().join("config.json");
    let state_dir = temp.path().join("provider-state");
    let fake_state = temp.path().join("fake-ssh-state.json");
    let log_path = temp.path().join("fake-ssh.log");

    fs::write(
        &config_path,
        serde_json::json!({
            "profiles": {
                "default": {
                    "image": "exeuntu",
                    "sync_workspace": false,
                    "ssh": { "extra_args": [] },
                    "cleanup": { "on_close": "keep" }
                }
            }
        })
        .to_string(),
    )
    .unwrap();

    std::env::set_var("SMOL_SANDBOX_EXE_DEV_CONFIG", &config_path);
    std::env::set_var("SMOL_SANDBOX_EXE_DEV_STATE_DIR", &state_dir);
    std::env::set_var("FAKE_EXE_DEV_SSH_STATE", &fake_state);
    std::env::set_var("FAKE_EXE_DEV_SSH_LOG", &log_path);
    std::env::set_var("FAKE_EXE_DEV_READY_FAILS", "0");
    std::env::set_var("SMOL_EXE_DEV_READY_INITIAL_DELAY_MS", "1");
    std::env::set_var("SMOL_EXE_DEV_READY_MAX_DELAY_MS", "2");
    std::env::set_var("SMOL_EXE_DEV_READY_TIMEOUT_MS", "2000");

    let provider = env!("CARGO_BIN_EXE_smol-sandbox-exe-dev");
    let client = SandboxProviderJsonlClient::start(provider).await.unwrap();
    let workspace = tempfile::tempdir().unwrap();

    let _session = client
        .open(OpenSandboxRequest {
            metadata: Metadata::new("req_open", "sbxgrp_cleanup"),
            profile: ProfileRef {
                provider: "exe-dev".to_string(),
                name: "default".to_string(),
            },
            workspace_sync: WorkspaceSync {
                host_path: workspace.path().to_path_buf(),
            },
            cwd: None,
        })
        .await
        .unwrap();

    let cleaned = client
        .cleanup_group(CleanupSandboxGroupRequest {
            metadata: Metadata::new("req_cleanup", "sbxgrp_cleanup"),
            sandbox_group_id: "sbxgrp_cleanup".to_string(),
        })
        .await
        .unwrap();
    assert_eq!(cleaned, 1);
    assert!(state_dir.read_dir().unwrap().next().is_none());
    client.shutdown().await.unwrap();

    let log = fs::read_to_string(log_path).unwrap();
    assert!(log.contains("exe.dev rm smol-workflows-exe-dev-sbxgrp-cleanup-"));
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn provider_syncs_workspace_with_tar_over_fake_ssh() {
    let _guard = lock_env();
    let temp = tempfile::tempdir().unwrap();
    let fake_ssh_bin = write_fake_ssh_on_path(temp.path());
    let _path_guard = prepend_path(&fake_ssh_bin);
    let config_path = temp.path().join("config.json");
    let state_dir = temp.path().join("provider-state");
    let fake_state = temp.path().join("fake-ssh-state.json");
    let log_path = temp.path().join("fake-ssh.log");
    let remote_root = temp.path().join("remote-root");
    let workspace = temp.path().join("workspace-src");

    fs::create_dir(&workspace).unwrap();
    fs::write(workspace.join("hello.txt"), b"hello from workspace\n").unwrap();
    fs::create_dir_all(workspace.join("dir")).unwrap();
    fs::write(workspace.join("dir/nested.bin"), [0u8, 1, 2, 255]).unwrap();
    fs::create_dir_all(workspace.join(".git")).unwrap();
    fs::write(workspace.join(".git/secret"), b"do not upload").unwrap();
    fs::create_dir_all(workspace.join("target")).unwrap();
    fs::write(workspace.join("target/build.log"), b"do not upload").unwrap();

    fs::write(
        &config_path,
        serde_json::json!({
            "profiles": {
                "default": {
                    "image": "exeuntu",
                    "sync_workspace": true,
                    "workspace_sync": { "mode": "tar", "exclude": [".git", "target"] },
                    "ssh": { "extra_args": [] },
                    "cleanup": { "on_close": "delete" }
                }
            }
        })
        .to_string(),
    )
    .unwrap();

    std::env::set_var("SMOL_SANDBOX_EXE_DEV_CONFIG", &config_path);
    std::env::set_var("SMOL_SANDBOX_EXE_DEV_STATE_DIR", &state_dir);
    std::env::set_var("FAKE_EXE_DEV_SSH_STATE", &fake_state);
    std::env::set_var("FAKE_EXE_DEV_SSH_LOG", &log_path);
    std::env::set_var("FAKE_EXE_DEV_REMOTE_ROOT", &remote_root);
    std::env::set_var("FAKE_EXE_DEV_READY_FAILS", "0");
    std::env::set_var("SMOL_EXE_DEV_READY_INITIAL_DELAY_MS", "1");
    std::env::set_var("SMOL_EXE_DEV_READY_MAX_DELAY_MS", "2");
    std::env::set_var("SMOL_EXE_DEV_READY_TIMEOUT_MS", "2000");

    let provider = env!("CARGO_BIN_EXE_smol-sandbox-exe-dev");
    let client = SandboxProviderJsonlClient::start(provider).await.unwrap();

    let session = client
        .open(OpenSandboxRequest {
            metadata: Metadata::new("req_open", "sbxgrp_sync"),
            profile: ProfileRef {
                provider: "exe-dev".to_string(),
                name: "default".to_string(),
            },
            workspace_sync: WorkspaceSync {
                host_path: workspace.clone(),
            },
            cwd: None,
        })
        .await
        .unwrap();

    assert_eq!(
        fs::read(remote_root.join("home/exedev/workspace/hello.txt")).unwrap(),
        b"hello from workspace\n"
    );
    assert_eq!(
        fs::read(remote_root.join("home/exedev/workspace/dir/nested.bin")).unwrap(),
        vec![0u8, 1, 2, 255]
    );
    assert!(!remote_root
        .join("home/exedev/workspace/.git/secret")
        .exists());
    assert!(!remote_root
        .join("home/exedev/workspace/target/build.log")
        .exists());

    client.close(session).await.unwrap();
    client.shutdown().await.unwrap();

    let log = fs::read_to_string(log_path).unwrap();
    assert!(log.contains(".exe.xyz tar -C /home/exedev/workspace -xf -"));
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn provider_file_apis_use_base64_and_session_cwd() {
    let _guard = lock_env();
    let temp = tempfile::tempdir().unwrap();
    let fake_ssh_bin = write_fake_ssh_on_path(temp.path());
    let _path_guard = prepend_path(&fake_ssh_bin);
    let config_path = temp.path().join("config.json");
    let state_dir = temp.path().join("provider-state");
    let fake_state = temp.path().join("fake-ssh-state.json");
    let log_path = temp.path().join("fake-ssh.log");
    let remote_root = temp.path().join("remote-root");

    fs::write(
        &config_path,
        serde_json::json!({
            "profiles": {
                "default": {
                    "image": "exeuntu",
                    "sync_workspace": false,
                    "ssh": { "extra_args": [] },
                    "cleanup": { "on_close": "delete" }
                }
            }
        })
        .to_string(),
    )
    .unwrap();

    std::env::set_var("SMOL_SANDBOX_EXE_DEV_CONFIG", &config_path);
    std::env::set_var("SMOL_SANDBOX_EXE_DEV_STATE_DIR", &state_dir);
    std::env::set_var("FAKE_EXE_DEV_SSH_STATE", &fake_state);
    std::env::set_var("FAKE_EXE_DEV_SSH_LOG", &log_path);
    std::env::set_var("FAKE_EXE_DEV_REMOTE_ROOT", &remote_root);
    std::env::set_var("FAKE_EXE_DEV_READY_FAILS", "0");
    std::env::set_var("SMOL_EXE_DEV_READY_INITIAL_DELAY_MS", "1");
    std::env::set_var("SMOL_EXE_DEV_READY_MAX_DELAY_MS", "2");
    std::env::set_var("SMOL_EXE_DEV_READY_TIMEOUT_MS", "2000");

    let provider = env!("CARGO_BIN_EXE_smol-sandbox-exe-dev");
    let client = SandboxProviderJsonlClient::start(provider).await.unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let session = client
        .open(OpenSandboxRequest {
            metadata: Metadata::new("req_open", "sbxgrp_files"),
            profile: ProfileRef {
                provider: "exe-dev".to_string(),
                name: "default".to_string(),
            },
            workspace_sync: WorkspaceSync {
                host_path: workspace.path().to_path_buf(),
            },
            cwd: None,
        })
        .await
        .unwrap();

    client
        .create_dir_all(SessionPathRequest {
            session: session.clone(),
            path: "nested/dir".to_string(),
        })
        .await
        .unwrap();
    assert!(remote_root
        .join("home/exedev/workspace/nested/dir")
        .is_dir());

    client
        .write_file(WriteFileRequest {
            session: session.clone(),
            path: "nested/dir/data.bin".to_string(),
            content_base64: "AAECA/8=".to_string(),
        })
        .await
        .unwrap();
    assert_eq!(
        fs::read(remote_root.join("home/exedev/workspace/nested/dir/data.bin")).unwrap(),
        vec![0u8, 1, 2, 3, 255]
    );

    let read = client
        .read_file(SessionPathRequest {
            session: session.clone(),
            path: "/home/exedev/workspace/nested/dir/data.bin".to_string(),
        })
        .await
        .unwrap();
    assert_eq!(read.content_base64, "AAECA/8=");

    let temp_dir = client
        .create_temp_dir(CreateTempDirRequest {
            session: session.clone(),
            prefix: "smol test".to_string(),
        })
        .await
        .unwrap();
    assert!(temp_dir
        .path
        .starts_with("/home/exedev/workspace/smol-test."));
    assert!(remote_root
        .join(temp_dir.path.trim_start_matches('/'))
        .is_dir());

    let remove_request = SessionPathRequest {
        session: session.clone(),
        path: "nested/dir/data.bin".to_string(),
    };
    client.remove(remove_request.clone()).await.unwrap();
    client.remove(remove_request).await.unwrap();
    assert!(!remote_root
        .join("home/exedev/workspace/nested/dir/data.bin")
        .exists());

    client.close(session).await.unwrap();
    client.shutdown().await.unwrap();

    let log = fs::read_to_string(log_path).unwrap();
    assert!(log.contains(".exe.xyz mkdir -p -- /home/exedev/workspace/nested/dir"));
    assert!(log.contains("cat -- /home/exedev/workspace/nested/dir/data.bin"));
    assert!(log.contains("rm -rf -- /home/exedev/workspace/nested/dir/data.bin"));
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn provider_exec_streams_events_and_handles_stdin_env_and_cwd() {
    let _guard = lock_env();
    let temp = tempfile::tempdir().unwrap();
    let fake_ssh_bin = write_fake_ssh_on_path(temp.path());
    let _path_guard = prepend_path(&fake_ssh_bin);
    let config_path = temp.path().join("config.json");
    let state_dir = temp.path().join("provider-state");
    let fake_state = temp.path().join("fake-ssh-state.json");
    let log_path = temp.path().join("fake-ssh.log");
    let remote_root = temp.path().join("remote-root");

    fs::write(
        &config_path,
        serde_json::json!({
            "profiles": {
                "default": {
                    "image": "exeuntu",
                    "sync_workspace": false,
                    "ssh": { "extra_args": [] },
                    "cleanup": { "on_close": "delete" }
                }
            }
        })
        .to_string(),
    )
    .unwrap();

    std::env::set_var("SMOL_SANDBOX_EXE_DEV_CONFIG", &config_path);
    std::env::set_var("SMOL_SANDBOX_EXE_DEV_STATE_DIR", &state_dir);
    std::env::set_var("FAKE_EXE_DEV_SSH_STATE", &fake_state);
    std::env::set_var("FAKE_EXE_DEV_SSH_LOG", &log_path);
    std::env::set_var("FAKE_EXE_DEV_REMOTE_ROOT", &remote_root);
    std::env::set_var("FAKE_EXE_DEV_READY_FAILS", "0");
    std::env::set_var("SMOL_EXE_DEV_READY_INITIAL_DELAY_MS", "1");
    std::env::set_var("SMOL_EXE_DEV_READY_MAX_DELAY_MS", "2");
    std::env::set_var("SMOL_EXE_DEV_READY_TIMEOUT_MS", "2000");

    let provider = env!("CARGO_BIN_EXE_smol-sandbox-exe-dev");
    let client = SandboxProviderJsonlClient::start(provider).await.unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let session = client
        .open(OpenSandboxRequest {
            metadata: Metadata::new("req_open", "sbxgrp_exec"),
            profile: ProfileRef {
                provider: "exe-dev".to_string(),
                name: "default".to_string(),
            },
            workspace_sync: WorkspaceSync {
                host_path: workspace.path().to_path_buf(),
            },
            cwd: None,
        })
        .await
        .unwrap();

    client
        .create_dir_all(SessionPathRequest {
            session: session.clone(),
            path: "nested".to_string(),
        })
        .await
        .unwrap();

    let mut env = BTreeMap::new();
    env.insert(
        "FOO".to_string(),
        "value with spaces and 'quote'".to_string(),
    );
    let mut events = Vec::new();
    let result = client
        .exec(
            SandboxExecRequest {
                session: session.clone(),
                argv: vec![
                    "sh".to_string(),
                    "-c".to_string(),
                    "printf 'out:%s\\n' \"$FOO\"; cat; printf 'err:%s\\n' \"$FOO\" >&2".to_string(),
                ],
                cwd: Some("nested".to_string()),
                env,
                stdin_base64: Some(BASE64_STANDARD.encode(b"stdin-data\n")),
            },
            |event| {
                events.push(event);
                Ok(())
            },
        )
        .await
        .unwrap();

    let expected_stdout = b"out:value with spaces and 'quote'\nstdin-data\n";
    let expected_stderr = b"err:value with spaces and 'quote'\n";
    assert_eq!(result.exit_code, 0);
    assert_eq!(
        BASE64_STANDARD.decode(result.stdout_base64).unwrap(),
        expected_stdout
    );
    assert_eq!(
        BASE64_STANDARD.decode(result.stderr_base64).unwrap(),
        expected_stderr
    );
    assert_eq!(events.first().unwrap().r#type, "started");
    assert_eq!(events.last().unwrap().r#type, "exited");
    assert!(events.iter().any(|event| event.r#type == "stdout"));
    assert!(events.iter().any(|event| event.r#type == "stderr"));

    let empty_argv_error = client
        .exec(
            SandboxExecRequest {
                session: session.clone(),
                argv: Vec::new(),
                cwd: None,
                env: BTreeMap::new(),
                stdin_base64: None,
            },
            |_| Ok(()),
        )
        .await
        .unwrap_err();
    match empty_argv_error {
        JsonlClientError::Provider(error) => assert_eq!(error.code, "invalid_request"),
        other => panic!("expected provider error, got {other}"),
    }

    client.close(session).await.unwrap();
    client.shutdown().await.unwrap();

    let log = fs::read_to_string(log_path).unwrap();
    assert!(log.contains(".exe.xyz cd /home/exedev/workspace/nested && exec env"));
    assert!(log.contains("FOO=value with spaces"));
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn provider_spawn_returns_pid_and_close_kills_tracked_pid() {
    let _guard = lock_env();
    let temp = tempfile::tempdir().unwrap();
    let fake_ssh_bin = write_fake_ssh_on_path(temp.path());
    let _path_guard = prepend_path(&fake_ssh_bin);
    let config_path = temp.path().join("config.json");
    let state_dir = temp.path().join("provider-state");
    let fake_state = temp.path().join("fake-ssh-state.json");
    let log_path = temp.path().join("fake-ssh.log");
    let remote_root = temp.path().join("remote-root");

    fs::write(
        &config_path,
        serde_json::json!({
            "profiles": {
                "default": {
                    "image": "exeuntu",
                    "sync_workspace": false,
                    "ssh": { "extra_args": [] },
                    "cleanup": { "on_close": "delete" }
                }
            }
        })
        .to_string(),
    )
    .unwrap();

    std::env::set_var("SMOL_SANDBOX_EXE_DEV_CONFIG", &config_path);
    std::env::set_var("SMOL_SANDBOX_EXE_DEV_STATE_DIR", &state_dir);
    std::env::set_var("FAKE_EXE_DEV_SSH_STATE", &fake_state);
    std::env::set_var("FAKE_EXE_DEV_SSH_LOG", &log_path);
    std::env::set_var("FAKE_EXE_DEV_REMOTE_ROOT", &remote_root);
    std::env::set_var("FAKE_EXE_DEV_READY_FAILS", "0");
    std::env::set_var("SMOL_EXE_DEV_READY_INITIAL_DELAY_MS", "1");
    std::env::set_var("SMOL_EXE_DEV_READY_MAX_DELAY_MS", "2");
    std::env::set_var("SMOL_EXE_DEV_READY_TIMEOUT_MS", "2000");

    let provider = env!("CARGO_BIN_EXE_smol-sandbox-exe-dev");
    let client = SandboxProviderJsonlClient::start(provider).await.unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let session = client
        .open(OpenSandboxRequest {
            metadata: Metadata::new("req_open", "sbxgrp_spawn"),
            profile: ProfileRef {
                provider: "exe-dev".to_string(),
                name: "default".to_string(),
            },
            workspace_sync: WorkspaceSync {
                host_path: workspace.path().to_path_buf(),
            },
            cwd: None,
        })
        .await
        .unwrap();

    client
        .create_dir_all(SessionPathRequest {
            session: session.clone(),
            path: "spawn-cwd".to_string(),
        })
        .await
        .unwrap();

    let mut env = BTreeMap::new();
    env.insert("SPAWN_FLAG".to_string(), "value with spaces".to_string());
    let result = client
        .spawn(SandboxSpawnRequest {
            session: session.clone(),
            argv: vec!["sh".to_string(), "-c".to_string(), "sleep 100".to_string()],
            cwd: Some("spawn-cwd".to_string()),
            env,
            stdin_base64: Some(BASE64_STANDARD.encode(b"spawn stdin\n")),
        })
        .await
        .unwrap();
    let pid = result.process_id.unwrap();
    assert!(!pid.is_empty());
    let uploaded_stdin = fs::read_dir(remote_root.join("home/exedev/workspace"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .map(|name| name.starts_with(".smol-spawn-stdin-"))
                .unwrap_or(false)
        })
        .expect("spawn stdin should be uploaded to a remote temp file");
    assert_eq!(fs::read(uploaded_stdin).unwrap(), b"spawn stdin\n");
    let persisted_state_path = state_dir
        .read_dir()
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let persisted_state: ProviderState =
        serde_json::from_str(&fs::read_to_string(persisted_state_path).unwrap()).unwrap();
    assert_eq!(persisted_state.spawned_pids, vec![pid.clone()]);

    client.close(session).await.unwrap();
    client.shutdown().await.unwrap();

    let log = fs::read_to_string(log_path).unwrap();
    assert!(log.contains("cd /home/exedev/workspace/spawn-cwd && { nohup "));
    assert!(log.contains("SPAWN_FLAG=value with spaces"));
    assert!(log.contains("sleep 100"));
    assert!(log.contains(".smol-spawn-stdin-"));
    assert!(log.contains(&format!("kill -TERM -- {pid}")));
    assert!(log.contains(&format!("kill -KILL -- {pid}")));
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn real_exe_dev_smoke_when_enabled() {
    if std::env::var("EXE_DEV_E2E").as_deref() != Ok("1") {
        eprintln!("EXE_DEV_E2E is not set to 1; skipping real exe.dev smoke validation");
        return;
    }

    let _guard = lock_env();
    let temp = tempfile::tempdir().unwrap();
    let config_path = temp.path().join("config.json");
    let state_dir = temp.path().join("provider-state");
    let workspace = temp.path().join("workspace");
    fs::create_dir(&workspace).unwrap();

    let ls_before = run_real_ssh("exe.dev", &["ls", "--json"]).await;
    assert!(
        ls_before.status.success(),
        "ssh exe.dev ls --json failed before smoke: {}",
        String::from_utf8_lossy(&ls_before.stderr)
    );
    let help = run_real_ssh("exe.dev", &["new", "--help"]).await;
    eprintln!(
        "exe.dev new --help exited with {:?}; stdout: {}; stderr: {}",
        help.status.code(),
        String::from_utf8_lossy(&help.stdout).trim(),
        String::from_utf8_lossy(&help.stderr).trim()
    );

    fs::write(
        &config_path,
        serde_json::json!({
            "profiles": {
                "default": {
                    "image": "exeuntu",
                    "cwd": "/home/exedev/workspace",
                    "sync_workspace": false,
                    "cleanup": { "on_close": "delete", "keep_env": "SMOL_EXE_DEV_KEEP" }
                }
            }
        })
        .to_string(),
    )
    .unwrap();

    std::env::set_var("SMOL_SANDBOX_EXE_DEV_CONFIG", &config_path);
    std::env::set_var("SMOL_SANDBOX_EXE_DEV_STATE_DIR", &state_dir);
    std::env::set_var("SMOL_EXE_DEV_READY_TIMEOUT_MS", "120000");

    let provider = env!("CARGO_BIN_EXE_smol-sandbox-exe-dev");
    let client = SandboxProviderJsonlClient::start(provider).await.unwrap();
    let mut session = None;
    let mut state = None;
    let result = async {
        let opened = client
            .open(OpenSandboxRequest {
                metadata: Metadata::new("req_real_open", "sbxgrp_real_e2e"),
                profile: ProfileRef {
                    provider: "exe-dev".to_string(),
                    name: "default".to_string(),
                },
                workspace_sync: WorkspaceSync {
                    host_path: workspace.clone(),
                },
                cwd: None,
            })
            .await
            .map_err(|source| format!("provider open failed: {source}"))?;
        let parsed_state = ProviderState::from_provider_state_json(
            opened
                .provider_state_json
                .as_deref()
                .ok_or_else(|| "open response did not include provider state".to_string())?,
        )
        .map_err(|source| format!("provider state JSON did not decode: {source}"))?;
        eprintln!(
            "real exe.dev smoke opened VM `{}` at `{}` with session `{}` cwd `{}`",
            parsed_state.vm_name, parsed_state.ssh_dest, parsed_state.session_id, parsed_state.cwd
        );
        session = Some(opened.clone());
        state = Some(parsed_state.clone());

        if !vm_listed(&parsed_state.vm_name).await? {
            return Err(format!(
                "VM `{}` did not appear in ssh exe.dev ls --json after open",
                parsed_state.vm_name
            ));
        }
        let pwd = run_real_ssh_remote_command(&parsed_state.ssh_dest, &["pwd"]).await;
        if !pwd.status.success() {
            return Err(format!(
                "direct SSH pwd failed for `{}`: {}",
                parsed_state.ssh_dest,
                String::from_utf8_lossy(&pwd.stderr)
            ));
        }
        let hostname = run_real_ssh_remote_command(&parsed_state.ssh_dest, &["hostname"]).await;
        if !hostname.status.success() {
            return Err(format!(
                "direct SSH hostname failed for `{}`: {}",
                parsed_state.ssh_dest,
                String::from_utf8_lossy(&hostname.stderr)
            ));
        }
        let quoted_cwd = shell_quote_for_real_smoke(&parsed_state.cwd);
        let cwd_probe = format!(
            "mkdir -p -- {quoted_cwd} && cd -- {quoted_cwd} && pwd && touch .smol-exe-dev-smoke"
        );
        let cwd_command = format!("sh -lc {}", shell_quote_for_real_smoke(&cwd_probe));
        let cwd_output = run_real_ssh_remote_command(&parsed_state.ssh_dest, &[&cwd_command]).await;
        if !cwd_output.status.success() {
            return Err(format!(
                "direct SSH cwd usability probe failed for `{}` cwd `{}`: {}",
                parsed_state.ssh_dest,
                parsed_state.cwd,
                String::from_utf8_lossy(&cwd_output.stderr)
            ));
        }
        Ok::<(), String>(())
    }
    .await;

    if let Some(opened) = session.take() {
        if let Err(source) = client.close(opened).await {
            if std::env::var("SMOL_EXE_DEV_KEEP").as_deref() != Ok("1") {
                if let Some(state) = state.as_ref() {
                    let _ = run_real_ssh("exe.dev", &["rm", &state.vm_name, "--json"]).await;
                }
            }
            panic!("failed to close real exe.dev smoke session: {source}");
        }
    }
    client.shutdown().await.unwrap();

    result.unwrap();
    let state = state.expect("real smoke should have opened a VM");
    if std::env::var("SMOL_EXE_DEV_KEEP").as_deref() == Ok("1") {
        assert_vm_listed(&state.vm_name, true).await;
        eprintln!(
            "SMOL_EXE_DEV_KEEP=1 preserved real exe.dev VM `{}` at `{}`; delete with: ssh exe.dev rm {} --json",
            state.vm_name, state.ssh_dest, state.vm_name
        );
    } else {
        assert_vm_listed(&state.vm_name, false).await;
        eprintln!(
            "real exe.dev smoke deleted VM `{}` at `{}`",
            state.vm_name, state.ssh_dest
        );
    }
}

#[cfg(unix)]
async fn assert_vm_listed(vm_name: &str, expected: bool) {
    let listed = vm_listed(vm_name)
        .await
        .unwrap_or_else(|source| panic!("failed to check real exe.dev VM `{vm_name}`: {source}"));
    assert_eq!(
        listed, expected,
        "expected listed={expected} for real exe.dev VM `{vm_name}`"
    );
}

#[cfg(unix)]
async fn vm_listed(vm_name: &str) -> Result<bool, String> {
    let output = run_real_ssh("exe.dev", &["ls", "--json"]).await;
    if !output.status.success() {
        return Err(format!(
            "ssh exe.dev ls --json failed while checking `{vm_name}`: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let vms = parse_ls_response(&stdout).map_err(|source| {
        format!(
            "failed to parse ssh exe.dev ls --json while checking `{vm_name}`: {source}; stdout: {stdout}"
        )
    })?;
    Ok(vms.iter().any(|vm| vm.vm_name == vm_name))
}

#[cfg(unix)]
async fn run_real_ssh(destination: &str, args: &[&str]) -> std::process::Output {
    Command::new("ssh")
        .arg("--")
        .arg(destination)
        .args(args)
        .output()
        .await
        .unwrap_or_else(|source| panic!("failed to run ssh for `{destination}`: {source}"))
}

#[cfg(unix)]
async fn run_real_ssh_remote_command(destination: &str, args: &[&str]) -> std::process::Output {
    Command::new("ssh")
        .arg("--")
        .arg(destination)
        .args(args)
        .output()
        .await
        .unwrap_or_else(|source| {
            panic!("failed to run ssh remote command for `{destination}`: {source}")
        })
}

#[cfg(unix)]
fn shell_quote_for_real_smoke(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    let mut quoted = String::from("'");
    for ch in value.chars() {
        if ch == '\'' {
            quoted.push_str("'\\''");
        } else {
            quoted.push(ch);
        }
    }
    quoted.push('\'');
    quoted
}

#[cfg(unix)]
struct PathGuard {
    previous: Option<std::ffi::OsString>,
}

#[cfg(unix)]
impl Drop for PathGuard {
    fn drop(&mut self) {
        match self.previous.as_ref() {
            Some(previous) => std::env::set_var("PATH", previous),
            None => std::env::remove_var("PATH"),
        }
    }
}

#[cfg(unix)]
fn prepend_path(dir: &Path) -> PathGuard {
    let previous = std::env::var_os("PATH");
    let mut paths = vec![PathBuf::from(dir)];
    if let Some(previous) = previous.as_ref() {
        paths.extend(std::env::split_paths(previous));
    }
    let joined = std::env::join_paths(paths).unwrap();
    std::env::set_var("PATH", joined);
    PathGuard { previous }
}

#[cfg(unix)]
fn write_fake_ssh_on_path(dir: &Path) -> PathBuf {
    let bin_dir = dir.join("bin");
    fs::create_dir(&bin_dir).unwrap();
    let path = bin_dir.join("ssh");
    fs::write(
        &path,
        r#"#!/usr/bin/env python3
import json
import os
import re
import shlex
import shutil
import subprocess
import sys
import tarfile

args = sys.argv[1:]
log_path = os.environ["FAKE_EXE_DEV_SSH_LOG"]
state_path = os.environ["FAKE_EXE_DEV_SSH_STATE"]
remote_root = os.environ.get("FAKE_EXE_DEV_REMOTE_ROOT")

with open(log_path, "a", encoding="utf-8") as log:
    log.write(" ".join(args) + "\n")

if os.path.exists(state_path):
    with open(state_path, "r", encoding="utf-8") as handle:
        state = json.load(handle)
else:
    state = {"vm_name": None, "ready_attempts": 0, "removed": [], "mktemp_counter": 0, "next_pid": 4241, "spawned_pids": [], "killed_pids": []}

def save():
    with open(state_path, "w", encoding="utf-8") as handle:
        json.dump(state, handle)

def fail(message):
    print("unexpected fake ssh invocation: " + " ".join(args) + " (" + message + ")", file=sys.stderr)
    sys.exit(9)

def control_error(code, message):
    print(json.dumps({"error": {"code": code, "message": message}}))
    sys.exit(1)

def validate_vm_name(name):
    # Mirror exe.dev's current lowercase DNS-label-like VM naming constraints
    # where practical: short, lower alphanumeric plus hyphen, no leading/trailing hyphen.
    if len(name) < 1 or len(name) > 52:
        control_error("invalid_name", "invalid VM name: must be between 1 and 52 characters")
    if not re.fullmatch(r"[a-z0-9](?:[a-z0-9-]*[a-z0-9])?", name):
        control_error("invalid_name", "invalid VM name: use lowercase letters, digits, and interior hyphens only")

def parse_new(rest):
    if len(rest) not in (6, 8):
        fail("new must be: new --name <name> --image <image> [--region <region>] --json")
    if rest[0] != "new" or rest[1] != "--name" or rest[3] != "--image" or rest[-1] != "--json":
        fail("new argument order did not match current exe.dev CLI shape")
    if len(rest) == 8 and rest[5] != "--region":
        fail("new region flag was not in the expected position")
    name = rest[2]
    validate_vm_name(name)
    if os.environ.get("FAKE_EXE_DEV_REJECT_NEW") == "1":
        control_error("create_failed", "fake exe.dev rejected new VM request")
    return name

def local_path(remote_path):
    if not remote_root:
        fail("remote filesystem operation requires FAKE_EXE_DEV_REMOTE_ROOT")
    normalized = os.path.normpath("/" + remote_path.lstrip("/"))
    return os.path.join(remote_root, normalized.lstrip("/"))

def mkdir_remote(remote_path):
    if remote_root:
        os.makedirs(local_path(remote_path), exist_ok=True)

def parse_exec_parts(parts):
    if len(parts) < 5 or parts[0] != "cd" or parts[2] != "&&" or parts[3] != "exec":
        return None
    cwd = parts[1]
    index = 4
    env = os.environ.copy()
    if index < len(parts) and parts[index] == "env":
        probe = index + 1
        while probe < len(parts) and parts[probe] != "--" and "=" in parts[probe]:
            probe += 1
        if probe < len(parts) and parts[probe] == "--":
            index += 1
            while index < len(parts) and parts[index] != "--":
                key, value = parts[index].split("=", 1)
                env[key] = value
                index += 1
            index += 1
    argv = parts[index:]
    if not argv:
        fail("exec command did not include argv")
    return cwd, env, argv

def run_exec_shell(parts):
    parsed = parse_exec_parts(parts)
    if parsed is None:
        return False
    cwd, env, argv = parsed
    stdin = sys.stdin.buffer.read()
    process = subprocess.Popen(
        argv,
        cwd=local_path(cwd),
        env=env,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    stdout, stderr = process.communicate(stdin)
    sys.stdout.buffer.write(stdout)
    sys.stdout.buffer.flush()
    sys.stderr.buffer.write(stderr)
    sys.stderr.buffer.flush()
    sys.exit(process.returncode)

def handle_spawn_shell(command, parts):
    if len(parts) < 7 or parts[0] != "cd" or parts[2] != "&&" or parts[3] != "{" or parts[4] != "nohup" or "& echo $!" not in command:
        return False
    state["next_pid"] = int(state.get("next_pid", 4241)) + 1
    pid = str(state["next_pid"])
    state.setdefault("spawned_pids", []).append(pid)
    save()
    print(pid)
    return True

def handle_kill_shell(parts):
    if not parts or parts[0] != "kill":
        return False
    pids = [part for part in parts if part.isdigit()]
    state.setdefault("killed_pids", []).extend(pids)
    save()
    return True

def handle_shell(command):
    parts = shlex.split(command)
    if handle_kill_shell(parts):
        return
    if handle_spawn_shell(command, parts):
        return
    if run_exec_shell(parts):
        return
    if parts[:3] == ["tar", "-C", "/home/exedev/workspace"] and parts[3:] == ["-xf", "-"]:
        extract_workspace_tar("/home/exedev/workspace")
        return
    if parts and parts[0] == "tar" and "-C" in parts and parts[-2:] == ["-xf", "-"]:
        extract_workspace_tar(parts[parts.index("-C") + 1])
        return
    if command.startswith("mkdir -p -- ") and " && cat > " in command:
        before, after = command.split(" && cat > ", 1)
        parent = shlex.split(before)[-1]
        path = shlex.split(after)[0]
        mkdir_remote(parent)
        with open(local_path(path), "wb") as handle:
            handle.write(sys.stdin.buffer.read())
        return
    if parts[:3] == ["mkdir", "-p", "--"] and len(parts) == 4:
        mkdir_remote(parts[3])
        return
    if parts[:2] == ["cat", "--"] and len(parts) == 3:
        with open(local_path(parts[2]), "rb") as handle:
            shutil.copyfileobj(handle, sys.stdout.buffer)
        return
    if parts[:3] == ["rm", "-rf", "--"] and len(parts) == 4:
        path = local_path(parts[3])
        if os.path.isdir(path) and not os.path.islink(path):
            shutil.rmtree(path, ignore_errors=True)
        else:
            try:
                os.unlink(path)
            except FileNotFoundError:
                pass
        return
    if parts[:2] == ["mktemp", "-d"] and len(parts) == 3:
        template = parts[2]
        state["mktemp_counter"] = int(state.get("mktemp_counter", 0)) + 1
        suffix = f"fake{state['mktemp_counter']:06d}"
        remote_path = template.replace("XXXXXX", suffix)
        mkdir_remote(remote_path)
        save()
        print(remote_path)
        return
    fail("unknown remote shell command")

def extract_workspace_tar(remote_cwd):
    dest_dir = local_path(remote_cwd)
    os.makedirs(dest_dir, exist_ok=True)
    with tarfile.open(fileobj=sys.stdin.buffer, mode="r|*") as archive:
        for member in archive:
            name = member.name
            while name.startswith("./"):
                name = name[2:]
            if not name or name == ".":
                continue
            target = os.path.normpath(os.path.join(dest_dir, name))
            if os.path.commonpath([dest_dir, target]) != dest_dir:
                fail("tar member escaped destination")
            if member.isdir():
                os.makedirs(target, exist_ok=True)
            elif member.isfile():
                os.makedirs(os.path.dirname(target), exist_ok=True)
                source = archive.extractfile(member)
                if source is None:
                    fail("regular tar member had no file object")
                with open(target, "wb") as handle:
                    shutil.copyfileobj(source, handle)

if not args:
    print("missing destination", file=sys.stderr)
    sys.exit(2)

dest = args[0]
rest = args[1:]

if dest == "exe.dev":
    if rest and rest[0] == "new":
        name = parse_new(rest)
        state["vm_name"] = name
        state["ready_attempts"] = 0
        save()
        # Deliberately omit ssh_dest so the provider must resolve it through ls --json.
        print(json.dumps({"vm_name": name, "status": "running"}))
        sys.exit(0)
    if rest == ["ls", "--json"]:
        name = state.get("vm_name")
        vms = [] if not name else [{"vm_name": name, "ssh_dest": name + ".exe.xyz", "status": "running"}]
        print(json.dumps({"vms": vms}))
        sys.exit(0)
    if len(rest) == 3 and rest[0] == "rm" and rest[2] == "--json":
        validate_vm_name(rest[1])
        if rest[1] != state.get("vm_name"):
            control_error("not_found", "VM not found: " + rest[1])
        state.setdefault("removed", []).append(rest[1])
        state["vm_name"] = None
        save()
        print(json.dumps({"ok": True}))
        sys.exit(0)
    fail("unknown exe.dev control command")

if dest.endswith(".exe.xyz"):
    if rest == ["true"]:
        state["ready_attempts"] = int(state.get("ready_attempts", 0)) + 1
        save()
        fail_count = int(os.environ.get("FAKE_EXE_DEV_READY_FAILS", "0"))
        if state["ready_attempts"] <= fail_count:
            print("not ready yet", file=sys.stderr)
            sys.exit(255)
        sys.exit(0)
    if len(rest) == 3 and rest[:2] == ["mkdir", "-p"]:
        mkdir_remote(rest[2])
        sys.exit(0)
    if len(rest) == 1:
        handle_shell(rest[0])
        sys.exit(0)
    fail("unknown direct VM command")

fail("unknown destination")
"#,
    )
    .unwrap();
    let mut perms = fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).unwrap();
    bin_dir
}
