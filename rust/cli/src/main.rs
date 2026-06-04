mod dto_history;

use clap::{Arg, Command as ClapCommand};
use comfy_table::{presets::NOTHING, Cell, Table};
use dto_history::{
    HistoryAttempt, HistoryOptions, HistoryRunDetail, HistoryRunSummary, HistoryStep,
    HistoryStepAgent, HistoryTokenUsage, HistoryTokenUsageTotals, HistoryWorkflowRunResource,
    OutputFormat,
};
use log::{LevelFilter, Log, Metadata, Record};
use rusqlite::OptionalExtension;
use serde::Serialize;
use serde_json::{Map, Value};
use smol_workflow_engine::agent_providers::{create_agent_provider, AgentProviderResult};
use smol_workflow_engine::durable::json::WorkflowRunJSON;
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
use std::time::{SystemTime, UNIX_EPOCH};

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
        Some(("history", matches)) => {
            let mut args: Vec<String> = matches
                .get_many::<String>("history-args")
                .map(|values| values.cloned().collect())
                .unwrap_or_default();
            if let Some(output) = matches.get_one::<String>("output") {
                args.extend(["--output".to_string(), output.clone()]);
            }
            history_command(args).await
        }
        _ => Ok(()),
    }
}

async fn history_command(argv: Vec<String>) -> anyhow::Result<()> {
    let (run_id, options) = parse_history_options(argv)?;
    if !options.db_path.exists() {
        anyhow::bail!(
            "history database {} was not found; pass --db or run a workflow first",
            options.db_path.display()
        );
    }
    let store = SqliteDurableStore::open(&options.db_path)?;

    if let Some(run_id) = run_id {
        let detail = load_history_detail(&store, &run_id)?.ok_or_else(|| {
            anyhow::anyhow!(
                "workflow run {run_id} was not found in {}; check --db",
                options.db_path.display()
            )
        })?;
        match options.format {
            OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&detail)?),
            OutputFormat::Table => print_history_detail_table(&detail),
        }
    } else {
        let runs = load_history_runs(&store, &options)?;
        match options.format {
            OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&runs)?),
            OutputFormat::Table => print_history_runs_table(&runs),
        }
    }
    Ok(())
}

fn parse_history_options(argv: Vec<String>) -> anyhow::Result<(Option<String>, HistoryOptions)> {
    let mut run_id = None;
    let mut db_path = PathBuf::from("smol-workflows.db");
    let mut format = OutputFormat::Table;
    let mut state = None;
    let mut name = None;
    let mut since = None;
    let mut until = None;
    let mut limit = 50usize;
    let mut index = 0;

    while index < argv.len() {
        let token = &argv[index];
        if token == "--db" || token.starts_with("--db=") {
            let parsed = parse_required_history_flag(
                token,
                argv.get(index + 1).map(String::as_str),
                "--db",
            )?;
            db_path = PathBuf::from(parsed.value);
            if parsed.consumed_next {
                index += 1;
            }
            index += 1;
            continue;
        }
        if token == "--output" || token.starts_with("--output=") {
            let parsed = parse_required_history_flag(
                token,
                argv.get(index + 1).map(String::as_str),
                "--output",
            )?;
            format = parse_output_format(&parsed.value)?;
            if parsed.consumed_next {
                index += 1;
            }
            index += 1;
            continue;
        }
        if token == "-o" {
            let Some(value) = argv.get(index + 1) else {
                anyhow::bail!("Missing value for -o");
            };
            if value.starts_with('-') {
                anyhow::bail!("Missing value for -o");
            }
            format = parse_output_format(value)?;
            index += 2;
            continue;
        }
        if token == "--state" || token.starts_with("--state=") {
            let parsed = parse_required_history_flag(
                token,
                argv.get(index + 1).map(String::as_str),
                "--state",
            )?;
            validate_history_state(&parsed.value)?;
            state = Some(parsed.value);
            if parsed.consumed_next {
                index += 1;
            }
            index += 1;
            continue;
        }
        if token == "--name" || token.starts_with("--name=") {
            let parsed = parse_required_history_flag(
                token,
                argv.get(index + 1).map(String::as_str),
                "--name",
            )?;
            name = Some(parsed.value);
            if parsed.consumed_next {
                index += 1;
            }
            index += 1;
            continue;
        }
        if token == "--since" || token.starts_with("--since=") {
            let parsed = parse_required_history_flag(
                token,
                argv.get(index + 1).map(String::as_str),
                "--since",
            )?;
            since = Some(parse_i64(&parsed.value, "--since")?);
            if parsed.consumed_next {
                index += 1;
            }
            index += 1;
            continue;
        }
        if token == "--until" || token.starts_with("--until=") {
            let parsed = parse_required_history_flag(
                token,
                argv.get(index + 1).map(String::as_str),
                "--until",
            )?;
            until = Some(parse_i64(&parsed.value, "--until")?);
            if parsed.consumed_next {
                index += 1;
            }
            index += 1;
            continue;
        }
        if token == "--limit" || token.starts_with("--limit=") {
            let parsed = parse_required_history_flag(
                token,
                argv.get(index + 1).map(String::as_str),
                "--limit",
            )?;
            limit = parse_positive_usize(&parsed.value, "--limit")?;
            if parsed.consumed_next {
                index += 1;
            }
            index += 1;
            continue;
        }
        if token.starts_with('-') {
            anyhow::bail!("Unknown history option: {token}");
        }
        if run_id.replace(token.clone()).is_some() {
            anyhow::bail!("history accepts at most one run id");
        }
        index += 1;
    }

    Ok((
        run_id,
        HistoryOptions {
            db_path,
            format,
            state,
            name,
            since,
            until,
            limit,
        },
    ))
}

