use serde_json::{json, Value};
use smol_workflow_engine::js_runtime::rquickjs::RQuickJSWorkflowRuntime;
use smol_workflow_engine::js_runtime::{
    WorkflowJSRuntime, WorkflowModuleInput, WorkflowModuleOutput, WorkflowRef, WorkflowRuntimeCall,
    WorkflowRuntimePoll, WorkflowRuntimeRequest, WorkflowRuntimeRequestResolution,
};
use std::fs;
use std::time::Duration;

#[derive(Default, Debug)]
struct RunTrace {
    calls: Vec<WorkflowRuntimeCall>,
    requests: Vec<WorkflowRuntimeRequest>,
}

fn run_to_completion(
    input: WorkflowModuleInput,
) -> anyhow::Result<(WorkflowModuleOutput, RunTrace)> {
    let mut execution = RQuickJSWorkflowRuntime::new().start_module(input)?;
    let mut trace = RunTrace::default();

    for _ in 0..100 {
        match execution.poll()? {
            WorkflowRuntimePoll::Call(call) => trace.calls.push(call),
            WorkflowRuntimePoll::Request(request) => {
                let id = request.id().to_string();
                let result = match &request {
                    WorkflowRuntimeRequest::Agent {
                        prompt, options, ..
                    } => echo_agent_result(prompt, options.as_ref()),
                    WorkflowRuntimeRequest::Workflow {
                        workflow_ref, args, ..
                    } => json!({
                        "workflowRef": workflow_ref,
                        "args": args,
                    }),
                };
                trace.requests.push(request);
                execution.resolve_request(&id, WorkflowRuntimeRequestResolution::Ok(result))?;
            }
            WorkflowRuntimePoll::Complete(output) => return Ok((output, trace)),
            WorkflowRuntimePoll::Pending => continue,
        }
    }

    anyhow::bail!("workflow did not complete within poll limit")
}

fn echo_agent_result(prompt: &str, options: Option<&Value>) -> Value {
    match options {
        Some(options) => json!({ "echo": prompt, "options": options }),
        None => json!({ "echo": prompt }),
    }
}

#[test]
fn rquickjs_executes_default_async_workflow_fixture() {
    let source = fs::read_to_string("tests/fixtures/injected-globals.workflow.js")
        .expect("fixture should exist");
    let (output, trace) = run_to_completion(WorkflowModuleInput::new(
        source,
        "injected-globals.workflow.js",
        json!({ "my-arg1": "alpha", "my-arg2": "beta" }),
    ))
    .expect("workflow should execute");

    assert_eq!(
        trace.calls[0],
        WorkflowRuntimeCall::Phase {
            name: "Research".into(),
            options: None
        }
    );
    assert_eq!(
        trace.calls[1],
        WorkflowRuntimeCall::Log {
            values: vec![
                json!("received"),
                json!({ "my-arg1": "alpha", "my-arg2": "beta" })
            ]
        }
    );
    assert_eq!(trace.requests.len(), 2);
    assert_eq!(output.result["first"]["echo"], "first: alpha");
    assert_eq!(output.result["second"]["echo"], "second: beta");
    assert_eq!(
        output.result["args"],
        json!({ "my-arg1": "alpha", "my-arg2": "beta" })
    );
}

#[test]
fn rquickjs_executes_top_level_module_result_fixture() {
    let source = fs::read_to_string("tests/fixtures/module-result.workflow.js")
        .expect("fixture should exist");
    let (output, trace) = run_to_completion(WorkflowModuleInput::new(
        source,
        "module-result.workflow.js",
        json!({ "my-arg1": "one", "my-arg2": "two" }),
    ))
    .expect("workflow should execute");

    assert_eq!(
        trace.calls[0],
        WorkflowRuntimeCall::Phase {
            name: "ModuleResult".into(),
            options: None
        }
    );
    assert_eq!(
        trace.calls[1],
        WorkflowRuntimeCall::Log {
            values: vec![
                json!("module result args"),
                json!({ "my-arg1": "one", "my-arg2": "two" })
            ]
        }
    );
    assert_eq!(trace.requests.len(), 2);
    assert_eq!(output.result["first"]["echo"], "first: one");
    assert_eq!(output.result["second"]["echo"], "second: two");
    assert_eq!(
        output.result["args"],
        json!({ "my-arg1": "one", "my-arg2": "two" })
    );
}

