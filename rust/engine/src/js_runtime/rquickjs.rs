//! QuickJS-backed implementation of the workflow JavaScript runtime boundary.
//!
//! Runtime overview:
//!
//! 1. Create a restricted QuickJS context with only the intrinsics needed by the
//!    workflow sandbox (`Promise`, `Proxy`, JSON, collections, regexps, etc.).
//!    QuickJS's eval intrinsic is still required for host-owned source/module
//!    evaluation, but user-visible dynamic evaluation is disabled before user
//!    workflow code runs. Node/browser/host globals are not provided and a second
//!    hardening pass replaces or hides known escape hatches such as `eval`,
//!    `Function`, `Date`,
//!    host IO globals, and `Math.random`.
//! 2. Evaluate `sandbox_prelude.js`. The prelude intentionally contains only the
//!    small JS-native pieces that are easier to express in JavaScript: the
//!    readonly `Proxy` factory plus pure helper globals like `parallel` and
//!    `pipeline`. Rust captures the temporary `__readonly` helper and removes it
//!    before user workflow code runs.
//! 3. Install Rust-owned workflow globals (`args`, `budget`, `agent`,
//!    `workflow`, `log`, and `phase`). Rust exposes two different kinds of
//!    protection and both are required:
//!
//!    - Global/property binding protection is done with
//!      `define_readonly_data_property`, which defines non-writable,
//!      non-configurable properties. Use this for public globals, hidden
//!      bootstrap helpers, disabled host globals, and host object properties like
//!      `Math.random`.
//!    - Object/value mutation protection is done with the captured readonly
//!      proxy. Use this for mutable-looking objects exposed from Rust, such as
//!      `args` and `budget`, so nested writes, new properties, deletes, and
//!      prototype changes throw instead of mutating the underlying object.
//!
//!    A protected global binding alone is not enough for object values: it stops
//!    `globalThis.args = ...`, but not `args.nested.value = ...`. Therefore any
//!    object/array exposed from Rust that should be immutable to workflow code
//!    must be wrapped with `readonly_proxy` before it is installed or passed to
//!    user code.
//!
//!    `agent(...)` and `workflow(...)` do not perform provider work inside
//!    QuickJS; they create pending JS promises, enqueue Rust-side requests, and
//!    save the JS resolve/reject functions for later.
//! 4. Declare and evaluate the user source as a real ES module. Module
//!    evaluation is represented by a QuickJS promise so top-level `await` is
//!    naturally supported. Literal top-level `return` is not supported because
//!    the source is parsed as ESM.
//! 5. Once module evaluation resolves, Rust reads the module namespace and starts
//!    the `default` export. Function defaults are called as
//!    `default(args, ctx)`, where `ctx` contains the workflow helpers. Value or
//!    promise defaults are used directly. Rust normalizes both forms into a
//!    single workflow promise.
//! 6. Polling drains the QuickJS job queue, emits queued calls/requests to the
//!    workflow core, and completes when the workflow promise resolves. The core
//!    later resolves requests through `resolve_request`, which resumes the saved
//!    JS promises by calling their captured resolve/reject functions.

use super::{
    ImportPolicy, WorkflowJSRuntime, WorkflowModuleInput, WorkflowModuleOutput,
    WorkflowRuntimeCall, WorkflowRuntimeExecution, WorkflowRuntimePoll, WorkflowRuntimeRequest,
    WorkflowRuntimeRequestResolution,
};
use anyhow::{anyhow, bail, Context as AnyhowContext};
use rquickjs::{
    context::intrinsic,
    object::{Accessor, Property},
    prelude::{Func, MutFn, Opt, Rest},
    promise::PromiseState,
    CatchResultExt, CaughtError, Context, Exception, Function, Module, Object, Persistent, Promise,
    Runtime, Undefined, Value,
};
use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    rc::Rc,
    time::Instant,
};