fn parse_required_history_flag(
    token: &str,
    next: Option<&str>,
    name: &str,
) -> anyhow::Result<ParsedFlag> {
    let Some(without_prefix) = token.strip_prefix("--") else {
        anyhow::bail!("Expected option, got: {token}");
    };
    if let Some((_key, value)) = without_prefix.split_once('=') {
        if value.is_empty() {
            anyhow::bail!("Missing value for {name}");
        }
        return Ok(ParsedFlag {
            value: value.to_string(),
            consumed_next: false,
        });
    }
    match next {
        Some(next) if !next.starts_with('-') => Ok(ParsedFlag {
            value: next.to_string(),
            consumed_next: true,
        }),
        _ => anyhow::bail!("Missing value for {name}"),
    }
}

fn parse_output_format(value: &str) -> anyhow::Result<OutputFormat> {
    match value.trim().to_ascii_lowercase().as_str() {
        "table" => Ok(OutputFormat::Table),
        "json" => Ok(OutputFormat::Json),
        _ => anyhow::bail!("--output must be one of table, json"),
    }
}

fn validate_history_state(value: &str) -> anyhow::Result<()> {
    match value {
        "pending" | "running" | "completed" | "failed" | "cancelled" => Ok(()),
        _ => anyhow::bail!("--state must be one of pending, running, completed, failed, cancelled"),
    }
}

fn parse_i64(value: &str, name: &str) -> anyhow::Result<i64> {
    value
        .trim()
        .parse::<i64>()
        .map_err(|_| anyhow::anyhow!("{name} must be an integer Unix epoch millisecond timestamp"))
}

