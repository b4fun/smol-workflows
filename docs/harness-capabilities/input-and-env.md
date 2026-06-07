# Harness input & environment capabilities

This document is the reference for how `smol-workflows` should pass agent input, and eventually execution-environment context, to supported agent harnesses when a workflow calls:

```ts
await agent(prompt, options)
```

For now, it focuses on **transporting the prompt safely** and on the minimum execution-environment requirement: providers must honor the workflow working directory (`cwd`). Here, “environment” means the agent execution context such as workspace/session/isolation, not process environment variables. Structured output and usage accounting are covered separately in [`structured-output.md`](./structured-output.md) and [`budget-and-usage.md`](./budget-and-usage.md).

## Objectives

1. **Avoid long prompt argv:** providers should not pass large prompts as a single CLI positional argument when the harness supports stdin, a prompt file, or an API body.
2. **Prefer streaming stdin:** for non-interactive CLI modes that explicitly accept stdin, send the workflow prompt through stdin and use a sentinel argument such as `-` only when the harness requires it.
3. **Use prompt files when stdin is not the harness contract:** for CLIs that support file references in the initial message, write the prompt to a temporary file and pass the file reference instead of the full prompt.
4. **Use API request bodies for server harnesses:** when a provider uses a server/session API, put prompt text in the JSON request body rather than in the process argv.
5. **Keep argv for flags and small selectors:** model names, agent types, output modes, schema-file paths, and other short configuration values are appropriate CLI arguments.
6. **Honor explicit model selection:** when `options.model` is present after workflow/phase defaulting, providers must map it to the harness's supported model selector instead of burying it in the prompt or silently ignoring it.
7. **Honor `cwd`:** every provider must run the harness in the workflow working directory, or pass the equivalent project/directory field when using a server/API transport.
8. **Support engine-managed worktree isolation:** when `agent(..., { isolation: "worktree" })` is requested, the workflow engine creates a fresh git worktree from the workflow `cwd` repository and passes that worktree as the provider `cwd` for the call.
9. **Do not reshape the host harness by default:** at this stage, providers should not add extra harness/provider modifications beyond the requested agent call. They should follow the host's normal setup, including pre-installed skills, instructions, context files, extensions, plugins, or agent configuration when the harness would normally load them.
10. **Preserve diagnostics without leaking prompt text:** logs may include prompt length, temp-file paths, cwd, and argument shapes, but should avoid dumping full prompts.

## Cross-provider input transport rules

Provider implementations should choose the first supported transport in this order:

1. **Server/API body** for harnesses already driven by an HTTP/RPC/session API.
2. **Stdin** for CLI print/exec modes that document stdin input.
3. **Prompt file reference** for CLIs that document file inclusion in the initial message.
4. **Positional argv** only for short prompts or when no better supported transport exists.

Recommended implementation pattern:

```rust
let prompt = build_provider_prompt(&input);

if provider_accepts_stdin {
    args.push("-".into());
    run_provider_command(provider, command, &args, Some(&prompt), cwd, timeout).await?;
} else if provider_accepts_prompt_file && prompt.len() > MAX_PROMPT_ARG_LENGTH {
    let temp = temp_dir("smol-wf-provider-")?;
    let prompt_path = temp.path().join("prompt.md");
    fs::write(&prompt_path, &prompt)?;
    args.push(format!("@{}", prompt_path.to_string_lossy()));
    run_provider_command(provider, command, &args, None, cwd, timeout).await?;
} else {
    args.push(prompt);
    run_provider_command(provider, command, &args, None, cwd, timeout).await?;
}
```

`MAX_PROMPT_ARG_LENGTH` should be conservative. The exact shell/OS limit varies, and process argv contains more than the user prompt. A threshold around tens of KiB is safer than waiting for platform `ARG_MAX` failures.

The current Rust helper logs command, cwd, stdin length, and timeout. This is the right shape: diagnostics should indicate whether stdin is used without exposing the stdin contents.

## Working directory (`cwd`) requirement

Every external provider must honor the workflow working directory. This ensures that relative paths, repository discovery, context-file discovery, tool execution, and provider session state are rooted in the same workspace the workflow intended.

Expected behavior:

- If the provider is launched as a CLI process, set the child process current directory to the workflow `cwd`.
- If the provider exposes a CLI flag for the project directory, prefer the most explicit provider-supported mechanism when needed, but still keep the child process `cwd` aligned when possible.
- If the provider is driven through a server/session API, include the workflow directory in the request/session creation field expected by that API.
- If no workflow `cwd` is supplied, use the runner/provider default consistently.
- Temporary prompt/schema/output files may live outside `cwd`, but provider-visible relative paths in prompts and tool calls should resolve from `cwd`.

Provider status:

| Provider | Expected `cwd` transport |
| --- | --- |
| `debug` | No external process; receives context in memory. |
| `codex` | Set child process `cwd` for `codex exec`; optionally use `--cd` if a future implementation needs an explicit Codex root flag. |
| `claude-code` | Set child process `cwd` for `claude --print`. |
| `pi` | Set child process `cwd` for `pi --print`. |
| `opencode` | For CLI mode, set child process `cwd`; for server/session mode, pass the directory in session/message API parameters. |

## Agent run isolation

`isolation: "worktree"` is an engine-level capability, not a provider-level capability. Providers do not create worktrees themselves. Instead, the workflow engine:

1. checks that the workflow `cwd` is inside a git repository;
2. creates a temporary git worktree from `HEAD` using a short-lived branch named with the pattern `smol-wf/agent-run/<lowercase-ulid>`;
3. maps the original workflow `cwd` to the corresponding path inside that worktree, preserving subdirectory workflows;
4. passes the isolated path as `input.context.cwd` to the provider;
5. captures isolation metadata on the agent run summary (`kind`, `branch`, `worktreePath`, and provider `cwd`);
6. removes the worktree and deletes the temporary branch after the agent run completes.

If the workflow `cwd` is not inside a git repository, `isolation: "worktree"` fails with a clear error. The isolated worktree is intended for agent calls that may modify files without touching the caller's working tree. Temporary prompt/schema/output files may still live outside the isolated worktree. Cleanup is best-effort on failures: normal provider errors still trigger worktree removal and branch deletion, while process-kill or machine-failure scenarios may leave stale git worktree metadata for manual cleanup.

## Model selection (`model`)

`model` is a provider-specific selector for the model to use for one `agent(prompt, options)` call. It is intentionally separate from `provider`:

- `provider` selects the harness integration, such as `codex`, `claude-code`, `pi`, or `opencode`.
- `model` selects the model within the chosen harness when that harness exposes a model override.

Expected selection logic:

