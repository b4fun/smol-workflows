use serde_json::json;
use smol_workflow_engine::agent_providers::{
    AgentProvider, AgentProviderResult, AgentProviderRunInput, AgentProviderSchemaMode,
    AgentProviderUsageMode, AgentUsage, DebugAgentProvider,
};
use smol_workflow_engine::events::{WorkflowEvent, WorkflowEventSink};
use smol_workflow_engine::workflow::{run_workflow, RunWorkflowOptions};
use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(format!("tests/fixtures/{name}"))
}

fn asset_path(name: &str) -> PathBuf {
    PathBuf::from(format!("tests/assets/workflow_integration/{name}"))
}

fn copy_asset(name: &str, destination: &Path) {
    fs::copy(asset_path(name), destination).expect("workflow asset should be copied");
}

fn example_path(name: &str) -> PathBuf {
    PathBuf::from(format!("../../examples/{name}"))
}

fn block_on<T>(future: impl Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime should be created")
        .block_on(future)
}

struct FixedUsageProvider;
struct RawEventsProvider;
struct OptionsEchoProvider;
struct ConcurrentProbeProvider {
    current: AtomicUsize,
    max: AtomicUsize,
}

struct DynamicSchedulingProbeProvider {
    current: AtomicUsize,
    follow_up_started_while_slow_running: AtomicBool,
}

struct CwdProbeProvider {
    cwd: Mutex<Option<PathBuf>>,
}

struct SchemaRetryProvider {
    prompts: Mutex<Vec<String>>,
    always_invalid: bool,
}

#[derive(Default)]
struct CollectingEventSink {
    events: Mutex<Vec<WorkflowEvent>>,
}

