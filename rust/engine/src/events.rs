//! Workflow event stream types.
//!
//! This module contains the shared Rust representation of the smol-workflows
//! JSONL event envelope documented in `docs/usages/events.md`. The top-level
//! envelope is owned by smol-workflows; individual event `data` payloads are
//! owned by their event type. In particular, `workflow.agent_event` data is raw
//! provider-owned data and should not be normalized into a common agent schema.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

/// Known workflow event types.
///
/// The JSON representation is still the string value documented for the event
/// stream, such as `"workflow.started"`. The enum gives Rust producers and
/// sinks type-safe matching for known event types while preserving forward
/// compatibility through [`WorkflowEventType::Other`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum WorkflowEventType {
    /// A workflow scope started.
    ///
    /// The root `workflow.started` is the first event in a stream. Child
    /// workflows invoked with `workflow(...)` may emit additional started
    /// events with `metadata.workflowDepth > 0`.
    Started,
    /// Workflow code called `phase(name)`.
    Phase,
    /// Workflow code called `log(...)`.
    Log,
    /// A workflow-owned `agent(...)` call started.
    AgentStarted,
    /// Raw provider event payload associated with an `agent(...)` call.
    AgentEvent,
    /// A workflow-owned `agent(...)` call completed successfully.
    AgentCompleted,
    /// A workflow-owned `agent(...)` call failed.
    AgentFailed,
    /// A workflow scope completed successfully.
    Result,
    /// A workflow scope failed after event streaming had started.
    Error,
    /// Unknown or future event type.
    ///
    /// Consumers should ignore unknown event types unless they explicitly
    /// support them. Keeping the original string allows lossless round-tripping.
    Other(String),
}

impl WorkflowEventType {
    /// Return the JSON string representation of this event type.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Started => "workflow.started",
            Self::Phase => "workflow.phase",
            Self::Log => "workflow.log",
            Self::AgentStarted => "workflow.agent_started",
            Self::AgentEvent => "workflow.agent_event",
            Self::AgentCompleted => "workflow.agent_completed",
            Self::AgentFailed => "workflow.agent_failed",
            Self::Result => "workflow.result",
            Self::Error => "workflow.error",
            Self::Other(event_type) => event_type.as_str(),
        }
    }
}

impl From<&str> for WorkflowEventType {
    fn from(value: &str) -> Self {
        match value {
            "workflow.started" => Self::Started,
            "workflow.phase" => Self::Phase,
            "workflow.log" => Self::Log,
            "workflow.agent_started" => Self::AgentStarted,
            "workflow.agent_event" => Self::AgentEvent,
            "workflow.agent_completed" => Self::AgentCompleted,
            "workflow.agent_failed" => Self::AgentFailed,
            "workflow.result" => Self::Result,
            "workflow.error" => Self::Error,
            value => Self::Other(value.to_string()),
        }
    }
}

impl From<String> for WorkflowEventType {
    fn from(value: String) -> Self {
        WorkflowEventType::from(value.as_str())
    }
}

impl std::fmt::Display for WorkflowEventType {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl Serialize for WorkflowEventType {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for WorkflowEventType {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let event_type = String::deserialize(deserializer)?;
        Ok(Self::from(event_type))
    }
}

/// Optional metadata for correlating workflow events.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowEventMetadata {
    /// Durable workflow run ID when available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// Opaque workflow step ID for an event associated with a runtime step.
    ///
    /// For example, `workflow.agent_event` uses this to identify the
    /// `agent(...)` request. The value is intentionally opaque; consumers must
    /// not infer ordering from it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step_id: Option<String>,
    /// Agent provider name for provider-owned events.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Provider session/thread/conversation ID when the provider exposes one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Workflow nesting depth for this event scope.
    ///
    /// The root workflow has depth `0`; a direct child workflow has depth `1`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workflow_depth: Option<u32>,
    /// Opaque parent `workflow(...)` step ID for nested workflow events.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_step_id: Option<String>,
}

