# smol-workflow-sandbox

Types, JSON Schema, and a stdio JSONL RPC client for smol-workflows sandbox providers.

## JSON protocol v1

Protocol v1 is plain JSONL over stdin/stdout. Field names are `snake_case` and match the Rust serde types.

The protocol is intentionally local-first: the workflow runner discovers a provider executable from `PATH`, launches it on the same machine with `serve`, sends JSON request objects on stdin, and reads JSON response/event objects from stdout. Diagnostics go to stderr.

Current clients use serialized request handling per provider process: the client sends one request, reads any events for that request, reads its final response, then sends the next request. Providers do not need to support multiple concurrent in-flight requests or interleaved events from different request IDs. Request IDs remain required for correlation, diagnostics, and future compatibility with multiplexed clients.

Provider executable naming:

```txt
smol-sandbox-<provider>
```

Launch shape:

```sh
smol-sandbox-local-worktree serve
```

The old one-shot command model is not the target protocol. Providers should implement the long-lived `serve` protocol instead of separate `open`, `close`, or `exec` subcommands.

## Message envelopes

Request:

```json
{"id":"req_1","method":"open","params":{}}
```

Success response:

```json
{"id":"req_1","result":{}}
```

Provider-declared error:

```json
{"id":"req_1","error":{"code":"bad_profile","message":"unknown profile","retryable":false}}
```

Streaming/progress event:

```json
{"id":"req_2","event":{"type":"stdout","data_base64":"aGVsbG8K"}}
```

## Methods

Expected methods align with the environment capability documented in `docs/harness-capabilities/environment.md`:

```txt
capabilities
open
close
cleanup_group
create_temp_dir
create_dir_all
write_file
read_file
remove
exec
spawn
shutdown
```

The provider owns profile lookup and sandbox implementation details. The runtime passes the provider-local profile name and local workspace path during `open`; it does not pass a profile directory.

Byte-oriented JSONL fields use base64. Command stdin/stdout/stderr and file contents should be represented as `stdin_base64`, `stdout_base64`, `stderr_base64`, event `data_base64`, and file `content_base64`. Large binary payloads should prefer `write_file`/`read_file` over command stdout/stderr streams when possible.

## Versioning

Each lifecycle request includes metadata with the protocol version:

```json
{
  "metadata": {
    "protocol_version": "sandbox.v1",
    "request_id": "req_01",
    "sandbox_group_id": "sbxgrp_01"
  }
}
```

The checked-in JSON Schema for v1 is at [`schema/sandbox.v1.schema.json`](schema/sandbox.v1.schema.json). Example JSONL envelope payloads are under [`tests/fixtures`](tests/fixtures).

## Provider sketch

A provider process can be implemented in any language that can read/write JSON Lines.

Pseudo-code:

```py
for line in stdin:
    request = json.loads(line)
    if request["method"] == "capabilities":
        respond(request["id"], {"capabilities": {"exec": True}})
    elif request["method"] == "shutdown":
        respond(request["id"], {})
        break
    else:
        respond_error(request["id"], "unsupported", "method is not implemented")
```

## Rust client

The Rust client should expose extracted success payloads rather than raw envelopes. Provider errors should become `SandboxProviderClientError::Provider`, while malformed protocol messages should become protocol errors.

## Updating the schema

After changing protocol v1 types, regenerate the checked-in JSON Schema:

```sh
SMOL_UPDATE_SANDBOX_SCHEMA=1 cargo test -p smol-workflow-sandbox generated_schema_matches_checked_in_schema
```

Then run:

```sh
cargo test -p smol-workflow-sandbox
```
