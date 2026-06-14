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
    let _: CapabilitiesRequest = read_fixture("capabilities_request.json");
    let _: CapabilitiesResponse = read_fixture("capabilities_response.json");
    let _: OpenSandboxRequest = read_fixture("open_request.json");
    let _: OpenSandboxResponse = read_fixture("open_response.json");
    let _: CloseSandboxRequest = read_fixture("close_request.json");
    let _: CloseSandboxResponse = read_fixture("close_response.json");
    let _: CleanupSandboxGroupRequest = read_fixture("cleanup_group_request.json");
    let _: CleanupSandboxGroupResponse = read_fixture("cleanup_group_response.json");
    let _: ExecRequest = read_fixture("exec_request.json");
    let _: ExecResponse = read_fixture("exec_response.json");

    // Provider-declared failures can omit the success payload.
    let capabilities_error: CapabilitiesResponse = read_fixture("error_response.json");
    assert_eq!(
        capabilities_error
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