type WorkflowIntrinsics = (
    intrinsic::Eval,
    intrinsic::Json,
    intrinsic::Promise,
    intrinsic::Proxy,
    intrinsic::MapSet,
    intrinsic::RegExp,
);

const BLOCKED_GLOBALS: &[&str] = &[
    "eval",
    "Function",
    "AsyncFunction",
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
];

const INTERNAL_GLOBALS: &[&str] = &["__readonly"];

/// Workflow JavaScript runtime backed by QuickJS via `rquickjs`.
#[derive(Debug, Default, Clone, Copy)]
pub struct RQuickJSWorkflowRuntime;

impl RQuickJSWorkflowRuntime {
    pub fn new() -> Self {
        Self
    }
}

impl WorkflowJSRuntime for RQuickJSWorkflowRuntime {
    fn start_module(
        &self,
        input: WorkflowModuleInput,
    ) -> anyhow::Result<Box<dyn WorkflowRuntimeExecution>> {
        log::debug!(
            "quickjs start_module source={} args_type={} budget_total={:?} budget_spent={}",
            input.source_name,
            json_value_type(&input.args),
            input.budget.total,
            input.budget.spent
        );
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

        let mut execution = RQuickJSWorkflowExecution {
            state: Rc::new(RefCell::new(RuntimeState::default())),
            module_namespace: None,
            module_eval_promise: None,
            workflow_promise: None,
            readonly: None,
            context,
            runtime,
        };
        execution.start(input)?;
        Ok(Box::new(execution))
    }
}

struct RQuickJSWorkflowExecution {
    // State and persistent JS values must be dropped before the context/runtime.
    state: Rc<RefCell<RuntimeState>>,
    module_namespace: Option<Persistent<Object<'static>>>,
    module_eval_promise: Option<Persistent<Promise<'static>>>,
    workflow_promise: Option<Persistent<Promise<'static>>>,
    readonly: Option<Persistent<Function<'static>>>,
    context: Context,
    #[allow(dead_code)]
    runtime: Runtime,
}

#[derive(Default)]
struct RuntimeState {
    calls: VecDeque<WorkflowRuntimeCall>,
    requests: VecDeque<WorkflowRuntimeRequest>,
    pending_requests: HashMap<String, PendingRequest>,
    next_request_id: u64,
    current_phase: Option<String>,
    budget: super::WorkflowBudgetSnapshot,
}

#[derive(Clone)]
struct PendingRequest {
    resolve: Persistent<Function<'static>>,
    reject: Persistent<Function<'static>>,
}

impl RQuickJSWorkflowExecution {
    fn start(&mut self, input: WorkflowModuleInput) -> anyhow::Result<()> {
        let context = self.context.clone();
        context.with(|ctx| -> anyhow::Result<()> {
            evaluate_sandbox_prelude(&ctx)?;

            let RuntimeGlobals {
                source_name,
                source,
                readonly,
            } = install_runtime_globals(&ctx, input, Rc::clone(&self.state))?;
            self.readonly = Some(readonly);
            self.evaluate_module(&ctx, source_name, source)?;
            Ok(())
        })
    }

    fn evaluate_module(
        &mut self,
        ctx: &rquickjs::Ctx<'_>,
        source_name: String,
        source: String,
    ) -> anyhow::Result<()> {
        log::debug!("quickjs evaluate_module source={source_name}");
        let module = Module::declare(ctx.clone(), source_name, source)
            .catch(ctx)
            .map_err(|error| anyhow!("failed to declare workflow module: {error:?}"))?;
        let (module, promise) = module
            .eval()
            .catch(ctx)
            .map_err(|error| anyhow!("failed to evaluate workflow module: {error:?}"))?;
        let namespace = module
            .namespace()
            .context("failed to get workflow module namespace")?;

        self.module_namespace = Some(Persistent::save(ctx, namespace));
        self.module_eval_promise = Some(Persistent::save(ctx, promise));
        Ok(())
    }

