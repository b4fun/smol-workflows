use schemars::schema_for;
use serde::de::DeserializeOwned;
use smol_workflow_sandbox::*;
use std::fs;
use std::path::PathBuf;

fn manifest_path(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative)
}

fn read_fixture<T: DeserializeOwned>(name: &str) -> T {
    let path = manifest_path(&format!("tests/fixtures/{name}"));
    let json = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
    serde_json::from_str(&json)
        .unwrap_or_else(|error| panic!("failed to deserialize {}: {error}", path.display()))
}

#[test]
fn fixture_json_deserializes() {
    let open_request: JsonlRequestEnvelope<OpenSandboxRequest> =
        read_fixture("open_request_envelope.json");
    assert_eq!(open_request.method, "open");
    assert_eq!(open_request.params.profile.provider, "local-worktree");

    let open_response: JsonlResponseEnvelope = read_fixture("open_response_envelope.json");
    let session: SandboxSession = serde_json::from_value(open_response.result.unwrap()).unwrap();
    assert_eq!(session.id, "session_1");

    let exec_request: JsonlRequestEnvelope<SandboxExecRequest> =
        read_fixture("exec_request_envelope.json");
    assert_eq!(exec_request.method, "exec");
    assert_eq!(exec_request.params.stdin_base64.as_deref(), Some("AAEC"));

    let exec_event: JsonlResponseEnvelope = read_fixture("exec_event_envelope.json");
    let event: SandboxExecEvent = serde_json::from_value(exec_event.event.unwrap()).unwrap();
    assert_eq!(event.r#type, "stdout");
    assert_eq!(event.data_base64.as_deref(), Some("aGVsbG8K"));

    let exec_response: JsonlResponseEnvelope = read_fixture("exec_response_envelope.json");
    let output: SandboxExecResult = serde_json::from_value(exec_response.result.unwrap()).unwrap();
    assert_eq!(output.exit_code, 0);
    assert_eq!(output.stdout_base64, "b2sK");

    let error_response: JsonlResponseEnvelope = read_fixture("error_response_envelope.json");
    assert_eq!(
        error_response
            .error
            .as_ref()
            .map(|error| error.code.as_str()),
        Some("bad_profile")
    );
}

#[test]
fn generated_schema_matches_checked_in_schema() {
    let schema = schema_for!(SandboxProviderV1Schema);
    let generated = serde_json::to_string_pretty(&schema).expect("schema serializes") + "\n";
    let path = manifest_path("schema/sandbox.v1.schema.json");

    if std::env::var_os("SMOL_UPDATE_SANDBOX_SCHEMA").is_some() {
        fs::write(&path, generated).expect("schema file can be updated");
        return;
    }

    let checked_in = fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "failed to read {}; run with SMOL_UPDATE_SANDBOX_SCHEMA=1 to create it: {error}",
            path.display()
        )
    });
    assert_eq!(generated, checked_in, "sandbox v1 JSON Schema is stale");
}
