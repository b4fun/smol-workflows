# Rust JS runtime experiment

## Objective

Evaluate JavaScript runtime options for the Rust engine without depending on a `node` executable. The runtime needs to support the current smol workflow shape:

- workflow files are JavaScript/ESM-like modules,
- metadata is exported as `export const meta = ...`,
- workflow results are exported as `export default ...`,
- top-level await/module-result workflows should be possible,
- workflow code receives protected globals such as `args`, `agent`, `parallel`, `pipeline`, `log`, and `phase`.

This document records an experiment. It is not the final workflow runner design.

## Implemented experiment

Implemented/updated:

- `rust/engine/src/js_runtime_experiment.rs`
- `rust/engine/examples/js_runtime_experiment.rs`
- `rust/engine/examples/js_runtime_bench.rs`
- `rust/engine/tests/js_runtime_experiment.rs`
- dependencies: `boa_engine`, `rquickjs`, `serde`, `serde_json`

The Rust experiment currently uses a small workflow-module transform rather than a full ESM loader:

- `export const meta` -> `const meta`
- `export default async function` -> `globalThis.__default = async function`
- `export default function` -> `globalThis.__default = function`
- `export default` -> `globalThis.__default =`

After that transform, the runtime injects a compatibility subset:

- `args`
- `agent`
- `parallel`
- `pipeline`
- `log`
- `phase`

`agent` is a local echo stub in this experiment. That keeps the experiment focused on JS runtime viability before adding Rust orchestration and provider messaging.

## Runtime options evaluated

### Boa (`boa_engine`)

Status: **works for the basic workflow fixture**.

Verified by:

- test: `boa_embedded_executes_workflow_like_esm_js_fixture`
- fixture: `rust/engine/tests/fixtures/injected-globals.workflow.js`

Observed behavior:

- default-exported async workflow function executed,
- `args`, `phase`, `log`, `parallel`, and `agent` worked for the fixture,
- final JSON result matched expectations.

Complexity notes:

- Pure Rust dependency.
- No external JS runtime binary.
- Easy to embed with `Context::eval`.
- JSON interop is convenient via `JsValue::from_json` / `to_json`.
- Promise/microtask handling requires explicit `context.run_jobs()`.
- This experiment did not implement direct ESM loading; it used the ESM-to-script transform above.
- A production runner would still need proper module loading or a more robust transformation path.

Release microbenchmark, 100 iterations, new runtime/context per run:

```json
{
  "approach": "boa-embedded",
  "iterations": 100,
  "total_ms": 98.457417,
  "mean_ms": 0.98457417,
  "min_ms": 0.661667,
  "max_ms": 2.116875
}
```

### rquickjs / QuickJS (`rquickjs`)

Status: **best native option so far**.

Verified by:

- test: `rquickjs_embedded_executes_workflow_like_esm_js_fixture`
- test: `rquickjs_embedded_executes_top_level_module_result_fixture`
- fixture: `rust/engine/tests/fixtures/injected-globals.workflow.js`
- fixture: `rust/engine/tests/fixtures/module-result.workflow.js`

Observed behavior:

- default-exported async workflow function executed,
- top-level module-result workflow executed,
- `args`, `phase`, `log`, `parallel`, and `agent` worked for the fixtures,
- final JSON result matched expectations.

Complexity notes:

- Embeds QuickJS through Rust bindings.
- No external `qjs` or `node` executable.
- API is compact: `Runtime::new`, context creation, `ctx.eval`, then drain jobs with `ctx.execute_pending_job()`.
- Promise handling was straightforward for this experiment.
- This experiment still used the same ESM-to-script transform instead of a full module loader.
- It is C-backed rather than pure Rust, but it is self-contained as a Rust crate build dependency.

Release microbenchmark, 100 iterations, new runtime/context per run:

```json
{
  "approach": "rquickjs-embedded",
  "iterations": 100,
  "total_ms": 57.408834,
  "mean_ms": 0.57408834,
  "min_ms": 0.450125,
  "max_ms": 0.761
}
```

### deno_core / V8 (`deno_core`)

Status: **works in a standalone probe, but is much heavier**.

A temporary standalone probe was created under `/tmp/smol-deno-core-probe` so the main crate did not inherit the V8 dependency during this experiment.

Probe dependencies:

- `deno_core = "0.402.0"`
- `futures = "0.3"`
- `serde_json = "1"`
- `anyhow = "1"`

