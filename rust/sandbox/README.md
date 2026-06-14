# smol-workflow-sandbox

Types, JSON Schema, and a local binary plugin client for smol-workflows sandboxes.

## JSON protocol v1

Protocol v1 is plain JSON over stdin/stdout. Field names are `snake_case` and match the Rust serde types.

The protocol is intentionally local-first: the workflow runner invokes a provider executable on the same machine and sends one JSON request on stdin. The plugin writes one JSON response to stdout and diagnostics to stderr.

Required subcommands:

```txt
capabilities
open
close
cleanup-group
```

Future/optional subcommands:

```txt
exec
```

Common rules:

- The plugin receives the subcommand as `argv[1]`.
- The plugin reads exactly one JSON request from stdin.
- The plugin writes exactly one JSON response to stdout.
- Human-readable diagnostics go to stderr.
- Exit code `0` means the plugin produced a response.
- Non-zero exit code means plugin/protocol failure.
- Provider-declared operation failures should be returned as JSON `{ "error": ... }` with exit code `0` when possible.

Each request includes metadata with the protocol version:

```json
{
  "metadata": {
    "protocol_version": "sandbox.v1",
    "request_id": "req_01",
    "sandbox_group_id": "sbxgrp_01"
  }
}
```

The checked-in JSON Schema for v1 is at [`schema/sandbox.v1.schema.json`](schema/sandbox.v1.schema.json). Example request/response payloads are under [`tests/fixtures`](tests/fixtures).

## Response shape

Responses use optional success payloads plus optional provider errors.

Success example:

```json
{
  "session": {
    "id": "sbx_01",
    "cwd": "/workspace",
    "capabilities": {
      "exec": false
    }
  }
}
```

Provider-declared failure example:

```json
{
  "error": {
    "code": "bad_profile",
    "message": "unknown sandbox profile",
    "retryable": false
  }
}
```

## Bash provider sketch

```sh
#!/usr/bin/env sh
set -eu

cmd="$1"
input="$(cat)"

case "$cmd" in
  capabilities)
    printf '%s\n' '{"capabilities":{"exec":false}}'
    ;;
  open)
    printf '%s\n' '{"session":{"id":"sbx_example","cwd":"/workspace","capabilities":{"exec":false}}}'
    ;;
  close)
    printf '%s\n' '{}'
    ;;
  cleanup-group)
    printf '%s\n' '{"cleaned_count":0}'
    ;;
  *)
    printf '%s\n' '{"error":{"code":"unknown_command","message":"unknown command","retryable":false}}'
    ;;
esac
```

## Rust client

The plugin client returns extracted success payloads:

```rust
use smol_workflow_sandbox::{CapabilitiesRequest, Metadata, SandboxProviderPlugin};

# async fn example() -> Result<(), Box<dyn std::error::Error>> {
let plugin = SandboxProviderPlugin::new("smol-sandbox-example");
let capabilities = plugin
    .capabilities(&CapabilitiesRequest {
        metadata: Metadata::new("req_01", "sbxgrp_01"),
    })
    .await?;
# Ok(())
# }
```

If a provider returns `{ "error": ... }`, the client returns `PluginClientError::Provider`. If a success response omits the required success payload, the client returns `PluginClientError::Protocol`.

## Updating the schema

After changing protocol v1 types, regenerate the checked-in JSON Schema:

```sh
SMOL_UPDATE_SANDBOX_SCHEMA=1 cargo test -p smol-workflow-sandbox generated_schema_matches_checked_in_schema
```

Then run:

```sh
cargo test -p smol-workflow-sandbox
```
