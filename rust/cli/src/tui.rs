use anyhow::Context;
use crossterm::event::{
    self, Event as CrosstermEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};
use ratatui::{Frame, Terminal};
use serde_json::Value;
use smol_workflow_engine::events::{WorkflowEvent, WorkflowEventType};
use std::fs;
use std::io::{self, Stdout};
use std::path::{Path, PathBuf};
use std::time::{Duration as StdDuration, Instant};
use time::format_description::well_known::Rfc3339;
use time::{Duration as TimeDuration, OffsetDateTime, UtcOffset};

const MIN_REPLAY_SPEED: f64 = 0.1;
const MAX_REPLAY_SPEED: f64 = 64.0;

#[derive(Clone)]
pub struct ReplayCommandOptions {
    pub path: PathBuf,
    pub check: bool,
    pub timed: bool,
    pub speed: f64,
    pub max_delay: Option<StdDuration>,
}

pub fn replay_command(options: ReplayCommandOptions) -> anyhow::Result<()> {
    let events = read_event_records(&options.path)?;
    if options.check {
        print_check_summary(&options.path, &events);
        return Ok(());
    }

    run_replay_tui(events, options)
}

pub fn parse_duration(value: &str) -> anyhow::Result<StdDuration> {
    let value = value.trim();
    if let Some(ms) = value.strip_suffix("ms") {
        return Ok(StdDuration::from_millis(ms.trim().parse()?));
    }
    if let Some(seconds) = value.strip_suffix('s') {
        let seconds: f64 = seconds.trim().parse()?;
        if seconds.is_sign_negative() || !seconds.is_finite() {
            anyhow::bail!("duration must be a finite non-negative value");
        }
        return Ok(StdDuration::from_secs_f64(seconds));
    }
    Ok(StdDuration::from_millis(value.parse()?))
}

#[derive(Clone)]
struct EventRecord {
    event: WorkflowEvent,
    raw: Value,
}

