use rusqlite::Connection;
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

static DB_COUNTER: AtomicUsize = AtomicUsize::new(0);

fn node() -> String {
    std::env::var("NODE").unwrap_or_else(|_| "node".to_string())
}

fn smol_wf() -> Command {
    Command::new(env!("CARGO_BIN_EXE_smol-wf"))
}

fn smol_wf_run(script: &str) -> Command {
    let db_id = DB_COUNTER.fetch_add(1, Ordering::SeqCst);
    let db_path = std::env::temp_dir().join(format!(
        "smol-wf-cli-test-{}-{db_id}.db",
        std::process::id()
    ));
    let mut command = smol_wf();
    command.arg("run").arg(script).arg("--db").arg(db_path);
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

fn configure_default_app_dir(command: &mut Command, root: &Path) {
    #[cfg(target_os = "windows")]
    {
        command.env("APPDATA", root.join("appdata"));
    }
    #[cfg(target_os = "macos")]
    {
        command.env("HOME", root.join("home"));
    }
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    {
        command.env("XDG_STATE_HOME", root.join("state"));
    }
}

fn expected_default_db_path(root: &Path) -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        root.join("appdata")
            .join("smol-workflows")
            .join("workflows.db")
    }
    #[cfg(target_os = "macos")]
    {
        root.join("home")
            .join("Library")
            .join("Application Support")
            .join("smol-workflows")
            .join("workflows.db")
    }
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    {
        root.join("state")
            .join("smol-workflows")
            .join("workflows.db")
    }
}