#[test]
fn rquickjs_blocks_common_host_access_and_randomness() {
    let (output, _trace) = run_to_completion(WorkflowModuleInput::new(
        r#"
export const meta = { name: "sandbox", description: "sandbox" };
const blockedGlobalNames = [
  'eval',
  'Function',
  'AsyncFunction',
  'Date',
  'fetch',
  'XMLHttpRequest',
  'WebSocket',
  'EventSource',
  'navigator',
  'location',
  'Deno',
  'Bun',
  'process',
  'require',
  'Buffer',
  '__dirname',
  '__filename',
];

export default {
  randomBlocked: (() => { try { Math.random(); return false; } catch { return true; } })(),
  randomOverwriteBlocked: (() => { try { Math.random = () => 0; return false; } catch { return true; } })(),
  randomStillBlocked: (() => { try { Math.random(); return false; } catch { return true; } })(),
  blockedGlobals: Object.fromEntries(blockedGlobalNames.map(name => [name, typeof globalThis[name]])),
  blockedGlobalMutationBlocked: Object.fromEntries(blockedGlobalNames.map((name) => {
    try { globalThis[name] = 1; return [name, false]; } catch { return [name, true]; }
  })),
};
"#,
        "sandbox.workflow.js",
        json!({}),
    ))
    .expect("workflow should execute");

    assert_eq!(output.result["randomBlocked"], true);
    assert_eq!(output.result["randomOverwriteBlocked"], true);
    assert_eq!(output.result["randomStillBlocked"], true);
    for name in [
        "eval",
        "Function",
        "AsyncFunction",
        "Date",
        "fetch",
        "XMLHttpRequest",
        "WebSocket",
        "EventSource",
        "navigator",
        "location",
        "Deno",
        "Bun",
        "process",
        "require",
        "Buffer",
        "__dirname",
        "__filename",
    ] {
        assert_eq!(
            output.result["blockedGlobals"][name], "undefined",
            "expected {name} to be blocked"
        );
        assert_eq!(
            output.result["blockedGlobalMutationBlocked"][name], true,
            "expected {name} global binding to be readonly"
        );
    }
}

#[test]
fn rquickjs_agent_inherits_current_phase_in_request_options() {
    let (output, trace) = run_to_completion(WorkflowModuleInput::new(
        r#"
export const meta = { name: "phase", description: "phase" };
phase("Research");
export default await agent("hello");
"#,
        "phase.workflow.js",
        json!({}),
    ))
    .expect("workflow should execute");

    assert_eq!(output.result["echo"], "hello");
    assert_eq!(output.result["options"], json!({ "phase": "Research" }));
    assert_eq!(
        trace.requests[0],
        WorkflowRuntimeRequest::Agent {
            id: "1".into(),
            prompt: "hello".into(),
            options: Some(json!({ "phase": "Research" })),
        }
    );
}

#[test]
fn rquickjs_supports_child_workflow_requests() {
    let (output, trace) = run_to_completion(WorkflowModuleInput::new(
        r#"
export const meta = { name: "parent", description: "parent" };
const first = await workflow("child-by-name", { x: 1 });
const second = await workflow({ scriptPath: "./child.workflow.js" }, { y: 2 });
export default { first, second };
"#,
        "parent.workflow.js",
        json!({}),
    ))
    .expect("workflow should execute");

    assert_eq!(trace.requests.len(), 2);
    assert_eq!(
        trace.requests[0],
        WorkflowRuntimeRequest::Workflow {
            id: "1".into(),
            workflow_ref: WorkflowRef::Name("child-by-name".into()),
            args: Some(json!({ "x": 1 })),
        }
    );
    assert_eq!(
        trace.requests[1],
        WorkflowRuntimeRequest::Workflow {
            id: "2".into(),
            workflow_ref: WorkflowRef::ScriptPath {
                script_path: "./child.workflow.js".into()
            },
            args: Some(json!({ "y": 2 })),
        }
    );
    assert_eq!(
        output.result["first"]["workflowRef"],
        json!("child-by-name")
    );
    assert_eq!(
        output.result["second"]["workflowRef"],
        json!({ "scriptPath": "./child.workflow.js" })
    );
}

