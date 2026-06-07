use ratatui::text::Line;
use serde_json::Value;

use crate::tui::truncate;

pub(super) fn should_show_in_timeline(_data: &Value) -> bool {
    true
}

pub(super) fn timeline_label(label: &str, data: &Value) -> String {
    if let Some(text) = provider_texts(data).first() {
        format!("{label} {}", truncate(text, 60))
    } else if let Some(usage) = provider_usage_summary(data) {
        format!("{label} {usage}")
    } else {
        label.to_string()
    }
}

pub(super) fn details_lines(provider_name: &str, label: &str, data: &Value) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(format!("{provider_name} event: {label}"))];
    if let Some(usage) = provider_usage_summary(data) {
        lines.push(Line::from(format!("usage: {usage}")));
    }
    let texts = provider_texts(data);
    if !texts.is_empty() {
        lines.push(Line::raw(""));
        lines.push(Line::from("text:"));
        for text in texts.iter().take(8) {
            for line in text.lines() {
                lines.push(Line::from(format!("  {line}")));
            }
        }
        if texts.len() > 8 {
            lines.push(Line::from(format!(
                "  … {} more text fragments",
                texts.len() - 8
            )));
        }
        return lines;
    }

    lines.push(Line::raw(""));
    lines.extend(
        serde_json::to_string_pretty(data)
            .unwrap_or_else(|_| "<invalid>".into())
            .lines()
            .map(|line| Line::from(line.to_string())),
    );
    lines
}

fn provider_texts(data: &Value) -> Vec<String> {
    let mut texts = Vec::new();
    collect_provider_texts(data, &mut texts);
    texts.dedup();
    texts
}

fn collect_provider_texts(value: &Value, texts: &mut Vec<String>) {
    match value {
        Value::Array(items) => {
            for item in items {
                collect_provider_texts(item, texts);
            }
        }
        Value::Object(object) => {
            for (key, value) in object {
                match (key.as_str(), value) {
                    ("text" | "result" | "delta", Value::String(text))
                        if !text.trim().is_empty() =>
                    {
                        texts.push(text.clone());
                    }
                    _ => collect_provider_texts(value, texts),
                }
            }
        }
        _ => {}
    }
}

fn provider_usage_summary(data: &Value) -> Option<String> {
    find_usage_like_object(data).map(format_usage_like_object)
}

fn find_usage_like_object(value: &Value) -> Option<&serde_json::Map<String, Value>> {
    match value {
        Value::Object(object) => {
            if object.contains_key("usage") {
                if let Some(usage) = object.get("usage").and_then(Value::as_object) {
                    return Some(usage);
                }
            }
            if object.contains_key("tokens") {
                if let Some(tokens) = object.get("tokens").and_then(Value::as_object) {
                    return Some(tokens);
                }
            }
            for value in object.values() {
                if let Some(usage) = find_usage_like_object(value) {
                    return Some(usage);
                }
            }
            None
        }
        Value::Array(items) => items.iter().find_map(find_usage_like_object),
        _ => None,
    }
}

fn format_usage_like_object(usage: &serde_json::Map<String, Value>) -> String {
    let mut parts = Vec::new();
    for key in [
        "input_tokens",
        "output_tokens",
        "total_tokens",
        "cached_input_tokens",
        "input",
        "output",
        "total",
        "reasoning_output_tokens",
        "reasoning",
    ] {
        if let Some(value) = usage.get(key).and_then(Value::as_u64) {
            parts.push(format!("{key}={value}"));
        }
    }
    if parts.is_empty() {
        serde_json::to_string(&Value::Object(usage.clone())).unwrap_or_default()
    } else {
        parts.join(" ")
    }
}
