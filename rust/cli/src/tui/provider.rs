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
            Some("pi") => pi::should_show_in_timeline(&record.event.data),
            Some("codex") => codex::should_show_in_timeline(&record.event.data),
            Some("claude-code") => claude_code::should_show_in_timeline(&record.event.data),
            Some("opencode") => opencode::should_show_in_timeline(&record.event.data),
            _ => common::should_show_in_timeline(&record.event.data),
        }
}

pub(super) fn timeline_label(record: &EventRecord) -> String {
    match provider(record) {
        Some("pi") => pi::timeline_label(&record.event.data),
        Some("codex") => codex::timeline_label(&record.event.data),
        Some("claude-code") => claude_code::timeline_label(&record.event.data),
        Some("opencode") => opencode::timeline_label(&record.event.data),
        _ => common::timeline_label("raw", &record.event.data),
    }
}

pub(super) fn details_lines(record: &EventRecord) -> Vec<Line<'static>> {
    match provider(record) {
        Some("pi") => pi::details_lines(&record.event.data),
        Some("codex") => codex::details_lines(&record.event.data),
        Some("claude-code") => claude_code::details_lines(&record.event.data),
        Some("opencode") => opencode::details_lines(&record.event.data),
        _ => common::details_lines("provider", "raw", &record.event.data),
    }
}

fn provider(record: &EventRecord) -> Option<&str> {
    record
        .event
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.provider.as_deref())
}