#[test]
fn rquickjs_exposes_budget_global() {
    let mut input = WorkflowModuleInput::new(
        r#"
export const meta = { name: "budget", description: "budget" };
export default {
  total: budget.total,
  spent: budget.spent(),
  remaining: budget.remaining(),
};
"#,
        "budget.workflow.js",
        json!({}),
    );
    input.budget.total = Some(100);
    input.budget.spent = 40;

    let (output, _trace) = run_to_completion(input).expect("workflow should execute");

    assert_eq!(
        output.result,
        json!({ "total": 100, "spent": 40, "remaining": 60 })
    );
}

#[test]
fn rquickjs_parallel_queues_multiple_agent_requests() {
    let mut execution = RQuickJSWorkflowRuntime::new()
        .start_module(WorkflowModuleInput::new(
            r#"
export const meta = { name: "parallel", description: "parallel" };
export default await parallel([
  () => agent("first"),
  () => agent("second"),
]);
"#,
            "parallel.workflow.js",
            json!({}),
        ))
        .expect("workflow should start");

    let first = loop {
        match execution.poll().expect("workflow should poll") {
            WorkflowRuntimePoll::Request(request) => break request,
            WorkflowRuntimePoll::Pending => continue,
            other => panic!("expected first request, got {other:?}"),
        }
    };
    assert_eq!(
        first,
        WorkflowRuntimeRequest::Agent {
            id: "1".into(),
            prompt: "first".into(),
            options: None,
        }
    );

    let second = execution.poll().expect("workflow should poll");
    assert_eq!(second, WorkflowRuntimePoll::Request(first.clone()));

    execution
        .resolve_request("1", WorkflowRuntimeRequestResolution::Ok(json!("one")))
        .expect("first request should resolve");

    let second = loop {
        match execution.poll().expect("workflow should poll") {
            WorkflowRuntimePoll::Request(request) => break request,
            WorkflowRuntimePoll::Pending => continue,
            other => panic!("expected second request, got {other:?}"),
        }
    };
    assert_eq!(
        second,
        WorkflowRuntimeRequest::Agent {
            id: "2".into(),
            prompt: "second".into(),
            options: None,
        }
    );

    execution
        .resolve_request("2", WorkflowRuntimeRequestResolution::Ok(json!("two")))
        .expect("second request should resolve");

    let output = loop {
        match execution.poll().expect("workflow should poll") {
            WorkflowRuntimePoll::Complete(output) => break output,
            WorkflowRuntimePoll::Pending => continue,
            other => panic!("expected completion, got {other:?}"),
        }
    };
    assert_eq!(output.result, json!(["one", "two"]));
}

#[test]
fn rquickjs_readonly_proxy_preserves_identity() {
    let mut execution = RQuickJSWorkflowRuntime::new()
        .start_module(WorkflowModuleInput::new(
            r#"
export const meta = { name: "readonly-identity", description: "readonly identity" };
export default {
  sameNestedObject: args.nested === args.nested,
  sameNestedArray: args.items === args.items,
};
"#,
            "readonly-identity.workflow.js",
            json!({ "nested": { "value": 1 }, "items": [1, 2] }),
        ))
        .expect("workflow should start");

    let output = loop {
        match execution.poll().expect("workflow should poll") {
            WorkflowRuntimePoll::Complete(output) => break output,
            WorkflowRuntimePoll::Pending => continue,
            other => panic!("expected completion, got {other:?}"),
        }
    };

    assert_eq!(
        output.result,
        json!({ "sameNestedObject": true, "sameNestedArray": true })
    );
}

