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
use smol_workflow_engine::agent_providers::create_agent_provider;
use smol_workflow_engine::durable::runner::{run_local_durable_workflow, LocalDurableRunOptions};
use smol_workflow_engine::durable::sqlite::SqliteDurableStore;
use smol_workflow_engine::events::{WorkflowEvent, WorkflowEventType};
use smol_workflow_engine::workflow::AgentSessionLogSink;
use std::fs;
use std::io::{self, Stdout};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc};
use std::time::{Duration as StdDuration, Instant};
use time::format_description::well_known::Rfc3339;
use time::{Duration as TimeDuration, OffsetDateTime, UtcOffset};
use tokio::sync::watch;

mod provider;

const MIN_REPLAY_SPEED: f64 = 0.1;
const MAX_REPLAY_SPEED: f64 = 64.0;
const SEARCH_DEBOUNCE: StdDuration = StdDuration::from_millis(150);
const LIVE_POLL_INTERVAL: StdDuration = StdDuration::from_millis(33);
const LIVE_EVENTS_PER_TICK: usize = 256;
const TIMELINE_BOTTOM_MARGIN: u16 = 2;
const BREATHING_LIGHT_INTERVAL_NANOS: i128 = 140_000_000;
const DONE_TICK_DELAY: StdDuration = StdDuration::from_millis(700);

#[derive(Clone)]
pub struct ReplayCommandOptions {
    pub path: PathBuf,
    pub check: bool,
    pub timed: bool,
    pub speed: f64,
    pub max_delay: Option<StdDuration>,
}

pub struct RunCommandOptions {
    pub script_path: PathBuf,
    pub args: Value,
    pub agent_provider: String,
    pub db_path: PathBuf,
    pub db_path_is_default: bool,
    pub budget_total: Option<u64>,
    pub max_parallel_agent_requests: Option<usize>,
    pub resume_run_id: Option<String>,
    pub session_log_sink: Option<Arc<dyn AgentSessionLogSink>>,
}

pub fn replay_command(options: ReplayCommandOptions) -> anyhow::Result<()> {
    let events = read_event_records(&options.path)?;
    if options.check {
        print_check_summary(&options.path, &events);
        return Ok(());
    }

    run_replay_tui(events, options)
}

struct ChannelWorkflowEventSink {
    tx: mpsc::Sender<WorkflowEvent>,
}

impl ChannelWorkflowEventSink {
    fn new(tx: mpsc::Sender<WorkflowEvent>) -> Self {
        Self { tx }
    }
}

#[async_trait::async_trait]
impl smol_workflow_engine::events::WorkflowEventSink for ChannelWorkflowEventSink {
    async fn emit(&self, event: WorkflowEvent) -> anyhow::Result<()> {
        self.tx
            .send(event)
            .map_err(|_| anyhow::anyhow!("TUI event receiver stopped"))
    }
}

pub fn run_command(options: RunCommandOptions) -> anyhow::Result<()> {
    let (event_tx, event_rx) = mpsc::channel::<WorkflowEvent>();
    let (result_tx, result_rx) = mpsc::channel::<anyhow::Result<()>>();
    let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);

    std::thread::spawn(move || {
        let result = run_workflow_in_thread(options, event_tx, cancel_rx);
        let _ = result_tx.send(result);
    });

    run_live_tui(event_rx, result_rx, cancel_tx)
}

fn run_workflow_in_thread(
    options: RunCommandOptions,
    event_tx: mpsc::Sender<WorkflowEvent>,
    cancel_rx: watch::Receiver<bool>,
) -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let provider: Arc<dyn smol_workflow_engine::agent_providers::AgentProvider> =
            Arc::from(create_agent_provider(&options.agent_provider)?);
        if options.db_path_is_default {
            if let Some(parent) = options.db_path.parent() {
                fs::create_dir_all(parent).map_err(|error| {
                    anyhow::anyhow!(
                        "failed to create default database directory {}: {error}",
                        parent.display()
                    )
                })?;
            }
        }
        let mut store = SqliteDurableStore::open(&options.db_path)?;
        let mut durable_options =
            LocalDurableRunOptions::new(options.script_path, options.args, provider);
        durable_options.budget_total = options.budget_total;
        durable_options.max_parallel_agent_requests = options.max_parallel_agent_requests;
        durable_options.resume_run_id = options.resume_run_id;
        durable_options.cancel_rx = Some(cancel_rx);
        durable_options.event_sink = Some(Arc::new(ChannelWorkflowEventSink::new(event_tx)));
        durable_options.session_log_sink = options.session_log_sink;
        run_local_durable_workflow(&mut store, durable_options)
            .await
            .map(|_| ())
    })
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

