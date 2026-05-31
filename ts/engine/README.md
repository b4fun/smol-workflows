# @smol-workflow/engine

Minimal workflow engine for smol-workflow.

## Usage

```sh
smol-wf run user-script.js --args-my-arg1 "arg-value-1" --args-my-arg2 "arg-value-2"
```

Use the explicit simple backend:

```sh
smol-wf run user-script.js --backend simple --args-my-arg1 "arg-value-1"
```

Use the Absurd SQLite durable backend and wait for completion:

```sh
smol-wf run user-script.js \
  --backend absurd \
  --args-my-arg1 "arg-value-1"
```

Load workflow args from a JSON file:

```sh
smol-wf run user-script.js --args-from-file args.json
```

The engine injects these globals into an isolated runner:

- `args`
- `agent`
- `parallel`
- `log`
- `phase`

For now, `agent(prompt)` returns an echo string.

## Absurd SQLite backend

The engine includes an experimental Absurd SQLite durable backend.

CLI usage:

```sh
smol-wf absurd init

smol-wf absurd submit ../../examples/hello.mjs \
  --args-name Ada

smol-wf absurd worker \
  --concurrency 2
```

For deterministic one-shot processing in tests or demos:

```sh
smol-wf absurd work-batch \
  --batch-size 1
```

Programmatic usage:

```ts
import { createAbsurdWorkflowBackend } from "@smol-workflow/engine/backends/absurd";

const backend = createAbsurdWorkflowBackend({
  dbPath: "./smol-workflows.db",
});

await backend.init();
backend.registerWorkflowTask();

await backend.submitWorkflow({
  scriptPath: "./examples/hello.mjs",
  args: { name: "Ada" },
});

const worker = await backend.startWorker({ concurrency: 2 });
```

By default, the backend uses `./smol-workflows.db` and tries to find the Absurd SQLite extension automatically from `SMOL_WF_ABSURD_EXTENSION`, `ABSURD_DATABASE_EXTENSION_PATH`, or common local build paths like `target/release/libabsurd.<ext>`. You can still pass `--db` and `--extension` explicitly.

The backend provides durable workflow invocation and durable `agent` calls. Absurd owns queueing, claiming, retries, completion, and failure. Workflow `agent(prompt, { key })` calls are checkpointed through Absurd `ctx.step(...)`; if no key is provided, the engine derives a deterministic key from the prompt/options.
