use clap::{Arg, Command};
use log::{LevelFilter, Log, Metadata, Record};
use serde_json::{Map, Value};
use smol_workflow_engine::agent_providers::create_agent_provider;
use smol_workflow_engine::durable::runner::{run_local_durable_workflow, LocalDurableRunOptions};
use smol_workflow_engine::durable::sqlite::SqliteDurableStore;
use smol_workflow_engine::workflow::{run_workflow, RunWorkflowOptions};
use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
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
        _ => Ok(()),
    }
}

async fn run_command(script_path: String, argv: Vec<String>) -> anyhow::Result<()> {
    let options = parse_run_options(argv)?;
    init_logging(options.log_level);
    log::debug!(
        "cli run script={} backend={} agent_provider={} budget_allowance={:?}",
        script_path,
        options.backend,
        options.agent_provider,
        options.budget_allowance
    );
    if options.backend != "simple" && options.backend != "sqlite" {
        anyhow::bail!("Unsupported backend: {}", options.backend);
    }

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
    let result = if options.backend == "sqlite" {
        let mut store = SqliteDurableStore::open(&options.db_path)?;
        let mut durable_options = LocalDurableRunOptions::new(
            PathBuf::from(script_path),
            Value::Object(options.args),
            provider,
        );
        durable_options.budget_total = options.budget_allowance;
        durable_options.max_parallel_agent_requests = options.max_parallel_agent_requests;
        durable_options.resume_run_id = options.resume_run_id;
        durable_options.on_log = Some(&on_log);
        durable_options.on_phase = Some(&on_phase);
        run_local_durable_workflow(&mut store, durable_options)
            .await?
            .workflow
    } else {
        run_workflow(RunWorkflowOptions {
            script_path: PathBuf::from(script_path),
            args: Value::Object(options.args),
            agent_provider: provider,
            budget_total: options.budget_allowance,
            budget_spent: 0,
            nesting_depth: 0,
            max_parallel_agent_requests: options.max_parallel_agent_requests,
            agent_runner: None,
            on_log: Some(&on_log),
            on_phase: Some(&on_phase),
        })
        .await?
    };

    println!("{}", serde_json::to_string_pretty(&result.output.result)?);
    Ok(())
}

#[derive(Debug)]
struct RunCliOptions {
    backend: String,
    agent_provider: String,
    args: Map<String, Value>,
    budget_allowance: Option<u64>,
    max_parallel_agent_requests: Option<usize>,
    db_path: PathBuf,
    resume_run_id: Option<String>,
    log_level: LevelFilter,
}

fn parse_run_options(argv: Vec<String>) -> anyhow::Result<RunCliOptions> {
    let mut workflow_arg_tokens = Vec::new();
    let mut backend = "simple".to_string();
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

        if token == "--backend" || token.starts_with("--backend=") {
            let parsed = parse_flag_token(token, argv.get(index + 1).map(String::as_str))?;
            backend = parsed.value;
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
        backend,
        agent_provider,
        args: parse_workflow_args(&workflow_arg_tokens)?,
        budget_allowance,
        max_parallel_agent_requests,
        db_path,
        resume_run_id,
        log_level,
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

fn format_log_value(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        value => serde_json::to_string(value).unwrap_or_else(|_| String::from("<unprintable>")),
    }
}

fn cli_command() -> Command {
    Command::new("smol-wf")
        .about("CLI for the smol-workflows Rust engine")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(
            Command::new("run")
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
                    "Run options:\n  --backend simple|sqlite\n  --db smol-workflows.db\n  --resume-run run_id\n  --agent-provider debug|claude-code|codex|opencode|pi\n  --budget-allowance outputTokens\n  --max-parallel-agents count\n  --log-level off|error|warn|info|debug|trace\n  --debug\n  --args-<name> value\n  --args-from-file <json-file>",
                ),
        )
}