#[derive(Clone)]
struct WorkflowScopeTab {
    label: String,
    workflow_depth: u32,
    parent_step_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TimeDisplayMode {
    Elapsed,
    LocalTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FocusPane {
    Timeline,
    Details,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlaybackState {
    Playing,
    Paused,
}

struct TuiReplayApp {
    source_events: Vec<EventRecord>,
    events: Vec<EventRecord>,
    tabs: Vec<WorkflowScopeTab>,
    active_tab: usize,
    selected: usize,
    selected_by_tab: Vec<usize>,
    details_scroll: usize,
    focus_pane: FocusPane,
    raw_details: bool,
    time_display: TimeDisplayMode,
    root_start_time: Option<OffsetDateTime>,
    local_offset: Option<UtcOffset>,
    search_open: bool,
    search_query: String,
    warnings: Vec<String>,
    playback: PlaybackState,
    timed: bool,
    speed: f64,
    max_delay: Option<StdDuration>,
    next_due: Option<Instant>,
    should_quit: bool,
}

impl TuiReplayApp {
    fn new(source_events: Vec<EventRecord>, options: &ReplayCommandOptions) -> Self {
        let events = Vec::new();
        let tabs = build_scope_tabs(&events);
        let selected_by_tab = vec![0; tabs.len()];
        let root_start_time = root_start_time(&source_events);
        let local_offset = UtcOffset::current_local_offset().ok();
        Self {
            warnings: validate_events(&source_events),
            source_events,
            tabs,
            events,
            active_tab: 0,
            selected: 0,
            selected_by_tab,
            details_scroll: 0,
            focus_pane: FocusPane::Timeline,
            raw_details: false,
            time_display: TimeDisplayMode::Elapsed,
            root_start_time,
            local_offset,
            search_open: false,
            search_query: String::new(),
            playback: PlaybackState::Paused,
            timed: options.timed,
            speed: normalize_replay_speed(options.speed),
            max_delay: options.max_delay,
            next_due: options.timed.then_some(Instant::now()),
            should_quit: false,
        }
    }

    fn visible_indices(&self) -> Vec<usize> {
        let Some(tab) = self.tabs.get(self.active_tab) else {
            return Vec::new();
        };
        let query = self.search_query.to_ascii_lowercase();
        self.events
            .iter()
            .enumerate()
            .filter(|(_, record)| event_in_scope(record, tab))
            .filter(|(_, record)| should_show_in_timeline(record))
            .filter(|(_, record)| {
                query.is_empty()
                    || searchable_event_text(record)
                        .to_ascii_lowercase()
                        .contains(&query)
                    || serde_json::to_string(&record.raw)
                        .unwrap_or_default()
                        .to_ascii_lowercase()
                        .contains(&query)
            })
            .map(|(index, _)| index)
            .collect()
    }

    fn selected_event_index(&self) -> Option<usize> {
        self.visible_indices().get(self.selected).copied()
    }

    fn selected_event(&self) -> Option<&EventRecord> {
        self.selected_event_index()
            .and_then(|index| self.events.get(index))
    }

    fn replay_complete(&self) -> bool {
        self.events.len() >= self.source_events.len()
    }

    fn rebuild_tabs(&mut self) {
        let previous_active = self.tabs.get(self.active_tab).cloned();
        self.tabs = build_scope_tabs(&self.events);
        if self.tabs.is_empty() {
            self.tabs.push(WorkflowScopeTab {
                label: "root".to_string(),
                workflow_depth: 0,
                parent_step_id: None,
            });
        }
        self.selected_by_tab.resize(self.tabs.len(), 0);
        if let Some(previous_active) = previous_active {
            if let Some(index) = self.tabs.iter().position(|tab| {
                tab.workflow_depth == previous_active.workflow_depth
                    && tab.parent_step_id == previous_active.parent_step_id
            }) {
                self.active_tab = index;
            } else {
                self.active_tab = self.active_tab.min(self.tabs.len().saturating_sub(1));
            }
        } else {
            self.active_tab = 0;
        }
        self.clamp_selection();
    }

    fn select_latest_visible(&mut self) {
        let len = self.visible_indices().len();
        if len > 0 {
            self.selected = len - 1;
            self.remember_selection();
        }
    }

    fn reveal_next_event(&mut self) {
        if let Some(event) = self.source_events.get(self.events.len()).cloned() {
            self.events.push(event);
            self.rebuild_tabs();
            self.select_latest_visible();
        }
    }

    fn hide_last_event(&mut self) {
        if self.events.pop().is_some() {
            self.rebuild_tabs();
            self.select_latest_visible();
            self.reset_details_scroll();
        }
    }

    fn schedule_next_due(&mut self, now: Instant) {
        if self.replay_complete() {
            self.next_due = None;
            self.playback = PlaybackState::Paused;
            return;
        }
        let next_index = self.events.len();
        let delay = if self.timed {
            replay_delay(
                self.source_events.get(next_index.saturating_sub(1)),
                self.source_events.get(next_index),
                self.speed,
                self.max_delay,
            )
        } else {
            StdDuration::ZERO
        };
        self.next_due = Some(now + delay);
    }

    fn tick_playback(&mut self) {
        if self.playback != PlaybackState::Playing {
            return;
        }
        let now = Instant::now();
        if self.next_due.is_some_and(|due| now >= due) {
            self.reveal_next_event();
            self.schedule_next_due(now);
        }
    }

    fn poll_timeout(&self) -> StdDuration {
        if self.playback != PlaybackState::Playing {
            return StdDuration::from_millis(200);
        }
        match self.next_due {
            Some(due) => due
                .saturating_duration_since(Instant::now())
                .min(StdDuration::from_millis(200)),
            None => StdDuration::from_millis(200),
        }
    }

    fn toggle_playback(&mut self) {
        if self.replay_complete() && self.playback == PlaybackState::Paused {
            return;
        }
        self.playback = match self.playback {
            PlaybackState::Playing => PlaybackState::Paused,
            PlaybackState::Paused => {
                self.schedule_next_due(Instant::now());
                PlaybackState::Playing
            }
        };
    }

    fn remember_selection(&mut self) {
        if let Some(slot) = self.selected_by_tab.get_mut(self.active_tab) {
            *slot = self.selected;
        }
    }

    fn restore_selection_for_active_tab(&mut self) {
        self.selected = self
            .selected_by_tab
            .get(self.active_tab)
            .copied()
            .unwrap_or(0);
        self.clamp_selection();
    }

    fn clamp_selection(&mut self) {
        let len = self.visible_indices().len();
        if len == 0 {
            self.selected = 0;
        } else if self.selected >= len {
            self.selected = len - 1;
        }
        self.remember_selection();
    }

    fn reset_details_scroll(&mut self) {
        self.details_scroll = 0;
    }

    fn select_next(&mut self) {
        let len = self.visible_indices().len();
        if len > 0 {
            let previous = self.selected;
            self.selected = (self.selected + 1).min(len - 1);
            if self.selected != previous {
                self.reset_details_scroll();
            }
        }
    }

    fn select_previous(&mut self) {
        let previous = self.selected;
        self.selected = self.selected.saturating_sub(1);
        if self.selected != previous {
            self.reset_details_scroll();
        }
    }

    fn page_down(&mut self) {
        let len = self.visible_indices().len();
        if len > 0 {
            let previous = self.selected;
            self.selected = (self.selected + 10).min(len - 1);
            if self.selected != previous {
                self.reset_details_scroll();
            }
        }
    }

    fn page_up(&mut self) {
        let previous = self.selected;
        self.selected = self.selected.saturating_sub(10);
        if self.selected != previous {
            self.reset_details_scroll();
        }
    }

    fn first(&mut self) {
        let previous = self.selected;
        self.selected = 0;
        if self.selected != previous {
            self.reset_details_scroll();
        }
    }

    fn last(&mut self) {
        let len = self.visible_indices().len();
        if len > 0 {
            let previous = self.selected;
            self.selected = len - 1;
            if self.selected != previous {
                self.reset_details_scroll();
            }
        }
    }

    fn scroll_details_down(&mut self) {
        self.details_scroll = self.details_scroll.saturating_add(1);
    }

    fn scroll_details_up(&mut self) {
        self.details_scroll = self.details_scroll.saturating_sub(1);
    }

    fn page_details_down(&mut self) {
        self.details_scroll = self.details_scroll.saturating_add(10);
    }

    fn page_details_up(&mut self) {
        self.details_scroll = self.details_scroll.saturating_sub(10);
    }

    fn details_top(&mut self) {
        self.details_scroll = 0;
    }

    fn next_tab(&mut self) {
        if !self.tabs.is_empty() {
            self.remember_selection();
            self.active_tab = (self.active_tab + 1) % self.tabs.len();
            self.restore_selection_for_active_tab();
        }
    }

    fn previous_tab(&mut self) {
        if !self.tabs.is_empty() {
            self.remember_selection();
            self.active_tab = if self.active_tab == 0 {
                self.tabs.len() - 1
            } else {
                self.active_tab - 1
            };
            self.restore_selection_for_active_tab();
        }
    }

    fn handle_key(&mut self, key: KeyEvent) {
        if key.kind == KeyEventKind::Release {
            return;
        }

        if self.search_open {
            self.handle_search_key(key);
            return;
        }

        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.should_quit = true
            }
            KeyCode::Char(' ') => self.toggle_playback(),
            KeyCode::Char('n') => {
                self.playback = PlaybackState::Paused;
                self.reveal_next_event();
            }
            KeyCode::Char('p') => {
                self.playback = PlaybackState::Paused;
                self.hide_last_event();
            }
            KeyCode::Char('+') | KeyCode::Char('=') => {
                self.speed = (self.speed * 2.0).min(MAX_REPLAY_SPEED);
                self.schedule_next_due(Instant::now());
            }
            KeyCode::Char('-') => {
                self.speed = (self.speed / 2.0).max(MIN_REPLAY_SPEED);
                self.schedule_next_due(Instant::now());
            }
            KeyCode::Char('0') => {
                self.speed = 1.0;
                self.schedule_next_due(Instant::now());
            }
            KeyCode::Down | KeyCode::Char('j') => match self.focus_pane {
                FocusPane::Timeline => self.select_next(),
                FocusPane::Details => self.scroll_details_down(),
            },
            KeyCode::Up | KeyCode::Char('k') => match self.focus_pane {
                FocusPane::Timeline => self.select_previous(),
                FocusPane::Details => self.scroll_details_up(),
            },
            KeyCode::PageDown => match self.focus_pane {
                FocusPane::Timeline => self.page_down(),
                FocusPane::Details => self.page_details_down(),
            },
            KeyCode::PageUp => match self.focus_pane {
                FocusPane::Timeline => self.page_up(),
                FocusPane::Details => self.page_details_up(),
            },
            KeyCode::Home => match self.focus_pane {
                FocusPane::Timeline => self.first(),
                FocusPane::Details => self.details_top(),
            },
            KeyCode::End => self.last(),
            KeyCode::Tab => self.next_tab(),
            KeyCode::BackTab => self.previous_tab(),
            KeyCode::Right => self.focus_pane = FocusPane::Details,
            KeyCode::Left => self.focus_pane = FocusPane::Timeline,
            KeyCode::Char('r') => {
                self.raw_details = !self.raw_details;
                self.reset_details_scroll();
            }
            KeyCode::Char('t') => {
                self.time_display = match self.time_display {
                    TimeDisplayMode::Elapsed => TimeDisplayMode::LocalTime,
                    TimeDisplayMode::LocalTime => TimeDisplayMode::Elapsed,
                }
            }
            KeyCode::Char('/') => self.search_open = true,
            _ => {}
        }
        self.clamp_selection();
    }

    fn handle_search_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Enter => self.search_open = false,
            KeyCode::Backspace => {
                self.search_query.pop();
                self.selected = 0;
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.search_query.clear();
                self.selected = 0;
            }
            KeyCode::Char(ch) => {
                self.search_query.push(ch);
                self.selected = 0;
            }
            _ => {}
        }
        self.clamp_selection();
    }
}

