use serde_json::json;
use std::process::Command;

fn smol_wf() -> Command {
    Command::new(env!("CARGO_BIN_EXE_smol-wf"))
}

#[test]
fn run_passes_prefixed_cli_args_into_workflow_args() {
    let output = smol_wf()
        .args([
            "run",
            "../../ts/engine/test/fixtures/cli-args.workflow.js",
            "--args-my-arg1",
            "world",
            "--args-flag",
            "--args-repeat=one",
            "--args-repeat",
            "two",
        ])
        .output()
        .expect("smol-wf should run");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout should be JSON");
    assert_eq!(
        stdout,
        json!({
            "args": {
                "my-arg1": "world",
                "flag": true,
                "repeat": ["one", "two"]
            },
            "result": "echo: hello world"
        })
    );
}

#[test]
fn run_loads_workflow_args_from_json_file() {
    let output = smol_wf()
        .args([
            "run",
            "../../ts/engine/test/fixtures/cli-args.workflow.js",
            "--args-from-file",
            "../../ts/engine/test/fixtures/args.json",
            "--args-my-arg1",
            "file-arg-1",
        ])
        .output()
        .expect("smol-wf should run");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout should be JSON");
    assert_eq!(stdout["args"]["fromFile"], "file-value");
    assert_eq!(stdout["args"]["nested"]["value"], "nested-file-value");
    assert_eq!(stdout["args"]["my-arg1"], "file-arg-1");
    assert_eq!(stdout["result"], "echo: hello file-arg-1");
}

#[test]
fn run_rejects_unprefixed_run_args() {
    let output = smol_wf()
        .args([
            "run",
            "../../ts/engine/test/fixtures/cli-args.workflow.js",
            "--my-arg1",
            "world",
        ])
        .output()
        .expect("smol-wf should run");

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Unknown option: --my-arg1"));
}

#[test]
fn run_supports_agent_provider_debug() {
    let output = smol_wf()
        .args([
            "run",
            "../../ts/engine/test/fixtures/cli-args.workflow.js",
            "--agent-provider",
            "debug",
            "--args-my-arg1",
            "provider",
        ])
        .output()
        .expect("smol-wf should run");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout should be JSON");
    assert_eq!(stdout["result"], "echo: hello provider");
}