impl CollectingEventSink {
    fn events(&self) -> Vec<WorkflowEvent> {
        self.events.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl WorkflowEventSink for CollectingEventSink {
    async fn emit(&self, event: WorkflowEvent) -> anyhow::Result<()> {
        self.events.lock().unwrap().push(event);
        Ok(())
    }
}

#[async_trait::async_trait]
impl AgentProvider for FixedUsageProvider {
    fn name(&self) -> &str {
        "fixed-usage"
    }

    fn schema_mode(&self) -> AgentProviderSchemaMode {
        AgentProviderSchemaMode::Builtin
    }

    fn usage_mode(&self) -> AgentProviderUsageMode {
        AgentProviderUsageMode::Builtin
    }

    async fn run(&self, input: AgentProviderRunInput) -> anyhow::Result<AgentProviderResult> {
        Ok(AgentProviderResult {
            output: json!(format!("fixed: {}", input.prompt)),
            session_id: None,
            model: None,
            usage: Some(AgentUsage {
                input_tokens: Some(100),
                output_tokens: Some(7),
                total_tokens: Some(107),
                ..AgentUsage::default()
            }),
            isolation: None,
            raw: None,
        })
    }
}

#[async_trait::async_trait]
impl AgentProvider for RawEventsProvider {
    fn name(&self) -> &str {
        "raw-events"
    }

    fn schema_mode(&self) -> AgentProviderSchemaMode {
        AgentProviderSchemaMode::Builtin
    }

    fn usage_mode(&self) -> AgentProviderUsageMode {
        AgentProviderUsageMode::Builtin
    }

    async fn run(&self, input: AgentProviderRunInput) -> anyhow::Result<AgentProviderResult> {
        Ok(AgentProviderResult {
            output: json!({ "answer": input.prompt }),
            session_id: Some("raw-session-1".to_string()),
            model: Some("raw-model".to_string()),
            usage: Some(AgentUsage {
                input_tokens: Some(3),
                output_tokens: Some(5),
                total_tokens: Some(8),
                ..AgentUsage::default()
            }),
            isolation: None,
            raw: Some(json!({
                "events": [
                    { "type": "provider.start", "prompt": input.prompt },
                    { "type": "provider.done", "session": "raw-session-1" }
                ],
                "stderr": "ignored for event payload extraction"
            })),
        })
    }
}

impl ConcurrentProbeProvider {
    fn new() -> Self {
        Self {
            current: AtomicUsize::new(0),
            max: AtomicUsize::new(0),
        }
    }

    fn max_concurrent(&self) -> usize {
        self.max.load(Ordering::SeqCst)
    }
}

impl DynamicSchedulingProbeProvider {
    fn new() -> Self {
        Self {
            current: AtomicUsize::new(0),
            follow_up_started_while_slow_running: AtomicBool::new(false),
        }
    }

    fn follow_up_started_while_slow_running(&self) -> bool {
        self.follow_up_started_while_slow_running
            .load(Ordering::SeqCst)
    }
}

impl CwdProbeProvider {
    fn new() -> Self {
        Self {
            cwd: Mutex::new(None),
        }
    }

    fn cwd(&self) -> Option<PathBuf> {
        self.cwd.lock().unwrap().clone()
    }
}

impl SchemaRetryProvider {
    fn new(always_invalid: bool) -> Self {
        Self {
            prompts: Mutex::new(Vec::new()),
            always_invalid,
        }
    }

    fn prompts(&self) -> Vec<String> {
        self.prompts.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl AgentProvider for CwdProbeProvider {
    fn name(&self) -> &str {
        "cwd-probe"
    }

    fn schema_mode(&self) -> AgentProviderSchemaMode {
        AgentProviderSchemaMode::Builtin
    }

    fn usage_mode(&self) -> AgentProviderUsageMode {
        AgentProviderUsageMode::Builtin
    }

    async fn run(&self, input: AgentProviderRunInput) -> anyhow::Result<AgentProviderResult> {
        let cwd = input
            .context
            .cwd
            .clone()
            .ok_or_else(|| anyhow::anyhow!("provider cwd missing"))?;
        fs::write(cwd.join("agent-created.txt"), "isolated")?;
        *self.cwd.lock().unwrap() = Some(cwd.clone());
        Ok(AgentProviderResult {
            output: json!({ "cwd": cwd.to_string_lossy() }),
            session_id: None,
            model: None,
            usage: None,
            isolation: None,
            raw: None,
        })
    }
}

#[async_trait::async_trait]
impl AgentProvider for SchemaRetryProvider {
    fn name(&self) -> &str {
        "schema-retry"
    }

    fn schema_mode(&self) -> AgentProviderSchemaMode {
        AgentProviderSchemaMode::Builtin
    }

    fn usage_mode(&self) -> AgentProviderUsageMode {
        AgentProviderUsageMode::Builtin
    }

    async fn run(&self, input: AgentProviderRunInput) -> anyhow::Result<AgentProviderResult> {
        let mut prompts = self.prompts.lock().unwrap();
        prompts.push(input.prompt);
        let attempt = prompts.len();
        drop(prompts);

        let output = if self.always_invalid || attempt == 1 {
            json!({ "wrong": true })
        } else {
            json!({ "summary": "corrected" })
        };
        Ok(AgentProviderResult {
            output,
            session_id: None,
            model: None,
            usage: Some(AgentUsage {
                output_tokens: Some(1),
                ..Default::default()
            }),
            isolation: None,
            raw: None,
        })
    }
}

#[async_trait::async_trait]
impl AgentProvider for DynamicSchedulingProbeProvider {
    fn name(&self) -> &str {
        "dynamic-scheduling-probe"
    }

    fn schema_mode(&self) -> AgentProviderSchemaMode {
        AgentProviderSchemaMode::Builtin
    }

    fn usage_mode(&self) -> AgentProviderUsageMode {
        AgentProviderUsageMode::Builtin
    }

    async fn run(&self, input: AgentProviderRunInput) -> anyhow::Result<AgentProviderResult> {
        self.current.fetch_add(1, Ordering::SeqCst);
        match input.prompt.as_str() {
            "fast-parent" => tokio::time::sleep(Duration::from_millis(25)).await,
            "slow" => tokio::time::sleep(Duration::from_millis(200)).await,
            "follow-up" => {
                if self.current.load(Ordering::SeqCst) > 1 {
                    self.follow_up_started_while_slow_running
                        .store(true, Ordering::SeqCst);
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            _ => {}
        }
        self.current.fetch_sub(1, Ordering::SeqCst);
        Ok(AgentProviderResult {
            output: json!(input.prompt),
            session_id: None,
            model: None,
            usage: None,
            isolation: None,
            raw: None,
        })
    }
}

#[async_trait::async_trait]
impl AgentProvider for ConcurrentProbeProvider {
    fn name(&self) -> &str {
        "concurrent-probe"
    }

    fn schema_mode(&self) -> AgentProviderSchemaMode {
        AgentProviderSchemaMode::Builtin
    }

    fn usage_mode(&self) -> AgentProviderUsageMode {
        AgentProviderUsageMode::Builtin
    }

    async fn run(&self, input: AgentProviderRunInput) -> anyhow::Result<AgentProviderResult> {
        let now = self.current.fetch_add(1, Ordering::SeqCst) + 1;
        self.max.fetch_max(now, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(50)).await;
        self.current.fetch_sub(1, Ordering::SeqCst);
        Ok(AgentProviderResult {
            output: json!(input.prompt),
            session_id: None,
            model: None,
            usage: None,
            isolation: None,
            raw: None,
        })
    }
}

#[async_trait::async_trait]
impl AgentProvider for OptionsEchoProvider {
    fn name(&self) -> &str {
        "options-echo"
    }

    fn schema_mode(&self) -> AgentProviderSchemaMode {
        AgentProviderSchemaMode::Builtin
    }

    fn usage_mode(&self) -> AgentProviderUsageMode {
        AgentProviderUsageMode::Builtin
    }

    async fn run(&self, input: AgentProviderRunInput) -> anyhow::Result<AgentProviderResult> {
        Ok(AgentProviderResult {
            output: json!({
                "prompt": input.prompt,
                "options": input.options,
                "context": {
                    "phase": input.context.phase,
                }
            }),
            session_id: None,
            model: None,
            usage: None,
            isolation: None,
            raw: None,
        })
    }
}

fn run_debug(
    script_path: PathBuf,
    args: serde_json::Value,
) -> smol_workflow_engine::workflow::RunWorkflowResult {
    let provider = Arc::new(DebugAgentProvider::new());
    block_on(run_workflow(RunWorkflowOptions {
        script_path,
        args,
        agent_provider: provider.clone(),
        budget_total: None,
        budget_spent: 0,
        nesting_depth: 0,
        max_parallel_agent_requests: None,
        agent_runner: None,
        cancel_rx: None,
        event_sink: None,
        event_parent_step_id: None,
        event_stream_start: None,
        session_log_sink: None,
    }))
    .expect("workflow should run")
}

#[test]
fn event_sink_emits_documented_success_stream_with_agent_events() {
    let temp = tempfile::tempdir().expect("tempdir should be created");
    let script_path = temp.path().join("events-success.workflow.js");
    fs::write(
        &script_path,
        r#"
export const meta = { name: "events-success", description: "Events success" };
phase("Inspect");
log("checking", { target: "cluster" });
export default { agent: await agent("inspect cluster") };
"#,
    )
    .expect("workflow should be written");

    let event_sink = Arc::new(CollectingEventSink::default());
    let event_sink_dyn: Arc<dyn WorkflowEventSink> = event_sink.clone();
    let result = block_on(run_workflow(RunWorkflowOptions {
        script_path,
        args: json!({}),
        agent_provider: Arc::new(RawEventsProvider),
        budget_total: None,
        budget_spent: 0,
        nesting_depth: 0,
        max_parallel_agent_requests: None,
        agent_runner: None,
        cancel_rx: None,
        event_sink: Some(event_sink_dyn),
        event_parent_step_id: None,
        event_stream_start: None,
        session_log_sink: None,
    }))
    .expect("workflow should run");

    assert_eq!(
        result.output.result,
        json!({ "agent": { "answer": "inspect cluster" } })
    );
    let events = event_sink.events();
    let event_types: Vec<&str> = events
        .iter()
        .map(|event| event.event_type.as_str())
        .collect();
    assert_eq!(
        event_types,
        vec![
            "workflow.started",
            "workflow.phase",
            "workflow.log",
            "workflow.agent_event",
            "workflow.agent_event",
            "workflow.result",
        ]
    );
    assert!(events[0].elapsed_nanos.is_none());
    for event in events.iter().skip(1) {
        assert!(
            event.elapsed_nanos.is_some(),
            "event {} should include elapsedNanos",
            event.event_type
        );
    }
    let start_time = events[0].data["startTime"].as_str().unwrap();
    assert!(start_time.contains('T'));
    assert!(start_time.ends_with('Z'));
    assert_eq!(events[1].data, json!({ "name": "Inspect" }));
    assert_eq!(
        events[2].data,
        json!({ "message": "checking {\"target\":\"cluster\"}" })
    );
    assert_eq!(
        events[3].data,
        json!({ "type": "provider.start", "prompt": "inspect cluster" })
    );
    assert_eq!(
        events[4].data,
        json!({ "type": "provider.done", "session": "raw-session-1" })
    );
    for event in &events[3..=4] {
        let metadata = event
            .metadata
            .as_ref()
            .expect("agent metadata should exist");
        assert!(!metadata.step_id.as_deref().unwrap().is_empty());
        assert_eq!(metadata.provider.as_deref(), Some("raw-events"));
        assert_eq!(metadata.session_id.as_deref(), Some("raw-session-1"));
        assert_eq!(metadata.run_id, None);
    }
    assert_eq!(
        events[5].data,
        json!({
            "tokenUsage": {
                "inputTokens": 3,
                "outputTokens": 5,
                "totalTokens": 8,
            },
            "results": { "agent": { "answer": "inspect cluster" } },
        })
    );
}

#[test]
fn event_sink_emits_child_workflow_lifecycle_with_scope_metadata() {
    let temp = tempfile::tempdir().expect("tempdir should be created");
    let parent_path = temp.path().join("parent.workflow.mjs");
    let child_path = temp.path().join("child.workflow.mjs");
    fs::write(
        &parent_path,
        r#"
export const meta = { name: "parent-events", description: "Parent events" };
log("parent before");
const child = await workflow({ scriptPath: "./child.workflow.mjs" }, { item: "one" });
log("parent after");
export default { child };
"#,
    )
    .expect("parent workflow should be written");
    fs::write(
        &child_path,
        r#"
export const meta = { name: "child-events", description: "Child events" };
phase("Child phase");
log("child item", args.item);
export default { item: args.item };
"#,
    )
    .expect("child workflow should be written");

    let event_sink = Arc::new(CollectingEventSink::default());
    let event_sink_dyn: Arc<dyn WorkflowEventSink> = event_sink.clone();
    let result = block_on(run_workflow(RunWorkflowOptions {
        script_path: parent_path,
        args: json!({}),
        agent_provider: Arc::new(DebugAgentProvider::new()),
        budget_total: None,
        budget_spent: 0,
        nesting_depth: 0,
        max_parallel_agent_requests: None,
        agent_runner: None,
        cancel_rx: None,
        event_sink: Some(event_sink_dyn),
        event_parent_step_id: None,
        event_stream_start: None,
        session_log_sink: None,
    }))
    .expect("workflow should run");

    assert_eq!(result.output.result, json!({ "child": { "item": "one" } }));
    let events = event_sink.events();
    let event_types: Vec<&str> = events
        .iter()
        .map(|event| event.event_type.as_str())
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

    let root_started = events[0].metadata.as_ref().unwrap();
    assert_eq!(root_started.workflow_depth, Some(0));
    assert_eq!(root_started.parent_step_id, None);
    let child_started = events[2].metadata.as_ref().unwrap();
    assert_eq!(child_started.workflow_depth, Some(1));
    let parent_step_id = child_started
        .parent_step_id
        .as_ref()
        .expect("child lifecycle should include parentStepId")
        .clone();
    assert!(!parent_step_id.is_empty());
    for event in &events[2..=5] {
        let metadata = event.metadata.as_ref().unwrap();
        assert_eq!(metadata.workflow_depth, Some(1));
        assert_eq!(
            metadata.parent_step_id.as_deref(),
            Some(parent_step_id.as_str())
        );
        assert!(
            event.elapsed_nanos.is_some(),
            "child event {} should use stream elapsedNanos",
            event.event_type
        );
    }
    let root_result = events.last().unwrap().metadata.as_ref().unwrap();
    assert_eq!(root_result.workflow_depth, Some(0));
    assert_eq!(root_result.parent_step_id, None);
}

#[test]
fn event_sink_emits_child_error_before_root_error_with_scope_metadata() {
    let temp = tempfile::tempdir().expect("tempdir should be created");
    let parent_path = temp.path().join("parent-error.workflow.mjs");
    let child_path = temp.path().join("child-error.workflow.mjs");
    fs::write(
        &parent_path,
        r#"
export const meta = { name: "parent-child-error", description: "Parent child error" };
await workflow({ scriptPath: "./child-error.workflow.mjs" }, {});
export default { unreachable: true };
"#,
    )
    .expect("parent workflow should be written");
    fs::write(
        &child_path,
        r#"
export const meta = { name: "child-error", description: "Child error" };
log("child before error");
throw new Error("child exploded");
"#,
    )
    .expect("child workflow should be written");

    let event_sink = Arc::new(CollectingEventSink::default());
    let event_sink_dyn: Arc<dyn WorkflowEventSink> = event_sink.clone();
    let error = block_on(run_workflow(RunWorkflowOptions {
        script_path: parent_path,
        args: json!({}),
        agent_provider: Arc::new(DebugAgentProvider::new()),
        budget_total: None,
        budget_spent: 0,
        nesting_depth: 0,
        max_parallel_agent_requests: None,
        agent_runner: None,
        cancel_rx: None,
        event_sink: Some(event_sink_dyn),
        event_parent_step_id: None,
        event_stream_start: None,
        session_log_sink: None,
    }))
    .expect_err("workflow should fail");

    assert!(format!("{error:#}").contains("child exploded"));
    let events = event_sink.events();
    let event_types: Vec<&str> = events
        .iter()
        .map(|event| event.event_type.as_str())
        .collect();
    assert_eq!(
        event_types,
        vec![
            "workflow.started",
            "workflow.started",
            "workflow.log",
            "workflow.error",
            "workflow.error",
        ]
    );

    let child_started_metadata = events[1].metadata.as_ref().unwrap();
    assert_eq!(child_started_metadata.workflow_depth, Some(1));
    let parent_step_id = child_started_metadata
        .parent_step_id
        .as_ref()
        .expect("child started should include parentStepId")
        .clone();
    for event in &events[1..=3] {
        let metadata = event.metadata.as_ref().unwrap();
        assert_eq!(metadata.workflow_depth, Some(1));
        assert_eq!(
            metadata.parent_step_id.as_deref(),
            Some(parent_step_id.as_str())
        );
    }
    assert!(events[3].data["message"]
        .as_str()
        .unwrap()
        .contains("child exploded"));
    let root_error_metadata = events[4].metadata.as_ref().unwrap();
    assert_eq!(root_error_metadata.workflow_depth, Some(0));
    assert_eq!(root_error_metadata.parent_step_id, None);
    assert!(events[4].data["message"]
        .as_str()
        .unwrap()
        .contains("child exploded"));
}

#[test]
fn event_sink_emits_error_stream_on_workflow_failure() {
    let temp = tempfile::tempdir().expect("tempdir should be created");
    let script_path = temp.path().join("events-error.workflow.js");
    fs::write(
        &script_path,
        r#"
export const meta = { name: "events-error", description: "Events error" };
log("before failure");
throw new Error("integration boom");
"#,
    )
    .expect("workflow should be written");

    let event_sink = Arc::new(CollectingEventSink::default());
    let event_sink_dyn: Arc<dyn WorkflowEventSink> = event_sink.clone();
    let error = block_on(run_workflow(RunWorkflowOptions {
        script_path,
        args: json!({}),
        agent_provider: Arc::new(DebugAgentProvider::new()),
        budget_total: None,
        budget_spent: 0,
        nesting_depth: 0,
        max_parallel_agent_requests: None,
        agent_runner: None,
        cancel_rx: None,
        event_sink: Some(event_sink_dyn),
        event_parent_step_id: None,
        event_stream_start: None,
        session_log_sink: None,
    }))
    .expect_err("workflow should fail");

    assert!(format!("{error:#}").contains("integration boom"));
    let events = event_sink.events();
    let event_types: Vec<&str> = events
        .iter()
        .map(|event| event.event_type.as_str())
        .collect();
    assert_eq!(
        event_types,
        vec!["workflow.started", "workflow.log", "workflow.error"]
    );
    assert_eq!(events[1].data, json!({ "message": "before failure" }));
    assert!(events[2].data["message"]
        .as_str()
        .unwrap()
        .contains("integration boom"));
    assert!(events[2].elapsed_nanos.is_some());
}

#[test]
fn runs_workflow_extra_sleep_before_agent() {
    let result = run_debug(fixture_path("sleep.workflow.js"), json!({}));
    assert_eq!(result.output.result["slept"], true);
    assert_eq!(result.output.result["result"], "echo: after sleep");
    assert_eq!(result.agent_calls.len(), 1);
}

#[test]
fn runs_child_workflow_that_uses_workflow_extra_sleep() {
    let result = run_debug(fixture_path("sleep-parent.workflow.js"), json!({}));
    assert_eq!(
        result.output.result,
        json!({
            "parentSlept": true,
            "child": {
                "childSlept": true,
                "value": "from-parent",
            },
        })
    );
    assert_eq!(result.workflow_calls.len(), 1);
}

fn run_with_provider(
    script_path: PathBuf,
    provider: Arc<dyn AgentProvider>,
) -> anyhow::Result<smol_workflow_engine::workflow::RunWorkflowResult> {
    block_on(run_workflow(RunWorkflowOptions {
        script_path,
        args: json!({}),
        agent_provider: provider,
        budget_total: None,
        budget_spent: 0,
        nesting_depth: 0,
        max_parallel_agent_requests: None,
        agent_runner: None,
        cancel_rx: None,
        event_sink: None,
        event_parent_step_id: None,
        event_stream_start: None,
        session_log_sink: None,
    }))
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
fn runs_worktree_isolated_agent_in_fresh_git_worktree() {
    let repo = tempfile::tempdir().expect("temp repo");
    git(repo.path(), &["init"]);
    git(
        repo.path(),
        &["config", "user.email", "test@example.invalid"],
    );
    git(repo.path(), &["config", "user.name", "Test User"]);
    copy_asset(
        "worktree-isolated-agent.workflow.js",
        &repo.path().join("workflow.mjs"),
    );
    fs::write(repo.path().join("tracked.txt"), "tracked").expect("tracked file");
    git(repo.path(), &["add", "."]);
    git(repo.path(), &["commit", "-m", "initial"]);

    let provider = Arc::new(CwdProbeProvider::new());
    let result = run_with_provider(repo.path().join("workflow.mjs"), provider.clone())
        .expect("workflow should run with worktree isolation");

    let isolated_cwd = provider.cwd().expect("provider cwd should be captured");
    assert_ne!(isolated_cwd, repo.path());
    assert!(!repo.path().join("agent-created.txt").exists());
    assert!(
        !isolated_cwd.exists(),
        "isolated worktree should be cleaned up after the agent run"
    );
    assert_eq!(
        result.output.result["cwd"],
        json!(isolated_cwd.to_string_lossy())
    );
    let isolation = result.agent_runs[0]
        .isolation
        .as_ref()
        .expect("agent run should include isolation info");
    assert_eq!(isolation.kind, "worktree");
    let branch = isolation.branch.as_deref().expect("branch name");
    assert!(
        branch.starts_with("smol-wf/agent-run/"),
        "unexpected branch name: {branch}"
    );
    assert_eq!(
        isolation.worktree_path.as_deref(),
        Some(isolated_cwd.to_string_lossy().as_ref())
    );
    assert_eq!(
        isolation.cwd.as_deref(),
        Some(isolated_cwd.to_string_lossy().as_ref())
    );
    let branch_output = Command::new("git")
        .args(["branch", "--list", branch])
        .current_dir(repo.path())
        .output()
        .expect("git branch should run");
    assert!(branch_output.status.success());
    assert!(
        String::from_utf8_lossy(&branch_output.stdout)
            .trim()
            .is_empty(),
        "isolation branch should be deleted during cleanup"
    );
}

#[test]
fn worktree_isolation_requires_git_repository() {
    let workspace = tempfile::tempdir().expect("temp workspace");
    copy_asset(
        "worktree-isolated-agent.workflow.js",
        &workspace.path().join("workflow.mjs"),
    );

    let error = run_with_provider(
        workspace.path().join("workflow.mjs"),
        Arc::new(CwdProbeProvider::new()),
    )
    .unwrap_err()
    .to_string();
    assert!(
        error.contains("requires the workflow cwd to be inside a git repository"),
        "unexpected error: {error}"
    );
}

#[test]
fn runs_injected_globals_fixture_with_debug_provider() {
    let result = run_debug(
        fixture_path("injected-globals.workflow.js"),
        json!({ "my-arg1": "arg-value-1", "my-arg2": "arg-value-2" }),
    );

    assert_eq!(
        result.output.result,
        json!({
            "first": "echo: first: arg-value-1",
            "second": "echo: second: arg-value-2",
            "args": { "my-arg1": "arg-value-1", "my-arg2": "arg-value-2" }
        })
    );
    assert_eq!(
        result.logs,
        vec![vec![
            json!("received"),
            json!({ "my-arg1": "arg-value-1", "my-arg2": "arg-value-2" })
        ]]
    );
    assert_eq!(result.phases[0].name, "Research");
}

#[test]
fn runs_module_result_fixture_with_debug_provider() {
    let result = run_debug(
        fixture_path("module-result.workflow.js"),
        json!({ "my-arg1": "arg-value-1", "my-arg2": "arg-value-2" }),
    );

    assert_eq!(
        result.output.result,
        json!({
            "first": "echo: first: arg-value-1",
            "second": "echo: second: arg-value-2",
            "args": { "my-arg1": "arg-value-1", "my-arg2": "arg-value-2" }
        })
    );
}

#[test]
fn rejects_missing_metadata_and_missing_default_export() {
    let provider = Arc::new(DebugAgentProvider::new());
    let no_meta = block_on(run_workflow(RunWorkflowOptions {
        script_path: fixture_path("no-meta.workflow.js"),
        args: json!({}),
        agent_provider: provider.clone(),
        budget_total: None,
        budget_spent: 0,
        nesting_depth: 0,
        max_parallel_agent_requests: None,
        agent_runner: None,
        cancel_rx: None,
        event_sink: None,
        event_parent_step_id: None,
        event_stream_start: None,
        session_log_sink: None,
    }))
    .unwrap_err();
    assert!(no_meta
        .to_string()
        .contains("Workflow script must export valid literal metadata"));

    let missing_default = block_on(run_workflow(RunWorkflowOptions {
        script_path: fixture_path("missing-default.workflow.js"),
        args: json!({}),
        agent_provider: provider.clone(),
        budget_total: None,
        budget_spent: 0,
        nesting_depth: 0,
        max_parallel_agent_requests: None,
        agent_runner: None,
        cancel_rx: None,
        event_sink: None,
        event_parent_step_id: None,
        event_stream_start: None,
        session_log_sink: None,
    }))
    .unwrap_err();
    assert!(missing_default
        .to_string()
        .contains("workflow module must default export a workflow result or function"));
}

#[test]
fn runs_parallel_and_pipeline_fixtures() {
    let parallel = run_debug(fixture_path("parallel-errors.workflow.js"), json!({}));
    assert_eq!(
        parallel.output.result,
        json!(["echo: ok:first", null, null, "echo: ok:last"])
    );

    let pipeline = run_debug(
        fixture_path("pipeline.workflow.js"),
        json!({ "items": ["a", "bad", "c"] }),
    );
    assert_eq!(
        pipeline.output.result,
        json!([
            "echo: stage2:echo: stage1:a:a:0:a:0",
            null,
            "echo: stage2:echo: stage1:c:c:2:c:2"
        ])
    );
}

#[test]
fn runs_child_workflow_fixture() {
    let result = run_debug(
        fixture_path("parent-workflow.workflow.js"),
        json!({ "value": "from-parent" }),
    );

    assert_eq!(
        result.output.result,
        json!({
            "parentArg": "from-parent",
            "child": {
                "childArg": "from-parent",
                "childAgent": "echo: child:from-parent"
            }
        })
    );
    assert_eq!(
        result
            .phases
            .iter()
            .map(|phase| phase.name.as_str())
            .collect::<Vec<_>>(),
        vec!["Parent", "Child"]
    );
}

#[test]
fn rejects_nested_child_workflow_fixture() {
    let provider = Arc::new(DebugAgentProvider::new());
    let error = block_on(run_workflow(RunWorkflowOptions {
        script_path: fixture_path("nested-parent.workflow.js"),
        args: json!({}),
        agent_provider: provider.clone(),
        budget_total: None,
        budget_spent: 0,
        nesting_depth: 0,
        max_parallel_agent_requests: None,
        agent_runner: None,
        cancel_rx: None,
        event_sink: None,
        event_parent_step_id: None,
        event_stream_start: None,
        session_log_sink: None,
    }))
    .unwrap_err();

    assert!(
        format!("{error:#}").contains("Nested workflow() calls are limited to one level"),
        "unexpected error: {error:#}"
    );
}

#[test]
fn applies_phase_metadata_defaults() {
    let temp = tempfile::tempdir().expect("tempdir should be created");
    let script_path = temp.path().join("phase-defaults.workflow.js");
    copy_asset("phase-defaults.workflow.js", &script_path);

    let provider = Arc::new(OptionsEchoProvider);
    let result = block_on(run_workflow(RunWorkflowOptions {
        script_path,
        args: json!({}),
        agent_provider: provider.clone(),
        budget_total: None,
        budget_spent: 0,
        nesting_depth: 0,
        max_parallel_agent_requests: None,
        agent_runner: None,
        cancel_rx: None,
        event_sink: None,
        event_parent_step_id: None,
        event_stream_start: None,
        session_log_sink: None,
    }))
    .expect("workflow should run");

    assert_eq!(
        result.output.result["inherited"]["options"],
        json!({ "phase": "Research", "model": "opus" })
    );
    assert_eq!(
        result.output.result["explicit"]["options"],
        json!({ "model": "haiku", "phase": "Research" })
    );
    assert_eq!(
        result.output.result["phaseOverride"]["options"],
        json!({ "phase": "Verify", "model": "sonnet" })
    );
}

#[test]
fn agent_provider_option_overrides_default_provider() {
    let temp = tempfile::tempdir().expect("tempdir should be created");
    let script_path = temp.path().join("provider-override.workflow.js");
    copy_asset("provider-override.workflow.js", &script_path);

    let provider = Arc::new(FixedUsageProvider);
    let result = block_on(run_workflow(RunWorkflowOptions {
        script_path,
        args: json!({}),
        agent_provider: provider.clone(),
        budget_total: None,
        budget_spent: 0,
        nesting_depth: 0,
        max_parallel_agent_requests: None,
        agent_runner: None,
        cancel_rx: None,
        event_sink: None,
        event_parent_step_id: None,
        event_stream_start: None,
        session_log_sink: None,
    }))
    .expect("workflow should run");

    assert_eq!(result.output.result, json!("echo: override me"));
    assert_eq!(result.budget.spent, 5);
}

#[test]
fn runs_parallel_agent_requests_concurrently() {
    let temp = tempfile::tempdir().expect("tempdir should be created");
    let script_path = temp.path().join("parallel-agents.workflow.js");
    copy_asset("parallel-agents.workflow.js", &script_path);

    let provider = Arc::new(ConcurrentProbeProvider::new());
    let result = block_on(run_workflow(RunWorkflowOptions {
        script_path,
        args: json!({}),
        agent_provider: provider.clone(),
        budget_total: None,
        budget_spent: 0,
        nesting_depth: 0,
        max_parallel_agent_requests: None,
        agent_runner: None,
        cancel_rx: None,
        event_sink: None,
        event_parent_step_id: None,
        event_stream_start: None,
        session_log_sink: None,
    }))
    .expect("workflow should run");

    assert_eq!(result.output.result, json!(["first", "second", "third"]));
    assert!(
        provider.max_concurrent() > 1,
        "agent provider should have been called concurrently"
    );
}

#[test]
fn starts_follow_up_agent_requests_when_capacity_frees() {
    let temp = tempfile::tempdir().expect("tempdir should be created");
    let script_path = temp.path().join("dynamic-parallel-agents.workflow.js");
    copy_asset("dynamic-parallel-agents.workflow.js", &script_path);

    let provider = Arc::new(DynamicSchedulingProbeProvider::new());
    let result = block_on(run_workflow(RunWorkflowOptions {
        script_path,
        args: json!({}),
        agent_provider: provider.clone(),
        budget_total: None,
        budget_spent: 0,
        nesting_depth: 0,
        max_parallel_agent_requests: Some(2),
        agent_runner: None,
        cancel_rx: None,
        event_sink: None,
        event_parent_step_id: None,
        event_stream_start: None,
        session_log_sink: None,
    }))
    .expect("workflow should run");

    assert_eq!(result.output.result, json!(["follow-up", "slow"]));
    assert!(
        provider.follow_up_started_while_slow_running(),
        "follow-up request should start before the slow sibling finishes"
    );
}

#[test]
fn honors_parallel_agent_request_limit() {
    let temp = tempfile::tempdir().expect("tempdir should be created");
    let script_path = temp.path().join("limited-parallel-agents.workflow.js");
    copy_asset("limited-parallel-agents.workflow.js", &script_path);

    let provider = Arc::new(ConcurrentProbeProvider::new());
    let result = block_on(run_workflow(RunWorkflowOptions {
        script_path,
        args: json!({}),
        agent_provider: provider.clone(),
        budget_total: None,
        budget_spent: 0,
        nesting_depth: 0,
        max_parallel_agent_requests: Some(2),
        agent_runner: None,
        cancel_rx: None,
        event_sink: None,
        event_parent_step_id: None,
        event_stream_start: None,
        session_log_sink: None,
    }))
    .expect("workflow should run");

    assert_eq!(
        result.output.result,
        json!(["first", "second", "third", "fourth"])
    );
    assert_eq!(provider.max_concurrent(), 2);
}

#[test]
fn honors_serial_parallel_agent_request_limit() {
    let temp = tempfile::tempdir().expect("tempdir should be created");
    let script_path = temp.path().join("serial-parallel-agents.workflow.js");
    copy_asset("serial-parallel-agents.workflow.js", &script_path);

    let provider = Arc::new(ConcurrentProbeProvider::new());
    let result = block_on(run_workflow(RunWorkflowOptions {
        script_path,
        args: json!({}),
        agent_provider: provider.clone(),
        budget_total: None,
        budget_spent: 0,
        nesting_depth: 0,
        max_parallel_agent_requests: Some(1),
        agent_runner: None,
        cancel_rx: None,
        event_sink: None,
        event_parent_step_id: None,
        event_stream_start: None,
        session_log_sink: None,
    }))
    .expect("workflow should run");

    assert_eq!(result.output.result, json!(["first", "second", "third"]));
    assert_eq!(provider.max_concurrent(), 1);
}

#[test]
fn validates_schema_fixture_with_debug_provider() {
    let result = run_debug(fixture_path("schema-validation.workflow.js"), json!({}));

    assert_eq!(result.output.result, json!({ "summary": "debug-string" }));
    assert_eq!(result.agent_calls.len(), 1);
}

#[test]
fn exposes_shared_budget_across_agents_and_child_workflows() {
    let provider = Arc::new(FixedUsageProvider);
    let result = block_on(run_workflow(RunWorkflowOptions {
        script_path: fixture_path("budget-parent.workflow.js"),
        args: json!({}),
        agent_provider: provider,
        budget_total: Some(50),
        budget_spent: 0,
        nesting_depth: 0,
        max_parallel_agent_requests: None,
        agent_runner: None,
        cancel_rx: None,
        event_sink: None,
        event_parent_step_id: None,
        event_stream_start: None,
        session_log_sink: None,
    }))
    .expect("workflow should run");

    assert_eq!(
        result.output.result,
        json!({
            "initial": { "total": 50, "spent": 0, "remaining": 50 },
            "afterParentAgent": { "total": 50, "spent": 7, "remaining": 43 },
            "child": {
                "before": { "total": 50, "spent": 7, "remaining": 43 },
                "after": { "total": 50, "spent": 14, "remaining": 36 },
            },
            "afterChild": { "total": 50, "spent": 14, "remaining": 36 },
        })
    );
    assert_eq!(result.budget.total, Some(50));
    assert_eq!(result.budget.spent, 14);
}

#[test]
fn protects_workflow_globals_from_user_mutation() {
    let result = run_debug(
        fixture_path("protected-globals.workflow.js"),
        json!({ "my-arg1": "arg-value-1", "nested": { "value": "original-nested" } }),
    );

    assert_eq!(
        result.output.result,
        json!({
            "blocked": [
                "global-args-set",
                "input-set",
                "ctx-args-set",
                "nested-args-set",
                "agent-property-set",
                "parallel-define-property",
                "pipeline-property-set",
                "global-agent-reassign",
            ],
            "arg": "arg-value-1",
            "inputArg": "arg-value-1",
            "ctxArg": "arg-value-1",
            "nested": "original-nested",
            "agentExtra": null,
            "parallelExtra": null,
            "pipelineExtra": null,
            "agentResult": "echo: value: arg-value-1",
        })
    );
}

#[test]
fn validates_schema_backed_agent_output_and_retries_once() {
    let provider = Arc::new(SchemaRetryProvider::new(false));
    let result = block_on(run_workflow(RunWorkflowOptions {
        script_path: fixture_path("schema-validation.workflow.js"),
        args: json!({}),
        agent_provider: provider.clone(),
        budget_total: None,
        budget_spent: 0,
        nesting_depth: 0,
        max_parallel_agent_requests: None,
        agent_runner: None,
        cancel_rx: None,
        event_sink: None,
        event_parent_step_id: None,
        event_stream_start: None,
        session_log_sink: None,
    }))
    .expect("workflow should retry and run");

    assert_eq!(result.output.result, json!({ "summary": "corrected" }));
    let prompts = provider.prompts();
    assert_eq!(prompts.len(), 2);
    assert_eq!(prompts[0], "produce schema result");
    assert!(prompts[1].contains("Previous structured output failed JSON Schema validation."));
    assert!(prompts[1].contains("Return a corrected structured output"));
    assert!(prompts[1].contains("required property"));
    assert_eq!(result.budget.spent, 1);
}

#[test]
fn rejects_invalid_schema_backed_agent_output_after_retry() {
    let provider = Arc::new(SchemaRetryProvider::new(true));
    let error = block_on(run_workflow(RunWorkflowOptions {
        script_path: fixture_path("schema-validation.workflow.js"),
        args: json!({}),
        agent_provider: provider.clone(),
        budget_total: None,
        budget_spent: 0,
        nesting_depth: 0,
        max_parallel_agent_requests: None,
        agent_runner: None,
        cancel_rx: None,
        event_sink: None,
        event_parent_step_id: None,
        event_stream_start: None,
        session_log_sink: None,
    }))
    .unwrap_err();

    assert!(
        format!("{error:#}").contains("Structured output did not match JSON Schema"),
        "unexpected error: {error:#}"
    );
    assert_eq!(provider.prompts().len(), 2);
}

#[test]
fn updates_live_budget_from_agent_output_token_usage() {
    let provider = Arc::new(FixedUsageProvider);
    let result = block_on(run_workflow(RunWorkflowOptions {
        script_path: fixture_path("on-agent-usage-budget.workflow.js"),
        args: json!({}),
        agent_provider: provider.clone(),
        budget_total: Some(20),
        budget_spent: 0,
        nesting_depth: 0,
        max_parallel_agent_requests: None,
        agent_runner: None,
        cancel_rx: None,
        event_sink: None,
        event_parent_step_id: None,
        event_stream_start: None,
        session_log_sink: None,
    }))
    .expect("workflow should run");

    assert_eq!(
        result.output.result,
        json!({
            "before": 0,
            "first": "fixed: first custom usage",
            "afterFirst": 7,
            "second": "fixed: second custom usage",
            "afterSecond": 14,
        })
    );
    assert_eq!(result.budget.total, Some(20));
    assert_eq!(result.budget.spent, 14);
}

#[test]
fn runs_existing_examples_with_debug_provider() {
    for example in [
        "budget.mjs",
        "hello.mjs",
        "isolation.mjs",
        "refine-agent-providers.mjs",
        "stock.mjs",
        "workflow-child.mjs",
        "workflow-parent.mjs",
    ] {
        let result = run_debug(
            example_path(example),
            json!({ "name": "Rust", "items": ["alpha", "beta"] }),
        );
        assert!(
            result.output.result.is_object(),
            "{example} should return an object"
        );
    }
}