fn read_event_records(path: &Path) -> anyhow::Result<Vec<EventRecord>> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("failed to read event stream {}", path.display()))?;
    let mut events = Vec::new();
    for (line_index, line) in content.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let raw: Value = serde_json::from_str(line)
            .with_context(|| format!("invalid JSON on line {}", line_index + 1))?;
        let event: WorkflowEvent = serde_json::from_value(raw.clone())
            .with_context(|| format!("invalid workflow event on line {}", line_index + 1))?;
        events.push(EventRecord { event, raw });
    }
    Ok(events)
}

fn normalize_replay_speed(speed: f64) -> f64 {
    if !speed.is_finite() || speed <= 0.0 {
        1.0
    } else {
        speed.clamp(MIN_REPLAY_SPEED, MAX_REPLAY_SPEED)
    }
}

fn replay_delay(
    previous: Option<&EventRecord>,
    next: Option<&EventRecord>,
    speed: f64,
    max_delay: Option<StdDuration>,
) -> StdDuration {
    let previous_elapsed = previous
        .and_then(|record| record.event.elapsed_nanos)
        .unwrap_or(0);
    let next_elapsed = next
        .and_then(|record| record.event.elapsed_nanos)
        .unwrap_or(previous_elapsed);
    let nanos = next_elapsed.saturating_sub(previous_elapsed);
    let seconds = (nanos as f64 / 1_000_000_000.0) / normalize_replay_speed(speed);
    let mut delay = if seconds.is_finite() && seconds > 0.0 {
        StdDuration::from_secs_f64(seconds)
    } else {
        StdDuration::ZERO
    };
    if let Some(max_delay) = max_delay {
        delay = delay.min(max_delay);
    }
    delay
}

