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
                    WorkflowRuntimeRequest::Sleep { .. } => {
                        trace.requests.push(request);
                        execution
                            .resolve_request(&id, WorkflowRuntimeRequestResolution::OkUndefined)?;
                        continue;
                    }
                    WorkflowRuntimeRequest::SandboxExec {
                        profile, request, ..
                    } => json!({
                        "profile": profile,
                        "request": request,
                        "exitCode": 0,
                        "stdout": "sandbox stdout",
                        "stderr": "",
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
fn rquickjs_exception_messages_include_stack_for_module_errors() {
    let source =
        fs::read_to_string("tests/fixtures/stack-error.workflow.js").expect("fixture should exist");

    let error = run_to_completion(WorkflowModuleInput::new(
        source,
        "stack-error.workflow.js",
        json!({}),
    ))
    .unwrap_err();
    let error = format!("{error:#}");

    assert!(
        error.contains("workflow module evaluation rejected: boom from module"),
        "unexpected error: {error}"
    );
    assert!(
        error.contains("at fail (stack-error.workflow.js:"),
        "expected stack frame in error: {error}"
    );
}

#[test]
fn rquickjs_exception_messages_include_stack_for_default_function_errors() {
    let source = fs::read_to_string("tests/fixtures/function-stack-error.workflow.js")
        .expect("fixture should exist");

    let error = run_to_completion(WorkflowModuleInput::new(
        source,
        "function-stack-error.workflow.js",
        json!({}),
    ))
    .unwrap_err();
    let error = format!("{error:#}");

    assert!(
        error.contains("workflow module rejected: boom from function"),
        "unexpected error: {error}"
    );
    assert!(
        error.contains("at fail (function-stack-error.workflow.js:"),
        "expected stack frame in error: {error}"
    );
}

#[test]
fn rquickjs_exception_messages_render_thrown_strings() {
    let source = fs::read_to_string("tests/fixtures/string-error.workflow.js")
        .expect("fixture should exist");

    let error = run_to_completion(WorkflowModuleInput::new(
        source,
        "string-error.workflow.js",
        json!({}),
    ))
    .unwrap_err();
    let error = format!("{error:#}");

    assert!(
        error.contains("workflow module evaluation rejected: boom from string"),
        "unexpected error: {error}"
    );
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
fn rquickjs_exposes_workflow_extra_sleep_request() {
    let mut execution = RQuickJSWorkflowRuntime::new()
        .start_module(WorkflowModuleInput::new(
            r#"
import { sleep } from "workflow:extra";
export const meta = { name: "sleep", description: "sleep" };
const globalSleepType = typeof SW.extra.sleep;
const genericExtraType = typeof extra;
const value = await sleep(12);
export default { valueType: typeof value, globalSleepType, genericExtraType };
"#,
            "sleep.workflow.js",
            json!({}),
        ))
        .expect("workflow should start");

    let request = loop {
        match execution.poll().expect("workflow should poll") {
            WorkflowRuntimePoll::Request(request) => break request,
            WorkflowRuntimePoll::Pending => continue,
            other => panic!("expected sleep request, got {other:?}"),
        }
    };

    let id = match request {
        WorkflowRuntimeRequest::Sleep { id, duration_ms } => {
            assert_eq!(duration_ms, 12);
            id
        }
        other => panic!("expected sleep request, got {other:?}"),
    };

    execution
        .resolve_request(&id, WorkflowRuntimeRequestResolution::OkUndefined)
        .expect("sleep should resolve");

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
            "valueType": "undefined",
            "globalSleepType": "function",
            "genericExtraType": "undefined",
        })
    );
}

#[test]
fn rquickjs_exposes_workflow_sandbox_exec_request() {
    let source = fs::read_to_string("tests/assets/js_runtime/sandbox-exec.workflow.js")
        .expect("fixture should exist");
    let (output, trace) = run_to_completion(WorkflowModuleInput::new(
        source,
        "sandbox-exec.workflow.js",
        json!({}),
    ))
    .expect("workflow should execute");

    assert_eq!(trace.requests.len(), 1);
    match &trace.requests[0] {
        WorkflowRuntimeRequest::SandboxExec {
            profile, request, ..
        } => {
            assert_eq!(profile, "exe-dev/default");
            assert_eq!(request.command, "sh");
            assert_eq!(request.args, vec!["-lc", "pwd"]);
            assert_eq!(request.cwd.as_deref(), Some("/workspace"));
            assert_eq!(request.env.get("EXAMPLE").map(String::as_str), Some("1"));
            assert_eq!(request.stdin.as_deref(), Some("hello"));
        }
        other => panic!("expected sandbox exec request, got {other:?}"),
    }
    assert_eq!(
        output.result,
        json!({
            "value": {
                "profile": "exe-dev/default",
                "request": {
                    "command": "sh",
                    "args": ["-lc", "pwd"],
                    "cwd": "/workspace",
                    "env": { "EXAMPLE": "1" },
                    "stdin": "hello",
                },
                "exitCode": 0,
                "stdout": "sandbox stdout",
                "stderr": "",
            },
            "globalSandboxExecType": "function",
            "genericSandboxType": "undefined",
        })
    );
}

