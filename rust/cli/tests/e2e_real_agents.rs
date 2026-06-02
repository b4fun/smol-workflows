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
    assert_eq!(hello["name"], "Rust E2E", "provider={provider}");
    assert!(hello["plan"].is_string(), "provider={provider}");
    assert!(hello["drafts"].is_object(), "provider={provider}");
    assert!(hello["finalGreeting"].is_string(), "provider={provider}");

    assert_eq!(hello["budget"]["total"], 20000, "provider={provider}");
    assert!(hello["budget"]["spent"].is_number(), "provider={provider}");
    assert!(
        hello["budget"]["remaining"].is_number(),
        "provider={provider}"
    );

    let parent = run_example(
        provider,
        "workflow-parent",
        &workspace.script("workflow-parent.mjs"),
        &db_path,
        &["--args-items", "alpha", "--args-items", "beta"],
    );
    assert_eq!(
        parent["items"],
        serde_json::json!(["alpha", "beta"]),
        "provider={provider}"
    );
    assert_eq!(
        parent["childResults"].as_array().map(Vec::len),
        Some(2),
        "provider={provider}"
    );
    assert!(parent["synthesis"].is_string(), "provider={provider}");
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
