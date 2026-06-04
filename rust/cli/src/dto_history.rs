use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum OutputFormat {
    /// Human-readable table output.
    Table,
    /// Machine-readable JSON output.
    Json,
}

#[derive(Debug)]
pub struct HistoryOptions {
    /// Durable SQLite database path to read from.
    pub db_path: std::path::PathBuf,
    /// Selected output format.
    pub format: OutputFormat,
    /// Optional run state filter.
    pub state: Option<String>,
    /// Optional workflow metadata name filter.
    pub name: Option<String>,
    /// Optional lower created-at bound in epoch milliseconds.
    pub since: Option<i64>,
    /// Optional upper created-at bound in epoch milliseconds.
    pub until: Option<i64>,
    /// Maximum number of runs to list.
    pub limit: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryRunSummary {
    /// Workflow run ID.
    #[serde(rename = "runID")]
    pub run_id: String,
    /// Durable task ID that owns this run.
    pub task_id: String,
    /// Internal worker/owner ID; not emitted in list JSON.
    #[serde(skip)]
    pub worker_id: String,
    /// Top-level run ID for workflow trees.
    pub root_run_id: String,
    /// Current run state.
    pub state: String,
    /// Internal table label from metadata.name; not emitted in JSON.
    #[serde(skip)]
    pub workflow_name: String,
    /// Stored workflow metadata, or `{}` for old rows.
    pub metadata: Value,
    /// Workflow script path recorded for the run.
    pub script_path: Option<String>,
    /// Run creation timestamp.
    #[serde(serialize_with = "serialize_epoch_ms_iso8601")]
    pub created_at: i64,
    /// Last run update timestamp.
    #[serde(serialize_with = "serialize_epoch_ms_iso8601")]
    pub updated_at: i64,
    /// Number of attempts recorded for this run.
    pub attempts: u32,
    /// Number of completed durable steps.
    pub completed_steps: u32,
    /// Number of failed durable steps.
    pub failed_steps: u32,
    /// Total tokens reported for this run.
    pub total_tokens: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryRunDetail {
    /// Workflow run resource.
    pub workflow_run: HistoryWorkflowRunResource,
    /// Workflow result payload, matching `smol-wf run`'s `results` field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub results: Option<Value>,
    /// Aggregated token usage for the run.
    pub token_usage: HistoryTokenUsage,
    /// Attempts made for this run.
    pub attempts: Vec<HistoryAttempt>,
    /// Durable steps recorded for this run.
    pub steps: Vec<HistoryStep>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryTokenUsage {
    /// Input tokens across agent steps.
    pub input_tokens: u64,
    /// Cache read tokens across agent steps.
    pub cache_read_tokens: u64,
    /// Output tokens across agent steps.
    pub output_tokens: u64,
    /// Cache write tokens across agent steps.
    pub cache_write_tokens: u64,
    /// Total tokens across agent steps.
    pub total_tokens: u64,
    /// Token usage grouped by workflow phase.
    pub by_phase: BTreeMap<String, HistoryTokenUsageTotals>,
    /// Token usage without a workflow phase; used for table output only.
    #[serde(skip)]
    pub unphased: HistoryTokenUsageTotals,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryTokenUsageTotals {
    /// Input tokens in this bucket.
    pub input_tokens: u64,
    /// Cache read tokens in this bucket.
    pub cache_read_tokens: u64,
    /// Output tokens in this bucket.
    pub output_tokens: u64,
    /// Cache write tokens in this bucket.
    pub cache_write_tokens: u64,
    /// Total tokens in this bucket.
    pub total_tokens: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryWorkflowRunResource {
    /// Workflow run ID.
    #[serde(rename = "runID")]
    pub run_id: String,
    /// Durable task ID that owns this run.
    pub task_id: String,
    /// Worker/owner ID that submitted this run.
    pub worker_id: String,
    /// Top-level run ID for workflow trees.
    pub root_run_id: String,
    /// Current run state.
    pub state: String,
    /// Stored workflow metadata, or `{}` for old rows.
    pub metadata: Value,
    /// Workflow script path recorded for the run.
    pub script_path: Option<String>,
    /// Workflow arguments supplied for the run.
    pub args: Value,
    /// Run creation timestamp.
    #[serde(serialize_with = "serialize_epoch_ms_iso8601")]
    pub created_at: i64,
    /// Last run update timestamp.
    #[serde(serialize_with = "serialize_epoch_ms_iso8601")]
    pub updated_at: i64,
    /// Terminal failure reason, when failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_reason: Option<Value>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryAttempt {
    /// Attempt ID.
    pub attempt_id: String,
    /// Attempt sequence number.
    pub attempt: u32,
    /// Attempt state.
    pub state: String,
    /// Worker that claimed the attempt.
    pub worker_id: String,
    /// Attempt start timestamp.
    #[serde(serialize_with = "serialize_epoch_ms_iso8601")]
    pub started_at: i64,
    /// Attempt completion timestamp, if finished.
    #[serde(serialize_with = "serialize_optional_epoch_ms_iso8601")]
    pub completed_at: Option<i64>,
    /// Failure reason, when failed.
    pub failure_reason: Option<Value>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryStep {
    /// Durable step ID.
    pub step_id: String,
    /// Step kind, such as `agent` or `workflow`.
    pub step_kind: String,
    /// Checkpoint key used for replay.
    pub checkpoint_name: String,
    /// Step state.
    pub state: String,
    /// Number of times this step was attempted.
    pub attempts: u32,
    /// Step creation timestamp.
    #[serde(serialize_with = "serialize_epoch_ms_iso8601")]
    pub created_at: i64,
    /// Last step update timestamp.
    #[serde(serialize_with = "serialize_epoch_ms_iso8601")]
    pub updated_at: i64,
    /// Agent details for agent steps.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<HistoryStepAgent>,
    /// Token usage reported for this step.
    pub token_usage: HistoryTokenUsageTotals,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryStepAgent {
    /// Agent provider used for this step.
    pub provider: Option<String>,
    /// Model requested for this step.
    pub model: Option<String>,
    /// Workflow phase associated with this step.
    pub phase: Option<String>,
    /// Provider session ID, when available.
    pub session_id: Option<String>,
}

fn serialize_epoch_ms_iso8601<S>(timestamp_ms: &i64, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(&epoch_ms_to_iso8601(*timestamp_ms))
}

fn serialize_optional_epoch_ms_iso8601<S>(
    timestamp_ms: &Option<i64>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    match timestamp_ms {
        Some(timestamp_ms) => serializer.serialize_some(&epoch_ms_to_iso8601(*timestamp_ms)),
        None => serializer.serialize_none(),
    }
}

fn epoch_ms_to_iso8601(timestamp_ms: i64) -> String {
    let seconds = timestamp_ms.div_euclid(1000);
    let millis = timestamp_ms.rem_euclid(1000);
    let days = seconds.div_euclid(86_400);
    let seconds_of_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = seconds_of_day / 3600;
    let minute = (seconds_of_day % 3600) / 60;
    let second = seconds_of_day % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

fn civil_from_days(days_since_unix_epoch: i64) -> (i64, i64, i64) {
    let z = days_since_unix_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 }.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096).div_euclid(365);
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2).div_euclid(153);
    let day = doy - (153 * mp + 2).div_euclid(5) + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    if month <= 2 {
        year += 1;
    }
    (year, month, day)
}