The first compile downloaded and built a large dependency graph including `v8 v149.2.0`. After updating the probe for the current `deno_core` scope API (`deno_core::scope!(scope, runtime)`), it successfully executed a workflow-shaped script with the same injected global subset.

Single-run probe output contained the expected result:

- `first.echo == "first: alpha"`,
- `second.echo == "second: beta"`,
- expected logs, phases, and agent calls were present.

Release probe, 100 iterations, new `JsRuntime` per run:

```json
{
  "approach": "deno-core",
  "iterations": 100,
  "total_ms": 239.066584,
  "mean_ms": 2.39066584
}
```

Complexity notes:

- Self-contained from the user perspective: no `node` executable.
- Embeds V8, so the binary and dependency graph are much larger.
- API is more complex than Boa/rquickjs: `JsRuntime`, event loop polling, V8 scopes, and `serde_v8` conversion.
- Temporary release binary was about `57M`.
- The main release benchmark binary containing Boa+rquickjs was about `11M`.
- `cargo tree --no-dedupe` line count in the Deno probe was about `30480`, versus about `3212` for the main crate with Boa+rquickjs.

## Performance comparison

All numbers below are release builds running the same small workflow-shaped fixture. Each iteration creates a fresh runtime/context, so these numbers emphasize runtime setup plus execution rather than long-lived throughput.

| Runtime | Mean per run | Total / 100 | Notes |
| --- | ---: | ---: | --- |
| rquickjs | `0.574 ms` | `57.409 ms` | Fastest and simplest successful native option |
| Boa | `0.985 ms` | `98.457 ms` | Pure Rust, simple API, slightly slower here |
| deno_core | `2.391 ms` | `239.067 ms` | Works, but has a much heavier API/dependency/binary footprint |

## Complexity comparison

| Runtime | Self-contained | ESM story in this experiment | Async/promise handling | Integration complexity | Footprint |
| --- | --- | --- | --- | --- | --- |
| rquickjs | Yes | Transform to script | Drain pending jobs | Low/medium | Smallest practical option |
| Boa | Yes, pure Rust | Transform to script | `Context::run_jobs()` | Low/medium | Moderate |
| deno_core | Yes, embeds V8 | Transform to script in probe; true ESM possible with loader work | Runtime event loop | High | Heavy |

## quickjs-emscripten follow-up

Investigated: <https://github.com/justjake/quickjs-emscripten>

`quickjs-emscripten` is QuickJS compiled to WebAssembly with Emscripten and exposed through a TypeScript/JavaScript API. It is relevant for browser, edge, or plugin-style environments, but it is not a direct Rust-native replacement for `rquickjs`.

Important capabilities from the README/API:

- evaluates untrusted JS in isolated QuickJS contexts,
- supports ES modules via `context.evalCode(source, filename, { type: "module" })`,
- module evaluation returns module exports, or a promise resolving to module exports when top-level await is involved,
- host code can expose functions to QuickJS,
- runtime supports sandbox limits:
  - `runtime.setMemoryLimit(bytes)`,
  - `runtime.setMaxStackSize(bytes)`,
  - `runtime.setInterruptHandler(fn)`,
- runtime supports module loading with `runtime.setModuleLoader(...)`,
- sync and asyncify variants are available,
- supported hosts include browsers, NodeJS, Deno, Bun, Cloudflare Workers, and similar ES2020 + WebAssembly environments.

A temporary probe was created under `/tmp/qjs-emscripten-probe` with `quickjs-emscripten@0.32.0`. The probe executed a workflow-shaped ES module with:

- injected `args`,
- injected `agent`,
- injected `parallel`,
- injected `log`,
- injected `phase`,
- top-level await,
- `export const meta`,
- `export default { ... }`.

The module executed successfully after resolving the module-evaluation promise and calling `runtime.executePendingJobs()`.

Probe result included expected exports:

```json
{
  "default": {
    "first": { "echo": "first: alpha" },
    "second": { "echo": "second: beta" },
    "args": { "a": "alpha", "b": "beta" }
  },
  "meta": { "name": "x", "description": "y" }
}
```

Sandbox check in the probe:

- `Math.random` was patched to throw,
- probe confirmed:

```json
{
  "randomBlocked": true
}
```

Caveats:

- Unlike `rquickjs::Context::custom`, quickjs-emscripten does not appear to expose a simple high-level API for selecting individual QuickJS intrinsics. We can patch/delete/freeze globals at runtime, or build a custom C/Wasm variant if deeper intrinsic removal is required.
- It is a JavaScript package. For a native Rust CLI, using it would mean either running a JS host or embedding a Wasm runtime and driving the Emscripten module ourselves. Both are likely more complex than `rquickjs` for the native engine.
- For a Rust engine compiled to `wasm32-unknown-unknown`, quickjs-emscripten could be paired with the Rust Wasm module from the surrounding JS host, but it would not be a normal Rust crate dependency. The JS host would own the QuickJS Wasm runtime and the Rust Wasm side would call host APIs.

Where it fits:

| Use case | Fit |
| --- | --- |
| Native Rust CLI | Poor/indirect; prefer `rquickjs` |
| Browser-hosted workflow runner | Good |
| Cloudflare Worker / JS edge host | Good |
| Rust compiled to wasm with JS glue | Possible: JS host owns quickjs-emscripten, Rust calls host APIs |
| Pure Rust wasm module with embedded JS runtime | Not directly |
| Strong runtime limits in JS host | Good: memory, stack, interrupt handler |
| Fine-grained intrinsic selection | Weaker than `rquickjs`; patch globals or custom build |

Conclusion: quickjs-emscripten is a strong candidate for a **JS-hosted Wasm workflow runtime**, especially for browser, edge, or plugin scenarios. It does not replace `rquickjs` for the native Rust engine, but it is worth tracking as a separate runtime backend if we want the engine to run in web/worker environments.

## rquickjs sandbox and access blocking

For the native Rust engine, `rquickjs` gives us a strong sandbox baseline because QuickJS does not include Node/Deno/Bun host APIs unless we expose them. The default posture should be:

> workflow JS receives only the workflow API; everything else is denied.

### 1. Use a restricted context

The experiment proves viability, but a production runner should not use broad defaults such as `Context::full`. Use a custom context instead:

```rust
use rquickjs::{Context, Runtime};
use rquickjs::context::intrinsic;

type WorkflowIntrinsics = (
    intrinsic::Json,
    intrinsic::Promise,
    intrinsic::Proxy,
    intrinsic::MapSet,
    intrinsic::RegExp,
);

let runtime = Runtime::new()?;
let context = Context::custom::<WorkflowIntrinsics>(&runtime)?;
```

Deliberately omit:

- `Performance`,
- `WeakRef`,
- potentially `TypedArrays` unless needed.

The initial `rquickjs` implementation includes the `Eval` intrinsic because `rquickjs` requires it for host-driven source evaluation. The workflow sandbox still removes the user-visible `eval` and function-constructor globals before workflow code runs.

### 2. Block nondeterminism

`Math.random` should be patched before workflow code runs:

```js
Object.defineProperty(Math, "random", {
  value() {
    throw new TypeError("Math.random is disabled in smol workflow sandbox")
  },
  writable: false,
  configurable: false,
})

Object.freeze(Math)
```

If deterministic randomness is needed later, expose a seeded workflow API instead of using `Math.random`.

### 3. Block dynamic code evaluation

Even when the runtime needs host-side evaluation support to bootstrap workflow code, remove user-visible dynamic evaluation APIs before the workflow runs:

```js
for (const name of ["eval", "Function", "AsyncFunction"]) {
  Object.defineProperty(globalThis, name, {
    value: undefined,
    writable: false,
    configurable: false,
  })
}
```

### 4. Block network access

Direct network access is absent by default in QuickJS/rquickjs. Keep it absent by not exposing network-capable host APIs. Lock down common host globals explicitly:

```js
for (const name of [
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
]) {
  Object.defineProperty(globalThis, name, {
    value: undefined,
    writable: false,
    configurable: false,
  })
}
```

Important distinction: workflow JS cannot directly access the network, but `agent(...)` may cause the parent/provider to use the network. Provider behavior must be controlled separately.

### 5. Block filesystem access

Filesystem access is also absent by default in QuickJS/rquickjs. Do not expose `readFile`, `writeFile`, `fs`, process APIs, or native module loading. Lock down common host globals:

```js
for (const name of [
  "require",
  "process",
  "Deno",
  "Bun",
  "Buffer",
  "__dirname",
  "__filename",
]) {
  Object.defineProperty(globalThis, name, {
    value: undefined,
    writable: false,
    configurable: false,
  })
}
```

