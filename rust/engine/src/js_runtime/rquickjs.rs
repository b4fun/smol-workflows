//! QuickJS-backed implementation of the workflow JavaScript runtime boundary.

use super::{ImportPolicy, WorkflowJsRuntime, WorkflowModuleInput, WorkflowModuleOutput};
use anyhow::{anyhow, bail, Context as AnyhowContext};
use rquickjs::{
    context::intrinsic, promise::PromiseState, CatchResultExt, Context, Promise, Runtime,
};
use std::time::Instant;

type WorkflowIntrinsics = (
    intrinsic::Eval,
    intrinsic::Json,
    intrinsic::Promise,
    intrinsic::Proxy,
    intrinsic::MapSet,
    intrinsic::RegExp,
);

/// Workflow JavaScript runtime backed by QuickJS via `rquickjs`.
#[derive(Debug, Default, Clone, Copy)]
pub struct RquickJsWorkflowRuntime;

impl RquickJsWorkflowRuntime {
    pub fn new() -> Self {
        Self
    }
}

impl WorkflowJsRuntime for RquickJsWorkflowRuntime {
    fn execute_module(&self, input: WorkflowModuleInput) -> anyhow::Result<WorkflowModuleOutput> {
        if input.sandbox.import_policy != ImportPolicy::DenyAll {
            bail!("unsupported workflow import policy");
        }

        let runtime = Runtime::new().context("failed to create QuickJS runtime")?;
        runtime.set_memory_limit(input.sandbox.memory_limit_bytes);
        runtime.set_max_stack_size(input.sandbox.max_stack_size_bytes);

        let deadline = Instant::now() + input.sandbox.timeout;
        runtime.set_interrupt_handler(Some(Box::new(move || Instant::now() >= deadline)));

        let context = Context::custom::<WorkflowIntrinsics>(&runtime)
            .context("failed to create restricted QuickJS context")?;

        context.with(|ctx| -> anyhow::Result<WorkflowModuleOutput> {
            let args_json = serde_json::to_string(&input.args)
                .context("failed to serialize workflow args")?;
            let args_json_literal = serde_json::to_string(&args_json)
                .context("failed to quote workflow args JSON")?;
            let transformed = transform_workflow_module(&input.source);
            let wrapped = format!(
                r#"
globalThis.args = JSON.parse({args_json_literal});
{SANDBOX_PRELUDE}
const __workflowCtx = {{ args, agent, parallel, pipeline, log, phase }};
globalThis.__default = undefined;
(async () => {{
{transformed}
  if (typeof globalThis.__default === 'function') {{
    globalThis.__default = await globalThis.__default(args, __workflowCtx);
  }}
}})()
"#,
            );

            let promise: Promise<'_> = ctx
                .eval(wrapped.into_bytes())
                .catch(&ctx)
                .map_err(|error| anyhow!("failed to evaluate {}: {error:?}", input.source_name))?;

            while ctx.execute_pending_job() {}

            match promise.state() {
                PromiseState::Rejected => bail!("workflow module promise rejected"),
                PromiseState::Pending => bail!("workflow module promise did not settle"),
                PromiseState::Resolved => {}
            }

            let output_json: String = ctx
                .eval(
                    r#"JSON.stringify({ result: globalThis.__default, logs: globalThis.__logs, phases: globalThis.__phases, agentCalls: globalThis.__agentCalls })"#,
                )
                .catch(&ctx)
                .map_err(|error| anyhow!("failed to serialize workflow output: {error:?}"))?;

            serde_json::from_str(&output_json)
                .map_err(|error| anyhow!("failed to parse workflow output JSON: {error}"))
        })
    }
}

fn transform_workflow_module(source: &str) -> String {
    source
        .replace("export const meta", "const meta")
        .replace(
            "export default async function",
            "globalThis.__default = async function",
        )
        .replace("export default function", "globalThis.__default = function")
        .replace("export default", "globalThis.__default =")
}

const SANDBOX_PRELUDE: &str = r#"
globalThis.__logs = [];
globalThis.__phases = [];
globalThis.__agentCalls = [];

function __readonly(value) {
  if (typeof value !== 'object' || value === null) return value;
  return new Proxy(value, {
    get(target, property, receiver) { return __readonly(Reflect.get(target, property, receiver)); },
    set() { throw new TypeError('Cannot modify workflow args'); },
    defineProperty() { throw new TypeError('Cannot modify workflow args'); },
    deleteProperty() { throw new TypeError('Cannot modify workflow args'); },
    setPrototypeOf() { throw new TypeError('Cannot modify workflow args'); },
  });
}

function __defineWorkflowGlobal(name, value) {
  Object.defineProperty(globalThis, name, {
    value,
    writable: false,
    configurable: false,
    enumerable: true,
  });
}

globalThis.args = __readonly(globalThis.args);

async function __agent(prompt, options) {
  const call = { prompt };
  if (options !== undefined) call.options = options;
  globalThis.__agentCalls.push(call);
  const result = { echo: prompt };
  if (options !== undefined) result.options = options;
  return result;
}

async function __parallel(tasks) {
  return await Promise.all(tasks.map(async (task) => {
    try { return await task(); } catch { return null; }
  }));
}

async function __pipeline(items, ...stages) {
  return await Promise.all(items.map(async (item, index) => {
    let previous = item;
    for (const stage of stages) {
      try { previous = await stage(previous, item, index); } catch { return null; }
    }
    return previous;
  }));
}

function __log(...values) { globalThis.__logs.push(values); }
function __phase(name, options) {
  const event = { name };
  if (options !== undefined) event.options = options;
  globalThis.__phases.push(event);
}

Object.defineProperty(Math, 'random', {
  value() { throw new TypeError('Math.random is disabled in smol workflow sandbox'); },
  writable: false,
  configurable: false,
});
Object.freeze(Math);

for (const name of [
  'eval',
  'Function',
  'AsyncFunction',
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
]) {
  Object.defineProperty(globalThis, name, {
    value: undefined,
    writable: false,
    configurable: false,
  });
}

__defineWorkflowGlobal('agent', __agent);
__defineWorkflowGlobal('parallel', __parallel);
__defineWorkflowGlobal('pipeline', __pipeline);
__defineWorkflowGlobal('log', __log);
__defineWorkflowGlobal('phase', __phase);
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::js_runtime::WorkflowModuleInput;
    use serde_json::json;

    #[test]
    fn transforms_default_export_object() {
        let output = RquickJsWorkflowRuntime::new()
            .execute_module(WorkflowModuleInput::new(
                r#"
export const meta = { name: "inline", description: "inline" };
export default { ok: true, args };
"#,
                "inline.workflow.js",
                json!({ "value": 1 }),
            ))
            .expect("workflow should execute");

        assert_eq!(output.result, json!({ "ok": true, "args": { "value": 1 } }));
    }
}