    fn drain_jobs(&self) {
        self.context.with(|ctx| while ctx.execute_pending_job() {});
    }
}

impl WorkflowRuntimeExecution for RQuickJSWorkflowExecution {
    fn poll(&mut self) -> anyhow::Result<WorkflowRuntimePoll> {
        // TODO(async): expose all newly queued long-running requests in a batch so
        // `parallel([...agent(...)])` can run providers concurrently instead of
        // requiring the core to resolve one request before observing the next.
        // TODO(polling): document/enforce that callers should not repeatedly poll
        // while an unresolved request is outstanding; otherwise the same request
        // can be observed more than once.
        self.drain_jobs();

        let context = self.context.clone();
        context.with(|ctx| -> anyhow::Result<WorkflowRuntimePoll> {
            if let Some(call) = self.state.borrow_mut().calls.pop_front() {
                return Ok(WorkflowRuntimePoll::Call(call));
            }

            if let Some(request) = self.state.borrow().requests.front().cloned() {
                return Ok(WorkflowRuntimePoll::Request(request));
            }

            if self.workflow_promise.is_none() {
                match self.module_eval_state(&ctx)? {
                    PromiseState::Pending => return Ok(WorkflowRuntimePoll::Pending),
                    PromiseState::Rejected => {
                        bail!(
                            "workflow module evaluation rejected: {}",
                            self.module_eval_rejection_message(&ctx)
                        )
                    }
                    PromiseState::Resolved => self.start_default_export(&ctx)?,
                }
            }

            self.poll_workflow_promise(&ctx)
        })
    }

    fn take_pending_requests(&mut self) -> anyhow::Result<Vec<WorkflowRuntimeRequest>> {
        self.drain_jobs();
        Ok(self.state.borrow_mut().requests.drain(..).collect())
    }

    fn resolve_request(
        &mut self,
        id: &str,
        resolution: WorkflowRuntimeRequestResolution,
    ) -> anyhow::Result<()> {
        let resolution_json = match resolution {
            WorkflowRuntimeRequestResolution::Ok(value) => serde_json::json!({
                "ok": true,
                "value": value,
            }),
            WorkflowRuntimeRequestResolution::OkWithBudget { value, budget } => {
                self.state.borrow_mut().budget = budget;
                serde_json::json!({
                    "ok": true,
                    "value": value,
                })
            }
            WorkflowRuntimeRequestResolution::Err { message } => serde_json::json!({
                "ok": false,
                "message": message,
            }),
        };

        self.context.with(|ctx| -> anyhow::Result<()> {
            let pending = self
                .state
                .borrow()
                .pending_requests
                .get(id)
                .cloned()
                .ok_or_else(|| anyhow!("unknown workflow request id: {id}"))?;
            let resolution = rquickjs_serde::to_value(ctx.clone(), &resolution_json)
                .context("failed to convert workflow request resolution to QuickJS value")?;
            let resolution_object: Object<'_> = resolution
                .as_object()
                .cloned()
                .ok_or_else(|| anyhow!("request resolution was not an object"))?;
            let ok = resolution_object
                .get::<_, bool>("ok")
                .context("failed to read request resolution status")?;

            let resolved = if ok {
                let value: Value<'_> = resolution_object
                    .get("value")
                    .context("failed to read request resolution value")?;
                let resolve = pending
                    .resolve
                    .restore(&ctx)
                    .context("failed to restore request resolver")?;
                resolve
                    .call::<_, ()>((value,))
                    .catch(&ctx)
                    .map_err(|error| anyhow!("failed to resolve workflow request: {error:?}"))
            } else {
                let message = resolution_object
                    .get::<_, String>("message")
                    .unwrap_or_else(|_| "workflow request rejected".to_string());
                let error_constructor: Function = ctx
                    .globals()
                    .get("Error")
                    .context("failed to get Error constructor")?;
                let error_value: Value<'_> = error_constructor
                    .call((message,))
                    .catch(&ctx)
                    .map_err(|error| {
                        anyhow!("failed to construct request rejection error: {error:?}")
                    })?;
                let reject = pending
                    .reject
                    .restore(&ctx)
                    .context("failed to restore request rejecter")?;
                reject
                    .call::<_, ()>((error_value,))
                    .catch(&ctx)
                    .map_err(|error| anyhow!("failed to reject workflow request: {error:?}"))
            };

            if resolved.is_ok() {
                let mut state = self.state.borrow_mut();
                state.pending_requests.remove(id);
                state.requests.retain(|request| request.id() != id);
            }

            resolved
        })
    }
}

