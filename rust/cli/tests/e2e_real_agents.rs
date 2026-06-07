use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;

fn smol_wf() -> Command {
    Command::new(env!("CARGO_BIN_EXE_smol-wf"))
}

struct TempWorkspace {
    path: PathBuf,
}

impl TempWorkspace {
    fn new(provider: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "smol-wf-e2e-{}-{}",
            provider.replace(|ch: char| !ch.is_ascii_alphanumeric(), "-"),
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("temp e2e workspace should be created");
        copy_example("hello.mjs", &path);
        copy_example("workflow-parent.mjs", &path);
        copy_example("workflow-child.mjs", &path);
        copy_asset("events.mjs", &path);
        copy_asset("events-child.mjs", &path);
        Self { path }
    }

    fn script(&self, name: &str) -> String {
        self.path.join(name).to_string_lossy().into_owned()
    }
}

impl Drop for TempWorkspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn copy_example(name: &str, workspace: &Path) {
    fs::copy(Path::new("../../examples").join(name), workspace.join(name))
        .unwrap_or_else(|error| panic!("failed to copy example {name}: {error}"));
}

fn copy_asset(name: &str, workspace: &Path) {
    fs::copy(
        Path::new("tests/assets/e2e_real_agents").join(name),
        workspace.join(name),
    )
    .unwrap_or_else(|error| panic!("failed to copy e2e asset {name}: {error}"));
}