fn print_check_summary(path: &Path, events: &[EventRecord]) {
    let warnings = validate_events(events);
    let root_results = events
        .iter()
        .filter(|record| {
            record.event.event_type == WorkflowEventType::Result
                && record
                    .event
                    .metadata
                    .as_ref()
                    .and_then(|metadata| metadata.workflow_depth)
                    == Some(0)
        })
        .count();
    let agent_events = events
        .iter()
        .filter(|record| record.event.event_type == WorkflowEventType::AgentEvent)
        .count();
    println!("events: {}", path.display());
    println!("total: {}", events.len());
    println!("tabs: {}", build_scope_tabs(events).len());
    println!("agentEvents: {agent_events}");
    println!("rootResults: {root_results}");
    if warnings.is_empty() {
        println!("warnings: 0");
    } else {
        println!("warnings: {}", warnings.len());
        for warning in warnings {
            println!("- {warning}");
        }
    }
}

fn validate_events(events: &[EventRecord]) -> Vec<String> {
    let mut warnings = Vec::new();
    let root_started = events
        .iter()
        .filter(|record| {
            record.event.event_type == WorkflowEventType::Started
                && record
                    .event
                    .metadata
                    .as_ref()
                    .and_then(|metadata| metadata.workflow_depth)
                    .unwrap_or(0)
                    == 0
        })
        .count();
    if root_started == 0 {
        warnings.push("no root workflow.started event".to_string());
    } else if root_started > 1 {
        warnings.push(format!(
            "multiple root workflow.started events: {root_started}"
        ));
    }

    let has_terminal_root = events.iter().any(|record| {
        matches!(
            record.event.event_type,
            WorkflowEventType::Result | WorkflowEventType::Error
        ) && record
            .event
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.workflow_depth)
            .unwrap_or(0)
            == 0
    });
    if !has_terminal_root {
        warnings.push("no terminal root workflow.result or workflow.error event".to_string());
    }

    let mut previous_elapsed = 0u64;
    for (index, record) in events.iter().enumerate() {
        if record.event.data.is_null() {
            warnings.push(format!("event {} has null data", index + 1));
        }
        if let Some(elapsed) = record.event.elapsed_nanos {
            if elapsed < previous_elapsed {
                warnings.push(format!("event {} elapsedNanos decreases", index + 1));
            }
            previous_elapsed = elapsed;
        }
        let metadata = record.event.metadata.as_ref();
        let workflow_depth = metadata
            .and_then(|metadata| metadata.workflow_depth)
            .unwrap_or(0);
        if workflow_depth > 0
            && metadata
                .and_then(|metadata| metadata.parent_step_id.as_ref())
                .is_none()
        {
            warnings.push(format!(
                "nested event {} is missing parentStepId",
                index + 1
            ));
        }
    }

    warnings
}

fn build_scope_tabs(events: &[EventRecord]) -> Vec<WorkflowScopeTab> {
    let mut tabs = vec![WorkflowScopeTab {
        label: "root".to_string(),
        workflow_depth: 0,
        parent_step_id: None,
    }];

    for record in events {
        if record.event.event_type != WorkflowEventType::Started {
            continue;
        }
        let Some(metadata) = record.event.metadata.as_ref() else {
            continue;
        };
        let depth = metadata.workflow_depth.unwrap_or(0);
        if depth == 0 {
            continue;
        }
        let Some(parent_step_id) = metadata.parent_step_id.clone() else {
            continue;
        };
        if tabs.iter().any(|tab| {
            tab.workflow_depth == depth && tab.parent_step_id.as_ref() == Some(&parent_step_id)
        }) {
            continue;
        }
        tabs.push(WorkflowScopeTab {
            label: format!("child {}", short_id(&parent_step_id)),
            workflow_depth: depth,
            parent_step_id: Some(parent_step_id),
        });
    }

    tabs
}

#[derive(Debug, Clone, Copy)]
enum AgentEventRendererKind {
    Default,
    Pi,
    Codex,
    ClaudeCode,
    OpenCode,
}

impl AgentEventRendererKind {
    fn for_provider(provider: Option<&str>) -> Self {
        match provider {
            Some("pi") => Self::Pi,
            Some("codex") => Self::Codex,
            Some("claude-code") => Self::ClaudeCode,
            Some("opencode") => Self::OpenCode,
            _ => Self::Default,
        }
    }

    fn should_show_in_timeline(self, data: &Value) -> bool {
        match self {
            Self::Pi => data.get("type").and_then(Value::as_str) != Some("message_update"),
            _ => true,
        }
    }

    fn timeline_label(self, data: &Value) -> String {
        let label = self.provider_event_label(data);
        if let Some(text) = provider_texts(data).first() {
            format!("{label} {}", truncate(text, 60))
        } else if let Some(usage) = provider_usage_summary(data) {
            format!("{label} {usage}")
        } else {
            label
        }
    }