impl RQuickJSWorkflowExecution {
    fn module_eval_state(&self, ctx: &rquickjs::Ctx<'_>) -> anyhow::Result<PromiseState> {
        let promise = self
            .module_eval_promise
            .clone()
            .ok_or_else(|| anyhow!("workflow module evaluation was not started"))?
            .restore(ctx)
            .context("failed to restore workflow module evaluation promise")?;
        Ok(promise.state())
    }

    fn module_eval_rejection_message(&self, ctx: &rquickjs::Ctx<'_>) -> String {
        if let Some(promise) = self
            .module_eval_promise
            .clone()
            .and_then(|promise| promise.restore(ctx).ok())
        {
            let _ = promise.result::<Value<'_>>();
        }
        js_exception_message(ctx)
    }

    fn start_default_export(&mut self, ctx: &rquickjs::Ctx<'_>) -> anyhow::Result<()> {
        let namespace = self
            .module_namespace
            .clone()
            .ok_or_else(|| anyhow!("workflow module namespace is missing"))?
            .restore(ctx)
            .context("failed to restore workflow module namespace")?;
        if !namespace
            .contains_key("default")
            .context("failed to inspect workflow module default export")?
        {
            bail!("workflow module must default export a workflow result or function");
        }
        let default_export: Value<'_> = namespace
            .get("default")
            .context("workflow module must default export a workflow result or function")?;
        let promise = start_default_export(ctx, default_export)
            .context("failed to start workflow default export")?;
        self.workflow_promise = Some(Persistent::save(ctx, promise));
        Ok(())
    }

    fn poll_workflow_promise(
        &self,
        ctx: &rquickjs::Ctx<'_>,
    ) -> anyhow::Result<WorkflowRuntimePoll> {
        let promise = self
            .workflow_promise
            .clone()
            .ok_or_else(|| anyhow!("workflow default execution was not started"))?
            .restore(ctx)
            .context("failed to restore workflow promise")?;

        match promise.state() {
            PromiseState::Pending => Ok(WorkflowRuntimePoll::Pending),
            PromiseState::Rejected => {
                let result = promise.result::<Value<'_>>();
                bail!("workflow module rejected: {result:?}")
            }
            PromiseState::Resolved => {
                let result = promise
                    .result::<Value<'_>>()
                    .ok_or_else(|| anyhow!("workflow promise resolved without a result"))?
                    .catch(ctx)
                    .map_err(|error| anyhow!("failed to read workflow result: {error:?}"))?;
                let result = rquickjs_serde::from_value::<serde_json::Value>(result)
                    .context("failed to convert workflow result from QuickJS value")?;
                Ok(WorkflowRuntimePoll::Complete(WorkflowModuleOutput {
                    result,
                }))
            }
        }
    }
}