fn load_history_runs(
    store: &SqliteDurableStore,
    options: &HistoryOptions,
) -> anyhow::Result<Vec<HistoryRunSummary>> {
    let mut statement = store.connection().prepare(
        r#"
        SELECT
          r.run_id,
          r.task_id,
          t.submitted_by_owner_id,
          r.root_run_id,
          r.state,
          r.workflow_run_json,
          r.created_at,
          r.updated_at,
          COALESCE((SELECT COUNT(*) FROM sw_workflow_attempts a WHERE a.run_id = r.run_id), 0),
          COALESCE((SELECT COUNT(*) FROM sw_workflow_steps s WHERE s.run_id = r.run_id AND s.state = 'completed'), 0),
          COALESCE((SELECT COUNT(*) FROM sw_workflow_steps s WHERE s.run_id = r.run_id AND s.state = 'failed'), 0),
          COALESCE((SELECT SUM(
            COALESCE(
              json_extract(s.result_json, '$.usage.totalTokens'),
              COALESCE(json_extract(s.result_json, '$.usage.inputTokens'), 0)
                + COALESCE(json_extract(s.result_json, '$.usage.cacheReadTokens'), 0)
                + COALESCE(json_extract(s.result_json, '$.usage.outputTokens'), 0)
            )
          ) FROM sw_workflow_steps s WHERE s.run_id = r.run_id AND s.result_json IS NOT NULL), 0)
        FROM sw_workflow_runs r
        JOIN sw_workflow_tasks t ON t.task_id = r.task_id
        ORDER BY r.created_at DESC
        "#,
    )?;
    let mut rows = statement.query([])?;
    let mut runs = Vec::new();
    while let Some(row) = rows.next()? {
        let run = history_run_summary_from_row(row)?;
        if !history_run_matches(&run, options) {
            continue;
        }
        runs.push(run);
        if runs.len() >= options.limit {
            break;
        }
    }
    Ok(runs)
}