#[test]
fn rquickjs_exposes_extra_on_workflow_context_without_global_extra() {
    let (output, _trace) = run_to_completion(WorkflowModuleInput::new(
        r#"
export const meta = { name: "ctx-extra", description: "ctx extra" };
export default async function workflow(input, ctx) {
  const value = await ctx.extra.sleep(1);
  return {
    valueType: typeof value,
    ctxExtraSleepType: typeof ctx.extra.sleep,
    ctxSwType: typeof ctx.SW,
    globalExtraType: typeof extra,
  };
}
"#,
        "ctx-extra.workflow.js",
        json!({}),
    ))
    .expect("workflow should execute");

    assert_eq!(
        output.result,
        json!({
            "valueType": "undefined",
            "ctxExtraSleepType": "function",
            "ctxSwType": "undefined",
            "globalExtraType": "undefined",
        })
    );
}

#[test]
fn rquickjs_validates_workflow_extra_sleep_bounds() {
    fn expect_sleep_rejection(source: &str, max_sleep_ms: u64, expected: &str) {
        let mut execution = RQuickJSWorkflowRuntime::new()
            .with_max_sleep_ms(max_sleep_ms)
            .start_module(WorkflowModuleInput::new(
                source,
                "sleep-invalid.workflow.js",
                json!({}),
            ))
            .expect("workflow should start");

        for _ in 0..20 {
            match execution.poll() {
                Ok(WorkflowRuntimePoll::Pending) => continue,
                Ok(other) => panic!("expected sleep validation rejection, got {other:?}"),
                Err(error) => {
                    assert!(
                        format!("{error:#}").contains(expected),
                        "unexpected error: {error:#}"
                    );
                    return;
                }
            }
        }
        panic!("workflow did not reject invalid sleep within poll limit");
    }

    expect_sleep_rejection(
        r#"
import { sleep } from "workflow:extra";
export const meta = { name: "sleep-invalid", description: "sleep-invalid" };
export default await sleep(11);
"#,
        10,
        "duration exceeds the maximum",
    );
    expect_sleep_rejection(
        r#"
import { sleep } from "workflow:extra";
export const meta = { name: "sleep-invalid", description: "sleep-invalid" };
export default await sleep(-1);
"#,
        10,
        "finite non-negative number",
    );
    expect_sleep_rejection(
        r#"
import { sleep } from "workflow:extra";
export const meta = { name: "sleep-invalid", description: "sleep-invalid" };
export default await sleep(Infinity);
"#,
        10,
        "finite non-negative number",
    );
}

#[test]
fn rquickjs_blocks_non_extra_imports() {
    let result = RQuickJSWorkflowRuntime::new().start_module(WorkflowModuleInput::new(
        r#"
import fs from "node:fs";
export const meta = { name: "blocked", description: "blocked" };
export default fs;
"#,
        "blocked.workflow.js",
        json!({}),
    ));
    let error = match result {
        Ok(_) => panic!("workflow should reject non-extra imports"),
        Err(error) => error,
    };

    assert!(
        format!("{error:#}").contains("workflow imports are restricted"),
        "unexpected error: {error:#}"
    );
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
for (const name of ['args', 'budget', 'agent', 'workflow', 'log', 'phase', 'parallel', 'pipeline', 'SW']) {
  try { globalThis[name] = null; } catch { mutationBlocked.push(name); }
}

export default {
  dateType: typeof Date,
  readonlyType: typeof __readonly,
  extraType: typeof extra,
  swExtraSleepType: typeof SW.extra.sleep,
  swExtraMutationBlocked: (() => { try { SW.extra.sleep = null; return false; } catch { return true; } })(),
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
            "extraType": "undefined",
            "swExtraSleepType": "function",
            "swExtraMutationBlocked": true,
            "randomBlocked": true,
            "mutationBlocked": ["args", "budget", "agent", "workflow", "log", "phase", "parallel", "pipeline", "SW"],
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