fn json_value_type(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

fn evaluate_sandbox_prelude(ctx: &rquickjs::Ctx<'_>) -> anyhow::Result<()> {
    let module = Module::declare(
        ctx.clone(),
        "smol:workflow-sandbox-prelude".to_string(),
        include_str!("rquickjs_js/sandbox_prelude.js").to_string(),
    )
    .catch(ctx)
    .map_err(|error| anyhow!("failed to declare sandbox prelude: {error:?}"))?;
    let (_module, promise) = module
        .eval()
        .catch(ctx)
        .map_err(|error| anyhow!("failed to evaluate sandbox prelude: {error:?}"))?;

    while promise.state() == PromiseState::Pending {
        if !ctx.execute_pending_job() {
            return Ok(());
        }
    }

    if promise.state() == PromiseState::Rejected {
        let _ = promise.result::<Value<'_>>();
        bail!("sandbox prelude rejected: {}", js_exception_message(ctx));
    }

    Ok(())
}

fn js_exception_message(ctx: &rquickjs::Ctx<'_>) -> String {
    let error = ctx.catch();
    if let Some(object) = error.as_object() {
        if let Ok(message) = object.get::<_, String>("message") {
            if !message.is_empty() {
                return message;
            }
        }
        if let Ok(stack) = object.get::<_, String>("stack") {
            if !stack.is_empty() {
                return stack;
            }
        }
    }
    format!("{error:?}")
}

fn install_runtime_globals<'js>(
    ctx: &rquickjs::Ctx<'js>,
    input: WorkflowModuleInput,
    state: Rc<RefCell<RuntimeState>>,
) -> anyhow::Result<RuntimeGlobals> {
    let globals = ctx.globals();

    let WorkflowModuleInput {
        source,
        source_name,
        args,
        budget,
        sandbox: _,
    } = input;

    state.borrow_mut().budget = budget;

    let args = rquickjs_serde::to_value(ctx.clone(), &args)
        .context("failed to convert workflow args to QuickJS value")?;
    let readonly: Function = globals
        .get("__readonly")
        .context("failed to get readonly helper")?;
    let readonly = Persistent::save(ctx, readonly);
    let readonly_args =
        readonly_proxy(ctx, &readonly, args).context("failed to wrap workflow args as readonly")?;
    globals
        .prop("args", Property::from(readonly_args).enumerable())
        .context("failed to install readonly workflow args global")?;

    let budget = create_budget_object(ctx, Rc::clone(&state))?;
    let budget = readonly_proxy(ctx, &readonly, budget.into())
        .context("failed to wrap workflow budget as readonly")?;
    globals
        .prop("budget", Property::from(budget).enumerable())
        .context("failed to install workflow budget global")?;

    install_native_workflow_functions(&globals, state)?;
    harden_public_js_helpers(&globals)?;

    harden_workflow_sandbox(ctx, &globals)?;
    hide_internal_globals(&globals);

    Ok(RuntimeGlobals {
        source_name,
        source,
        readonly,
    })
}

struct RuntimeGlobals {
    source_name: String,
    source: String,
    readonly: Persistent<Function<'static>>,
}

fn start_default_export<'js>(
    ctx: &rquickjs::Ctx<'js>,
    default_export: Value<'js>,
) -> anyhow::Result<Promise<'js>> {
    let globals = ctx.globals();
    let result = if let Some(default_function) = default_export.as_function().cloned() {
        let args: Value<'js> = globals
            .get("args")
            .context("failed to get workflow args global")?;
        let workflow_context = create_workflow_context_object(ctx, &globals)?;
        default_function
            .call::<_, Value<'js>>((args, workflow_context))
            .catch(ctx)
    } else {
        Ok(default_export)
    };

    let (promise, resolve, reject) =
        Promise::new(ctx).context("failed to create workflow promise")?;
    match result {
        Ok(value) => resolve
            .call::<_, ()>((value,))
            .catch(ctx)
            .map_err(|error| anyhow!("failed to resolve workflow promise: {error:?}"))?,
        Err(CaughtError::Exception(error)) => reject
            .call::<_, ()>((error.into_value(),))
            .catch(ctx)
            .map_err(|error| anyhow!("failed to reject workflow promise: {error:?}"))?,
        Err(CaughtError::Value(error)) => reject
            .call::<_, ()>((error,))
            .catch(ctx)
            .map_err(|error| anyhow!("failed to reject workflow promise: {error:?}"))?,
        Err(CaughtError::Error(error)) => {
            return Err(anyhow!("failed to call workflow default export: {error:?}"));
        }
    }
    Ok(promise)
}