#[derive(Clone, Debug)]
struct EventRecord {
    event: WorkflowEvent,
    raw: Value,
}

#[derive(Clone, Debug)]
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

#[derive(Debug, Clone)]
struct TimelineViewRequest {
    generation: u64,
    events: Vec<EventRecord>,
    active_tab_key: Option<(u32, Option<String>)>,
    search_query: String,
}

#[derive(Debug, Clone)]
struct TimelineViewSnapshot {
    generation: u64,
    tabs: Vec<WorkflowScopeTab>,
    active_tab: usize,
    rows: Vec<TimelineRow>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LiveStatus {
    Running,
    Cancelling,
    Done,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum TimelineRow {
    Event {
        event_index: usize,
    },
    AgentGroup {
        event_indices: Vec<usize>,
    },
    AgentChild {
        event_index: usize,
        position: usize,
        total: usize,
    },
}

impl TimelineRow {
    fn event_index(&self) -> usize {
        match self {
            Self::Event { event_index } | Self::AgentChild { event_index, .. } => *event_index,
            Self::AgentGroup { event_indices } => event_indices[0],
        }
    }
}

fn spawn_timeline_view_worker() -> (
    mpsc::Sender<TimelineViewRequest>,
    mpsc::Receiver<TimelineViewSnapshot>,
) {
    let (request_tx, request_rx) = mpsc::channel::<TimelineViewRequest>();
    let (snapshot_tx, snapshot_rx) = mpsc::channel::<TimelineViewSnapshot>();
    std::thread::spawn(move || {
        while let Ok(mut request) = request_rx.recv() {
            while let Ok(next) = request_rx.try_recv() {
                request = next;
            }

            let mut tabs = build_scope_tabs(&request.events);
            if tabs.is_empty() {
                tabs.push(WorkflowScopeTab {
                    label: "root".to_string(),
                    workflow_depth: 0,
                    parent_step_id: None,
                });
            }
            let active_tab = request
                .active_tab_key
                .as_ref()
                .and_then(|(depth, parent_step_id)| {
                    tabs.iter().position(|tab| {
                        tab.workflow_depth == *depth && tab.parent_step_id == *parent_step_id
                    })
                })
                .unwrap_or(0);
            let visible = tabs
                .get(active_tab)
                .map(|tab| visible_indices_for(&request.events, tab, &request.search_query))
                .unwrap_or_default();
            let rows = build_timeline_rows(&visible, &request.events);

            if snapshot_tx
                .send(TimelineViewSnapshot {
                    generation: request.generation,
                    tabs,
                    active_tab,
                    rows,
                })
                .is_err()
            {
                break;
            }
        }
    });
    (request_tx, snapshot_rx)
}

struct TuiReplayApp {
    source_events: Vec<EventRecord>,
    events: Vec<EventRecord>,
    tabs: Vec<WorkflowScopeTab>,
    timeline_rows: Vec<TimelineRow>,
    view_request_tx: mpsc::Sender<TimelineViewRequest>,
    view_snapshot_rx: mpsc::Receiver<TimelineViewSnapshot>,
    view_generation: u64,
    applied_view_generation: u64,
    pending_select_latest: bool,
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
    search_input: String,
    search_query: String,
    search_changed_at: Option<Instant>,
    warnings: Vec<String>,
    playback: PlaybackState,
    timed: bool,
    speed: f64,
    max_delay: Option<StdDuration>,
    next_due: Option<Instant>,
    replay_done_at: Option<Instant>,
    live_status: Option<LiveStatus>,
    live_status_changed_at: Option<Instant>,
    live_error: Option<String>,
    confirm_quit: bool,
    should_quit: bool,
}

impl TuiReplayApp {
    fn new(source_events: Vec<EventRecord>, options: &ReplayCommandOptions) -> Self {
        let events = Vec::new();
        let tabs = build_scope_tabs(&events);
        let selected_by_tab = vec![0; tabs.len()];
        let root_start_time = root_start_time(&source_events);
        let local_offset = UtcOffset::current_local_offset().ok();
        let (view_request_tx, view_snapshot_rx) = spawn_timeline_view_worker();
        let mut app = Self {
            warnings: validate_events(&source_events),
            source_events,
            tabs,
            timeline_rows: Vec::new(),
            view_request_tx,
            view_snapshot_rx,
            view_generation: 0,
            applied_view_generation: 0,
            pending_select_latest: false,
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
            search_input: String::new(),
            search_query: String::new(),
            search_changed_at: None,
            playback: PlaybackState::Paused,
            timed: options.timed,
            speed: normalize_replay_speed(options.speed),
            max_delay: options.max_delay,
            next_due: options.timed.then_some(Instant::now()),
            replay_done_at: None,
            live_status: None,
            live_status_changed_at: None,
            live_error: None,
            confirm_quit: false,
            should_quit: false,
        };
        app.request_view_update();
        app
    }

    fn new_live() -> Self {
        let tabs = vec![WorkflowScopeTab {
            label: "root".to_string(),
            workflow_depth: 0,
            parent_step_id: None,
        }];
        let (view_request_tx, view_snapshot_rx) = spawn_timeline_view_worker();
        let mut app = Self {
            warnings: Vec::new(),
            source_events: Vec::new(),
            tabs,
            timeline_rows: Vec::new(),
            view_request_tx,
            view_snapshot_rx,
            view_generation: 0,
            applied_view_generation: 0,
            pending_select_latest: false,
            events: Vec::new(),
            active_tab: 0,
            selected: 0,
            selected_by_tab: vec![0],
            details_scroll: 0,
            focus_pane: FocusPane::Timeline,
            raw_details: false,
            time_display: TimeDisplayMode::Elapsed,
            root_start_time: None,
            local_offset: UtcOffset::current_local_offset().ok(),
            search_open: false,
            search_input: String::new(),
            search_query: String::new(),
            search_changed_at: None,
            playback: PlaybackState::Paused,
            timed: false,
            speed: 1.0,
            max_delay: None,
            next_due: None,
            replay_done_at: None,
            live_status: Some(LiveStatus::Running),
            live_status_changed_at: Some(Instant::now()),
            live_error: None,
            confirm_quit: false,
            should_quit: false,
        };
        app.request_view_update();
        app
    }

    fn visible_rows(&self) -> &[TimelineRow] {
        &self.timeline_rows
    }

    fn selected_event_index(&self) -> Option<usize> {
        self.visible_rows()
            .get(self.selected)
            .map(TimelineRow::event_index)
    }

    fn selected_event(&self) -> Option<&EventRecord> {
        self.selected_event_index()
            .and_then(|index| self.events.get(index))
    }

    fn replay_complete(&self) -> bool {
        self.events.len() >= self.source_events.len()
    }

    fn set_live_status(&mut self, status: LiveStatus) {
        if self.live_status != Some(status) {
            self.live_status = Some(status);
            self.live_status_changed_at = Some(Instant::now());
        }
    }

    fn mark_replay_done_if_complete(&mut self) {
        if self.replay_complete() {
            self.replay_done_at.get_or_insert_with(Instant::now);
        } else {
            self.replay_done_at = None;
        }
    }

    fn request_view_update(&mut self) {
        self.view_generation = self.view_generation.saturating_add(1);
        let active_tab_key = self
            .tabs
            .get(self.active_tab)
            .map(|tab| (tab.workflow_depth, tab.parent_step_id.clone()));
        let request = TimelineViewRequest {
            generation: self.view_generation,
            events: self.events.clone(),
            active_tab_key,
            search_query: self.search_query.clone(),
        };
        let _ = self.view_request_tx.send(request);
    }

    fn apply_view_snapshots(&mut self) {
        let mut latest = None;
        while let Ok(snapshot) = self.view_snapshot_rx.try_recv() {
            latest = Some(snapshot);
        }
        let Some(snapshot) = latest else {
            return;
        };
        if snapshot.generation < self.applied_view_generation {
            return;
        }
        self.applied_view_generation = snapshot.generation;
        self.tabs = snapshot.tabs;
        self.active_tab = snapshot.active_tab.min(self.tabs.len().saturating_sub(1));
        self.timeline_rows = snapshot.rows;
        self.selected_by_tab.resize(self.tabs.len(), 0);
        if self.pending_select_latest {
            self.pending_select_latest = false;
            self.select_latest_visible();
        } else {
            self.clamp_selection();
        }
    }

    fn select_latest_visible(&mut self) {
        let len = self.visible_rows().len();
        if len > 0 {
            self.selected = len - 1;
            self.remember_selection();
        }
    }

    fn push_live_events(&mut self, events: impl IntoIterator<Item = WorkflowEvent>) {
        let mut changed = false;
        for event in events {
            let raw = serde_json::to_value(&event).unwrap_or(Value::Null);
            let record = EventRecord { event, raw };
            if self.root_start_time.is_none() {
                let single = vec![record.clone()];
                self.root_start_time = root_start_time(&single);
            }
            self.source_events.push(record.clone());
            self.events.push(record);
            changed = true;
        }
        if changed {
            self.pending_select_latest |= self.focus_pane == FocusPane::Timeline;
            self.request_view_update();
        }
    }

    fn reveal_next_event(&mut self) {
        if let Some(event) = self.source_events.get(self.events.len()).cloned() {
            self.events.push(event);
            self.pending_select_latest = true;
            self.mark_replay_done_if_complete();
            self.request_view_update();
        }
    }

    fn hide_last_event(&mut self) {
        if self.events.pop().is_some() {
            self.pending_select_latest = true;
            self.mark_replay_done_if_complete();
            self.request_view_update();
            self.reset_details_scroll();
        }
    }

    fn schedule_next_due(&mut self, now: Instant) {
        if self.replay_complete() {
            self.next_due = None;
            self.playback = PlaybackState::Paused;
            self.mark_replay_done_if_complete();
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
        let len = self.visible_rows().len();
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
        let len = self.visible_rows().len();
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
        let len = self.visible_rows().len();
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
        let len = self.visible_rows().len();
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

    fn mark_search_changed(&mut self) {
        self.search_changed_at = Some(Instant::now());
    }

    fn apply_search_if_changed(&mut self) {
        if self.search_input != self.search_query {
            self.search_query = self.search_input.clone();
            self.selected = 0;
            self.reset_details_scroll();
            self.request_view_update();
        }
        self.search_changed_at = None;
    }

    fn tick_search(&mut self) {
        if self
            .search_changed_at
            .is_some_and(|changed_at| changed_at.elapsed() >= SEARCH_DEBOUNCE)
        {
            self.apply_search_if_changed();
        }
    }

    fn next_tab(&mut self) {
        if !self.tabs.is_empty() {
            self.remember_selection();
            self.active_tab = (self.active_tab + 1) % self.tabs.len();
            self.restore_selection_for_active_tab();
            self.request_view_update();
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
            self.request_view_update();
        }
    }

    fn live_is_active(&self) -> bool {
        matches!(
            self.live_status,
            Some(LiveStatus::Running | LiveStatus::Cancelling)
        )
    }

    fn handle_key(&mut self, key: KeyEvent) {
        if key.kind == KeyEventKind::Release {
            return;
        }

        if self.confirm_quit {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => self.should_quit = true,
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => self.confirm_quit = false,
                _ => {}
            }
            return;
        }

        if self.search_open {
            self.handle_search_key(key);
            return;
        }

        match key.code {
            KeyCode::Char('q') | KeyCode::Esc if self.live_is_active() => self.confirm_quit = true,
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
            KeyCode::Char('/') => {
                self.search_input = self.search_query.clone();
                self.search_changed_at = None;
                self.search_open = true;
            }
            _ => {}
        }
        self.clamp_selection();
    }

    fn handle_search_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.search_open = false;
                self.search_input = self.search_query.clone();
                self.search_changed_at = None;
            }
            KeyCode::Enter => {
                self.apply_search_if_changed();
                self.search_open = false;
            }
            KeyCode::Backspace => {
                self.search_input.pop();
                self.mark_search_changed();
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.search_input.clear();
                self.mark_search_changed();
            }
            KeyCode::Char(ch)
                if self.search_input.ends_with(ch) && key.kind == KeyEventKind::Repeat => {}
            KeyCode::Char(ch) => {
                self.search_input.push(ch);
                self.mark_search_changed();
            }
            _ => {}
        }
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

fn build_timeline_rows(visible_indices: &[usize], events: &[EventRecord]) -> Vec<TimelineRow> {
    let mut grouped_rows = Vec::<TimelineRow>::new();
    let mut group_row_by_key = std::collections::HashMap::<String, usize>::new();

    for event_index in visible_indices.iter().copied() {
        let record = &events[event_index];
        let Some(group_key) = agent_group_key(record) else {
            grouped_rows.push(TimelineRow::Event { event_index });
            continue;
        };

        if let Some(row_index) = group_row_by_key.get(&group_key).copied() {
            if let TimelineRow::AgentGroup { event_indices } = &mut grouped_rows[row_index] {
                event_indices.push(event_index);
            }
            continue;
        }

        group_row_by_key.insert(group_key, grouped_rows.len());
        grouped_rows.push(TimelineRow::AgentGroup {
            event_indices: vec![event_index],
        });
    }

    let mut rows = Vec::with_capacity(visible_indices.len());
    for row in grouped_rows {
        match row {
            TimelineRow::AgentGroup { event_indices } => {
                let total = event_indices.len();
                rows.push(TimelineRow::AgentGroup {
                    event_indices: event_indices.clone(),
                });
                for (position, event_index) in event_indices.into_iter().enumerate().skip(1) {
                    rows.push(TimelineRow::AgentChild {
                        event_index,
                        position,
                        total,
                    });
                }
            }
            TimelineRow::Event { event_index } => rows.push(TimelineRow::Event { event_index }),
            TimelineRow::AgentChild { .. } => {
                unreachable!("children are added in the expansion pass")
            }
        }
    }

    rows
}

fn visible_indices_for(
    events: &[EventRecord],
    tab: &WorkflowScopeTab,
    search_query: &str,
) -> Vec<usize> {
    let query = search_query.to_ascii_lowercase();
    events
        .iter()
        .enumerate()
        .filter(|(_, record)| event_in_scope(record, tab))
        .filter(|(_, record)| provider::should_show_in_timeline(record))
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

fn run_live_tui(
    event_rx: mpsc::Receiver<WorkflowEvent>,
    result_rx: mpsc::Receiver<anyhow::Result<()>>,
    cancel_tx: watch::Sender<bool>,
) -> anyhow::Result<()> {
    let mut terminal = setup_terminal()?;
    let mut app = TuiReplayApp::new_live();
    let result = run_live_tui_loop(&mut terminal, &mut app, event_rx, result_rx, cancel_tx);
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
        app.tick_search();
        app.tick_playback();
        app.apply_view_snapshots();
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

fn run_live_tui_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &mut TuiReplayApp,
    event_rx: mpsc::Receiver<WorkflowEvent>,
    result_rx: mpsc::Receiver<anyhow::Result<()>>,
    cancel_tx: watch::Sender<bool>,
) -> anyhow::Result<()> {
    loop {
        app.tick_search();
        let mut event_batch = Vec::new();
        for _ in 0..LIVE_EVENTS_PER_TICK {
            match event_rx.try_recv() {
                Ok(event) => event_batch.push(event),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => break,
            }
        }
        app.push_live_events(event_batch);
        app.apply_view_snapshots();
        match result_rx.try_recv() {
            Ok(Ok(())) => app.set_live_status(LiveStatus::Done),
            Ok(Err(error)) => {
                app.set_live_status(LiveStatus::Failed);
                app.live_error = Some(error.to_string());
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                if app.live_status == Some(LiveStatus::Running)
                    || app.live_status == Some(LiveStatus::Cancelling)
                {
                    app.set_live_status(LiveStatus::Failed);
                    app.live_error = Some("workflow task stopped without a result".to_string());
                }
            }
        }

        terminal.draw(|frame| render(frame, app))?;
        if app.should_quit {
            break;
        }
        if event::poll(LIVE_POLL_INTERVAL)? {
            if let CrosstermEvent::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Release
                    && key.code == KeyCode::Char('c')
                    && key.modifiers.contains(KeyModifiers::CONTROL)
                {
                    let _ = cancel_tx.send(true);
                    app.set_live_status(LiveStatus::Cancelling);
                } else {
                    app.handle_key(key);
                }
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
    if app.confirm_quit {
        render_quit_confirmation(frame);
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
    let rows = app.visible_rows();
    let title = if app.search_query.is_empty() {
        format!(
            " Timeline ({}/{}) ",
            app.selected.saturating_add(1),
            rows.len()
        )
    } else {
        format!(
            " Timeline ({}/{}) search: {} ",
            app.selected.saturating_add(1),
            rows.len(),
            app.search_query
        )
    };
    let focused = app.focus_pane == FocusPane::Timeline;
    let title_color = if focused { Color::Cyan } else { Color::Blue };
    let (_title_area, content_area) = render_pane_shell(frame, area, title, title_color, focused);
    let content_area = pad_content_area(content_area, 2, 0);

    let query = app.search_query.to_ascii_lowercase();
    let height = usize::from(content_area.height).max(1);
    let bottom_margin = usize::from(TIMELINE_BOTTOM_MARGIN);
    let virtual_len = rows.len().saturating_add(bottom_margin);
    let virtual_selected = if rows.is_empty() {
        0
    } else {
        app.selected.saturating_add(bottom_margin)
    };
    let start = scroll_start(virtual_selected, virtual_len, height);
    let end = start.saturating_add(height).min(virtual_len);
    let items = (start..end)
        .map(|row_index| {
            let Some(row) = rows.get(row_index) else {
                return ListItem::new(Line::from(""));
            };
            let event_index = row.event_index();
            let summary = timeline_row_summary(app, row);
            let selected = row_index == app.selected;
            let search_match = !query.is_empty() && summary.to_ascii_lowercase().contains(&query);
            let line = Line::from(vec![Span::styled(
                summary,
                timeline_event_style(&app.events[event_index], selected, search_match),
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
        WorkflowEventType::AgentStarted => Style::default().fg(Color::LightBlue),
        WorkflowEventType::AgentEvent => Style::default().fg(Color::Green),
        WorkflowEventType::AgentCompleted => Style::default().fg(Color::LightGreen),
        WorkflowEventType::AgentFailed => {
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
        }
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

fn render_quit_confirmation(frame: &mut Frame<'_>) {
    let area = centered_rect(60, 5, frame.area());
    let paragraph = Paragraph::new(vec![
        Line::from("Quit live workflow TUI?"),
        Line::from(""),
        Line::from("Press y to quit the UI, n or Esc to stay."),
    ])
    .block(Block::default().borders(Borders::ALL).title("Confirm quit"));
    frame.render_widget(Clear, area);
    frame.render_widget(paragraph, area);
}

fn render_search_overlay(frame: &mut Frame<'_>, app: &TuiReplayApp) {
    let area = centered_rect(70, 3, frame.area());
    let input = Paragraph::new(format!("/{}", app.search_input)).block(
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

fn breathing_light() -> &'static str {
    const FRAMES: [&str; 8] = ["·", "∙", "•", "●", "●", "•", "∙", "·"];
    let tick = (OffsetDateTime::now_utc().unix_timestamp_nanos() / BREATHING_LIGHT_INTERVAL_NANOS)
        as usize;
    FRAMES[tick % FRAMES.len()]
}

fn status_line(app: &TuiReplayApp) -> Line<'static> {
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
    if let Some(live_status) = app.live_status {
        let terminal_indicator = |fallback: &str| {
            if app
                .live_status_changed_at
                .is_some_and(|changed_at| changed_at.elapsed() >= DONE_TICK_DELAY)
            {
                fallback.to_string()
            } else {
                breathing_light().to_string()
            }
        };
        let (status, status_style) = match live_status {
            LiveStatus::Running => (
                format!("{} LIVE RUNNING", breathing_light()),
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
            LiveStatus::Cancelling => (
                format!("{} LIVE CANCELLING", breathing_light()),
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
            LiveStatus::Done => (
                format!("{} LIVE DONE", terminal_indicator("✓")),
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            LiveStatus::Failed => (
                format!("{} LIVE FAILED", terminal_indicator("✗")),
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
        };
        let error = app
            .live_error
            .as_ref()
            .map(|error| format!("  error: {}", truncate(error, 80)))
            .unwrap_or_default();
        return Line::from(vec![
            Span::raw(" "),
            Span::styled(status, status_style),
            Span::raw(format!(
                "  {run_id}  events {}  tab {}/{}  time {time_mode}{error}",
                app.events.len(),
                app.active_tab + 1,
                app.tabs.len(),
            )),
        ]);
    }
    let (playback, playback_style) = match app.playback {
        PlaybackState::Playing => (
            "REPLAY PLAYING".to_string(),
            Style::default()
                .fg(Color::Rgb(255, 165, 0))
                .add_modifier(Modifier::BOLD),
        ),
        PlaybackState::Paused if app.replay_complete() => {
            let indicator = if app
                .replay_done_at
                .is_none_or(|done_at| done_at.elapsed() >= DONE_TICK_DELAY)
            {
                "✓"
            } else {
                breathing_light()
            };
            (
                format!("{indicator} REPLAY DONE"),
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            )
        }
        PlaybackState::Paused => (
            "REPLAY PAUSED".to_string(),
            Style::default()
                .fg(Color::Rgb(255, 165, 0))
                .add_modifier(Modifier::BOLD),
        ),
    };
    Line::from(vec![
        Span::raw(" "),
        Span::styled(playback, playback_style),
        Span::raw(format!(
            "  {run_id}  events {}/{}  tab {}/{}  speed {:.2}x  time {time_mode}{}",
            app.events.len(),
            app.source_events.len(),
            app.active_tab + 1,
            app.tabs.len(),
            app.speed,
            warnings
        )),
    ])
}

fn timeline_row_summary(app: &TuiReplayApp, row: &TimelineRow) -> String {
    match row {
        TimelineRow::Event { event_index } => event_summary(app, &app.events[*event_index]),
        TimelineRow::AgentGroup { event_indices } => agent_group_summary(app, event_indices),
        TimelineRow::AgentChild {
            event_index,
            position,
            total,
        } => agent_child_summary(app, *event_index, *position, *total),
    }
}

fn agent_group_summary(app: &TuiReplayApp, matching: &[usize]) -> String {
    let record = &app.events[matching[0]];
    let event = &record.event;
    let elapsed = display_time(app, event);
    let metadata = event.metadata.as_ref();
    let provider = metadata
        .and_then(|metadata| metadata.provider.as_deref())
        .unwrap_or("<provider>");
    let session = matching
        .iter()
        .find_map(|index| {
            app.events[*index]
                .event
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.session_id.as_deref())
        })
        .map(short_id)
        .unwrap_or_else(|| "pending".to_string());
    let depth = metadata
        .and_then(|metadata| metadata.workflow_depth)
        .unwrap_or(0);
    let indent = timeline_indent(depth);
    let status = if matching
        .iter()
        .any(|index| app.events[*index].event.event_type == WorkflowEventType::AgentFailed)
    {
        "failed"
    } else if matching
        .iter()
        .any(|index| app.events[*index].event.event_type == WorkflowEventType::AgentCompleted)
    {
        "completed"
    } else {
        "running"
    };
    let provider_events = matching
        .iter()
        .filter(|index| app.events[**index].event.event_type == WorkflowEventType::AgentEvent)
        .count();
    format!(
        "{elapsed} {indent}agent {provider} {status} session={session} events={provider_events}"
    )
}

fn agent_child_summary(
    app: &TuiReplayApp,
    event_index: usize,
    position: usize,
    total: usize,
) -> String {
    let record = &app.events[event_index];
    let event = &record.event;
    let elapsed = display_time(app, event);
    let branch = if position + 1 == total {
        "└─"
    } else {
        "├─"
    };
    let depth = event
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.workflow_depth)
        .unwrap_or(0);
    let indent = timeline_indent(depth.saturating_add(1));
    let label = agent_group_item_label(record);
    format!("{elapsed} {indent}{branch} {label}")
}

fn agent_group_item_label(record: &EventRecord) -> String {
    match record.event.event_type {
        WorkflowEventType::AgentStarted => "started".to_string(),
        WorkflowEventType::AgentCompleted => "completed".to_string(),
        WorkflowEventType::AgentFailed => format!(
            "failed {}",
            record
                .event
                .data
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("")
        ),
        WorkflowEventType::AgentEvent => provider::timeline_label(record),
        _ => event_summary_without_time(record),
    }
}

fn timeline_indent(depth: u32) -> String {
    "  ".repeat(usize::try_from(depth).unwrap_or(usize::MAX / 2))
}

fn agent_group_key(record: &EventRecord) -> Option<String> {
    if !matches!(
        record.event.event_type,
        WorkflowEventType::AgentStarted
            | WorkflowEventType::AgentEvent
            | WorkflowEventType::AgentCompleted
            | WorkflowEventType::AgentFailed
    ) {
        return None;
    }
    let metadata = record.event.metadata.as_ref()?;
    metadata
        .step_id
        .as_ref()
        .or(metadata.session_id.as_ref())
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

fn event_summary_without_time(record: &EventRecord) -> String {
    let event = &record.event;
    match event.event_type {
        WorkflowEventType::Started => "workflow.started".to_string(),
        WorkflowEventType::Phase => format!(
            "workflow.phase {}",
            event.data.get("name").and_then(Value::as_str).unwrap_or("")
        ),
        WorkflowEventType::Log => format!(
            "workflow.log {}",
            truncate(
                event
                    .data
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or(""),
                80,
            )
        ),
        WorkflowEventType::AgentStarted => "agent.started".to_string(),
        WorkflowEventType::AgentEvent => provider::timeline_label(record),
        WorkflowEventType::AgentCompleted => "agent.completed".to_string(),
        WorkflowEventType::AgentFailed => "agent.failed".to_string(),
        WorkflowEventType::Result => "workflow.result".to_string(),
        WorkflowEventType::Error => format!(
            "workflow.error {}",
            event
                .data
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("")
        ),
        WorkflowEventType::Other(ref event_type) => event_type.clone(),
    }
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
        WorkflowEventType::AgentStarted => {
            let provider = event
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.provider.as_deref())
                .unwrap_or("<provider>");
            format!("{elapsed} {indent}agent.started {provider}")
        }
        WorkflowEventType::AgentEvent => {
            let metadata = event.metadata.as_ref();
            let provider = metadata
                .and_then(|metadata| metadata.provider.as_deref())
                .unwrap_or("<provider>");
            let session = metadata
                .and_then(|metadata| metadata.session_id.as_deref())
                .map(short_id)
                .unwrap_or_else(|| "<session>".to_string());
            let label = provider::timeline_label(record);
            format!("{elapsed} {indent}workflow.agent_event {provider} {session} {label}")
        }
        WorkflowEventType::AgentCompleted => {
            let provider = event
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.provider.as_deref())
                .unwrap_or("<provider>");
            format!("{elapsed} {indent}agent.completed {provider}")
        }
        WorkflowEventType::AgentFailed => {
            let provider = event
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.provider.as_deref())
                .unwrap_or("<provider>");
            let message = event
                .data
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("");
            format!("{elapsed} {indent}agent.failed {provider} {message}")
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
        WorkflowEventType::AgentStarted => format!(
            "agent started:\n{}",
            serde_json::to_string_pretty(&event.data).unwrap_or_else(|_| "<invalid>".into())
        ),
        WorkflowEventType::AgentEvent => {
            lines.extend(provider::details_lines(record));
            return lines;
        }
        WorkflowEventType::AgentCompleted => format!(
            "agent completed:\n{}",
            serde_json::to_string_pretty(&event.data).unwrap_or_else(|_| "<invalid>".into())
        ),
        WorkflowEventType::AgentFailed => format!(
            "agent failed: {}",
            event
                .data
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("<unknown>")
        ),
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use smol_workflow_engine::events::WorkflowEventMetadata;

    fn agent_record(
        event_type: WorkflowEventType,
        step_id: &str,
        elapsed_nanos: u64,
    ) -> EventRecord {
        let event = WorkflowEvent {
            event_type,
            elapsed_nanos: Some(elapsed_nanos),
            metadata: Some(WorkflowEventMetadata {
                step_id: Some(step_id.to_string()),
                provider: Some("test".to_string()),
                workflow_depth: Some(0),
                ..WorkflowEventMetadata::default()
            }),
            data: json!({}),
        };
        EventRecord {
            raw: serde_json::to_value(&event).unwrap(),
            event,
        }
    }

    fn log_record(message: &str, elapsed_nanos: u64) -> EventRecord {
        let event = WorkflowEvent {
            event_type: WorkflowEventType::Log,
            elapsed_nanos: Some(elapsed_nanos),
            metadata: Some(WorkflowEventMetadata {
                workflow_depth: Some(0),
                ..WorkflowEventMetadata::default()
            }),
            data: json!({ "message": message }),
        };
        EventRecord {
            raw: serde_json::to_value(&event).unwrap(),
            event,
        }
    }

    #[test]
    fn timeline_rows_group_interleaved_agent_events_by_step_id() {
        let events = vec![
            agent_record(WorkflowEventType::AgentStarted, "step_a", 0),
            agent_record(WorkflowEventType::AgentStarted, "step_b", 1),
            agent_record(WorkflowEventType::AgentEvent, "step_a", 2),
            agent_record(WorkflowEventType::AgentEvent, "step_b", 3),
            agent_record(WorkflowEventType::AgentCompleted, "step_a", 4),
            agent_record(WorkflowEventType::AgentCompleted, "step_b", 5),
        ];
        let visible = (0..events.len()).collect::<Vec<_>>();

        assert_eq!(
            build_timeline_rows(&visible, &events),
            vec![
                TimelineRow::AgentGroup {
                    event_indices: vec![0, 2, 4]
                },
                TimelineRow::AgentChild {
                    event_index: 2,
                    position: 1,
                    total: 3
                },
                TimelineRow::AgentChild {
                    event_index: 4,
                    position: 2,
                    total: 3
                },
                TimelineRow::AgentGroup {
                    event_indices: vec![1, 3, 5]
                },
                TimelineRow::AgentChild {
                    event_index: 3,
                    position: 1,
                    total: 3
                },
                TimelineRow::AgentChild {
                    event_index: 5,
                    position: 2,
                    total: 3
                },
            ]
        );
    }

    #[test]
    fn timeline_rows_keep_non_agent_events_in_stream_order() {
        let events = vec![
            log_record("before", 0),
            agent_record(WorkflowEventType::AgentStarted, "step_a", 1),
            agent_record(WorkflowEventType::AgentEvent, "step_a", 2),
            log_record("after", 3),
        ];
        let visible = (0..events.len()).collect::<Vec<_>>();

        assert_eq!(
            build_timeline_rows(&visible, &events),
            vec![
                TimelineRow::Event { event_index: 0 },
                TimelineRow::AgentGroup {
                    event_indices: vec![1, 2]
                },
                TimelineRow::AgentChild {
                    event_index: 2,
                    position: 1,
                    total: 2
                },
                TimelineRow::Event { event_index: 3 },
            ]
        );
    }
}