    fn provider_event_label(self, data: &Value) -> String {
        match self {
            Self::Codex => data
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("codex.raw")
                .to_string(),
            Self::Pi => data
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("pi.raw")
                .to_string(),
            Self::ClaudeCode => {
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
            Self::OpenCode => {
                if let Some(items) = data.get("response").and_then(Value::as_array) {
                    format!("response[{}]", items.len())
                } else if data.get("session").is_some() {
                    "session+response".to_string()
                } else {
                    "response".to_string()
                }
            }
            Self::Default => data
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("raw")
                .to_string(),
        }
    }

    fn provider_name(self) -> &'static str {
        match self {
            Self::Pi => "pi",
            Self::Codex => "codex",
            Self::ClaudeCode => "claude-code",
            Self::OpenCode => "opencode",
            Self::Default => "provider",
        }
    }

    fn details_lines(self, data: &Value) -> Vec<Line<'static>> {
        let label = self.provider_event_label(data);
        let mut lines = vec![Line::from(format!(
            "{} event: {label}",
            self.provider_name()
        ))];
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

fn agent_event_renderer(record: &EventRecord) -> AgentEventRendererKind {
    AgentEventRendererKind::for_provider(
        record
            .event
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.provider.as_deref()),
    )
}

fn should_show_in_timeline(record: &EventRecord) -> bool {
    record.event.event_type != WorkflowEventType::AgentEvent
        || agent_event_renderer(record).should_show_in_timeline(&record.event.data)
}

fn event_in_scope(record: &EventRecord, tab: &WorkflowScopeTab) -> bool {
    let metadata = record.event.metadata.as_ref();
    let depth = metadata
        .and_then(|metadata| metadata.workflow_depth)
        .unwrap_or(0);
    if tab.workflow_depth == 0 {
        return true;
    }
    depth == tab.workflow_depth
        && metadata.and_then(|metadata| metadata.parent_step_id.as_ref())
            == tab.parent_step_id.as_ref()
}

fn run_replay_tui(events: Vec<EventRecord>, options: ReplayCommandOptions) -> anyhow::Result<()> {
    let mut terminal = setup_terminal()?;
    let mut app = TuiReplayApp::new(events, &options);
    let result = run_tui_loop(&mut terminal, &mut app);
    restore_terminal(&mut terminal)?;
    result
}

fn setup_terminal() -> anyhow::Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    Ok(Terminal::new(backend)?)
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> anyhow::Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

fn run_tui_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &mut TuiReplayApp,
) -> anyhow::Result<()> {
    loop {
        app.tick_playback();
        terminal.draw(|frame| render(frame, app))?;
        if app.should_quit {
            break;
        }
        if event::poll(app.poll_timeout())? {
            if let CrosstermEvent::Key(key) = event::read()? {
                app.handle_key(key);
            }
        }
    }
    Ok(())
}

fn render(frame: &mut Frame<'_>, app: &TuiReplayApp) {
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Min(5),
            Constraint::Length(1),
        ])
        .split(frame.area());

    let status = status_line(app);
    frame.render_widget(Paragraph::new(status), root[0]);

    render_tab_bar(frame, app, root[1]);

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(root[2]);

    render_timeline(frame, app, body[0]);
    render_details(frame, app, body[1]);

    let footer = "q quit  Tab tabs  ←/→ pane  ↑/↓ scroll/select  / search  r raw/pretty  t time";
    frame.render_widget(Paragraph::new(footer), root[3]);

    if app.search_open {
        render_search_overlay(frame, app);
    }
}

fn render_tab_bar(frame: &mut Frame<'_>, app: &TuiReplayApp, area: ratatui::layout::Rect) {
    let mut spans = vec![Span::styled(
        " Workflows ",
        Style::default()
            .fg(Color::White)
            .bg(Color::Blue)
            .add_modifier(Modifier::BOLD),
    )];
    spans.push(Span::raw(" "));
    for (index, tab) in app.tabs.iter().enumerate() {
        let selected = index == app.active_tab;
        let style = if selected {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White).bg(Color::DarkGray)
        };
        spans.push(Span::styled(format!(" {} ", tab.label), style));
        spans.push(Span::raw(" "));
    }
    spans.push(Span::styled(
        " Tab/Shift+Tab ",
        Style::default().fg(Color::Gray),
    ));

    frame.render_widget(
        Paragraph::new(Line::from(spans)).block(Block::default().borders(Borders::ALL)),
        area,
    );
}

fn render_timeline(frame: &mut Frame<'_>, app: &TuiReplayApp, area: ratatui::layout::Rect) {
    let title = if app.search_query.is_empty() {
        format!(
            " Timeline ({}/{}) ",
            app.selected.saturating_add(1),
            app.visible_indices().len()
        )
    } else {
        format!(
            " Timeline ({}/{}) search: {} ",
            app.selected.saturating_add(1),
            app.visible_indices().len(),
            app.search_query
        )
    };
    let focused = app.focus_pane == FocusPane::Timeline;
    let title_color = if focused { Color::Cyan } else { Color::Blue };
    let (_title_area, content_area) = render_pane_shell(frame, area, title, title_color, focused);
    let content_area = pad_content_area(content_area, 2, 0);

    let visible = app.visible_indices();
    let query = app.search_query.to_ascii_lowercase();
    let height = usize::from(content_area.height).max(1);
    let start = scroll_start(app.selected, visible.len(), height);
    let items = visible
        .iter()
        .enumerate()
        .skip(start)
        .take(height)
        .map(|(visible_index, event_index)| {
            let summary = timeline_summary(app, &visible, *event_index);
            let selected = visible_index == app.selected;
            let search_match = !query.is_empty() && summary.to_ascii_lowercase().contains(&query);
            let line = Line::from(vec![Span::styled(
                summary,
                timeline_event_style(&app.events[*event_index], selected, search_match),
            )]);
            ListItem::new(line)
        })
        .collect::<Vec<_>>();
    frame.render_widget(List::new(items), content_area);
}

