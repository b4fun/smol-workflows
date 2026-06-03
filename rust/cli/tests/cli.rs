use serde_json::json;
use std::fs;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

static DB_COUNTER: AtomicUsize = AtomicUsize::new(0);

fn node() -> String {
    std::env::var("NODE").unwrap_or_else(|_| "node".to_string())
}

fn smol_wf() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_smol-wf"));
    let db_id = DB_COUNTER.fetch_add(1, Ordering::SeqCst);
    command.env(
        "SMOL_WF_DB",
        std::env::temp_dir().join(format!(
            "smol-wf-cli-test-{}-{db_id}.db",
            std::process::id()
        )),
    );
    command
}

#[test]
fn run_help_does_not_treat_h_as_script_path() {
    let output = smol_wf()
        .args(["run", "-h"])
        .output()
        .expect("smol-wf should run");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Usage: smol-wf run <workflow-script>"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("failed to resolve workflow script"));
}

fn temp_dir(name: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "smol-wf-cli-test-{}-{}-{name}",
        std::process::id(),
        std::thread::current().name().unwrap_or("thread")
    ));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).expect("temp dir should be created");
    path
}

#[test]
fn llm_list_workflows_discovers_repo_workflow_dirs() {
    let root = temp_dir("list-workflows");
    Command::new("git")
        .arg("init")
        .current_dir(&root)
        .output()
        .expect("git init should run");
    fs::create_dir_all(root.join(".agents/workflows/nested")).expect("workflow dir should exist");
    fs::create_dir_all(root.join(".claude/workflows")).expect("claude workflow dir should exist");
    fs::write(
        root.join(".agents/workflows/alpha.mjs"),
        r#"export const meta = { name: 'alpha', description: 'Alpha workflow' }
export default {}
"#,
    )
    .expect("workflow should be written");
    fs::write(
        root.join(".agents/workflows/nested/ignored.mjs"),
        r#"export default {}
"#,
    )
    .expect("non-workflow should be written");
    fs::write(
        root.join(".claude/workflows/beta.js"),
        r#"export const meta = { name: 'beta', description: 'Beta workflow' }
export default {}
"#,
    )
    .expect("workflow should be written");

    let output = smol_wf()
        .current_dir(root.join(".agents/workflows/nested"))
        .args(["llm", "list-workflows"])
        .output()
        .expect("smol-wf should run");
    let _ = fs::remove_dir_all(&root);

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("NAME"));
    assert!(stdout.contains("PATH"));
    assert!(stdout.contains("DESCRIPTION"));
    assert!(stdout.contains(".agents/workflows/alpha.mjs"));
    assert!(stdout.contains("alpha"));
    assert!(stdout.contains("Alpha workflow"));
    assert!(stdout.contains(".claude/workflows/beta.js"));
    assert!(stdout.contains("beta"));
    assert!(stdout.contains("Beta workflow"));
    assert!(!stdout.contains("ignored"));
}

#[test]
fn llm_list_workflows_reports_empty_table() {
    let root = temp_dir("list-workflows-empty");
    Command::new("git")
        .arg("init")
        .current_dir(&root)
        .output()
        .expect("git init should run");

    let output = smol_wf()
        .current_dir(&root)
        .args(["llm", "list-workflows"])
        .output()
        .expect("smol-wf should run");
    let _ = fs::remove_dir_all(&root);

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("NAME"));
    assert!(stdout.contains("PATH"));
    assert!(stdout.contains("DESCRIPTION"));
    assert!(!stdout.contains("No workflows found"));
}

