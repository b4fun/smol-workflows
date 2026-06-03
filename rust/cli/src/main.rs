use clap::{Arg, Command as ClapCommand};
use log::{LevelFilter, Log, Metadata, Record};
use serde::Serialize;
use serde_json::{Map, Value};
use smol_workflow_engine::agent_providers::{create_agent_provider, AgentProviderResult};
use smol_workflow_engine::durable::runner::{run_local_durable_workflow, LocalDurableRunOptions};
use smol_workflow_engine::durable::sqlite::SqliteDurableStore;
use smol_workflow_engine::metadata::{read_workflow_metadata, WorkflowMetadata};
use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    if let Err(error) = run_cli(env::args().skip(1).collect()).await {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

async fn run_cli(argv: Vec<String>) -> anyhow::Result<()> {
    let matches = match cli_command()
        .try_get_matches_from(std::iter::once("smol-wf".to_string()).chain(argv))
    {
        Ok(matches) => matches,
        Err(error) if error.use_stderr() => return Err(error.into()),
        Err(error) => {
            error.print()?;
            return Ok(());
        }
    };

    match matches.subcommand() {
        Some(("run", matches)) => {
            let script_path = matches
                .get_one::<String>("workflow-script")
                .expect("required by clap")
                .clone();
            let run_args = matches
                .get_many::<String>("run-args")
                .map(|values| values.cloned().collect())
                .unwrap_or_default();
            run_command(script_path, run_args).await
        }
        Some(("llm", matches)) => match matches.subcommand() {
            Some(("list-workflows", _)) => list_workflows_command(Vec::new()).await,
            _ => Ok(()),
        },
        _ => Ok(()),
    }
}

async fn list_workflows_command(argv: Vec<String>) -> anyhow::Result<()> {
    if !argv.is_empty() {
        anyhow::bail!("llm list-workflows does not accept options yet");
    }

    let cwd = env::current_dir()?;
    let repo_root = find_repo_root(&cwd).unwrap_or(cwd);
    let workflows = discover_workflows(&repo_root)?;
    print_workflows_table(&workflows);
    Ok(())
}

#[derive(Debug)]
struct DiscoveredWorkflow {
    path: String,
    metadata: WorkflowMetadata,
}

fn find_repo_root(start: &Path) -> Option<PathBuf> {
    let output = ProcessCommand::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(start)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8(output.stdout).ok()?;
    let root = stdout.trim();
    if root.is_empty() {
        None
    } else {
        Some(PathBuf::from(root))
    }
}

fn discover_workflows(repo_root: &Path) -> anyhow::Result<Vec<DiscoveredWorkflow>> {
    let mut workflows = Vec::new();
    for relative_dir in [".agents/workflows", ".claude/workflows"] {
        let dir = repo_root.join(relative_dir);
        collect_workflows_in_dir(repo_root, &dir, &mut workflows)?;
    }
    workflows.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(workflows)
}

fn collect_workflows_in_dir(
    repo_root: &Path,
    dir: &Path,
    workflows: &mut Vec<DiscoveredWorkflow>,
) -> anyhow::Result<()> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };

    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_workflows_in_dir(repo_root, &path, workflows)?;
            continue;
        }
        if !file_type.is_file() || !is_workflow_script_path(&path) {
            continue;
        }
        let Some(metadata) = read_workflow_metadata(&path)? else {
            continue;
        };
        workflows.push(DiscoveredWorkflow {
            path: relative_display_path(repo_root, &path),
            metadata,
        });
    }
    Ok(())
}

fn is_workflow_script_path(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some("js" | "mjs")
    )
}