fn scroll_start(selected: usize, len: usize, height: usize) -> usize {
    if len <= height || selected < height {
        0
    } else {
        (selected + 1)
            .saturating_sub(height)
            .min(len.saturating_sub(height))
    }
}

fn timeline_event_style(record: &EventRecord, selected: bool, search_match: bool) -> Style {
    if selected {
        return Style::default().fg(Color::Black).bg(Color::Cyan);
    }
    if search_match {
        return Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD);
    }
    event_type_style(&record.event.event_type)
}

fn event_type_style(event_type: &WorkflowEventType) -> Style {
    match event_type {
        WorkflowEventType::Started => Style::default().fg(Color::Cyan),
        WorkflowEventType::Phase => Style::default().fg(Color::Magenta),
        WorkflowEventType::Log => Style::default().fg(Color::Gray),
        WorkflowEventType::AgentEvent => Style::default().fg(Color::Green),
        WorkflowEventType::Result => Style::default().fg(Color::LightGreen),
        WorkflowEventType::Error => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        WorkflowEventType::Other(_) => Style::default().fg(Color::White),
    }
}

fn pad_content_area(area: ratatui::layout::Rect, left: u16, top: u16) -> ratatui::layout::Rect {
    ratatui::layout::Rect {
        x: area.x.saturating_add(left),
        y: area.y.saturating_add(top),
        width: area.width.saturating_sub(left),
        height: area.height.saturating_sub(top),
    }
}

fn render_details(frame: &mut Frame<'_>, app: &TuiReplayApp, area: ratatui::layout::Rect) {
    let title = if app.raw_details {
        " Details: raw JSON "
    } else {
        " Details: pretty "
    };
    let lines = app
        .selected_event()
        .map(|record| {
            let style = event_type_style(&record.event.event_type);
            if app.raw_details {
                raw_details_lines(record, style)
            } else {
                pretty_details_lines(app, record, style)
            }
        })
        .unwrap_or_else(|| vec![Line::raw("No event selected")]);
    let focused = app.focus_pane == FocusPane::Details;
    let title_color = if focused { Color::Cyan } else { Color::Blue };
    let (_title_area, content_area) =
        render_pane_shell(frame, area, title.to_string(), title_color, focused);
    let paragraph = Paragraph::new(pad_details_lines(lines))
        .scroll((u16::try_from(app.details_scroll).unwrap_or(u16::MAX), 0))
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, content_area);
}

fn render_pane_shell(
    frame: &mut Frame<'_>,
    area: ratatui::layout::Rect,
    title: String,
    title_bg: Color,
    focused: bool,
) -> (ratatui::layout::Rect, ratatui::layout::Rect) {
    let block = if focused {
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(title_bg))
    } else {
        Block::default().borders(Borders::ALL)
    };
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(inner);
    let title_style = if focused {
        Style::default()
            .fg(Color::Black)
            .bg(title_bg)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD)
    };
    frame.render_widget(Paragraph::new(title).style(title_style), chunks[0]);
    (chunks[0], chunks[1])
}

fn pad_details_lines(lines: Vec<Line<'static>>) -> Vec<Line<'static>> {
    lines
        .into_iter()
        .map(|line| {
            let mut spans = Vec::with_capacity(line.spans.len() + 1);
            spans.push(Span::raw("  "));
            spans.extend(line.spans);
            Line { spans, ..line }
        })
        .collect()
}

fn render_search_overlay(frame: &mut Frame<'_>, app: &TuiReplayApp) {
    let area = centered_rect(70, 3, frame.area());
    let input = Paragraph::new(format!("/{}", app.search_query)).block(
        Block::default()
            .borders(Borders::ALL)
            .title("Search timeline"),
    );
    frame.render_widget(Clear, area);
    frame.render_widget(input, area);
}

fn centered_rect(
    width_percent: u16,
    height: u16,
    area: ratatui::layout::Rect,
) -> ratatui::layout::Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(40),
            Constraint::Length(height),
            Constraint::Percentage(60),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - width_percent) / 2),
            Constraint::Percentage(width_percent),
            Constraint::Percentage((100 - width_percent) / 2),
        ])
        .split(vertical[1])[1]
}

fn status_line(app: &TuiReplayApp) -> String {
    let run_id = app
        .events
        .first()
        .and_then(|record| record.event.metadata.as_ref())
        .and_then(|metadata| metadata.run_id.as_deref())
        .unwrap_or("<unknown-run>");
    let warnings = if app.warnings.is_empty() {
        "".to_string()
    } else {
        format!("  warnings {}", app.warnings.len())
    };
    let time_mode = match app.time_display {
        TimeDisplayMode::Elapsed => "elapsed",
        TimeDisplayMode::LocalTime => "local",
    };
    let playback = match app.playback {
        PlaybackState::Playing => "REPLAY_PLAYING",
        PlaybackState::Paused if app.replay_complete() => "REPLAY_DONE",
        PlaybackState::Paused => "REPLAY_PAUSED",
    };
    format!(
        " {playback}  {run_id}  events {}/{}  tab {}/{}  speed {:.2}x  time {time_mode}{}",
        app.events.len(),
        app.source_events.len(),
        app.active_tab + 1,
        app.tabs.len(),
        app.speed,
        warnings
    )
}

