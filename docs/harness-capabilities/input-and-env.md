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
6. **Honor `cwd`:** every provider must run the harness in the workflow working directory, or pass the equivalent project/directory field when using a server/API transport.
7. **Do not reshape the host harness by default:** at this stage, providers should not add extra harness/provider modifications beyond the requested agent call. They should follow the host's normal setup, including pre-installed skills, instructions, context files, extensions, plugins, or agent configuration when the harness would normally load them.
8. **Preserve diagnostics without leaking prompt text:** logs may include prompt length, temp-file paths, cwd, and argument shapes, but should avoid dumping full prompts.

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

Examples:
  # Include files in initial message
  pi @prompt.md @image.png "What color is the sky?"

  # Non-interactive mode (process and exit)
  pi -p "List all .ts files in src/"
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
  -f, --file                          file(s) to attach to message
  --format                            format: default (formatted) or json (raw JSON events)
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