/// One smol-workflows event stream envelope.
///
/// Serialized events are intended to be written as JSON Lines: one complete
/// [`WorkflowEvent`] per line. The JSON field name for [`event_type`](Self::event_type)
/// is `type`.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowEvent {
    /// Event type discriminator, serialized as the top-level `type` field.
    #[serde(rename = "type")]
    pub event_type: WorkflowEventType,
    /// Nanoseconds since the root workflow stream start.
    ///
    /// The root `workflow.started` event may omit this. All later events,
    /// including child `workflow.started` events, should include it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elapsed_nanos: Option<u64>,
    /// Optional correlation metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<WorkflowEventMetadata>,
    /// Event payload.
    ///
    /// The payload shape is defined by the event type. For
    /// `workflow.agent_event`, this is raw provider-owned data.
    pub data: Value,
}

impl WorkflowEvent {
    /// Construct a `workflow.started` event.
    pub fn started(start_time: String) -> Self {
        Self {
            event_type: WorkflowEventType::Started,
            elapsed_nanos: None,
            metadata: None,
            data: serde_json::json!({ "startTime": start_time }),
        }
    }

    /// Construct a `workflow.log` event.
    pub fn log(message: String) -> Self {
        Self {
            event_type: WorkflowEventType::Log,
            elapsed_nanos: None,
            metadata: None,
            data: serde_json::json!({ "message": message }),
        }
    }

    /// Construct a `workflow.phase` event.
    pub fn phase(name: String, options: Option<Value>) -> Self {
        let mut data = serde_json::Map::new();
        data.insert("name".to_string(), Value::String(name));
        if let Some(options) = options {
            data.insert("options".to_string(), options);
        }
        Self {
            event_type: WorkflowEventType::Phase,
            elapsed_nanos: None,
            metadata: None,
            data: Value::Object(data),
        }
    }

    /// Construct a `workflow.result` event.
    pub fn result(
        input_tokens: u64,
        output_tokens: u64,
        total_tokens: u64,
        results: Value,
    ) -> Self {
        Self {
            event_type: WorkflowEventType::Result,
            elapsed_nanos: None,
            metadata: None,
            data: serde_json::json!({
                "tokenUsage": {
                    "inputTokens": input_tokens,
                    "outputTokens": output_tokens,
                    "totalTokens": total_tokens,
                },
                "results": results,
            }),
        }
    }

    /// Construct a `workflow.error` event.
    pub fn error(message: String, details: Option<String>) -> Self {
        let mut data = serde_json::Map::new();
        data.insert("message".to_string(), Value::String(message));
        if let Some(details) = details {
            data.insert("details".to_string(), Value::String(details));
        }
        Self {
            event_type: WorkflowEventType::Error,
            elapsed_nanos: None,
            metadata: None,
            data: Value::Object(data),
        }
    }

    /// Construct a `workflow.agent_started` event.
    pub fn agent_started(data: Value, metadata: WorkflowEventMetadata) -> Self {
        Self {
            event_type: WorkflowEventType::AgentStarted,
            elapsed_nanos: None,
            metadata: Some(metadata),
            data,
        }
    }

    /// Construct a `workflow.agent_event` event from raw provider data.
    pub fn agent_event(data: Value, metadata: WorkflowEventMetadata) -> Self {
        Self {
            event_type: WorkflowEventType::AgentEvent,
            elapsed_nanos: None,
            metadata: Some(metadata),
            data,
        }
    }

    /// Construct a `workflow.agent_completed` event.
    pub fn agent_completed(data: Value, metadata: WorkflowEventMetadata) -> Self {
        Self {
            event_type: WorkflowEventType::AgentCompleted,
            elapsed_nanos: None,
            metadata: Some(metadata),
            data,
        }
    }

    /// Construct a `workflow.agent_failed` event.
    pub fn agent_failed(data: Value, metadata: WorkflowEventMetadata) -> Self {
        Self {
            event_type: WorkflowEventType::AgentFailed,
            elapsed_nanos: None,
            metadata: Some(metadata),
            data,
        }
    }
}

/// Async receiver for workflow events.
///
/// Implementations may render events for humans, write JSONL, forward events to
/// another process, or collect them for tests. Returning an error makes workflow
/// execution fail, which is useful for strict machine-readable streams such as
/// CLI `--events` output.
#[async_trait::async_trait]
pub trait WorkflowEventSink: Send + Sync {
    /// Emit one workflow event.
    async fn emit(&self, event: WorkflowEvent) -> anyhow::Result<()>;
}