fn relative_display_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn print_workflows_table(workflows: &[DiscoveredWorkflow]) {
    let name_width = workflows
        .iter()
        .map(|workflow| workflow.metadata.name.chars().count())
        .chain(["NAME".len()])
        .max()
        .unwrap_or("NAME".len());
    let path_width = workflows
        .iter()
        .map(|workflow| workflow.path.chars().count())
        .chain(["PATH".len()])
        .max()
        .unwrap_or("PATH".len());

    println!(
        "{:<name_width$}  {:<path_width$}  DESCRIPTION",
        "NAME", "PATH"
    );
    for workflow in workflows {
        println!(
            "{:<name_width$}  {:<path_width$}  {}",
            workflow.metadata.name, workflow.path, workflow.metadata.description
        );
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CliRunReport {
    token_usage: CliTokenUsageReport,
    #[serde(rename = "runID")]
    run_id: String,
    results: Value,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CliTokenUsageReport {
    input_tokens: u64,
    output_tokens: u64,
    total_tokens: u64,
}

async fn run_command(script_path: String, argv: Vec<String>) -> anyhow::Result<()> {
    let options = parse_run_options(argv)?;
    init_logging(options.log_level);
    log::debug!(
        "cli run script={} db={} agent_provider={} budget_allowance={:?}",
        script_path,
        options.db_path.display(),
        options.agent_provider,
        options.budget_allowance
    );

    let provider: Arc<dyn smol_workflow_engine::agent_providers::AgentProvider> =
        Arc::from(create_agent_provider(&options.agent_provider)?);
    let on_phase = |phase: &smol_workflow_engine::workflow::WorkflowPhaseCall| {
        let mut stderr = io::stderr().lock();
        match &phase.options {
            Some(options) => {
                let _ = writeln!(
                    stderr,
                    "[phase] {} {}",
                    phase.name,
                    format_log_value(options)
                );
            }
            None => {
                let _ = writeln!(stderr, "[phase] {}", phase.name);
            }
        }
        let _ = stderr.flush();
    };
    let on_log = |values: &[Value]| {
        let values = values
            .iter()
            .map(format_log_value)
            .collect::<Vec<_>>()
            .join(" ");
        let mut stderr = io::stderr().lock();
        let _ = writeln!(stderr, "[log] {values}");
        let _ = stderr.flush();
    };
    let mut store = SqliteDurableStore::open(&options.db_path)?;
    let mut durable_options = LocalDurableRunOptions::new(
        PathBuf::from(script_path),
        Value::Object(options.args),
        provider,
    );
    durable_options.budget_total = options.budget_allowance;
    durable_options.max_parallel_agent_requests = options.max_parallel_agent_requests;
    durable_options.resume_run_id = options.resume_run_id;
    let on_agent_result = |_: &str, provider: &str, result: &AgentProviderResult| {
        if let Some(dir) = options.save_raw_sessions.as_deref() {
            save_raw_session(dir, provider, result)?;
        }
        Ok(())
    };
    durable_options.on_log = Some(&on_log);
    durable_options.on_phase = Some(&on_phase);
    durable_options.on_agent_result = Some(&on_agent_result);
    let result = run_local_durable_workflow(&mut store, durable_options).await?;
    let workflow = result.workflow;
    let report = CliRunReport {
        token_usage: CliTokenUsageReport {
            input_tokens: workflow.token_usage.input_tokens,
            output_tokens: workflow.token_usage.output_tokens,
            total_tokens: workflow.token_usage.total_tokens,
        },
        run_id: result.run_id,
        results: workflow.output.result,
    };

    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

#[derive(Debug)]
struct RunCliOptions {
    agent_provider: String,
    args: Map<String, Value>,
    budget_allowance: Option<u64>,
    max_parallel_agent_requests: Option<usize>,
    db_path: PathBuf,
    resume_run_id: Option<String>,
    log_level: LevelFilter,
    save_raw_sessions: Option<PathBuf>,
}

fn parse_run_options(argv: Vec<String>) -> anyhow::Result<RunCliOptions> {
    let mut workflow_arg_tokens = Vec::new();
    let mut agent_provider =
        env::var("SMOL_WF_AGENT_PROVIDER").unwrap_or_else(|_| "debug".to_string());
    let mut budget_allowance = env::var("SMOL_WF_BUDGET_ALLOWANCE")
        .ok()
        .map(|value| parse_non_negative_integer(&value, "SMOL_WF_BUDGET_ALLOWANCE"))
        .transpose()?;
    let mut log_level = env::var("SMOL_WF_LOG")
        .ok()
        .map(|value| parse_log_level(&value, "SMOL_WF_LOG"))
        .transpose()?
        .unwrap_or(LevelFilter::Off);
    let mut resume_run_id = None;
    let mut db_path = env::var("SMOL_WF_DB")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("smol-workflows.db"));
    let mut max_parallel_agent_requests = env::var("SMOL_WF_MAX_PARALLEL_AGENTS")
        .ok()
        .map(|value| parse_positive_usize(&value, "SMOL_WF_MAX_PARALLEL_AGENTS"))
        .transpose()?;
    let mut save_raw_sessions = None;
    let mut index = 0;

    while index < argv.len() {
        let token = &argv[index];

        if token == "--agent-provider" || token.starts_with("--agent-provider=") {
            let parsed = parse_flag_token(token, argv.get(index + 1).map(String::as_str))?;
            agent_provider = parsed.value;
            if parsed.consumed_next {
                index += 1;
            }
            index += 1;
            continue;
        }

        if token == "--resume-run" || token.starts_with("--resume-run=") {
            let parsed = parse_flag_token(token, argv.get(index + 1).map(String::as_str))?;
            resume_run_id = Some(parsed.value);
            if parsed.consumed_next {
                index += 1;
            }
            index += 1;
            continue;
        }

        if token == "--db" || token.starts_with("--db=") {
            let parsed = parse_flag_token(token, argv.get(index + 1).map(String::as_str))?;
            db_path = PathBuf::from(parsed.value);
            if parsed.consumed_next {
                index += 1;
            }
            index += 1;
            continue;
        }

        if token == "--save-raw-sessions" || token.starts_with("--save-raw-sessions=") {
            let parsed = parse_flag_token(token, argv.get(index + 1).map(String::as_str))?;
            let path = PathBuf::from(parsed.value);
            if !path.is_dir() {
                anyhow::bail!("--save-raw-sessions must point to an existing directory");
            }
            save_raw_sessions = Some(path);
            if parsed.consumed_next {
                index += 1;
            }
            index += 1;
            continue;
        }

        if token == "--budget-allowance" || token.starts_with("--budget-allowance=") {
            let parsed = parse_flag_token(token, argv.get(index + 1).map(String::as_str))?;
            budget_allowance = Some(parse_non_negative_integer(
                &parsed.value,
                "--budget-allowance",
            )?);
            if parsed.consumed_next {
                index += 1;
            }
            index += 1;
            continue;
        }

        if token == "--log-level" || token.starts_with("--log-level=") {
            let parsed = parse_flag_token(token, argv.get(index + 1).map(String::as_str))?;
            log_level = parse_log_level(&parsed.value, "--log-level")?;
            if parsed.consumed_next {
                index += 1;
            }
            index += 1;
            continue;
        }

        if token == "--max-parallel-agents" || token.starts_with("--max-parallel-agents=") {
            let parsed = parse_flag_token(token, argv.get(index + 1).map(String::as_str))?;
            max_parallel_agent_requests = Some(parse_positive_usize(
                &parsed.value,
                "--max-parallel-agents",
            )?);
            if parsed.consumed_next {
                index += 1;
            }
            index += 1;
            continue;
        }

        if token == "--debug" {
            log_level = LevelFilter::Debug;
            index += 1;
            continue;
        }

        if is_workflow_arg_token(token) {
            workflow_arg_tokens.push(token.clone());
            if !token.contains('=') {
                if let Some(next) = argv.get(index + 1) {
                    if !next.starts_with("--") {
                        workflow_arg_tokens.push(next.clone());
                        index += 1;
                    }
                }
            }
            index += 1;
            continue;
        }

        anyhow::bail!(
            "Unknown option: {token}. Run arguments must use --args-<name> or --args-from-file."
        );
    }

    Ok(RunCliOptions {
        agent_provider,
        args: parse_workflow_args(&workflow_arg_tokens)?,
        budget_allowance,
        max_parallel_agent_requests,
        db_path,
        resume_run_id,
        log_level,
        save_raw_sessions,
    })
}

fn parse_workflow_args(argv: &[String]) -> anyhow::Result<Map<String, Value>> {
    let mut args = Map::new();
    let mut index = 0;

    while index < argv.len() {
        let token = &argv[index];
        if !token.starts_with("--") {
            anyhow::bail!("Unexpected positional argument: {token}");
        }

        if token == "--args-from-file" {
            let Some(file_path) = argv.get(index + 1) else {
                anyhow::bail!("Missing value for --args-from-file");
            };
            if file_path.starts_with("--") {
                anyhow::bail!("Missing value for --args-from-file");
            }
            merge_args(&mut args, read_args_file(file_path)?);
            index += 2;
            continue;
        }

        if let Some(file_path) = token.strip_prefix("--args-from-file=") {
            if file_path.is_empty() {
                anyhow::bail!("Missing value for --args-from-file");
            }
            merge_args(&mut args, read_args_file(file_path)?);
            index += 1;
            continue;
        }

        let Some(raw_arg) = token.strip_prefix("--args-") else {
            anyhow::bail!(
                "Unknown option: {token}. Run arguments must use --args-<name> or --args-from-file."
            );
        };

        if let Some((key, value)) = raw_arg.split_once('=') {
            assign_arg(&mut args, key, Value::String(value.to_string()));
            index += 1;
            continue;
        }

        let key = raw_arg;
        let value = match argv.get(index + 1) {
            Some(next) if !next.starts_with("--") => {
                index += 1;
                Value::String(next.clone())
            }
            _ => Value::Bool(true),
        };
        assign_arg(&mut args, key, value);
        index += 1;
    }

    Ok(args)
}

fn read_args_file(file_path: &str) -> anyhow::Result<Map<String, Value>> {
    let value: Value = serde_json::from_str(&fs::read_to_string(file_path)?)?;
    match value {
        Value::Object(object) => Ok(object),
        _ => anyhow::bail!("--args-from-file must contain a JSON object"),
    }
}

fn merge_args(args: &mut Map<String, Value>, from_file: Map<String, Value>) {
    for (key, value) in from_file {
        assign_arg(args, &key, value);
    }
}

fn assign_arg(args: &mut Map<String, Value>, key: &str, value: Value) {
    match args.remove(key) {
        None => {
            args.insert(key.to_string(), value);
        }
        Some(Value::Array(mut values)) => {
            values.push(value);
            args.insert(key.to_string(), Value::Array(values));
        }
        Some(previous) => {
            args.insert(key.to_string(), Value::Array(vec![previous, value]));
        }
    }
}

fn is_workflow_arg_token(token: &str) -> bool {
    token == "--args-from-file"
        || token.starts_with("--args-from-file=")
        || token.starts_with("--args-")
}

struct ParsedFlag {
    value: String,
    consumed_next: bool,
}

fn parse_flag_token(token: &str, next: Option<&str>) -> anyhow::Result<ParsedFlag> {
    let Some(without_prefix) = token.strip_prefix("--") else {
        anyhow::bail!("Expected option, got: {token}");
    };

    if let Some((_key, value)) = without_prefix.split_once('=') {
        return Ok(ParsedFlag {
            value: value.to_string(),
            consumed_next: false,
        });
    }

    match next {
        Some(next) if !next.starts_with("--") => Ok(ParsedFlag {
            value: next.to_string(),
            consumed_next: true,
        }),
        _ => Ok(ParsedFlag {
            value: "true".to_string(),
            consumed_next: false,
        }),
    }
}

fn parse_non_negative_integer(value: &str, name: &str) -> anyhow::Result<u64> {
    let trimmed = value.trim();
    let parsed = trimmed
        .parse::<u64>()
        .map_err(|_| anyhow::anyhow!("{name} must be a non-negative integer"))?;
    if parsed.to_string() != trimmed {
        anyhow::bail!("{name} must be a non-negative integer");
    }
    Ok(parsed)
}

fn parse_positive_usize(value: &str, name: &str) -> anyhow::Result<usize> {
    let parsed = parse_non_negative_integer(value, name)?;
    if parsed == 0 {
        anyhow::bail!("{name} must be greater than zero");
    }
    usize::try_from(parsed).map_err(|_| anyhow::anyhow!("{name} is too large"))
}

fn parse_log_level(value: &str, name: &str) -> anyhow::Result<LevelFilter> {
    match value.trim().to_ascii_lowercase().as_str() {
        "off" | "none" | "quiet" => Ok(LevelFilter::Off),
        "error" => Ok(LevelFilter::Error),
        "warn" | "warning" => Ok(LevelFilter::Warn),
        "info" => Ok(LevelFilter::Info),
        "debug" => Ok(LevelFilter::Debug),
        "trace" => Ok(LevelFilter::Trace),
        _ => anyhow::bail!("{name} must be one of off, error, warn, info, debug, trace"),
    }
}

static LOGGER: DimStderrLogger = DimStderrLogger;
static LOGGER_LEVEL: AtomicUsize = AtomicUsize::new(0);

struct DimStderrLogger;

impl Log for DimStderrLogger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level().to_level_filter() <= current_log_level()
    }

    fn log(&self, record: &Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let mut stderr = io::stderr().lock();
        let _ = writeln!(
            stderr,
            "\x1b[2m[{}] {}\x1b[0m",
            record.level().to_string().to_ascii_lowercase(),
            record.args()
        );
        let _ = stderr.flush();
    }

    fn flush(&self) {
        let _ = io::stderr().flush();
    }
}

fn init_logging(level: LevelFilter) {
    LOGGER_LEVEL.store(level_to_usize(level), Ordering::Relaxed);
    if log::set_logger(&LOGGER).is_ok() {
        log::set_max_level(LevelFilter::Trace);
    }
}

fn current_log_level() -> LevelFilter {
    usize_to_level(LOGGER_LEVEL.load(Ordering::Relaxed))
}

fn level_to_usize(level: LevelFilter) -> usize {
    match level {
        LevelFilter::Off => 0,
        LevelFilter::Error => 1,
        LevelFilter::Warn => 2,
        LevelFilter::Info => 3,
        LevelFilter::Debug => 4,
        LevelFilter::Trace => 5,
    }
}

fn usize_to_level(value: usize) -> LevelFilter {
    match value {
        1 => LevelFilter::Error,
        2 => LevelFilter::Warn,
        3 => LevelFilter::Info,
        4 => LevelFilter::Debug,
        5 => LevelFilter::Trace,
        _ => LevelFilter::Off,
    }
}

fn save_raw_session(
    root: &Path,
    provider: &str,
    result: &AgentProviderResult,
) -> anyhow::Result<()> {
    let Some(session_id) = result.session_id.as_deref() else {
        return Ok(());
    };
    let Some(raw) = result.raw.as_ref() else {
        return Ok(());
    };
    ensure_safe_path_component(provider, "provider name")?;
    ensure_safe_path_component(session_id, "session id")?;
    let provider_dir = root.join(provider);
    fs::create_dir_all(&provider_dir)?;
    let path = provider_dir.join(format!("{session_id}.jsonl"));
    let mut file = fs::File::create(path)?;

    if let Some(events) = raw.get("events").and_then(Value::as_array) {
        write_json_lines(&mut file, events)?;
    } else if let Some(items) = raw.as_array() {
        write_json_lines(&mut file, items)?;
    } else {
        writeln!(file, "{}", serde_json::to_string(raw)?)?;
    }
    Ok(())
}

fn write_json_lines(file: &mut fs::File, values: &[Value]) -> anyhow::Result<()> {
    for value in values {
        writeln!(file, "{}", serde_json::to_string(value)?)?;
    }
    Ok(())
}

fn ensure_safe_path_component(value: &str, label: &str) -> anyhow::Result<()> {
    if value.is_empty()
        || value.contains('/')
        || value.contains('\\')
        || value == "."
        || value == ".."
    {
        anyhow::bail!("provider {label} is not safe for a raw session file path: {value}");
    }
    Ok(())
}

fn format_log_value(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        value => serde_json::to_string(value).unwrap_or_else(|_| String::from("<unprintable>")),
    }
}

fn cli_command() -> ClapCommand {
    ClapCommand::new("smol-wf")
        .about("CLI for the smol-workflows Rust engine")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(
            ClapCommand::new("run")
                .about("Run a workflow script")
                .arg(
                    Arg::new("workflow-script")
                        .value_name("workflow-script")
                        .help("Workflow JavaScript module to run")
                        .required(true),
                )
                .arg(
                    Arg::new("run-args")
                        .value_name("run-options")
                        .help("Run options and workflow args")
                        .num_args(0..)
                        .trailing_var_arg(true)
                        .allow_hyphen_values(true),
                )
                .after_help(
                    "Run options:\n  --db smol-workflows.db\n  --resume-run run_id\n  --agent-provider debug|claude-code|codex|opencode|pi\n  --budget-allowance outputTokens\n  --max-parallel-agents count\n  --save-raw-sessions dir\n  --log-level off|error|warn|info|debug|trace\n  --debug\n  --args-<name> value\n  --args-from-file <json-file>",
                ),
        )
        .subcommand(
            ClapCommand::new("llm")
                .about("LLM-facing helper commands")
                .subcommand_required(true)
                .arg_required_else_help(true)
                .subcommand(ClapCommand::new("list-workflows").about("List discoverable workflows")),
        )
}
