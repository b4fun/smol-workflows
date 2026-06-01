use smol_workflow_engine::metadata::read_workflow_metadata;

fn fixture_path(name: &str) -> String {
    format!("../../ts/engine/test/fixtures/{name}")
}

#[test]
fn reads_exported_pure_literal_metadata() {
    let metadata = read_workflow_metadata(fixture_path("metadata-pure.workflow.js"))
        .expect("metadata read should not fail")
        .expect("metadata should be present");

    assert_eq!(metadata.name, "phase-provider-metadata");
    assert_eq!(
        metadata.description,
        "Exercise phase-level provider and model defaults"
    );
    assert_eq!(
        metadata.when_to_use.as_deref(),
        Some("Use for parser tests")
    );
    assert_eq!(metadata.phases.len(), 2);
    assert_eq!(metadata.phases[0].title, "Research");
    assert_eq!(metadata.phases[0].provider.as_deref(), Some("pi"));
    assert_eq!(metadata.phases[0].model.as_deref(), Some("opus"));
    assert_eq!(metadata.phases[1].title, "Verify");
    assert_eq!(metadata.phases[1].provider.as_deref(), Some("codex"));
}

#[test]
fn supports_comments_quoted_keys_and_nested_braces_in_strings() {
    let metadata = read_workflow_metadata(fixture_path("metadata-comments.workflow.js"))
        .expect("metadata read should not fail")
        .expect("metadata should be present");

    assert_eq!(metadata.name, "quoted-keys");
    assert_eq!(
        metadata.description,
        "description with { braces } in a string"
    );
    assert_eq!(metadata.phases.len(), 1);
    assert_eq!(
        metadata.phases[0].detail.as_deref(),
        Some("detail with // not a comment and /* not a comment */")
    );
    assert_eq!(metadata.phases[0].provider.as_deref(), Some("debug"));
}

#[test]
fn returns_none_when_required_fields_are_missing() {
    assert!(
        read_workflow_metadata(fixture_path("metadata-missing-description.workflow.js"))
            .expect("metadata read should not fail")
            .is_none()
    );
}

#[test]
fn rejects_non_literal_metadata() {
    assert!(
        read_workflow_metadata(fixture_path("metadata-dynamic.workflow.js"))
            .expect("metadata read should not fail")
            .is_none()
    );
    assert!(
        read_workflow_metadata(fixture_path("metadata-call.workflow.js"))
            .expect("metadata read should not fail")
            .is_none()
    );
}

#[test]
fn ignores_non_exported_metadata() {
    assert!(
        read_workflow_metadata(fixture_path("metadata-not-exported.workflow.js"))
            .expect("metadata read should not fail")
            .is_none()
    );
}
