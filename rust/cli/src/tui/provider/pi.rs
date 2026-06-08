use ratatui::text::Line;
use serde_json::Value;

use super::common;

pub(super) fn should_show_in_timeline(data: &Value) -> bool {
    data.get("type").and_then(Value::as_str) != Some("message_update")
}

pub(super) fn timeline_label(data: &Value) -> String {
    common::timeline_label(&event_label(data), data)
}

pub(super) fn details_lines(data: &Value) -> Vec<Line<'static>> {
    common::details_lines("pi", &event_label(data), data)
}

fn event_label(data: &Value) -> String {
    data.get("type")
        .and_then(Value::as_str)
        .unwrap_or("pi.raw")
        .to_string()
}
