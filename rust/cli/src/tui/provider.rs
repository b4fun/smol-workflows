mod claude_code;
mod codex;
mod common;
mod opencode;
mod pi;

use ratatui::text::Line;
use smol_workflow_engine::events::WorkflowEventType;

use super::EventRecord;

pub(super) fn should_show_in_timeline(record: &EventRecord) -> bool {
    record.event.event_type != WorkflowEventType::AgentEvent
        || match provider(record) {
            Some("pi") => pi::should_show_in_timeline(provider_event(record)),
            Some("codex") => codex::should_show_in_timeline(provider_event(record)),
            Some("claude-code") => claude_code::should_show_in_timeline(provider_event(record)),
            Some("opencode") => opencode::should_show_in_timeline(provider_event(record)),
            _ => common::should_show_in_timeline(provider_event(record)),
        }
}

pub(super) fn timeline_label(record: &EventRecord) -> String {
    match provider(record) {
        Some("pi") => pi::timeline_label(provider_event(record)),
        Some("codex") => codex::timeline_label(provider_event(record)),
        Some("claude-code") => claude_code::timeline_label(provider_event(record)),
        Some("opencode") => opencode::timeline_label(provider_event(record)),
        _ => common::timeline_label("raw", provider_event(record)),
    }
}

pub(super) fn details_lines(record: &EventRecord) -> Vec<Line<'static>> {
    match provider(record) {
        Some("pi") => pi::details_lines(provider_event(record)),
        Some("codex") => codex::details_lines(provider_event(record)),
        Some("claude-code") => claude_code::details_lines(provider_event(record)),
        Some("opencode") => opencode::details_lines(provider_event(record)),
        _ => common::details_lines("provider", "raw", provider_event(record)),
    }
}

fn provider_event(record: &EventRecord) -> &serde_json::Value {
    record
        .event
        .data
        .get("providerEvent")
        .unwrap_or(&record.event.data)
}

fn provider(record: &EventRecord) -> Option<&str> {
    record
        .event
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.provider.as_deref())
}