fn create_workflow_context_object<'js>(
    ctx: &rquickjs::Ctx<'js>,
    globals: &Object<'js>,
) -> anyhow::Result<Object<'js>> {
    let workflow_context = Object::new(ctx.clone()).context("failed to create workflow context")?;
    for name in [
        "args", "agent", "parallel", "pipeline", "workflow", "budget", "log", "phase",
    ] {
        let value: Value<'js> = globals
            .get(name)
            .with_context(|| format!("failed to get workflow context value {name}"))?;
        workflow_context
            .prop(name, Property::from(value).enumerable())
            .with_context(|| format!("failed to install workflow context value {name}"))?;
    }
    Ok(workflow_context)
}

fn readonly_proxy<'js>(
    ctx: &rquickjs::Ctx<'js>,
    readonly: &Persistent<Function<'static>>,
    value: Value<'js>,
) -> anyhow::Result<Value<'js>> {
    let readonly = readonly
        .clone()
        .restore(ctx)
        .context("failed to restore readonly proxy helper")?;
    readonly
        .call((value,))
        .catch(ctx)
        .map_err(|error| anyhow!("failed to create readonly proxy: {error:?}"))
}

fn harden_public_js_helpers<'js>(globals: &Object<'js>) -> anyhow::Result<()> {
    for name in ["parallel", "pipeline"] {
        let value: Value<'_> = globals
            .get(name)
            .with_context(|| format!("failed to get JS workflow helper {name}"))?;
        define_readonly_data_property(globals.ctx(), globals, name, value, true)
            .with_context(|| format!("failed to harden JS workflow helper {name}"))?;
    }
    Ok(())
}

fn harden_workflow_sandbox<'js>(
    ctx: &rquickjs::Ctx<'js>,
    globals: &Object<'js>,
) -> anyhow::Result<()> {
    let math: Object<'_> = globals.get("Math").context("failed to get Math global")?;
    let random = Function::new(
        ctx.clone(),
        |ctx: rquickjs::Ctx<'_>| -> rquickjs::Result<()> {
            Err(Exception::throw_message(
                &ctx,
                "Math.random is disabled in smol workflow sandbox",
            ))
        },
    )
    .context("failed to create disabled Math.random function")?;
    define_readonly_data_property(ctx, &math, "random", random.into_value(), false)
        .context("failed to replace Math.random")?;

    for name in BLOCKED_GLOBALS {
        define_readonly_data_property(ctx, globals, name, Undefined.into_value(ctx.clone()), false)
            .with_context(|| format!("failed to block workflow global {name}"))?;
    }

    Ok(())
}

fn hide_internal_globals<'js>(globals: &Object<'js>) {
    for name in INTERNAL_GLOBALS {
        let _ = define_readonly_data_property(
            globals.ctx(),
            globals,
            name,
            Undefined.into_value(globals.ctx().clone()),
            false,
        );
    }
}

fn define_readonly_data_property<'js>(
    ctx: &rquickjs::Ctx<'js>,
    target: &Object<'js>,
    name: &str,
    value: Value<'js>,
    enumerable: bool,
) -> anyhow::Result<()> {
    let descriptor = Object::new(ctx.clone()).context("failed to create property descriptor")?;
    descriptor
        .set("value", value)
        .context("failed to set property descriptor value")?;
    descriptor
        .set("writable", false)
        .context("failed to set property descriptor writable flag")?;
    descriptor
        .set("configurable", false)
        .context("failed to set property descriptor configurable flag")?;
    descriptor
        .set("enumerable", enumerable)
        .context("failed to set property descriptor enumerable flag")?;

    let object: Object<'js> = ctx
        .globals()
        .get("Object")
        .context("failed to get Object")?;
    let define_property: Function<'js> = object
        .get("defineProperty")
        .context("failed to get Object.defineProperty")?;
    define_property
        .call::<_, ()>((target.clone(), name, descriptor))
        .catch(ctx)
        .map_err(|error| anyhow!("Object.defineProperty failed for {name}: {error:?}"))
}

