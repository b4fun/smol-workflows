use ratatui::text::Line;
use serde_json::Value;

use super::common;

pub(super) fn should_show_in_timeline(data: &Value) -> bool {
    common::should_show_in_timeline(data)
}

pub(super) fn timeline_label(data: &Value) -> String {
    common::timeline_label(&event_label(data), data)
}

pub(super) fn details_lines(data: &Value) -> Vec<Line<'static>> {
    common::details_lines("opencode", &event_label(data), data)
}

fn event_label(data: &Value) -> String {
    if let Some(items) = data.get("response").and_then(Value::as_array) {
        format!("response[{}]", items.len())
    } else if data.get("session").is_some() {
        "session+response".to_string()
    } else {
        "response".to_string()
    }
}
