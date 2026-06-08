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
    common::details_lines("claude-code", &event_label(data), data)
}

fn event_label(data: &Value) -> String {
    if let Some(response_type) = data
        .get("response")
        .and_then(|response| response.get("type"))
        .and_then(Value::as_str)
    {
        format!("response.{response_type}")
    } else if data
        .get("response")
        .and_then(|response| response.get("result"))
        .is_some()
    {
        "response.result".to_string()
    } else {
        "response".to_string()
    }
}
