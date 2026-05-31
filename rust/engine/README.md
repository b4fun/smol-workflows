# smol-workflow-engine

Rust implementation of the smol-workflows engine.

This crate is a minimal placeholder for the future Rust port of `ts/engine`.

## JavaScript runtime

`src/js_runtime` defines the engine-independent boundary for executing workflow-shaped JavaScript modules. The first implementation is `js_runtime::rquickjs::RquickJsWorkflowRuntime`, a restricted QuickJS runtime intended to become the native Rust workflow runner backend.