#[test]
fn run_passes_prefixed_cli_args_into_workflow_args() {
    let output = smol_wf()
        .args([
            "run",
            "../engine/tests/fixtures/cli-args.workflow.js",
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
    assert!(stdout["runID"].as_str().is_some());
    assert!(stdout.get("agentRuns").is_none());
    assert_eq!(stdout["tokenUsage"]["inputTokens"], 3);
    assert_eq!(stdout["tokenUsage"]["outputTokens"], 5);
    assert_eq!(stdout["tokenUsage"]["totalTokens"], 8);
    assert_eq!(stdout["tokenUsage"].as_object().unwrap().len(), 3);
    assert_eq!(
        stdout["results"],
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
            "../engine/tests/fixtures/cli-args.workflow.js",
            "--args-from-file",
            "../engine/tests/fixtures/args.json",
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
    assert_eq!(stdout["results"]["args"]["fromFile"], "file-value");
    assert_eq!(
        stdout["results"]["args"]["nested"]["value"],
        "nested-file-value"
    );
    assert_eq!(stdout["results"]["args"]["my-arg1"], "file-arg-1");
    assert_eq!(stdout["results"]["result"], "echo: hello file-arg-1");
}

#[test]
fn run_rejects_unprefixed_run_args() {
    let output = smol_wf()
        .args([
            "run",
            "../engine/tests/fixtures/cli-args.workflow.js",
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
    assert_eq!(stdout["results"]["budget"]["total"], 20);
    assert!(stdout["tokenUsage"]["outputTokens"].as_u64().unwrap() > 0);
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
    assert_eq!(stdout["results"]["budget"]["total"], 15);
}

#[test]
fn run_rejects_missing_raw_sessions_directory() {
    let missing = std::env::temp_dir().join(format!(
        "smol-wf-cli-missing-raw-sessions-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&missing);

    let output = smol_wf()
        .args([
            "run",
            "../engine/tests/fixtures/cli-args.workflow.js",
            "--save-raw-sessions",
            missing.to_str().expect("path should be utf8"),
        ])
        .output()
        .expect("smol-wf should run");

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("--save-raw-sessions must point to an existing directory"));
}

#[test]
#[cfg(unix)]
fn run_saves_raw_provider_sessions() {
    let root = temp_dir("raw-sessions");
    let bin_dir = root.join("bin");
    let raw_dir = root.join("raw");
    fs::create_dir_all(&bin_dir).expect("bin dir should exist");
    fs::create_dir_all(&raw_dir).expect("raw dir should exist");
    let fake_claude = fs::canonicalize("../engine/tests/fixtures/fake-claude-provider.mjs")
        .expect("fake claude fixture should exist");
    let wrapper = bin_dir.join("claude");
    fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nexec {} {} \"$@\"\n",
            node(),
            fake_claude.display()
        ),
    )
    .expect("wrapper should be written");
    let mut permissions = fs::metadata(&wrapper).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&wrapper, permissions).unwrap();

    let output = smol_wf()
        .env(
            "PATH",
            format!(
                "{}:{}",
                bin_dir.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .args([
            "run",
            "../engine/tests/fixtures/cli-args.workflow.js",
            "--agent-provider",
            "claude-code",
            "--save-raw-sessions",
            raw_dir.to_str().expect("path should be utf8"),
            "--args-my-arg1",
            "raw",
        ])
        .output()
        .expect("smol-wf should run");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let raw_file = raw_dir.join("claude-code/claude-session-1.jsonl");
    assert!(raw_file.exists(), "raw session file should be written");
    let lines = fs::read_to_string(&raw_file).expect("raw file should be readable");
    let mut lines = lines.lines();
    let first: serde_json::Value = serde_json::from_str(lines.next().expect("one JSONL line"))
        .expect("raw session line should be JSON");
    assert!(
        lines.next().is_none(),
        "raw object should be written as one JSONL line"
    );
    assert_eq!(first["response"]["session_id"], "claude-session-1");
    assert_eq!(first["response"]["result"], "fake claude: hello raw");
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn run_rejects_invalid_budget_allowance() {
    let output = smol_wf()
        .args([
            "run",
            "../engine/tests/fixtures/cli-args.workflow.js",
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
            "../engine/tests/fixtures/cli-args.workflow.js",
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
    assert_eq!(stdout["results"]["result"], "echo: hello provider");
}

#[test]
fn run_supports_dim_debug_logging() {
    let output = smol_wf()
        .args([
            "run",
            "../engine/tests/fixtures/cli-args.workflow.js",
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
    assert_eq!(stdout["results"]["result"], "echo: hello logging");

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
    assert_eq!(stdout["results"], json!(["echo: first", "echo: second"]));
    assert!(stdout.get("agentRuns").is_none());

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("starting agent request id=1 in_flight_after_start=1 max_parallel=1"));
}

#[test]
fn run_rejects_removed_backend_flag() {
    let output = smol_wf()
        .args([
            "run",
            "../engine/tests/fixtures/cli-args.workflow.js",
            "--backend",
            "sqlite",
        ])
        .output()
        .expect("smol-wf should run");

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Unknown option: --backend"));
}

#[test]
fn run_uses_sqlite_backend_by_default() {
    let dir = std::env::temp_dir().join(format!(
        "smol-wf-cli-sqlite-{}-{}",
        std::process::id(),
        "backend"
    ));
    fs::create_dir_all(&dir).expect("tempdir should be created");
    let script_path = dir.join("sqlite.workflow.mjs");
    let db_path = dir.join("workflow.db");
    fs::write(
        &script_path,
        r#"
export const meta = { name: "cli-sqlite", description: "CLI SQLite backend" };
export default { result: await agent("sqlite") };
"#,
    )
    .expect("workflow fixture should be written");

    let output = smol_wf()
        .args([
            "run",
            script_path.to_str().expect("path should be utf8"),
            "--db",
            db_path.to_str().expect("db path should be utf8"),
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
    assert_eq!(stdout["results"], json!({ "result": "echo: sqlite" }));
    assert!(stdout["runID"].as_str().unwrap().starts_with("run_"));
    assert!(
        db_path.exists(),
        "sqlite backend should create a database file"
    );
    let _ = fs::remove_dir_all(&dir);
}