fn timeline_summary(app: &TuiReplayApp, visible: &[usize], event_index: usize) -> String {
    let record = &app.events[event_index];
    if record.event.event_type != WorkflowEventType::AgentEvent {
        return event_summary(app, record);
    }

    let Some(group_key) = agent_group_key(record) else {
        return event_summary(app, record);
    };
    let matching = visible
        .iter()
        .copied()
        .filter(|index| agent_group_key(&app.events[*index]).as_deref() == Some(group_key.as_str()))
        .collect::<Vec<_>>();
    let first = matching.first().copied() == Some(event_index);
    if first {
        let event = &record.event;
        let elapsed = display_time(app, event);
        let metadata = event.metadata.as_ref();
        let provider = metadata
            .and_then(|metadata| metadata.provider.as_deref())
            .unwrap_or("<provider>");
        let session = metadata
            .and_then(|metadata| metadata.session_id.as_deref())
            .map(short_id)
            .unwrap_or_else(|| "<session>".to_string());
        let depth = metadata
            .and_then(|metadata| metadata.workflow_depth)
            .unwrap_or(0);
        let indent = timeline_indent(depth);
        format!(
            "{elapsed} {indent}agent {provider} session={session} events={}",
            matching.len()
        )
    } else {
        let position = matching
            .iter()
            .position(|index| *index == event_index)
            .unwrap_or(0);
        let branch = if position + 1 == matching.len() {
            "└─"
        } else {
            "├─"
        };
        let event = &record.event;
        let elapsed = display_time(app, event);
        let depth = event
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.workflow_depth)
            .unwrap_or(0);
        let indent = timeline_indent(depth.saturating_add(1));
        let label = agent_event_renderer(record).timeline_label(&event.data);
        format!("{elapsed} {indent}{branch} {label}")
    }
}

fn timeline_indent(depth: u32) -> String {
    "  ".repeat(usize::try_from(depth).unwrap_or(usize::MAX / 2))
}

fn agent_group_key(record: &EventRecord) -> Option<String> {
    if record.event.event_type != WorkflowEventType::AgentEvent {
        return None;
    }
    let metadata = record.event.metadata.as_ref()?;
    metadata
        .session_id
        .as_ref()
        .or(metadata.step_id.as_ref())
        .cloned()
}

fn searchable_event_text(record: &EventRecord) -> String {
    let event = &record.event;
    let mut parts = vec![
        event.event_type.to_string(),
        serde_json::to_string(&event.data).unwrap_or_default(),
    ];
    if let Some(metadata) = event.metadata.as_ref() {
        if let Some(provider) = metadata.provider.as_deref() {
            parts.push(provider.to_string());
        }
        if let Some(session) = metadata.session_id.as_deref() {
            parts.push(session.to_string());
        }
        if let Some(step) = metadata.step_id.as_deref() {
            parts.push(step.to_string());
        }
        if let Some(parent) = metadata.parent_step_id.as_deref() {
            parts.push(parent.to_string());
        }
    }
    parts.join(" ")
}

fn event_summary(app: &TuiReplayApp, record: &EventRecord) -> String {
    let event = &record.event;
    let elapsed = display_time(app, event);
    let depth = event
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.workflow_depth)
        .unwrap_or(0);
    let indent = timeline_indent(depth);
    match event.event_type {
        WorkflowEventType::Started => {
            if depth == 0 {
                format!("{elapsed} workflow.started")
            } else {
                let child = event
                    .metadata
                    .as_ref()
                    .and_then(|metadata| metadata.parent_step_id.as_deref())
                    .map(short_id)
                    .unwrap_or_else(|| "<child>".to_string());
                format!("{elapsed} {indent}workflow.started child={child}")
            }
        }
        WorkflowEventType::Phase => format!(
            "{elapsed} {indent}workflow.phase {}",
            event.data.get("name").and_then(Value::as_str).unwrap_or("")
        ),
        WorkflowEventType::Log => format!(
            "{elapsed} {indent}workflow.log {}",
            truncate(
                event
                    .data
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or(""),
                80
            )
        ),
        WorkflowEventType::AgentEvent => {
            let metadata = event.metadata.as_ref();
            let provider = metadata
                .and_then(|metadata| metadata.provider.as_deref())
                .unwrap_or("<provider>");
            let session = metadata
                .and_then(|metadata| metadata.session_id.as_deref())
                .map(short_id)
                .unwrap_or_else(|| "<session>".to_string());
            let label = agent_event_renderer(record).timeline_label(&event.data);
            format!("{elapsed} {indent}workflow.agent_event {provider} {session} {label}")
        }
        WorkflowEventType::Result => format!("{elapsed} {indent}workflow.result"),
        WorkflowEventType::Error => format!(
            "{elapsed} {indent}workflow.error {}",
            event
                .data
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("")
        ),
        WorkflowEventType::Other(ref event_type) => format!("{elapsed} {indent}{event_type}"),
    }
}