1. Apply workflow/runtime defaulting first, including any phase-level `model` metadata that should apply to the current `phase(...)`.
2. If no effective model is present, do not pass a model flag or API field; let the selected harness use its normal default/session model.
3. If an effective model is present, resolve any runner-supported aliases for the selected provider before invoking the harness.
4. Pass the resolved model through the harness's supported model selector, such as `--model <model>` for CLI providers or a request-body `model` field for server/session providers.
5. If a supplied model cannot be resolved or represented for the selected provider, fail the call with a clear configuration error rather than silently falling back to the harness default.
6. Preserve the originally requested `options.model` for diagnostics and fallback reporting, but report the observed/resolved model from provider metadata separately as described in [`budget-and-usage.md`](./budget-and-usage.md#model-reporting).

Provider status:

| Provider | Expected model transport |
| --- | --- |
| `debug` | No external selection; may echo `options.model` in diagnostics/result metadata when supplied. |
| `codex` | Pass the resolved model with `codex exec --model <model>`. |
| `claude-code` | Pass the resolved model with `claude --model <model>`. |
| `pi` | Pass the resolved model with `pi --model <model>`. |
| `opencode` | CLI mode passes `opencode run --model <model>`; server/session mode converts the resolved model into the API `model` body shape expected by OpenCode, including `providerID`/`modelID` when required. |

Model resolution should never be prompt-mediated. If a harness does not expose a model selector for a future provider, that provider should reject explicit `model` values or document a provider-specific exception.

### Thinking/reasoning settings

The workflow SDK does not currently expose a generic `thinking`, `effort`, `reasoning`, or `variant` agent option. Provider integrations should therefore not infer thinking settings from the generic `model` field except where the target harness explicitly defines that syntax and the runner's model-alias layer intentionally resolves to it.

Expected behavior:

1. Do not prompt-mediate thinking/reasoning settings.
2. Do not overload `model` with provider-specific effort settings unless the harness documents that combined syntax, such as Pi's `model:<thinking>` shorthand.
3. If a future alias-resolution layer maps one workflow alias to multiple provider settings, such as `{ model, thinking }` or `{ model, variant }`, apply those settings through the harness's native flags/API fields and preserve the expanded values in diagnostics.
4. If a provider exposes a thinking display flag separately from a reasoning-effort setting, keep those concepts separate. Displaying thinking blocks is not the same as selecting a reasoning effort.

Provider status from current help output:

| Provider | Help surface | Expected mapping today |
| --- | --- | --- |
| `debug` | No external harness thinking concept. | No-op; useful tests should not depend on thinking selection. |
| `codex` | `codex exec --help` documents `--model` and generic `-c key=value` config overrides, but no first-class thinking/reasoning-effort flag. | Do not map a generic workflow option today. A future Codex-specific option may use documented config keys through `-c`, but should not be inferred from `model`. |
| `claude-code` | `claude --help` documents `--effort <level>` with `low`, `medium`, `high`, `xhigh`, and `max`. | No generic workflow mapping today. A future provider-specific option can pass `--effort <level>`; do not encode effort in `model`. |
| `pi` | `pi --help` documents `--thinking <level>` with `off`, `minimal`, `low`, `medium`, `high`, and `xhigh`; `--model <pattern>` also supports optional `:<thinking>`. | Preserve `:<thinking>` only when the resolved Pi model string intentionally includes it. Otherwise, a future provider-specific option should pass `--thinking <level>` separately. |
| `opencode` | `opencode run --help` documents `--variant` as provider-specific reasoning effort and `--thinking` as a boolean to show thinking blocks. | Treat `--variant` as reasoning-effort selection and `--thinking` as display control. Neither is part of generic `model` today. |

### Model selector syntax and alias resolution

The workflow `model` string is treated as a runner-resolved selector before it is handed to a provider. The selector syntax is:

```txt
<model-id>[?provider=<model-provider>&thinking=<level>]
```

Examples:

```txt
gpt-5.5
gpt-5.5?provider=github-copilot
gpt-5.5?provider=github-copilot&thinking=medium
github-copilot/gpt-5.5
```

Semantics:

- `<model-id>` is the model identifier within a model provider.
- `provider` is the model provider qualifier, not the smol-workflows agent provider selected by `agent(..., { provider })` or `--agent-provider`.
- `thinking` is a portable reasoning-effort hint. It is resolved with the model selector but applied through provider-specific reasoning controls, not through prompt text.
- A slash-qualified model such as `github-copilot/gpt-5.5` is equivalent to `gpt-5.5?provider=github-copilot` for harnesses that use `provider/model` syntax.
- If both slash qualification and `?provider=...` are present, they must agree. Conflicting provider qualifiers are configuration errors.
- Unknown query parameters are configuration errors. They should not be silently ignored because misspellings would otherwise change model selection.

Model aliases can be supplied by the runner, for example from repeated CLI flags such as:

```sh
--model-map='deep:gpt-5.5?provider=github-copilot&thinking=medium'
--model-map='fast:gpt-5.4-nano?provider=github-copilot&thinking=low'
```

Alias resolution order:

1. Determine the effective workflow model string after per-call options, phase metadata, and runtime defaults are applied.
2. If the effective string exactly matches a model-map alias key, replace it with that alias value.
3. If there is no alias match, treat the effective string as a literal model selector so provider-native names such as `sonnet` continue to work.
4. Expand aliases at most once. Alias values are parsed directly as selectors, even if they happen to match another alias key.
5. Parse the resulting value as the model selector syntax above.
6. Validate that the selected agent provider can represent the resolved model provider and thinking settings.
7. Invoke the harness using native model and reasoning controls.

For example, `model: "deep"` with `--model-map=deep:gpt-5.5?provider=github-copilot&thinking=medium` resolves to model ID `gpt-5.5`, model provider `github-copilot`, and thinking level `medium`. A provider that supports provider-qualified model IDs should pass the model as `github-copilot/gpt-5.5`, then pass `medium` through the harness's reasoning setting.

## Agent type selection (`agentType`)

`agentType` is a provider-specific selector for the kind of coding agent/subagent to use for one `agent(prompt, options)` call. It is intentionally separate from `provider` and `model`:

- `provider` selects the harness integration, such as `codex`, `claude-code`, `pi`, or `opencode`.
- `model` selects the model when the harness exposes a model override.
- `agentType` selects a named agent/subagent when the harness exposes an agent selector.

Because agent concepts differ across harnesses, providers should only map `agentType` when the target harness has a clear supported mechanism. Providers should not invent ambiguous mappings to skills, system prompts, profiles, or config files unless documented as a provider-specific option.

Current support matrix:

| Provider | Harness support | Current smol-workflows mapping | Expected behavior |
| --- | --- | --- | --- |
| `debug` | No external harness agent concept. | Ignored. | Keep as a no-op; useful tests should not depend on external agent selection. |
| `codex` | Codex supports subagent workflows and custom agents, but `codex exec --help` does not expose a direct `--agent <name>` selector. Built-in/custom subagents are invoked by asking Codex to spawn/use them. | Ignored. | Do not map `agentType` to a CLI flag. Workflow authors can mention Codex subagents in the prompt, or a future explicit Codex-specific option can wrap the prompt to request a subagent. |
| `claude-code` | Claude Code exposes `--agent <agent>` and `--agents <json>`. | Supported. | CLI mode passes `--agent <agentType>`. Do not overload `agentType` for `--agents <json>`, which defines agents rather than selecting one. |
| `pi` | Pi exposes skills, extensions, prompt templates, tools, and system prompt options, but no direct named-agent selector in `pi --help`. | Ignored. | Do not map `agentType`; mapping it to `--skill`, `--system-prompt`, or a prompt template would be ambiguous. |
| `opencode` | OpenCode exposes `--agent <agent>` for CLI mode and an `agent` field in server/session requests. | Supported. | CLI mode passes `--agent <agentType>`; server/structured modes include `"agent": "<agentType>"` in session/message request bodies. |

Provider-specific notes:

- For Codex and Pi, prefer calling out the desired subagent, custom agent, or role behavior directly in the workflow prompt. Codex subagents are prompt-mediated rather than exposed as a root `codex exec --agent` flag, and Pi does not expose a direct named-agent selector. Pi-native mechanisms such as skills/extensions should remain explicit options outside `agentType` if needed.


## Host harness setup

At this stage, `smol-workflows` should avoid extra modifications to the underlying harness/provider environment. Provider integrations should launch or call the harness in the same shape a user would normally use it, except for the minimum flags needed for non-interactive execution, output parsing, prompt transport, structured output, and `cwd` selection.

Expected behavior:

- Do not disable host-installed skills, instructions, context-file discovery, extensions, plugins, or agent configuration by default.
- Do not inject extra workflow-specific instructions outside the prompt unless required for a specific capability such as structured output.
- Do not force a clean/isolated harness profile unless the workflow/provider option explicitly asks for isolation.
- Let each harness load its normal project/user configuration from the selected `cwd` and host setup.
- If a provider must suppress part of the host setup for correctness, document that as an explicit provider-specific exception.

This keeps workflow agent calls aligned with the user's existing coding-agent setup while still standardizing how smol-workflows transports input and reads output.

## `debug`

### Behavior

`debug` is local and does not spawn an external harness.

### Expected approach

No stdin/file transport is required. The provider receives `AgentProviderRunInput.prompt` in memory and returns deterministic local output.

### References

- No external harness source applies. `debug` is a deterministic local test provider, not an external CLI integration.

## `codex`

### CLI help finding

Current `codex exec --help` documents:

```txt
Usage: codex exec [OPTIONS] [PROMPT]

Arguments:
  [PROMPT]
          Initial instructions for the agent. If not provided as an argument (or if `-` is used),
          instructions are read from stdin. If stdin is piped and a prompt is also provided, stdin
          is appended as a `<stdin>` block
```

### Current behavior

The provider already uses stdin:

```sh
codex exec \
  --json \
  --output-last-message <temp-output-file> \
  --output-schema <temp-schema-file> \
  -
```

and passes the workflow prompt to process stdin.

### Expected approach

Keep using stdin for the initial prompt.

Implementation requirements:

- Pass `-` as the prompt sentinel.
- Write `input.prompt` to stdin.
- Continue using temp files for output/schema paths, because those are short path arguments and are part of the Codex CLI contract.
- Avoid passing prompt text as positional argv, even for short prompts, to keep behavior uniform and avoid accidental argv-size failures.

### References

- `codex exec --help`, observed with `codex-cli 0.135.0`.
- Codex non-interactive docs: <https://developers.openai.com/codex/noninteractive>

## `claude-code`

### CLI help finding

Current `claude --help` documents:

```txt
Claude Code - starts an interactive session by default, use -p/--print for non-interactive output

Arguments:
  prompt                                Your prompt

Options:
  -p, --print                           Print response and exit (useful for pipes).
  --input-format <format>               Input format (only works with --print): "text" (default), or "stream-json"
  --output-format <format>              Output format (only works with --print): "text", "json", or "stream-json"
```

The help does not show a dedicated prompt-file flag for local prompt text, but it does explicitly describe `--print` as useful for pipes and provides `--input-format text` for print mode.

### Current behavior

The provider currently passes the prompt as a positional CLI argument:

```sh
claude --output-format json --print '<prompt>'
```

### Expected approach

Prefer stdin in print mode:

```sh
claude \
  --output-format json \
  --input-format text \
  --print
```

and write the workflow prompt to stdin.

Implementation requirements:

- Do not pass large prompt text as the final positional argument.
- Use `--input-format text` explicitly when sending stdin.
- Keep `--json-schema '<schema-json>'` as a short argument for structured-output calls unless/until Claude Code provides a schema-file option.
- Preserve existing JSON output parsing and usage extraction.

### References

- `claude --help`, observed with Claude Code `2.1.161`.
- Claude Code CLI reference: <https://code.claude.com/docs/en/cli-reference>

## `pi`

### CLI help finding

Current `pi --help` documents:

```txt
Usage:
  pi [options] [@files...] [messages...]

Options:
  --provider <name>              Provider name (default: google)
  --model <pattern>              Model pattern or ID (supports "provider/id" and optional ":<thinking>")
  --models <patterns>            Comma-separated model patterns for Ctrl+P cycling
  --thinking <level>             Set thinking level: off, minimal, low, medium, high, xhigh

Examples:
  # Include files in initial message
  pi @prompt.md @image.png "What color is the sky?"

  # Non-interactive mode (process and exit)
  pi -p "List all .ts files in src/"

  # Use different model
  pi --provider openai --model gpt-4o-mini "Help me refactor this code"

  # Use model with provider prefix (no --provider needed)
  pi --model openai/gpt-4o "Help me refactor this code"

  # Use model with thinking level shorthand
  pi --model sonnet:high "Solve this complex problem"
```

### Current behavior

The provider already avoids oversized argv for long prompts. It writes the prompt to a temp file when the prompt exceeds a conservative threshold, then passes an `@<path>` reference:

```sh
pi --print --mode json @/tmp/smol-wf-pi-.../prompt.md
```

For shorter prompts, it currently passes the prompt as a positional message argument.

### Expected approach

Use Pi's documented `@file` initial-message support for long prompts. This is preferable to passing long prompt text as argv.

Implementation requirements:

- Write long prompts to a temporary text/Markdown file.
- Pass the prompt file as `@<path>`.
- Keep generated structured-output extensions as separate `--extension <path>` files.
- When an effective workflow `model` is present, pass it via Pi's model selector, not in the prompt.
- Prefer the fully qualified `provider/model` form when the model resolution layer knows the provider, because Pi help documents that this form avoids needing a separate `--provider` flag.
- If the selected model intentionally includes Pi's thinking shorthand, preserve the `:<thinking>` suffix; otherwise pass resolved `thinking` as `--thinking <level>`.
- Do not use Pi's `--models` cycling list for one-off workflow model selection; it is an interactive cycling setting, while `agent(..., { model })` should select a single model.
- Consider always using `@file` for generated/schema-augmented prompts if consistency is more important than avoiding temp-file creation.

### References

- `pi --help`, observed with Pi `0.78.0`.
- Pi documentation: <https://github.com/earendil-works/pi-mono/tree/main/packages/coding-agent/docs>

## `opencode`

### CLI help finding

Current `opencode run --help` documents:

```txt
opencode run [message..]

Positionals:
  message  message to send

Options:
  -m, --model                         model to use in the format of provider/model
      --agent                         agent to use
      --format                        format: default (formatted) or json (raw JSON events)
  -f, --file                          file(s) to attach to message
      --dir                           directory to run in, path on remote server if attaching
      --variant                       model variant (provider-specific reasoning effort, e.g., high,
                                      max, minimal)
      --thinking                      show thinking blocks
```

Current `opencode --help` also documents related provider/model commands and top-level selection:

```txt
Commands:
  opencode providers           manage AI providers and credentials
  opencode models [provider]   list all available models

Options:
  -m, --model                  model to use in the format of provider/model
```

The help shows message text as positional argv and `--file` as an attachment mechanism. It does not document stdin as the initial-message transport for `opencode run`, and `--file` should not be assumed to be equivalent to replacing the prompt with file contents.

### Current behavior

Unstructured calls currently pass the prompt as positional argv:

```sh
opencode run --format json '<prompt>'
```

Structured-output calls already use the OpenCode server/session API and send prompt text in the JSON request body:

```json
{
  "parts": [{ "type": "text", "text": "...prompt..." }],
  "format": { "type": "json_schema", "schema": {}, "retryCount": 2 }
}
```

### Expected approach

Prefer the server/session API for any call where prompt length could exceed safe argv limits. The API body is the correct transport for large prompt text.

Implementation requirements:

- Keep structured-output calls on the server/session API path.
- For unstructured calls, either:
  - route through the same server/session API when prompt length exceeds the threshold, or
  - use the API path for all OpenCode calls for consistency.
- Do not treat `--file <prompt-file>` as a prompt replacement unless OpenCode documents that exact semantics.
- Keep CLI positional message use only for short prompts if the provider remains CLI-based.
- When an effective workflow `model` is present in CLI mode, pass it as `--model <provider/model>`.
- When using the server/session API, split the same resolved `provider/model` value into OpenCode's request-body model object, e.g. `{ "providerID": "<provider>", "modelID": "<model>" }`.
- Require an OpenCode model value to include a provider component before invoking the harness, because the documented `--model` format is `provider/model` and the server API needs provider identity separately.
- Treat resolved `thinking` as OpenCode's model variant/reasoning-effort selector: pass it as `--variant <level>` in CLI mode and as top-level request body field `"variant": "<level>"` in server/session mode.
- Keep OpenCode's `--thinking` boolean separate; it controls whether thinking blocks are shown and is not the same as selecting reasoning effort.

OpenCode implementation notes:

- The generated OpenCode JS SDK for `POST /session/{sessionID}/message` includes body fields `model`, `agent`, `format`, `parts`, and `variant`.
- The generated OpenCode types model session messages with `model: { providerID, modelID, variant? }`.
- OpenCode core applies `session.model?.variant` through catalog model variants before constructing the provider request.

### References

- `opencode run --help`, observed with OpenCode `1.15.13`.
- OpenCode official CLI docs: <https://opencode.ai/docs/cli>
- OpenCode source repository: <https://github.com/anomalyco/opencode>

## Known risks and limitations

- Some harnesses append piped stdin to a positional prompt rather than replacing it. Do not pass both unless that behavior is desired.
- Prompt-file syntax is provider-specific. `@file` works for Pi, but should not be generalized to every CLI.
- File attachments may be semantically different from prompt text. A provider may treat attachments as context rather than as the user message.
- Positional argv can leak through process inspection, logs, crash reports, and shell history. It is not an appropriate transport for large or sensitive prompt payloads.
- Temp prompt files should be created in private temp directories and cleaned up after the provider call completes.