fn load_history_detail(
    store: &SqliteDurableStore,
    run_id: &str,
) -> anyhow::Result<Option<HistoryRunDetail>> {
    let summary = store
        .connection()
        .query_row(
            r#"
            SELECT
              r.run_id,
              r.task_id,
              t.submitted_by_owner_id,
              r.root_run_id,
              r.state,
              r.workflow_run_json,
              r.created_at,
              r.updated_at,
              COALESCE((SELECT COUNT(*) FROM sw_workflow_attempts a WHERE a.run_id = r.run_id), 0),
              COALESCE((SELECT COUNT(*) FROM sw_workflow_steps s WHERE s.run_id = r.run_id AND s.state = 'completed'), 0),
              COALESCE((SELECT COUNT(*) FROM sw_workflow_steps s WHERE s.run_id = r.run_id AND s.state = 'failed'), 0),
              COALESCE((SELECT SUM(
                COALESCE(
                  json_extract(s.result_json, '$.usage.totalTokens'),
                  COALESCE(json_extract(s.result_json, '$.usage.inputTokens'), 0)
                    + COALESCE(json_extract(s.result_json, '$.usage.cacheReadTokens'), 0)
                    + COALESCE(json_extract(s.result_json, '$.usage.outputTokens'), 0)
                )
              ) FROM sw_workflow_steps s WHERE s.run_id = r.run_id AND s.result_json IS NOT NULL), 0)
            FROM sw_workflow_runs r
            JOIN sw_workflow_tasks t ON t.task_id = r.task_id
            WHERE r.run_id = ?1
            "#,
            [run_id],
            history_run_summary_from_row,
        )
        .optional()?;
    let Some(summary) = summary else {
        return Ok(None);
    };

    let (args_json, completed_payload_json, failure_reason_json): (
        String,
        Option<String>,
        Option<String>,
    ) = store.connection().query_row(
        r#"
        SELECT args_json, completed_payload_json, failure_reason_json
        FROM sw_workflow_runs
        WHERE run_id = ?1
        "#,
        [run_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;

    let completed_payload = parse_optional_json_string(completed_payload_json.as_deref())?;
    let results = history_results(completed_payload);
    let workflow_run = history_workflow_run_resource(
        summary,
        parse_json_string(&args_json)?,
        parse_optional_json_string(failure_reason_json.as_deref())?,
    );

    Ok(Some(HistoryRunDetail {
        workflow_run,
        results,
        token_usage: load_history_token_usage(store, run_id)?,
        attempts: load_history_attempts(store, run_id)?,
        steps: load_history_steps(store, run_id)?,
    }))
}

fn history_results(completed_payload: Option<Value>) -> Option<Value> {
    completed_payload.and_then(|payload| payload.get("result").cloned().or_else(|| Some(payload)))
}

fn history_workflow_run_resource(
    summary: HistoryRunSummary,
    args: Value,
    failure_reason: Option<Value>,
) -> HistoryWorkflowRunResource {
    HistoryWorkflowRunResource {
        run_id: summary.run_id,
        task_id: summary.task_id,
        worker_id: summary.worker_id,
        root_run_id: summary.root_run_id,
        state: summary.state,
        metadata: summary.metadata,
        script_path: summary.script_path,
        args,
        created_at: summary.created_at,
        updated_at: summary.updated_at,
        failure_reason,
    }
}

fn history_run_summary_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<HistoryRunSummary> {
    let workflow_run_json: String = row.get(5)?;
    let workflow_run = serde_json::from_str::<WorkflowRunJSON>(&workflow_run_json).ok();
    let script_path = workflow_run
        .as_ref()
        .map(|run| run.script_path.to_string_lossy().to_string());
    let metadata = workflow_run.as_ref().and_then(|run| run.metadata.clone());
    let metadata_json = metadata
        .as_ref()
        .and_then(|metadata| serde_json::to_value(metadata).ok())
        .unwrap_or_else(|| Value::Object(Map::new()));
    let workflow_name = metadata
        .as_ref()
        .map(|metadata| metadata.name.clone())
        .filter(|name| !name.is_empty())
        .unwrap_or_default();
    Ok(HistoryRunSummary {
        run_id: row.get(0)?,
        task_id: row.get(1)?,
        worker_id: row.get(2)?,
        root_run_id: row.get(3)?,
        state: row.get(4)?,
        workflow_name,
        metadata: metadata_json,
        script_path,
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
        attempts: row.get::<_, i64>(8)? as u32,
        completed_steps: row.get::<_, i64>(9)? as u32,
        failed_steps: row.get::<_, i64>(10)? as u32,
        total_tokens: row.get::<_, i64>(11)? as u64,
    })
}

fn history_run_matches(run: &HistoryRunSummary, options: &HistoryOptions) -> bool {
    if options
        .state
        .as_deref()
        .is_some_and(|state| state != run.state)
    {
        return false;
    }
    if options.since.is_some_and(|since| run.created_at < since) {
        return false;
    }
    if options.until.is_some_and(|until| run.created_at > until) {
        return false;
    }
    if let Some(name) = options.name.as_ref() {
        let needle = name.to_ascii_lowercase();
        if !run.workflow_name.to_ascii_lowercase().contains(&needle) {
            return false;
        }
    }
    true
}

fn load_history_attempts(
    store: &SqliteDurableStore,
    run_id: &str,
) -> anyhow::Result<Vec<HistoryAttempt>> {
    let mut statement = store.connection().prepare(
        r#"
        SELECT attempt_id, attempt, state, worker_id, started_at, completed_at, failure_reason_json
        FROM sw_workflow_attempts
        WHERE run_id = ?1
        ORDER BY attempt
        "#,
    )?;
    let rows = statement.query_map([run_id], |row| {
        let failure_reason_json: Option<String> = row.get(6)?;
        Ok(HistoryAttempt {
            attempt_id: row.get(0)?,
            attempt: row.get::<_, i64>(1)? as u32,
            state: row.get(2)?,
            worker_id: row.get(3)?,
            started_at: row.get(4)?,
            completed_at: row.get(5)?,
            failure_reason: parse_optional_json_string(failure_reason_json.as_deref())
                .unwrap_or(None),
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

fn load_history_steps(
    store: &SqliteDurableStore,
    run_id: &str,
) -> anyhow::Result<Vec<HistoryStep>> {
    let mut statement = store.connection().prepare(
        r#"
        SELECT step_id, step_kind, checkpoint_name, state, attempts, created_at, updated_at, input_json, result_json
        FROM sw_workflow_steps
        WHERE run_id = ?1
        ORDER BY created_at, step_id
        "#,
    )?;
    let rows = statement.query_map([run_id], |row| {
        let input_json: String = row.get(7)?;
        let input = serde_json::from_str::<Value>(&input_json).unwrap_or(Value::Null);
        let result_json: Option<String> = row.get(8)?;
        let result = result_json
            .as_deref()
            .and_then(|json| serde_json::from_str::<Value>(json).ok());
        let session_id = result
            .as_ref()
            .and_then(|value| value.get("sessionId"))
            .and_then(Value::as_str)
            .map(ToString::to_string);
        let agent = history_step_agent(&input, session_id);
        let token_usage = result
            .as_ref()
            .and_then(history_usage_from_result)
            .unwrap_or_default();
        Ok(HistoryStep {
            step_id: row.get(0)?,
            step_kind: row.get(1)?,
            checkpoint_name: row.get(2)?,
            state: row.get(3)?,
            attempts: row.get::<_, i64>(4)? as u32,
            created_at: row.get(5)?,
            updated_at: row.get(6)?,
            agent,
            token_usage,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

fn history_step_agent(input: &Value, session_id: Option<String>) -> Option<HistoryStepAgent> {
    if input.get("kind").and_then(Value::as_str) != Some("agent") {
        return None;
    }
    let options = input.get("options");
    let context = input.get("context");
    Some(HistoryStepAgent {
        provider: input
            .get("provider")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        model: options
            .and_then(|options| options.get("model"))
            .and_then(Value::as_str)
            .map(ToString::to_string),
        phase: context
            .and_then(|context| context.get("phase"))
            .and_then(Value::as_str)
            .map(ToString::to_string),
        key: context
            .and_then(|context| context.get("key"))
            .and_then(Value::as_str)
            .map(ToString::to_string),
        session_id,
    })
}

fn load_history_token_usage(
    store: &SqliteDurableStore,
    run_id: &str,
) -> anyhow::Result<HistoryTokenUsage> {
    let mut usage = HistoryTokenUsage::default();
    let mut statement = store.connection().prepare(
        r#"
        SELECT input_json, result_json
        FROM sw_workflow_steps
        WHERE run_id = ?1
          AND result_json IS NOT NULL
        "#,
    )?;
    let rows = statement.query_map([run_id], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;

    for row in rows {
        let (input_json, result_json) = row?;
        let input = serde_json::from_str::<Value>(&input_json).unwrap_or(Value::Null);
        let result = serde_json::from_str::<Value>(&result_json).unwrap_or(Value::Null);
        let Some(step_usage) = history_usage_from_result(&result) else {
            continue;
        };
        add_history_usage(&mut usage, &step_usage);
        if let Some(phase) = input
            .get("context")
            .and_then(|context| context.get("phase"))
            .and_then(Value::as_str)
        {
            let phase_usage = usage.by_phase.entry(phase.to_string()).or_default();
            add_history_usage_totals(phase_usage, &step_usage);
        } else {
            add_history_usage_totals(&mut usage.unphased, &step_usage);
        }
    }
    Ok(usage)
}

fn history_usage_from_result(result: &Value) -> Option<HistoryTokenUsageTotals> {
    let usage = result.get("usage")?;
    let input_tokens = usage
        .get("inputTokens")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let cache_read_tokens = usage
        .get("cacheReadTokens")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let output_tokens = usage
        .get("outputTokens")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let total_tokens = usage
        .get("totalTokens")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| {
            input_tokens
                .saturating_add(cache_read_tokens)
                .saturating_add(output_tokens)
        });
    Some(HistoryTokenUsageTotals {
        input_tokens,
        output_tokens,
        cache_read_tokens,
        cache_write_tokens: usage
            .get("cacheWriteTokens")
            .and_then(Value::as_u64)
            .unwrap_or_default(),
        total_tokens,
    })
}

fn add_history_usage(total: &mut HistoryTokenUsage, usage: &HistoryTokenUsageTotals) {
    total.input_tokens = total.input_tokens.saturating_add(usage.input_tokens);
    total.output_tokens = total.output_tokens.saturating_add(usage.output_tokens);
    total.cache_read_tokens = total
        .cache_read_tokens
        .saturating_add(usage.cache_read_tokens);
    total.cache_write_tokens = total
        .cache_write_tokens
        .saturating_add(usage.cache_write_tokens);
    total.total_tokens = total.total_tokens.saturating_add(usage.total_tokens);
}

fn history_usage_has_tokens(usage: &HistoryTokenUsageTotals) -> bool {
    usage.input_tokens > 0
        || usage.output_tokens > 0
        || usage.cache_read_tokens > 0
        || usage.cache_write_tokens > 0
        || usage.total_tokens > 0
}

fn add_history_usage_totals(total: &mut HistoryTokenUsageTotals, usage: &HistoryTokenUsageTotals) {
    total.input_tokens = total.input_tokens.saturating_add(usage.input_tokens);
    total.output_tokens = total.output_tokens.saturating_add(usage.output_tokens);
    total.cache_read_tokens = total
        .cache_read_tokens
        .saturating_add(usage.cache_read_tokens);
    total.cache_write_tokens = total
        .cache_write_tokens
        .saturating_add(usage.cache_write_tokens);
    total.total_tokens = total.total_tokens.saturating_add(usage.total_tokens);
}

fn parse_json_string(value: &str) -> anyhow::Result<Value> {
    serde_json::from_str(value).map_err(Into::into)
}

fn parse_optional_json_string(value: Option<&str>) -> anyhow::Result<Option<Value>> {
    value.map(parse_json_string).transpose()
}

fn print_history_runs_table(runs: &[HistoryRunSummary]) {
    let mut table = history_table();
    table.set_header(vec![
        "RUN ID",
        "STATE",
        "WORKFLOW",
        "CREATED",
        "ATTEMPTS",
        "STEPS",
        "TOTAL TOKENS",
    ]);
    for run in runs {
        table.add_row(vec![
            Cell::new(&run.run_id),
            Cell::new(&run.state),
            Cell::new(&run.workflow_name),
            Cell::new(humanize_timestamp(run.created_at)),
            Cell::new(run.attempts),
            Cell::new(run.completed_steps),
            Cell::new(run.total_tokens),
        ]);
    }
    apply_history_table_padding(&mut table);
    println!("{table}");
}

fn print_history_detail_table(detail: &HistoryRunDetail) {
    let run = &detail.workflow_run;
    println!("Run:       {}", run.run_id);
    println!("State:     {}", run.state);
    println!("Workflow:  {}", workflow_name_from_metadata(&run.metadata));
    if let Some(script_path) = run.script_path.as_ref() {
        println!("Script:    {script_path}");
    }
    println!("Created:   {}", humanize_timestamp(run.created_at));
    let completed_steps = detail
        .steps
        .iter()
        .filter(|step| step.state == "completed")
        .count();
    let failed_steps = detail
        .steps
        .iter()
        .filter(|step| step.state == "failed")
        .count();
    println!("Attempts:  {}", detail.attempts.len());
    println!("Steps:     {completed_steps} completed, {failed_steps} failed");
    println!();

    let mut token_usage = history_table();
    token_usage.set_header(vec!["PHASE", "INPUT", "CACHE READ", "OUTPUT", "TOTAL"]);
    for (phase, usage) in &detail.token_usage.by_phase {
        token_usage.add_row(vec![
            Cell::new(phase),
            Cell::new(usage.input_tokens),
            Cell::new(usage.cache_read_tokens),
            Cell::new(usage.output_tokens),
            Cell::new(usage.total_tokens),
        ]);
    }
    if history_usage_has_tokens(&detail.token_usage.unphased) {
        token_usage.add_row(vec![
            Cell::new("(none)"),
            Cell::new(detail.token_usage.unphased.input_tokens),
            Cell::new(detail.token_usage.unphased.cache_read_tokens),
            Cell::new(detail.token_usage.unphased.output_tokens),
            Cell::new(detail.token_usage.unphased.total_tokens),
        ]);
    }
    apply_history_table_padding(&mut token_usage);
    println!("Token Usage");
    println!();
    println!("{token_usage}");
    println!();

    let mut attempts = history_table();
    attempts.set_header(vec!["ATTEMPT", "STATE", "STARTED", "COMPLETED"]);
    for attempt in &detail.attempts {
        attempts.add_row(vec![
            Cell::new(attempt.attempt),
            Cell::new(&attempt.state),
            Cell::new(humanize_timestamp(attempt.started_at)),
            Cell::new(
                attempt
                    .completed_at
                    .map(humanize_timestamp)
                    .unwrap_or_else(|| "-".to_string()),
            ),
        ]);
    }
    apply_history_table_padding(&mut attempts);
    println!("Attempts");
    println!();
    println!("{attempts}");
    println!();

    let mut steps = history_table();
    steps.set_header(vec![
        "KIND",
        "STATE",
        "PHASE",
        "PROVIDER",
        "MODEL",
        "ATTEMPTS",
        "INPUT",
        "CACHE READ",
        "OUTPUT",
        "TOTAL",
        "SESSION",
    ]);
    for step in &detail.steps {
        steps.add_row(vec![
            Cell::new(&step.step_kind),
            Cell::new(&step.state),
            Cell::new(
                step.agent
                    .as_ref()
                    .and_then(|agent| agent.phase.as_deref())
                    .unwrap_or("-"),
            ),
            Cell::new(
                step.agent
                    .as_ref()
                    .and_then(|agent| agent.provider.as_deref())
                    .unwrap_or("-"),
            ),
            Cell::new(
                step.agent
                    .as_ref()
                    .and_then(|agent| agent.model.as_deref())
                    .unwrap_or("-"),
            ),
            Cell::new(step.attempts),
            Cell::new(step.token_usage.input_tokens),
            Cell::new(step.token_usage.cache_read_tokens),
            Cell::new(step.token_usage.output_tokens),
            Cell::new(step.token_usage.total_tokens),
            Cell::new(
                step.agent
                    .as_ref()
                    .and_then(|agent| agent.session_id.as_deref())
                    .unwrap_or("-"),
            ),
        ]);
    }
    apply_history_table_padding(&mut steps);
    println!("Steps");
    println!();
    println!("{steps}");
}

fn workflow_name_from_metadata(metadata: &Value) -> &str {
    metadata
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
}

fn history_table() -> Table {
    let mut table = Table::new();
    table.load_preset(NOTHING);
    table
}

fn apply_history_table_padding(table: &mut Table) {
    for column in table.column_iter_mut() {
        column.set_padding((0, 2));
    }
}

fn humanize_timestamp(timestamp_ms: i64) -> String {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(timestamp_ms);
    let delta_ms = now_ms.saturating_sub(timestamp_ms);
    if delta_ms < 0 {
        return format!("in {}", humanize_duration(delta_ms.saturating_abs()));
    }
    format!("{} ago", humanize_duration(delta_ms))
}

fn humanize_duration(duration_ms: i64) -> String {
    let seconds = (duration_ms / 1000).max(0);
    if seconds < 60 {
        return format!("{seconds}s");
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        return format!("{minutes}m");
    }
    let hours = minutes / 60;
    if hours < 24 {
        return format!("{hours}h");
    }
    let days = hours / 24;
    if days < 30 {
        return format!("{days}d");
    }
    let months = days / 30;
    if months < 12 {
        return format!("{months}mo");
    }
    format!("{}y", months / 12)
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
    let mut agent_provider = "debug".to_string();
    let mut budget_allowance = None;
    let mut log_level = LevelFilter::Off;
    let mut resume_run_id = None;
    let mut db_path = PathBuf::from("smol-workflows.db");
    let mut max_parallel_agent_requests = None;
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
            ClapCommand::new("history")
                .about("Get workflow runs history")
                .arg(
                    Arg::new("output")
                        .short('o')
                        .long("output")
                        .value_name("table|json")
                        .help("Output format")
                        .num_args(1),
                )
                .arg(
                    Arg::new("history-args")
                        .value_name("history-options")
                        .help("History options")
                        .num_args(0..)
                        .trailing_var_arg(true)
                        .allow_hyphen_values(true),
                )
                .after_help(
                    "History options:\n  --db smol-workflows.db\n  -o, --output table|json\n  --state pending|running|completed|failed|cancelled\n  --name metadata-name-substring\n  --since unixEpochMs\n  --until unixEpochMs\n  --limit count",
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