fn raw_details_lines(record: &EventRecord, style: Style) -> Vec<Line<'static>> {
    serde_json::to_string_pretty(&record.raw)
        .unwrap_or_else(|_| "<invalid>".into())
        .lines()
        .map(|line| Line::from(Span::styled(line.to_string(), style)))
        .collect()
}

fn pretty_details_lines(
    app: &TuiReplayApp,
    record: &EventRecord,
    style: Style,
) -> Vec<Line<'static>> {
    let event = &record.event;
    let metadata = event.metadata.as_ref();
    let mut lines = Vec::new();
    lines.push(Line::from(vec![
        Span::raw("type: "),
        Span::styled(
            event.event_type.to_string(),
            style.add_modifier(Modifier::BOLD),
        ),
    ]));
    lines.push(Line::from(format!("time: {}", display_time(app, event))));
    if let Some(metadata) = metadata {
        lines.push(Line::from(format!(
            "workflowDepth: {}",
            metadata.workflow_depth.unwrap_or(0)
        )));
        if let Some(parent) = metadata.parent_step_id.as_deref() {
            lines.push(Line::from(format!("parentStepId: {parent}")));
        }
        if let Some(step) = metadata.step_id.as_deref() {
            lines.push(Line::from(format!("stepId: {step}")));
        }
        if let Some(provider) = metadata.provider.as_deref() {
            lines.push(Line::from(format!("provider: {provider}")));
        }
        if let Some(session) = metadata.session_id.as_deref() {
            lines.push(Line::from(format!("sessionId: {session}")));
        }
    }
    lines.push(Line::raw(""));
    let body = match event.event_type {
        WorkflowEventType::Started => format!(
            "started: {}",
            event
                .data
                .get("startTime")
                .and_then(Value::as_str)
                .unwrap_or("<unknown>")
        ),
        WorkflowEventType::Phase => format!(
            "phase: {}",
            event.data.get("name").and_then(Value::as_str).unwrap_or("")
        ),
        WorkflowEventType::Log => format!(
            "log: {}",
            event
                .data
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("")
        ),
        WorkflowEventType::AgentEvent => {
            lines.extend(agent_event_renderer(record).details_lines(&event.data));
            return lines;
        }
        WorkflowEventType::Result => format!(
            "result:\n{}",
            serde_json::to_string_pretty(&event.data).unwrap_or_else(|_| "<invalid>".into())
        ),
        WorkflowEventType::Error => format!(
            "error: {}",
            event
                .data
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("<unknown>")
        ),
        WorkflowEventType::Other(_) => {
            serde_json::to_string_pretty(&event.data).unwrap_or_else(|_| "<invalid>".into())
        }
    };
    lines.extend(body.lines().map(|line| Line::from(line.to_string())));
    lines
}

fn root_start_time(events: &[EventRecord]) -> Option<OffsetDateTime> {
    events.iter().find_map(|record| {
        let metadata = record.event.metadata.as_ref();
        let depth = metadata
            .and_then(|metadata| metadata.workflow_depth)
            .unwrap_or(0);
        if record.event.event_type == WorkflowEventType::Started && depth == 0 {
            record
                .event
                .data
                .get("startTime")
                .and_then(Value::as_str)
                .and_then(|value| OffsetDateTime::parse(value, &Rfc3339).ok())
        } else {
            None
        }
    })
}

fn display_time(app: &TuiReplayApp, event: &WorkflowEvent) -> String {
    match app.time_display {
        TimeDisplayMode::Elapsed => event
            .elapsed_nanos
            .map(format_elapsed)
            .unwrap_or_else(|| "+00:00:00.000".to_string()),
        TimeDisplayMode::LocalTime => format_local_time(app, event)
            .or_else(|| event.elapsed_nanos.map(format_elapsed))
            .unwrap_or_else(|| "+00:00:00.000".to_string()),
    }
}

fn format_local_time(app: &TuiReplayApp, event: &WorkflowEvent) -> Option<String> {
    let start = app.root_start_time?;
    let offset = app.local_offset.unwrap_or(UtcOffset::UTC);
    let nanos = event.elapsed_nanos.unwrap_or(0);
    let nanos = i64::try_from(nanos).unwrap_or(i64::MAX);
    let local = (start + TimeDuration::nanoseconds(nanos)).to_offset(offset);
    Some(format!(
        "{:02}:{:02}:{:02}.{:03}",
        local.hour(),
        local.minute(),
        local.second(),
        local.millisecond()
    ))
}

fn format_elapsed(nanos: u64) -> String {
    let millis = nanos / 1_000_000;
    let seconds = millis / 1_000;
    let minutes = seconds / 60;
    let hours = minutes / 60;
    format!(
        "+{:02}:{:02}:{:02}.{:03}",
        hours,
        minutes % 60,
        seconds % 60,
        millis % 1_000
    )
}

fn short_id(value: &str) -> String {
    let suffix = value.strip_prefix("step_").unwrap_or(value);
    suffix.chars().take(8).collect()
}

fn truncate(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let truncated = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}…")
    } else {
        truncated
    }
}
