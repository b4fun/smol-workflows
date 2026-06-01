use serde_json::json;
use std::fs;
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
fn run_supports_budget_allowance_flag() {
    let output = smol_wf()
        .args([
            "run",
            "../../examples/budget.mjs",
            "--budget-allowance",
            "20",
            "--args-topic",
            "rust cli budget",
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
    assert_eq!(stdout["budget"]["total"], 20);
}

#[test]
fn run_supports_budget_allowance_env() {
    let output = smol_wf()
        .env("SMOL_WF_BUDGET_ALLOWANCE", "15")
        .args([
            "run",
            "../../examples/budget.mjs",
            "--args-topic",
            "rust cli budget env",
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
    assert_eq!(stdout["budget"]["total"], 15);
}

#[test]
fn run_rejects_invalid_budget_allowance() {
    let output = smol_wf()
        .args([
            "run",
            "../../ts/engine/test/fixtures/cli-args.workflow.js",
            "--budget-allowance",
            "-1",
        ])
        .output()
        .expect("smol-wf should run");

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("--budget-allowance must be a non-negative integer"));
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

#[test]
fn run_supports_dim_debug_logging() {
    let output = smol_wf()
        .args([
            "run",
            "../../ts/engine/test/fixtures/cli-args.workflow.js",
            "--log-level",
            "debug",
            "--args-my-arg1",
            "logging",
        ])
        .output()
        .expect("smol-wf should run");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout should remain JSON");
    assert_eq!(stdout["result"], "echo: hello logging");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("\x1b[2m[debug]"));
    assert!(stderr.contains("cli run script="));
    assert!(stderr.contains("run_workflow start"));
}

#[test]
fn run_applies_max_parallel_agents_flag() {
    let path = std::env::temp_dir().join(format!(
        "smol-wf-cli-parallel-{}-{}.mjs",
        std::process::id(),
        "limit"
    ));
    fs::write(
        &path,
        r#"
export const meta = { name: "cli-parallel", description: "CLI parallel limit" };
export default await parallel([
  () => agent("first"),
  () => agent("second"),
]);
"#,
    )
    .expect("workflow fixture should be written");

    let output = smol_wf()
        .args([
            "run",
            path.to_str().expect("path should be utf8"),
            "--log-level",
            "debug",
            "--max-parallel-agents",
            "1",
        ])
        .output()
        .expect("smol-wf should run");
    let _ = fs::remove_file(&path);

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout should remain JSON");
    assert_eq!(stdout, json!(["echo: first", "echo: second"]));

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("starting agent request id=1 in_flight_after_start=1 max_parallel=1"));
}
