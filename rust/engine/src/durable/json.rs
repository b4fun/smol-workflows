//! Typed JSON payloads stored in durable SQLite `*_json` columns.
//!
//! SQLite enforces `json_valid(...)`; these serde types are the application-level
//! schema used when creating and reading durable runner payloads.

use crate::metadata::WorkflowMetadata;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LocalTaskParamsJSON {
    pub mode: DurableRunMode,
    pub script_path: PathBuf,
    pub args: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub budget_total: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRunJSON {
    pub mode: DurableRunMode,
    pub script_path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<WorkflowMetadata>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum DurableRunMode {
    Local,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FailureReasonJSON {
    pub message: String,
}