fn create_budget_object<'js>(
    ctx: &rquickjs::Ctx<'js>,
    state: Rc<RefCell<RuntimeState>>,
) -> anyhow::Result<Object<'js>> {
    let object = Object::new(ctx.clone()).context("failed to create workflow budget object")?;

    let total_state = Rc::clone(&state);
    object
        .prop(
            "total",
            Accessor::from(
                move |ctx: rquickjs::Ctx<'js>| -> rquickjs::Result<Value<'js>> {
                    rquickjs_serde::to_value(ctx, total_state.borrow().budget.total).map_err(
                        |error| rquickjs::Error::IntoJs {
                            from: "WorkflowBudgetSnapshot.total",
                            to: "value",
                            message: Some(error.to_string()),
                        },
                    )
                },
            )
            .enumerable(),
        )
        .context("failed to install workflow budget total")?;

    let spent_state = Rc::clone(&state);
    object
        .prop(
            "spent",
            Property::from(Func::from(move || spent_state.borrow().budget.spent)).enumerable(),
        )
        .context("failed to install workflow budget spent function")?;

    object
        .prop(
            "remaining",
            Property::from(Func::from(move || {
                let budget = &state.borrow().budget;
                match budget.total {
                    Some(total) => total.saturating_sub(budget.spent) as f64,
                    None => f64::INFINITY,
                }
            }))
            .enumerable(),
        )
        .context("failed to install workflow budget remaining function")?;

    Ok(object)
}

