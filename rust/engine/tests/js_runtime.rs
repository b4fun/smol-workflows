use serde_json::json;
use smol_workflow_engine::js_runtime::rquickjs::RquickJsWorkflowRuntime;
use smol_workflow_engine::js_runtime::{WorkflowJsRuntime, WorkflowModuleInput};
use std::fs;

#[test]
fn rquickjs_executes_default_async_workflow_fixture() {
    let source = fs::read_to_string("../../ts/engine/test/fixtures/injected-globals.workflow.js")
        .expect("fixture should exist");
    let output = RquickJsWorkflowRuntime::new()
        .execute_module(WorkflowModuleInput::new(
            source,
            "injected-globals.workflow.js",
            json!({ "my-arg1": "alpha", "my-arg2": "beta" }),
        ))
        .expect("workflow should execute");

    assert_eq!(output.phases[0].name, "Research");
    assert_eq!(
        output.logs[0],
        vec![
            json!("received"),
            json!({ "my-arg1": "alpha", "my-arg2": "beta" })
        ]
    );
    assert_eq!(output.agent_calls.len(), 2);
    assert_eq!(output.result["first"]["echo"], "first: alpha");
    assert_eq!(output.result["second"]["echo"], "second: beta");
    assert_eq!(
        output.result["args"],
        json!({ "my-arg1": "alpha", "my-arg2": "beta" })
    );
}

#[test]
fn rquickjs_executes_top_level_module_result_fixture() {
    let source = fs::read_to_string("../../ts/engine/test/fixtures/module-result.workflow.js")
        .expect("fixture should exist");
    let output = RquickJsWorkflowRuntime::new()
        .execute_module(WorkflowModuleInput::new(
            source,
            "module-result.workflow.js",
            json!({ "my-arg1": "one", "my-arg2": "two" }),
        ))
        .expect("workflow should execute");

    assert_eq!(output.phases[0].name, "ModuleResult");
    assert_eq!(
        output.logs[0],
        vec![
            json!("module result args"),
            json!({ "my-arg1": "one", "my-arg2": "two" })
        ]
    );
    assert_eq!(output.agent_calls.len(), 2);
    assert_eq!(output.result["first"]["echo"], "first: one");
    assert_eq!(output.result["second"]["echo"], "second: two");
    assert_eq!(
        output.result["args"],
        json!({ "my-arg1": "one", "my-arg2": "two" })
    );
}

#[test]
fn rquickjs_blocks_common_host_access_and_randomness() {
    let output = RquickJsWorkflowRuntime::new()
        .execute_module(WorkflowModuleInput::new(
            r#"
export const meta = { name: "sandbox", description: "sandbox" };
export default {
  randomBlocked: (() => { try { Math.random(); return false; } catch { return true; } })(),
  fetchType: typeof fetch,
  requireType: typeof require,
  processType: typeof process,
  evalType: typeof eval,
  functionType: typeof Function,
};
"#,
            "sandbox.workflow.js",
            json!({}),
        ))
        .expect("workflow should execute");

    assert_eq!(output.result["randomBlocked"], true);
    assert_eq!(output.result["fetchType"], "undefined");
    assert_eq!(output.result["requireType"], "undefined");
    assert_eq!(output.result["processType"], "undefined");
    assert_eq!(output.result["evalType"], "undefined");
    assert_eq!(output.result["functionType"], "undefined");
}