fn real_agent_providers() -> Vec<String> {
    std::env::var("SMOL_WF_E2E_AGENT_PROVIDERS")
        .or_else(|_| std::env::var("SMOL_WF_E2E_AGENT_PROVIDER"))
        .unwrap_or_else(|_| "pi,claude-code,codex,opencode".to_string())
        .split(',')
        .map(str::trim)
        .filter(|provider| !provider.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn max_parallel_agents() -> String {
    std::env::var("SMOL_WF_E2E_MAX_PARALLEL_AGENTS").unwrap_or_else(|_| "2".to_string())
}

fn run_example(
    provider: &str,
    label: &str,
    example: &str,
    db_path: &Path,
    extra_args: &[&str],
) -> Value {
    eprintln!("real-agent e2e provider={provider} example={label} start");
    let max_parallel = max_parallel_agents();
    let db_path = db_path.to_string_lossy().into_owned();
    let mut args = vec![
        "run",
        example,
        "--db",
        db_path.as_str(),
        "--agent-provider",
        provider,
        "--max-parallel-agents",
        max_parallel.as_str(),
    ];
    args.extend_from_slice(extra_args);

    let output = smol_wf().args(args).output().expect("smol-wf should run");

    assert!(
        output.status.success(),
        "workflow {example} failed with provider {provider}\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    eprintln!("real-agent e2e provider={provider} example={label} done");
    serde_json::from_slice(&output.stdout).expect("workflow stdout should be JSON")
}

fn run_events_example(provider: &str, example: &str, db_path: &Path) -> Vec<Value> {
    eprintln!("real-agent events e2e provider={provider} start");
    let max_parallel = max_parallel_agents();
    let db_path = db_path.to_string_lossy().into_owned();
    let output = smol_wf()
        .args([
            "run",
            example,
            "--db",
            db_path.as_str(),
            "--agent-provider",
            provider,
            "--max-parallel-agents",
            max_parallel.as_str(),
            "--events",
            "--args-provider",
            provider,
        ])
        .output()
        .expect("smol-wf should run");

    assert!(
        output.status.success(),
        "events workflow {example} failed with provider {provider}\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("events stdout should be UTF-8");
    let events = stdout
        .lines()
        .map(|line| serde_json::from_str(line).expect("event line should be JSON"))
        .collect::<Vec<Value>>();
    eprintln!("real-agent events e2e provider={provider} done");
    events
}

fn assert_events_flow(provider: &str, events: &[Value]) {
    assert!(
        events.len() >= 5,
        "provider={provider} should emit lifecycle, log/phase, agent, result events: {events:#?}"
    );
    assert_event_envelopes(provider, events);
    assert_no_workflow_errors(provider, events);
    assert_lifecycle_order(provider, events);
    assert_nested_workflow_scope(provider, events);

    let run_id = events[0]["metadata"]["runId"]
        .as_str()
        .unwrap_or_else(|| panic!("provider={provider} root started should include runId"));

    for event in events {
        assert_eq!(event["metadata"]["runId"], run_id, "provider={provider}");
    }

    assert!(
        events.iter().any(|event| {
            event["type"] == "workflow.phase"
                && event["metadata"]["workflowDepth"] == 0
                && event["data"]["name"] == "Prepare event test"
                && event["elapsedNanos"].as_u64().is_some()
        }),
        "provider={provider} should emit root workflow.phase: {events:#?}"
    );
    assert!(
        events.iter().any(|event| {
            event["type"] == "workflow.log"
                && event["metadata"]["workflowDepth"] == 0
                && event["data"]["message"]
                    .as_str()
                    .is_some_and(|message| message.contains(provider))
                && event["elapsedNanos"].as_u64().is_some()
        }),
        "provider={provider} should emit root workflow.log: {events:#?}"
    );

    let root_results = events
        .iter()
        .filter(|event| {
            event["type"] == "workflow.result"
                && event["metadata"]["workflowDepth"].as_u64() == Some(0)
        })
        .collect::<Vec<_>>();
    assert_eq!(
        root_results.len(),
        1,
        "provider={provider} should emit exactly one root result: {events:#?}"
    );
    let root_result = root_results[0];
    assert_eq!(
        root_result["data"]["results"]["provider"], provider,
        "provider={provider}"
    );
    assert_eq!(
        root_result["data"]["results"]["child"]["provider"], provider,
        "provider={provider}"
    );
    assert!(
        root_result["data"]["results"]["child"]["answer"].is_string(),
        "provider={provider}"
    );
    assert!(
        root_result["elapsedNanos"].as_u64().is_some(),
        "provider={provider}"
    );

    let agent_events = events
        .iter()
        .filter(|event| event["type"] == "workflow.agent_event")
        .collect::<Vec<_>>();
    assert!(
        !agent_events.is_empty(),
        "provider={provider} should emit at least one workflow.agent_event: {events:#?}"
    );
    for event in &agent_events {
        assert_eq!(
            event["metadata"]["provider"], provider,
            "provider={provider}"
        );
        assert_eq!(event["metadata"]["workflowDepth"], 1, "provider={provider}");
        assert!(
            event["metadata"]["parentStepId"].as_str().is_some(),
            "provider={provider} nested agent event should include parentStepId"
        );
        assert!(
            event["metadata"]["stepId"].as_str().is_some(),
            "provider={provider}"
        );
        assert!(
            event["metadata"]["sessionId"].as_str().is_some(),
            "provider={provider}"
        );
        assert!(
            event["elapsedNanos"].as_u64().is_some(),
            "provider={provider}"
        );
    }

    assert_provider_agent_event_format(provider, &agent_events);
}

fn assert_event_envelopes(provider: &str, events: &[Value]) {
    let mut previous_elapsed = 0;
    for (index, event) in events.iter().enumerate() {
        let event_type = event["type"].as_str().unwrap_or_else(|| {
            panic!("provider={provider} event[{index}] should have string type: {event:#?}")
        });
        assert!(
            event.get("data").is_some(),
            "provider={provider} event[{index}] should include data: {event:#?}"
        );
        assert!(
            event["metadata"].is_object(),
            "provider={provider} event[{index}] should include metadata object: {event:#?}"
        );
        let workflow_depth = event["metadata"]["workflowDepth"].as_u64().unwrap_or_else(|| {
            panic!("provider={provider} event[{index}] should include numeric workflowDepth: {event:#?}")
        });
        if workflow_depth == 0 {
            assert!(
                event["metadata"].get("parentStepId").is_none(),
                "provider={provider} root event[{index}] should not include parentStepId: {event:#?}"
            );
        } else {
            let parent_step_id = event["metadata"]["parentStepId"].as_str().unwrap_or_else(|| {
                panic!("provider={provider} nested event[{index}] should include parentStepId: {event:#?}")
            });
            assert!(
                parent_step_id.starts_with("step_"),
                "provider={provider} nested event[{index}] parentStepId should be opaque step_* id: {event:#?}"
            );
        }

        match event_type {
            "workflow.started" => {
                if workflow_depth == 0 {
                    assert_eq!(index, 0, "provider={provider} root started should be first");
                    assert!(
                        event["elapsedNanos"].is_null(),
                        "provider={provider} root started should omit elapsedNanos: {event:#?}"
                    );
                } else {
                    previous_elapsed = assert_elapsed(provider, index, event, previous_elapsed);
                }
                assert!(
                    event["data"]["startTime"]
                        .as_str()
                        .is_some_and(|value| value.contains('T') && value.ends_with('Z')),
                    "provider={provider} started event should include RFC3339-ish startTime: {event:#?}"
                );
            }
            "workflow.phase" => {
                assert!(
                    event["data"]["name"].as_str().is_some(),
                    "provider={provider} phase event should include data.name: {event:#?}"
                );
                previous_elapsed = assert_elapsed(provider, index, event, previous_elapsed);
            }
            "workflow.log" => {
                assert!(
                    event["data"]["message"].as_str().is_some(),
                    "provider={provider} log event should include data.message: {event:#?}"
                );
                previous_elapsed = assert_elapsed(provider, index, event, previous_elapsed);
            }
            "workflow.agent_event" => {
                let step_id = event["metadata"]["stepId"].as_str().unwrap_or_else(|| {
                    panic!("provider={provider} agent event should include stepId: {event:#?}")
                });
                assert!(
                    step_id.starts_with("step_") && step_id.len() > "step_".len(),
                    "provider={provider} stepId should be opaque step_* id, got {step_id}: {event:#?}"
                );
                assert_eq!(
                    event["metadata"]["provider"], provider,
                    "provider={provider} agent event provider metadata mismatch: {event:#?}"
                );
                assert!(
                    event["metadata"]["sessionId"].as_str().is_some(),
                    "provider={provider} agent event should include sessionId: {event:#?}"
                );
                assert!(
                    event["data"].is_object() || event["data"].is_array(),
                    "provider={provider} agent event data should be raw provider object/array: {event:#?}"
                );
                previous_elapsed = assert_elapsed(provider, index, event, previous_elapsed);
            }
            "workflow.result" => {
                assert!(
                    event["data"]["tokenUsage"].is_object(),
                    "provider={provider} result should include tokenUsage: {event:#?}"
                );
                for field in ["inputTokens", "outputTokens", "totalTokens"] {
                    assert!(
                        event["data"]["tokenUsage"][field].as_u64().is_some(),
                        "provider={provider} result tokenUsage.{field} should be a number: {event:#?}"
                    );
                }
                assert!(
                    event["data"]["results"].is_object(),
                    "provider={provider} result should include object results: {event:#?}"
                );
                previous_elapsed = assert_elapsed(provider, index, event, previous_elapsed);
            }
            other => panic!("provider={provider} unexpected event type {other}: {event:#?}"),
        }
    }
}

fn assert_elapsed(provider: &str, index: usize, event: &Value, previous_elapsed: u64) -> u64 {
    let elapsed = event["elapsedNanos"].as_u64().unwrap_or_else(|| {
        panic!("provider={provider} event[{index}] should include numeric elapsedNanos: {event:#?}")
    });
    assert!(
        elapsed >= previous_elapsed,
        "provider={provider} elapsedNanos should be non-decreasing at event[{index}]: {event:#?}"
    );
    elapsed
}

fn assert_no_workflow_errors(provider: &str, events: &[Value]) {
    assert!(
        events.iter().all(|event| event["type"] != "workflow.error"),
        "provider={provider} successful e2e event stream should not include workflow.error: {events:#?}"
    );
}

fn assert_lifecycle_order(provider: &str, events: &[Value]) {
    assert_eq!(events[0]["type"], "workflow.started", "provider={provider}");
    assert_eq!(
        events.last().unwrap()["type"],
        "workflow.result",
        "provider={provider} final event should be root result: {events:#?}"
    );
    let first_phase = index_of_event(events, "workflow.phase")
        .unwrap_or_else(|| panic!("provider={provider} should emit workflow.phase: {events:#?}"));
    let first_log = index_of_event(events, "workflow.log")
        .unwrap_or_else(|| panic!("provider={provider} should emit workflow.log: {events:#?}"));
    let first_agent = index_of_event(events, "workflow.agent_event").unwrap_or_else(|| {
        panic!("provider={provider} should emit workflow.agent_event: {events:#?}")
    });
    let result_index = events.len() - 1;
    assert!(
        first_phase < first_log && first_log < first_agent && first_agent < result_index,
        "provider={provider} expected started -> phase -> log -> agent_event -> result order: {events:#?}"
    );
}

fn index_of_event(events: &[Value], event_type: &str) -> Option<usize> {
    events.iter().position(|event| event["type"] == event_type)
}

fn assert_nested_workflow_scope(provider: &str, events: &[Value]) {
    let child_started_index = events
        .iter()
        .position(|event| {
            event["type"] == "workflow.started"
                && event["metadata"]["workflowDepth"].as_u64() == Some(1)
        })
        .unwrap_or_else(|| {
            panic!("provider={provider} should emit child workflow.started: {events:#?}")
        });
    let child_parent_step_id = events[child_started_index]["metadata"]["parentStepId"]
        .as_str()
        .unwrap_or_else(|| panic!("provider={provider} child started should include parentStepId"));

    let child_result_index = events
        .iter()
        .position(|event| {
            event["type"] == "workflow.result"
                && event["metadata"]["workflowDepth"].as_u64() == Some(1)
                && event["metadata"]["parentStepId"].as_str() == Some(child_parent_step_id)
        })
        .unwrap_or_else(|| {
            panic!("provider={provider} should emit child workflow.result: {events:#?}")
        });
    let root_result_index = events.len() - 1;
    assert!(
        child_started_index < child_result_index && child_result_index < root_result_index,
        "provider={provider} expected child lifecycle to complete before root result: {events:#?}"
    );

    assert!(
        events.iter().any(|event| {
            event["type"] == "workflow.phase"
                && event["metadata"]["workflowDepth"].as_u64() == Some(1)
                && event["metadata"]["parentStepId"].as_str() == Some(child_parent_step_id)
                && event["data"]["name"] == "Child event test"
        }),
        "provider={provider} should emit child workflow.phase: {events:#?}"
    );
    assert!(
        events.iter().any(|event| {
            event["type"] == "workflow.log"
                && event["metadata"]["workflowDepth"].as_u64() == Some(1)
                && event["metadata"]["parentStepId"].as_str() == Some(child_parent_step_id)
                && event["data"]["message"]
                    .as_str()
                    .is_some_and(|message| message.contains(provider))
        }),
        "provider={provider} should emit child workflow.log: {events:#?}"
    );
    assert!(
        events.iter().any(|event| {
            event["type"] == "workflow.agent_event"
                && event["metadata"]["workflowDepth"].as_u64() == Some(1)
                && event["metadata"]["parentStepId"].as_str() == Some(child_parent_step_id)
        }),
        "provider={provider} should emit nested child workflow.agent_event: {events:#?}"
    );
}

fn assert_provider_agent_event_format(provider: &str, agent_events: &[&Value]) {
    match provider {
        "codex" => {
            let session = agent_events
                .iter()
                .find(|event| {
                    event["data"]["type"] == "session_meta"
                        || event["data"]["type"] == "thread.started"
                })
                .unwrap_or_else(|| {
                    panic!("codex should emit session_meta/thread.started event: {agent_events:#?}")
                });
            let payload_session_id = session["data"]["payload"]["id"]
                .as_str()
                .or_else(|| session["data"]["thread_id"].as_str());
            assert_eq!(
                payload_session_id,
                session["metadata"]["sessionId"].as_str(),
                "codex session event id should match metadata sessionId"
            );
            assert!(
                agent_events.iter().any(|event| {
                    event["data"]["type"] == "turn_complete"
                        || event["data"]["type"] == "turn.completed"
                }),
                "codex should emit turn completion event: {agent_events:#?}"
            );
        }
        "pi" => {
            assert!(
                agent_events
                    .iter()
                    .any(|event| event["data"]["type"].as_str().is_some()),
                "pi should emit typed raw session events: {agent_events:#?}"
            );
            assert!(
                agent_events.iter().any(|event| {
                    event["data"]["id"] == event["metadata"]["sessionId"]
                        || event["data"]["sessionId"] == event["metadata"]["sessionId"]
                        || event["data"]["session_id"] == event["metadata"]["sessionId"]
                }),
                "pi should emit at least one event carrying the session id: {agent_events:#?}"
            );
        }
        "claude-code" => {
            let event = agent_events
                .iter()
                .find(|event| event["data"]["response"].is_object())
                .unwrap_or_else(|| {
                    panic!("claude-code should emit response wrapper: {agent_events:#?}")
                });
            assert_eq!(
                event["data"]["response"]["session_id"], event["metadata"]["sessionId"],
                "claude-code response session_id should match metadata sessionId"
            );
            assert!(
                event["data"]["response"]["type"].as_str().is_some()
                    || event["data"]["response"]["result"].is_string(),
                "claude-code response should include provider result shape: {event:#?}"
            );
        }
        "opencode" => {
            let event = agent_events
                .iter()
                .find(|event| {
                    event["data"]["response"].is_object()
                        || event["data"]["response"].is_array()
                        || event["data"]["session"].is_object()
                })
                .unwrap_or_else(|| {
                    panic!("opencode should emit response/session wrapper: {agent_events:#?}")
                });
            assert!(
                contains_value(&event["data"], &event["metadata"]["sessionId"]),
                "opencode raw payload should contain metadata sessionId: {event:#?}"
            );
        }
        other => panic!("unsupported e2e provider for event format assertions: {other}"),
    }
}

fn contains_value(value: &Value, needle: &Value) -> bool {
    if value == needle {
        return true;
    }
    match value {
        Value::Array(items) => items.iter().any(|item| contains_value(item, needle)),
        Value::Object(object) => object.values().any(|value| contains_value(value, needle)),
        _ => false,
    }
}

fn run_provider_events_example(provider: &str) {
    let workspace = TempWorkspace::new(provider);
    eprintln!(
        "real-agent events e2e provider={provider} workspace={}",
        workspace.path.display()
    );
    let db_path = workspace.path.join("durable-events-e2e.db");
    let events = run_events_example(provider, &workspace.script("events.mjs"), &db_path);
    assert_events_flow(provider, &events);
}

fn run_provider_examples(provider: &str) {
    let workspace = TempWorkspace::new(provider);

    eprintln!(
        "real-agent e2e provider={provider} workspace={}",
        workspace.path.display()
    );

    let db_path = workspace.path.join("durable-e2e.db");

    let hello = run_example(
        provider,
        "hello",
        &workspace.script("hello.mjs"),
        &db_path,
        &["--budget-allowance", "20000", "--args-name", "Rust E2E"],
    );
    let hello_results = &hello["results"];
    assert_eq!(hello_results["name"], "Rust E2E", "provider={provider}");
    assert!(hello_results["plan"].is_string(), "provider={provider}");
    assert!(hello_results["drafts"].is_object(), "provider={provider}");
    assert!(
        hello_results["finalGreeting"].is_string(),
        "provider={provider}"
    );

    assert_eq!(
        hello_results["budget"]["total"], 20000,
        "provider={provider}"
    );
    assert!(
        hello_results["budget"]["spent"].is_number(),
        "provider={provider}"
    );
    assert!(
        hello_results["budget"]["remaining"].is_number(),
        "provider={provider}"
    );

    let parent = run_example(
        provider,
        "workflow-parent",
        &workspace.script("workflow-parent.mjs"),
        &db_path,
        &["--args-items", "alpha", "--args-items", "beta"],
    );
    let parent_results = &parent["results"];
    assert_eq!(
        parent_results["items"],
        serde_json::json!(["alpha", "beta"]),
        "provider={provider}"
    );
    assert_eq!(
        parent_results["childResults"].as_array().map(Vec::len),
        Some(2),
        "provider={provider}"
    );
    assert!(
        parent_results["synthesis"].is_string(),
        "provider={provider}"
    );
}

#[test]
#[ignore = "requires configured real agent providers; see rust/cli/README.md"]
fn e2e_real_agents_events() {
    let providers = real_agent_providers();
    assert!(
        !providers.is_empty(),
        "SMOL_WF_E2E_AGENT_PROVIDERS must include at least one provider"
    );

    let handles = providers
        .into_iter()
        .map(|provider| {
            thread::spawn(move || {
                eprintln!("real-agent events e2e provider={provider} start");
                run_provider_events_example(&provider);
                eprintln!("real-agent events e2e provider={provider} done");
                provider
            })
        })
        .collect::<Vec<_>>();

    for handle in handles {
        let provider = handle
            .join()
            .expect("real-agent events e2e provider thread panicked");
        eprintln!("real-agent events e2e provider completed: {provider}");
    }
}

#[test]
#[ignore = "requires configured real agent providers; see rust/cli/README.md"]
fn e2e_real_agents_examples() {
    let providers = real_agent_providers();
    assert!(
        !providers.is_empty(),
        "SMOL_WF_E2E_AGENT_PROVIDERS must include at least one provider"
    );

    let handles = providers
        .into_iter()
        .map(|provider| {
            thread::spawn(move || {
                eprintln!("real-agent e2e provider={provider} start");
                run_provider_examples(&provider);
                eprintln!("real-agent e2e provider={provider} done");
                provider
            })
        })
        .collect::<Vec<_>>();

    for handle in handles {
        let provider = handle
            .join()
            .expect("real-agent e2e provider thread panicked");
        eprintln!("real-agent e2e provider completed: {provider}");
    }
}