fn install_native_workflow_functions<'js>(
    globals: &Object<'js>,
    state: Rc<RefCell<RuntimeState>>,
) -> anyhow::Result<()> {
    let log_state = Rc::clone(&state);
    globals
        .prop(
            "log",
            Property::from(Func::from(MutFn::from(move |values: Rest<Value<'js>>| {
                let values = values
                    .0
                    .into_iter()
                    .map(rquickjs_serde::from_value::<serde_json::Value>)
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|error| rquickjs::Error::FromJs {
                        from: "value",
                        to: "serde_json::Value",
                        message: Some(error.to_string()),
                    })?;
                log_state
                    .borrow_mut()
                    .calls
                    .push_back(WorkflowRuntimeCall::Log { values });
                Ok::<(), rquickjs::Error>(())
            })))
            .enumerable(),
        )
        .context("failed to install workflow log global")?;

    let phase_state = Rc::clone(&state);
    globals
        .prop(
            "phase",
            Property::from(Func::from(MutFn::from(
                move |name: String, options: Opt<Value<'js>>| {
                    let options = match options.0 {
                        Some(value) => Some(
                            rquickjs_serde::from_value::<serde_json::Value>(value).map_err(
                                |error| rquickjs::Error::FromJs {
                                    from: "value",
                                    to: "serde_json::Value",
                                    message: Some(error.to_string()),
                                },
                            )?,
                        ),
                        None => None,
                    };
                    let mut state = phase_state.borrow_mut();
                    state.current_phase = Some(name.clone());
                    state
                        .calls
                        .push_back(WorkflowRuntimeCall::Phase { name, options });
                    Ok::<(), rquickjs::Error>(())
                },
            )))
            .enumerable(),
        )
        .context("failed to install workflow phase global")?;

    let agent_state = Rc::clone(&state);
    globals
        .prop(
            "agent",
            Property::from(Func::from(MutFn::from(
                move |ctx: rquickjs::Ctx<'js>, prompt: String, options: Opt<Value<'js>>| {
                    let options = match options.0 {
                        Some(value) => Some(
                            rquickjs_serde::from_value::<serde_json::Value>(value).map_err(
                                |error| rquickjs::Error::FromJs {
                                    from: "value",
                                    to: "serde_json::Value",
                                    message: Some(error.to_string()),
                                },
                            )?,
                        ),
                        None => None,
                    };
                    create_pending_request(&ctx, &agent_state, |id, state| {
                        let mut options = options.unwrap_or_else(|| serde_json::json!({}));
                        if let Some(current_phase) = state.current_phase.clone() {
                            if options.get("phase").is_none() {
                                options["phase"] = serde_json::Value::String(current_phase);
                            }
                        }
                        let options = if options.as_object().is_some_and(|object| object.is_empty())
                        {
                            None
                        } else {
                            Some(options)
                        };
                        WorkflowRuntimeRequest::Agent {
                            id,
                            prompt,
                            options,
                        }
                    })
                },
            )))
            .enumerable(),
        )
        .context("failed to install workflow agent global")?;

    let workflow_state = Rc::clone(&state);
    globals
        .prop(
            "workflow",
            Property::from(Func::from(MutFn::from(
                move |ctx: rquickjs::Ctx<'js>, workflow_ref: Value<'js>, args: Opt<Value<'js>>| {
                    let workflow_ref = rquickjs_serde::from_value::<super::WorkflowRef>(
                        workflow_ref,
                    )
                    .map_err(|error| rquickjs::Error::FromJs {
                        from: "value",
                        to: "WorkflowRef",
                        message: Some(error.to_string()),
                    })?;
                    let args = match args.0 {
                        Some(value) => Some(
                            rquickjs_serde::from_value::<serde_json::Value>(value).map_err(
                                |error| rquickjs::Error::FromJs {
                                    from: "value",
                                    to: "serde_json::Value",
                                    message: Some(error.to_string()),
                                },
                            )?,
                        ),
                        None => None,
                    };
                    create_pending_request(&ctx, &workflow_state, |id, _state| {
                        WorkflowRuntimeRequest::Workflow {
                            id,
                            workflow_ref,
                            args,
                        }
                    })
                },
            )))
            .enumerable(),
        )
        .context("failed to install workflow child workflow global")?;

    Ok(())
}

fn create_pending_request<'js>(
    ctx: &rquickjs::Ctx<'js>,
    state: &Rc<RefCell<RuntimeState>>,
    make_request: impl FnOnce(String, &mut RuntimeState) -> WorkflowRuntimeRequest,
) -> rquickjs::Result<Promise<'js>> {
    let (promise, resolve, reject) = ctx.promise()?;
    let mut state = state.borrow_mut();
    state.next_request_id += 1;
    let id = state.next_request_id.to_string();
    let request = make_request(id.clone(), &mut state);
    log::debug!("quickjs queued request id={} kind={}", id, request.kind());
    state.pending_requests.insert(
        id,
        PendingRequest {
            resolve: Persistent::save(ctx, resolve),
            reject: Persistent::save(ctx, reject),
        },
    );
    state.requests.push_back(request);
    Ok(promise)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::js_runtime::{WorkflowModuleInput, WorkflowRuntimePoll};
    use serde_json::json;

    #[test]
    fn executes_default_export_object() {
        let mut execution = RQuickJSWorkflowRuntime::new()
            .start_module(WorkflowModuleInput::new(
                r#"
export const meta = { name: "inline", description: "inline" };
export default { ok: true, args };
"#,
                "inline.workflow.js",
                json!({ "value": 1 }),
            ))
            .expect("workflow should start");

        let output = loop {
            match execution.poll().expect("workflow should poll") {
                WorkflowRuntimePoll::Complete(output) => break output,
                WorkflowRuntimePoll::Pending => continue,
                other => panic!("expected completion, got {other:?}"),
            }
        };

        assert_eq!(output.result, json!({ "ok": true, "args": { "value": 1 } }));
    }
}