Provider behavior is separate: an external agent provider may read or write files from its own process. The JS sandbox blocks direct workflow filesystem access; provider filesystem access needs its own policy.

### 6. Deny imports by default

The safest first module policy is:

```text
ImportPolicy::DenyAll
```

If imports are later required, use a strict resolver:

- allow only relative imports,
- canonicalize paths,
- require resolved paths to remain inside the workflow directory,
- reject absolute paths,
- reject `..` escapes after canonicalization,
- reject symlink escapes,
- reject `http:`, `https:`, `file:`, `node:`, and other URL/builtin schemes,
- reject native modules,
- allow only approved extensions such as `.js` / `.mjs`.

Sketch:

```rust
fn resolve_import(root: &Path, specifier: &str) -> anyhow::Result<PathBuf> {
    if specifier.starts_with("http:")
        || specifier.starts_with("https:")
        || specifier.starts_with("file:")
        || specifier.starts_with("node:")
    {
        anyhow::bail!("URL/builtin imports are disabled");
    }

    if !specifier.starts_with("./") && !specifier.starts_with("../") {
        anyhow::bail!("bare imports are disabled");
    }

    let root = root.canonicalize()?;
    let resolved = root.join(specifier).canonicalize()?;

    if !resolved.starts_with(&root) {
        anyhow::bail!("import escapes workflow sandbox");
    }

    if !matches!(resolved.extension().and_then(|ext| ext.to_str()), Some("js" | "mjs")) {
        anyhow::bail!("only JS modules are allowed");
    }

    Ok(resolved)
}
```

### 7. Enforce resource limits

Use QuickJS runtime limits for defense in depth:

```rust
runtime.set_memory_limit(64 * 1024 * 1024);
runtime.set_max_stack_size(1024 * 1024);

let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
runtime.set_interrupt_handler(Some(Box::new(move || {
    std::time::Instant::now() >= deadline
})));
```

This gives us:

- bounded JS heap,
- bounded stack/recursion,
- timeout for infinite loops or long CPU execution.

### 8. Expose only workflow globals

The only host-provided globals should be:

- `args`,
- `agent`,
- `parallel`,
- `pipeline`,
- `log`,
- `phase`.

Make them readonly/non-configurable where possible. Use proxies for readonly `args`.

### 9. Separate provider sandbox policy

The JS runtime sandbox and agent/provider sandbox are different layers. Even if JS has no network/filesystem access, providers may have access because they are host processes.

Suggested future provider policies:

```rust
pub enum ProviderSandbox {
    None,
    IsolatedTempDir,
    ReadOnlyCwd,
    NoFilesystem,
}

pub enum ProviderNetworkPolicy {
    Inherit,
    Deny,
    Allowlist(Vec<String>),
}
```

For deterministic/sandboxed workflow runs, use `debug` or another local constrained provider by default.

## Recommendation

Use **rquickjs** as the next self-contained native runtime experiment path.

Reasons:

1. It executed both tested workflow shapes: default async function and top-level module-result.
2. It was fastest in the small release benchmark.
3. Its embedding API is much smaller than `deno_core`.
4. It avoids both a `node` executable and V8's large binary/dependency footprint.
5. It provides a good sandbox baseline: no network/filesystem APIs by default, configurable intrinsics, and runtime resource limits.

Boa remains worth tracking because it is pure Rust and may be the better path if compiling the Rust engine itself to Wasm becomes a priority. However, rquickjs currently looks more practical for the native smol workflow runner.

`deno_core` is viable if V8 compatibility or a Deno-like module system becomes necessary, but it is overkill for the first self-contained runner milestone.

`quickjs-emscripten` is worth tracking for JS-hosted Wasm/browser/edge environments, but it is not the preferred native Rust CLI path.

## Verification commands run

```sh
cargo test -p smol-workflow-engine --test js_runtime_experiment -- --nocapture
cargo run --release -p smol-workflow-engine --example js_runtime_bench -- 100
cd /tmp/smol-deno-core-probe && cargo run --release
cd /tmp/qjs-emscripten-probe && node probe.mjs
```

The Rust tests passed for Boa and rquickjs. The standalone `deno_core` probe and the `quickjs-emscripten` probe also ran successfully.