fn git(repo: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .expect("git should run");
    assert!(
        output.status.success(),
        "git {} failed\nstdout:\n{}\nstderr:\n{}",
        args.join(" "),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
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
fn tui_replay_check_summarizes_event_stream() {
    let root = temp_dir("tui-replay-check");
    let events_path = root.join("events.jsonl");
    fs::write(
        &events_path,
        r#"{"type":"workflow.started","metadata":{"runId":"run_test","workflowDepth":0},"data":{"startTime":"2026-06-07T00:00:00Z"}}
{"type":"workflow.started","elapsedNanos":1,"metadata":{"runId":"run_test","workflowDepth":1,"parentStepId":"step_child"},"data":{"startTime":"2026-06-07T00:00:00Z"}}
{"type":"workflow.result","elapsedNanos":2,"metadata":{"runId":"run_test","workflowDepth":1,"parentStepId":"step_child"},"data":{"tokenUsage":{"inputTokens":0,"outputTokens":0,"totalTokens":0},"results":{}}}
{"type":"workflow.agent_event","elapsedNanos":3,"metadata":{"runId":"run_test","workflowDepth":0,"stepId":"step_agent","provider":"debug","sessionId":"debug-session"},"data":{"output":"ok"}}
{"type":"workflow.result","elapsedNanos":4,"metadata":{"runId":"run_test","workflowDepth":0},"data":{"tokenUsage":{"inputTokens":1,"outputTokens":2,"totalTokens":3},"results":{"ok":true}}}
"#,
    )
    .expect("events should be written");

    let output = smol_wf()
        .args([
            "tui",
            "replay",
            events_path.to_str().expect("path should be utf8"),
            "--check",
        ])
        .output()
        .expect("smol-wf should run");
    let _ = fs::remove_dir_all(&root);

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("total: 5"));
    assert!(stdout.contains("tabs: 2"));
    assert!(stdout.contains("agentEvents: 1"));
    assert!(stdout.contains("rootResults: 1"));
    assert!(stdout.contains("warnings: 0"));
}

#[test]
fn run_passes_prefixed_cli_args_into_workflow_args() {
    let output = smol_wf_run("../engine/tests/fixtures/cli-args.workflow.js")
        .args([
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
    let output = smol_wf_run("../engine/tests/fixtures/cli-args.workflow.js")
        .args([
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
    let output = smol_wf_run("../engine/tests/fixtures/cli-args.workflow.js")
        .args(["--my-arg1", "world"])
        .output()
        .expect("smol-wf should run");

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Unknown option: --my-arg1"));
}

#[test]
fn run_supports_budget_allowance_flag() {
    let output = smol_wf_run("../../examples/budget.mjs")
        .args([
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
fn run_events_emits_jsonl_lifecycle_and_suppresses_final_report() {
    let root = temp_dir("run-events");
    let script = root.join("events.workflow.mjs");
    fs::write(
        &script,
        r#"
export const meta = { name: "events-test", description: "Events test" };
phase("Inspect");
log("checking", { target: "world" });
export default { ok: true };
"#,
    )
    .expect("workflow should be written");

    let output = smol_wf_run(script.to_str().expect("script path should be utf8"))
        .arg("--events")
        .output()
        .expect("smol-wf should run");
    let _ = fs::remove_dir_all(&root);

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).is_empty(),
        "human progress should not be written when --events is set"
    );
    let events = String::from_utf8(output.stdout).expect("stdout should be utf8");
    let events: Vec<serde_json::Value> = events
        .lines()
        .map(|line| serde_json::from_str(line).expect("event line should be JSON"))
        .collect();
    assert_eq!(events.len(), 4);
    assert_eq!(events[0]["type"], "workflow.started");
    let run_id = events[0]["metadata"]["runId"]
        .as_str()
        .expect("run id should be present");
    assert!(events[0]["data"]["startTime"].as_str().is_some());
    assert_eq!(events[1]["type"], "workflow.phase");
    assert_eq!(events[1]["metadata"]["runId"], run_id);
    assert!(events[1]["elapsedNanos"].as_u64().is_some());
    assert_eq!(events[1]["data"]["name"], "Inspect");
    assert_eq!(events[2]["type"], "workflow.log");
    assert_eq!(
        events[2]["data"]["message"],
        "checking {\"target\":\"world\"}"
    );
    assert_eq!(events[3]["type"], "workflow.result");
    assert_eq!(events[3]["data"]["results"], json!({ "ok": true }));
    assert_eq!(events[3]["metadata"]["runId"], run_id);
}

#[test]
fn run_events_emits_child_workflow_lifecycle_with_scope_metadata() {
    let root = temp_dir("run-events-child");
    let parent = root.join("parent.workflow.mjs");
    let child = root.join("child.workflow.mjs");
    fs::write(
        &parent,
        r#"
export const meta = { name: "cli-parent-events", description: "CLI parent events" };
log("parent before");
const child = await workflow({ scriptPath: "./child.workflow.mjs" }, { item: "cli" });
log("parent after");
export default { child };
"#,
    )
    .expect("parent workflow should be written");
    fs::write(
        &child,
        r#"
export const meta = { name: "cli-child-events", description: "CLI child events" };
phase("Child");
log("child", args.item);
export default { item: args.item };
"#,
    )
    .expect("child workflow should be written");

    let output = smol_wf_run(parent.to_str().expect("script path should be utf8"))
        .arg("--events")
        .output()
        .expect("smol-wf should run");
    let _ = fs::remove_dir_all(&root);

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let events = String::from_utf8(output.stdout).expect("stdout should be utf8");
    let events: Vec<serde_json::Value> = events
        .lines()
        .map(|line| serde_json::from_str(line).expect("event line should be JSON"))
        .collect();
    let event_types: Vec<&str> = events
        .iter()
        .map(|event| event["type"].as_str().unwrap())
        .collect();
    assert_eq!(
        event_types,
        vec![
            "workflow.started",
            "workflow.log",
            "workflow.started",
            "workflow.phase",
            "workflow.log",
            "workflow.result",
            "workflow.log",
            "workflow.result",
        ]
    );

    let run_id = events[0]["metadata"]["runId"]
        .as_str()
        .expect("runId should be present")
        .to_string();
    for event in &events {
        assert_eq!(event["metadata"]["runId"], run_id);
    }
    assert_eq!(events[0]["metadata"]["workflowDepth"], 0);
    assert_eq!(events[2]["metadata"]["workflowDepth"], 1);
    let parent_step_id = events[2]["metadata"]["parentStepId"]
        .as_str()
        .expect("child started should include parentStepId")
        .to_string();
    assert!(!parent_step_id.is_empty());
    for event in &events[2..=5] {
        assert_eq!(event["metadata"]["workflowDepth"], 1);
        assert_eq!(event["metadata"]["parentStepId"], parent_step_id);
        assert!(event["elapsedNanos"].as_u64().is_some());
    }
    assert_eq!(events.last().unwrap()["metadata"]["workflowDepth"], 0);
    assert_eq!(
        events.last().unwrap()["data"]["results"],
        json!({ "child": { "item": "cli" } })
    );
}

#[test]
#[cfg(unix)]
fn run_events_emits_codex_agent_events_from_provider_raw_events() {
    let root = temp_dir("run-events-codex-agent");
    let bin_dir = root.join("bin");
    fs::create_dir_all(&bin_dir).expect("bin dir should exist");
    let fake_codex = fs::canonicalize("../engine/tests/fixtures/fake-codex-provider.mjs")
        .expect("fake codex fixture should exist");
    let wrapper = bin_dir.join("codex");
    fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nexec {} {} \"$@\"\n",
            node(),
            fake_codex.display()
        ),
    )
    .expect("wrapper should be written");
    let mut permissions = fs::metadata(&wrapper).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&wrapper, permissions).unwrap();

    let output = smol_wf_run("../engine/tests/fixtures/cli-args.workflow.js")
        .env(
            "PATH",
            format!(
                "{}:{}",
                bin_dir.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .args([
            "--agent-provider",
            "codex",
            "--events",
            "--args-my-arg1",
            "codex-events",
        ])
        .output()
        .expect("smol-wf should run");
    let _ = fs::remove_dir_all(&root);

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let events = String::from_utf8(output.stdout).expect("stdout should be utf8");
    let events: Vec<serde_json::Value> = events
        .lines()
        .map(|line| serde_json::from_str(line).expect("event line should be JSON"))
        .collect();
    let agent_events: Vec<&serde_json::Value> = events
        .iter()
        .filter(|event| event["type"] == "workflow.agent_event")
        .collect();
    assert!(
        agent_events.len() >= 3,
        "codex raw events should be emitted as workflow.agent_event: {events:#?}"
    );
    assert_eq!(agent_events[0]["metadata"]["provider"], "codex");
    assert_eq!(agent_events[0]["metadata"]["sessionId"], "codex-session-1");
    assert!(agent_events[0]["metadata"]["stepId"].as_str().is_some());
    assert_eq!(agent_events[0]["data"]["provider"], "codex");
    assert_eq!(agent_events[0]["data"]["sessionId"], "codex-session-1");
    assert_eq!(
        agent_events[0]["data"]["providerEvent"]["type"],
        "session_meta"
    );
    assert_eq!(
        agent_events[0]["data"]["providerEvent"]["payload"]["id"],
        "codex-session-1"
    );
    assert!(agent_events
        .iter()
        .any(|event| event["data"]["providerEvent"]["type"] == "turn_complete"));
    assert_eq!(events.last().unwrap()["type"], "workflow.result");
}

#[test]
#[cfg(unix)]
fn run_events_emits_agent_events_from_provider_raw_result() {
    let root = temp_dir("run-events-agent");
    let bin_dir = root.join("bin");
    fs::create_dir_all(&bin_dir).expect("bin dir should exist");
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

    let output = smol_wf_run("../engine/tests/fixtures/cli-args.workflow.js")
        .env(
            "PATH",
            format!(
                "{}:{}",
                bin_dir.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .args([
            "--agent-provider",
            "claude-code",
            "--events",
            "--args-my-arg1",
            "events",
        ])
        .output()
        .expect("smol-wf should run");
    let _ = fs::remove_dir_all(&root);

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let events = String::from_utf8(output.stdout).expect("stdout should be utf8");
    let events: Vec<serde_json::Value> = events
        .lines()
        .map(|line| serde_json::from_str(line).expect("event line should be JSON"))
        .collect();
    let agent_event = events
        .iter()
        .find(|event| event["type"] == "workflow.agent_event")
        .expect("agent event should be emitted");
    assert_eq!(agent_event["metadata"]["provider"], "claude-code");
    assert_eq!(agent_event["metadata"]["sessionId"], "claude-session-1");
    assert!(agent_event["metadata"]["stepId"].as_str().is_some());
    assert_eq!(agent_event["data"]["provider"], "claude-code");
    assert_eq!(agent_event["data"]["sessionId"], "claude-session-1");
    assert_eq!(
        agent_event["data"]["providerEvent"]["session_id"],
        "claude-session-1"
    );
    assert_eq!(events.last().unwrap()["type"], "workflow.result");
}

#[test]
fn run_events_emits_terminal_error_on_workflow_failure() {
    let root = temp_dir("run-events-error");
    let script = root.join("events-error.workflow.mjs");
    fs::write(
        &script,
        r#"
export const meta = { name: "events-error-test", description: "Events error test" };
log("before failure");
throw new Error("boom");
"#,
    )
    .expect("workflow should be written");

    let output = smol_wf_run(script.to_str().expect("script path should be utf8"))
        .arg("--events")
        .output()
        .expect("smol-wf should run");
    let _ = fs::remove_dir_all(&root);

    assert!(!output.status.success());
    let events = String::from_utf8(output.stdout).expect("stdout should be utf8");
    let events: Vec<serde_json::Value> = events
        .lines()
        .map(|line| serde_json::from_str(line).expect("event line should be JSON"))
        .collect();
    assert_eq!(events[0]["type"], "workflow.started");
    assert_eq!(events[1]["type"], "workflow.log");
    assert_eq!(events.last().unwrap()["type"], "workflow.error");
    assert!(events.last().unwrap()["data"]["message"]
        .as_str()
        .unwrap()
        .contains("boom"));
}

#[test]
fn run_rejects_missing_raw_sessions_directory() {
    let missing = std::env::temp_dir().join(format!(
        "smol-wf-cli-missing-raw-sessions-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&missing);

    let output = smol_wf_run("../engine/tests/fixtures/cli-args.workflow.js")
        .args([
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

    let output = smol_wf_run("../engine/tests/fixtures/cli-args.workflow.js")
        .env(
            "PATH",
            format!(
                "{}:{}",
                bin_dir.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .args([
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
        "fake Claude emits one raw provider event"
    );
    assert_eq!(first["session_id"], "claude-session-1");
    assert_eq!(first["result"], "fake claude: hello raw");
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn run_rejects_invalid_budget_allowance() {
    let output = smol_wf_run("../engine/tests/fixtures/cli-args.workflow.js")
        .args(["--budget-allowance", "-1"])
        .output()
        .expect("smol-wf should run");

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("--budget-allowance must be a non-negative integer"));
}

#[test]
fn run_supports_agent_provider_debug() {
    let output = smol_wf_run("../engine/tests/fixtures/cli-args.workflow.js")
        .args(["--agent-provider", "debug", "--args-my-arg1", "provider"])
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
    let output = smol_wf_run("../engine/tests/fixtures/cli-args.workflow.js")
        .args(["--log-level", "debug", "--args-my-arg1", "logging"])
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

    let output = smol_wf_run(path.to_str().expect("path should be utf8"))
        .args(["--log-level", "debug", "--max-parallel-agents", "1"])
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
fn run_reports_worktree_isolation_metadata() {
    let root = temp_dir("worktree-isolation");
    git(&root, &["init"]);
    git(&root, &["config", "user.email", "test@example.invalid"]);
    git(&root, &["config", "user.name", "Test User"]);
    fs::write(
        root.join("workflow.mjs"),
        r#"
export const meta = { name: "cli-isolation", description: "CLI worktree isolation" };
export default { result: await agent("isolated cli", { key: "isolated", isolation: "worktree", model: "debug/test-model" }) };
"#,
    )
    .expect("workflow fixture should be written");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "initial"]);

    let db_path = root.join("workflow.db");
    let output = smol_wf()
        .current_dir(&root)
        .args([
            "run",
            "workflow.mjs",
            "--db",
            db_path.to_str().expect("db path should be utf8"),
            "--agent-provider",
            "debug",
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
    assert_eq!(stdout["results"]["result"], "echo: isolated cli");
    assert!(stdout.get("agentRuns").is_none());
    let run_id = stdout["runID"].as_str().expect("run id should be present");

    let history = smol_wf()
        .current_dir(&root)
        .args([
            "history",
            run_id,
            "--db",
            db_path.to_str().expect("db path should be utf8"),
            "--output",
            "json",
        ])
        .output()
        .expect("smol-wf history should run");
    assert!(
        history.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&history.stderr)
    );
    let history_stdout: serde_json::Value =
        serde_json::from_slice(&history.stdout).expect("history stdout should be JSON");
    assert_eq!(
        history_stdout["steps"][0]["agent"]["model"],
        "debug/test-model"
    );
    let isolation = &history_stdout["steps"][0]["agent"]["isolation"];
    assert_eq!(isolation["kind"], "worktree");
    let branch = isolation["branch"]
        .as_str()
        .expect("branch should be present");
    assert!(
        branch.starts_with("smol-wf/agent-run/"),
        "unexpected branch name: {branch}"
    );
    let worktree_path = isolation["worktreePath"]
        .as_str()
        .expect("worktree path should be present");
    let cwd = isolation["cwd"].as_str().expect("cwd should be present");
    assert_eq!(cwd, worktree_path);
    assert!(
        !Path::new(worktree_path).exists(),
        "worktree should be cleaned up after the run"
    );

    let branch_output = Command::new("git")
        .args(["branch", "--list", branch])
        .current_dir(&root)
        .output()
        .expect("git branch should run");
    assert!(branch_output.status.success());
    assert!(
        String::from_utf8_lossy(&branch_output.stdout)
            .trim()
            .is_empty(),
        "isolation branch should be deleted after the run"
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn run_rejects_worktree_isolation_outside_git_repo() {
    let root = temp_dir("worktree-isolation-non-git");
    fs::write(
        root.join("workflow.mjs"),
        r#"
export const meta = { name: "cli-isolation", description: "CLI worktree isolation" };
export default await agent("isolated cli", { isolation: "worktree" });
"#,
    )
    .expect("workflow fixture should be written");

    let mut command = smol_wf();
    configure_default_app_dir(&mut command, &root);
    let output = command
        .current_dir(&root)
        .args(["run", "workflow.mjs", "--agent-provider", "debug"])
        .output()
        .expect("smol-wf should run");

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("requires the workflow cwd to be inside a git repository"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn run_rejects_removed_backend_flag() {
    let output = smol_wf_run("../engine/tests/fixtures/cli-args.workflow.js")
        .args(["--backend", "sqlite"])
        .output()
        .expect("smol-wf should run");

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Unknown option: --backend"));
}

#[test]
fn history_lists_runs_with_filters_and_formats() {
    let dir = temp_dir("history-list");
    let db_path = dir.join("history.db");
    let alpha = dir.join("alpha-history.mjs");
    let beta = dir.join("beta-history.mjs");
    fs::write(
        &alpha,
        r#"export const meta = { name: "alpha-meta", description: "alpha history" };
export default { result: await agent("alpha") };
"#,
    )
    .unwrap();
    fs::write(
        &beta,
        r#"export const meta = { name: "beta-meta", description: "beta history" };
export default { result: await agent("beta") };
"#,
    )
    .unwrap();

    for script in [&alpha, &beta] {
        let output = smol_wf()
            .args([
                "run",
                script.to_str().unwrap(),
                "--db",
                db_path.to_str().unwrap(),
            ])
            .output()
            .expect("smol-wf should run");
        assert!(
            output.status.success(),
            "stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let output = smol_wf()
        .args([
            "history",
            "--db",
            db_path.to_str().unwrap(),
            "-o",
            "json",
            "--state",
            "completed",
            "--name",
            "alpha",
        ])
        .output()
        .expect("smol-wf should run");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let runs: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let runs = runs.as_array().unwrap();
    assert_eq!(runs.len(), 1);
    assert!(runs[0].get("workflowName").is_none());
    assert!(runs[0]["totalTokens"].as_u64().unwrap() > 0);
    assert_eq!(runs[0]["metadata"]["name"], "alpha-meta");
    assert_eq!(runs[0]["state"], "completed");

    let output = smol_wf()
        .args([
            "history",
            "--db",
            db_path.to_str().unwrap(),
            "--output",
            "json",
            "--name",
            "alpha-history",
        ])
        .output()
        .expect("smol-wf should run");
    assert!(output.status.success());
    let path_name_runs: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(path_name_runs.as_array().unwrap().len(), 0);

    let connection = Connection::open(&db_path).unwrap();
    connection
        .execute(
            r#"UPDATE sw_workflow_runs
               SET workflow_run_json = json_remove(workflow_run_json, '$.metadata')
               WHERE run_id = ?1"#,
            [runs[0]["runID"].as_str().unwrap()],
        )
        .unwrap();
    let output = smol_wf()
        .args([
            "history",
            "--db",
            db_path.to_str().unwrap(),
            "-o",
            "json",
            "--state",
            "completed",
        ])
        .output()
        .expect("smol-wf should run");
    assert!(output.status.success());
    let legacy_runs: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let legacy_run = legacy_runs
        .as_array()
        .unwrap()
        .iter()
        .find(|run| run["runID"] == runs[0]["runID"])
        .expect("legacy row should be listed");
    assert_eq!(legacy_run["metadata"], json!({}));
    let created_at = runs[0]["createdAt"].as_str().unwrap();
    assert!(created_at.contains('T'));
    assert!(created_at.ends_with('Z'));

    let output = smol_wf()
        .args([
            "history",
            "--db",
            db_path.to_str().unwrap(),
            "--output",
            "json",
            "--until",
            "0",
        ])
        .output()
        .expect("smol-wf should run");
    assert!(output.status.success());
    let runs: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(runs.as_array().unwrap().len(), 0);

    let output = smol_wf()
        .args(["history", "--db", db_path.to_str().unwrap(), "--limit", "1"])
        .output()
        .expect("smol-wf should run");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("RUN ID"));
    assert!(stdout.contains("TOTAL TOKENS"));
    assert!(stdout.contains("beta-meta") || stdout.contains("alpha-meta"));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn history_shows_run_details_as_json() {
    let dir = temp_dir("history-detail");
    let db_path = dir.join("history.db");
    let script = dir.join("detail-history.mjs");
    fs::write(
        &script,
        r#"export const meta = { name: "detail-meta", description: "detail history" };
phase("Detail");
export default { result: await agent("detail") };
"#,
    )
    .unwrap();

    let output = smol_wf()
        .args([
            "run",
            script.to_str().unwrap(),
            "--db",
            db_path.to_str().unwrap(),
        ])
        .output()
        .expect("smol-wf should run");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let run_id = report["runID"].as_str().unwrap();

    let output = smol_wf()
        .args([
            "history",
            "--db",
            db_path.to_str().unwrap(),
            run_id,
            "--output",
            "json",
        ])
        .output()
        .expect("smol-wf should run");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let detail: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(detail.get("summary").is_none());
    assert_eq!(detail["workflowRun"]["runID"], run_id);
    assert!(detail["workflowRun"]["workerId"]
        .as_str()
        .unwrap()
        .starts_with("owner_"));
    assert!(detail["workflowRun"].get("workflowName").is_none());
    assert_eq!(detail["workflowRun"]["metadata"]["name"], "detail-meta");
    assert_eq!(
        detail["workflowRun"]["metadata"]["description"],
        "detail history"
    );
    assert_eq!(detail["workflowRun"]["state"], "completed");
    assert!(detail["workflowRun"]["createdAt"]
        .as_str()
        .unwrap()
        .ends_with('Z'));
    assert_eq!(detail["workflowRun"]["args"], json!({}));
    assert!(detail["workflowRun"].get("attempts").is_none());
    assert!(detail["workflowRun"].get("completedSteps").is_none());
    assert!(detail["workflowRun"].get("failedSteps").is_none());
    assert!(detail["workflowRun"].get("results").is_none());
    assert!(detail["workflowRun"].get("outputTokens").is_none());
    assert_eq!(detail["results"], json!({ "result": "echo: detail" }));
    assert_eq!(detail["tokenUsage"]["outputTokens"], 4);
    assert_eq!(detail["tokenUsage"]["byPhase"]["Detail"]["outputTokens"], 4);
    assert!(!detail["attempts"].as_array().unwrap().is_empty());
    assert!(detail["attempts"][0]["startedAt"]
        .as_str()
        .unwrap()
        .contains('T'));
    assert!(!detail["steps"].as_array().unwrap().is_empty());
    assert_eq!(detail["steps"][0]["agent"]["provider"], "debug");
    assert_eq!(detail["steps"][0]["agent"]["phase"], "Detail");
    assert_eq!(detail["steps"][0]["tokenUsage"]["outputTokens"], 4);
    assert!(detail["steps"][0].get("outputTokens").is_none());

    let output = smol_wf()
        .args(["history", run_id, "--db", db_path.to_str().unwrap()])
        .output()
        .expect("smol-wf should run");
    assert!(output.status.success());
    let table = String::from_utf8_lossy(&output.stdout);
    assert!(table.contains("Token Usage"));
    assert!(table.contains("PHASE"));
    assert!(table.contains("Detail"));
    assert!(table.contains("PROVIDER"));
    assert!(table.contains("MODEL"));
    assert!(table.contains("CACHE READ"));
    assert!(table.contains("debug"));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn history_reports_missing_run() {
    let dir = temp_dir("history-missing");
    let db_path = dir.join("history.db");
    let script = dir.join("missing-history.mjs");
    fs::write(
        &script,
        r#"export const meta = { name: "missing-meta", description: "missing history" };
export default { result: "ok" };
"#,
    )
    .unwrap();
    let output = smol_wf()
        .args([
            "run",
            script.to_str().unwrap(),
            "--db",
            db_path.to_str().unwrap(),
        ])
        .output()
        .expect("smol-wf should run");
    assert!(output.status.success());

    let output = smol_wf()
        .args(["history", "run_missing", "--db", db_path.to_str().unwrap()])
        .output()
        .expect("smol-wf should run");

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("workflow run run_missing was not found")
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn history_reports_missing_database() {
    let dir = temp_dir("history-missing-db");
    let db_path = dir.join("missing.db");

    let output = smol_wf()
        .args(["history", "--db", db_path.to_str().unwrap()])
        .output()
        .expect("smol-wf should run");

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("history database"));
    assert!(!db_path.exists());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn run_uses_platform_default_database_path_when_db_is_omitted() {
    let dir = temp_dir("default-db-path");
    let script_path = dir.join("default-db.workflow.mjs");
    fs::write(
        &script_path,
        r#"
export const meta = { name: "cli-default-db", description: "CLI default DB" };
export default { result: await agent("default db") };
"#,
    )
    .expect("workflow fixture should be written");

    let mut run = smol_wf();
    configure_default_app_dir(&mut run, &dir);
    let output = run
        .args(["run", script_path.to_str().expect("path should be utf8")])
        .output()
        .expect("smol-wf should run");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let db_path = expected_default_db_path(&dir);
    assert!(
        db_path.exists(),
        "default DB should be created at {db_path:?}"
    );
    assert!(
        !dir.join("smol-workflows.db").exists(),
        "legacy cwd default DB should not be created"
    );

    let mut history = smol_wf();
    configure_default_app_dir(&mut history, &dir);
    let history_output = history
        .args(["history", "--output", "json"])
        .output()
        .expect("smol-wf history should run");
    assert!(
        history_output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&history_output.stderr)
    );
    assert!(String::from_utf8_lossy(&history_output.stdout).contains("cli-default-db"));
    let _ = fs::remove_dir_all(&dir);
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