#[test]
fn rquickjs_timeout_is_refreshed_after_host_request_wait() {
    let mut input = WorkflowModuleInput::new(
        r#"
export const meta = { name: "timeout-refresh", description: "timeout refresh" };
const value = await agent("wait");
export default { value, afterWait: true };
"#,
        "timeout-refresh.workflow.js",
        json!({}),
    );
    input.sandbox.timeout = Duration::from_millis(10);

    let mut execution = RQuickJSWorkflowRuntime::new()
        .start_module(input)
        .expect("workflow should start");

    let request = loop {
        match execution.poll().expect("workflow should poll") {
            WorkflowRuntimePoll::Request(request) => break request,
            WorkflowRuntimePoll::Pending => continue,
            other => panic!("expected request, got {other:?}"),
        }
    };
    std::thread::sleep(Duration::from_millis(30));
    execution
        .resolve_request(
            request.id(),
            WorkflowRuntimeRequestResolution::Ok(json!("done")),
        )
        .expect("request should resolve after host wait");

    let output = loop {
        match execution.poll().expect("workflow should poll") {
            WorkflowRuntimePoll::Complete(output) => break output,
            WorkflowRuntimePoll::Pending => continue,
            other => panic!("expected completion, got {other:?}"),
        }
    };

    assert_eq!(output.result, json!({ "value": "done", "afterWait": true }));
}

#[test]
fn rquickjs_hides_internal_helpers_and_blocks_additional_host_access() {
    let mut execution = RQuickJSWorkflowRuntime::new()
        .start_module(WorkflowModuleInput::new(
            r#"
export const meta = { name: "sandbox", description: "sandbox" };
const mutationBlocked = [];
for (const name of ['args', 'budget', 'agent', 'workflow', 'log', 'phase', 'parallel', 'pipeline']) {
  try { globalThis[name] = null; } catch { mutationBlocked.push(name); }
}

export default {
  dateType: typeof Date,
  readonlyType: typeof __readonly,
  randomBlocked: (() => { try { Math.random(); return false; } catch { return true; } })(),
  mutationBlocked,
};
"#,
            "sandbox.workflow.js",
            json!({}),
        ))
        .expect("workflow should start");

    let output = loop {
        match execution.poll().expect("workflow should poll") {
            WorkflowRuntimePoll::Complete(output) => break output,
            WorkflowRuntimePoll::Pending => continue,
            other => panic!("expected completion, got {other:?}"),
        }
    };

    assert_eq!(
        output.result,
        json!({
            "dateType": "undefined",
            "readonlyType": "undefined",
            "randomBlocked": true,
            "mutationBlocked": ["args", "budget", "agent", "workflow", "log", "phase", "parallel", "pipeline"],
        })
    );
}

#[test]
fn rquickjs_budget_global_is_readonly() {
    let mut execution = RQuickJSWorkflowRuntime::new()
        .start_module(WorkflowModuleInput::new(
            r#"
export const meta = { name: "budget-readonly", description: "budget-readonly" };
const blocked = [];
for (const [label, mutate] of [
  ['set-total', () => { budget.total = 1 }],
  ['set-extra', () => { budget.extra = 'nope' }],
  ['delete-spent', () => { delete budget.spent }],
]) {
  try { mutate(); } catch { blocked.push(label); }
}
export default {
  blocked,
  total: budget.total,
  spent: budget.spent(),
  remaining: budget.remaining(),
  extra: budget.extra ?? null,
};
"#,
            "budget-readonly.workflow.js",
            json!({}),
        ))
        .expect("workflow should start");

    let output = loop {
        match execution.poll().expect("workflow should poll") {
            WorkflowRuntimePoll::Complete(output) => break output,
            WorkflowRuntimePoll::Pending => continue,
            other => panic!("expected completion, got {other:?}"),
        }
    };

    assert_eq!(
        output.result,
        json!({
            "blocked": ["set-total", "set-extra", "delete-spent"],
            "total": null,
            "spent": 0,
            "remaining": null,
            "extra": null,
        })
    );
}
