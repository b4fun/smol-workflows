//! Terminal UI for replaying or observing workflow event streams.
//!
//! # Memory model
//!
//! Large provider traces can be hundreds of megabytes. The TUI therefore avoids
//! keeping multiple full JSON copies of the same event:
//!
//! - replay input is parsed line-by-line with a `BufReader`, not loaded as one
//!   giant string;
//! - `EventRecord` stores the parsed `WorkflowEvent` behind an `Arc`, so
//!   `source_events` and currently revealed `events` share payloads during
//!   replay;
//! - timeline view rebuilds run on a worker thread using lightweight
//!   `TimelineViewEvent` summaries rather than cloning provider-owned JSON;
//! - expensive search strings are built only while a non-empty search query is
//!   applied;
//! - Pi `message_update` payloads are compacted because they are hidden from the
//!   default timeline and can dominate trace size with repeated partial message
//!   snapshots.
//!
//! Keep this shape in mind when adding new UI state: prefer indices, metadata,
//! or `Arc` references over cloning `WorkflowEvent.data` / raw provider payloads.
//!
//! # Layout and interaction model
//!
//! Keep this section in sync with user-visible TUI changes. When changing layout,
//! keybindings, modal behavior, header stats, save behavior, or pane rendering,
//! update this summary in the same change.
//!
//! The UI uses a compact btop-inspired layout with rounded Unicode borders:
//!
//! - the top header pane is `6` rows tall. Its left column contains the
//!   live/replay status indicator and a small bordered workflow-tabs sub-pane.
//!   In live mode the status includes a wall-clock `+HH:MM:SS` delta that is
//!   independent of the timeline time-display toggle; the right side contains
//!   workflow stats without a surrounding sub-border;
//! - workflow tabs show at most three labels. Root is `root`; child scopes use
//!   `c: <4-char-id>`. `Tab` / `Shift+Tab` cycle scopes and an overflow `+N`
//!   marker appears when additional scopes exist;
//! - header stats are fixed-width, left-aligned columns: workflow run ID,
//!   a one-line btop-style Braille event-rate graph plus observed count, token
//!   usage (`+in`, `+out`), and a full-width `file:` line below them;
//! - replay mode shows the source events file. Live mode shows `<unsaved>` until
//!   stopped, then offers `save?`; pressing `s` writes `$PWD/<run-id>.jsonl`.
//!   Exiting after a save restores the terminal and prints the saved path to
//!   stderr;
//! - the body is split into `¹timeline` and `²details` panes. Pane focus uses
//!   `1` / `2`; timeline/details scrolling uses `↑` / `↓` or `j` / `k`;
//! - timeline rows are derived from the event stream. Agent lifecycle/provider
//!   events are grouped by `metadata.stepId`, root scope shows nested workflows,
//!   child scopes filter by `parentStepId`, and the visible list has right-side
//!   scroll indicators plus an 8-row virtual bottom margin. Moving selection to
//!   the latest row re-enables follow-latest during live/replay updates;
//! - details has `pretty/raw` in the border (`p` / `r` select modes), a fixed
//!   top-right metadata overlay (`m` toggles), word-wrapped body text that flows
//!   around the overlay, right-side scroll indicators, and `y` copies the
//!   logical details text without terminal-wrap line breaks;
//! - live quit uses a button-style confirmation modal. Arrow keys / `h` / `l` /
//!   `Tab` select buttons; `Enter` activates. If a stopped live run is unsaved,
//!   `save & quit` is first and selected by default.

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
use ratatui::widgets::{Block, BorderType, Borders, Clear, List, ListItem, Paragraph};
use ratatui::{Frame, Terminal};
use serde_json::Value;
use smol_workflow_engine::agent_providers::create_agent_provider;
use smol_workflow_engine::durable::runner::{run_local_durable_workflow, LocalDurableRunOptions};
use smol_workflow_engine::durable::sqlite::SqliteDurableStore;
use smol_workflow_engine::events::{WorkflowEvent, WorkflowEventMetadata, WorkflowEventType};
use smol_workflow_engine::workflow::AgentSessionLogSink;
use std::fs;
use std::io::{self, BufRead, Stdout, Write};
use std::path::{Path, PathBuf};
use std::process::{Command as StdCommand, Stdio};
use std::sync::{mpsc, Arc};
use std::time::{Duration as StdDuration, Instant};
use time::format_description::well_known::Rfc3339;
use time::{Duration as TimeDuration, OffsetDateTime, UtcOffset};
use tokio::sync::watch;

mod provider;

const SEARCH_DEBOUNCE: StdDuration = StdDuration::from_millis(150);
const LIVE_POLL_INTERVAL: StdDuration = StdDuration::from_millis(33);
const LIVE_EVENTS_PER_TICK: usize = 256;
const TIMED_REPLAY_EVENTS_PER_TICK: usize = 512;
const DEFAULT_REPLAY_MAX_DELAY: StdDuration = StdDuration::from_millis(50);
const TIMELINE_BOTTOM_MARGIN: u16 = 8;
const EVENT_RATE_BIN_NANOS: u64 = 10_000_000_000;
const BREATHING_LIGHT_INTERVAL_NANOS: i128 = 140_000_000;
const DONE_TICK_DELAY: StdDuration = StdDuration::from_millis(700);

#[derive(Clone)]
pub struct ReplayCommandOptions {
    pub path: PathBuf,
    pub check: bool,
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
    /// Shared parsed event envelope. Replay keeps both the complete source
    /// stream and the revealed prefix; using `Arc` prevents every reveal from
    /// duplicating provider payload JSON.
    event: Arc<WorkflowEvent>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfirmQuitAction {
    Quit,
    SaveAndQuit,
    Stay,
}

/// Lightweight projection used by the background timeline worker.
///
/// This intentionally excludes `WorkflowEvent.data`: timeline grouping, tabs,
/// provider filtering, and non-search rendering only need event type/metadata.
/// When search is active, `search_text` contains a compact searchable string.
#[derive(Debug, Clone)]
struct TimelineViewEvent {
    event_type: WorkflowEventType,
    metadata: Option<WorkflowEventMetadata>,
    timeline_visible: bool,
    search_text: Option<String>,
}

#[derive(Debug, Clone)]
struct TimelineViewRequest {
    generation: u64,
    events: Vec<TimelineViewEvent>,
    active_tab_key: Option<(u32, Option<String>)>,
    search_query: String,
}

#[derive(Debug, Clone)]
struct TimelineViewSnapshot {
    generation: u64,
    events_len: usize,
    active_tab_key: Option<(u32, Option<String>)>,
    search_query: String,
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

            let mut tabs = build_scope_tabs_for_view(&request.events);
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
                .map(|tab| visible_indices_for_view(&request.events, tab, &request.search_query))
                .unwrap_or_default();
            let rows = build_timeline_rows_for_view(&visible, &request.events);

            if snapshot_tx
                .send(TimelineViewSnapshot {
                    generation: request.generation,
                    events_len: request.events.len(),
                    active_tab_key: request.active_tab_key,
                    search_query: request.search_query,
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
    follow_latest: bool,
    active_tab: usize,
    selected: usize,
    selected_by_tab: Vec<usize>,
    selected_event_anchor: Option<usize>,
    details_scroll: usize,
    focus_pane: FocusPane,
    raw_details: bool,
    metadata_open: bool,
    time_display: TimeDisplayMode,
    last_details_visible_text: String,
    root_start_time: Option<OffsetDateTime>,
    local_offset: Option<UtcOffset>,
    search_open: bool,
    search_input: String,
    search_query: String,
    search_changed_at: Option<Instant>,
    warnings: Vec<String>,
    events_file: Option<PathBuf>,
    saved_events_file: Option<PathBuf>,
    save_and_quit_requested: bool,
    root_result_token_usage: Option<(u64, u64)>,
    workflow_token_usage_by_key: std::collections::HashMap<String, (u64, u64)>,
    agent_token_usage_by_key: std::collections::HashMap<String, (u64, u64)>,
    observed_token_usage: Option<(u64, u64)>,
    event_rate_bins: std::collections::HashMap<u64, usize>,
    latest_event_rate_bin: u64,
    playback: PlaybackState,
    max_delay: Option<StdDuration>,
    next_due: Option<Instant>,
    replay_done_at: Option<Instant>,
    live_status: Option<LiveStatus>,
    live_started_at: Option<Instant>,
    live_finished_delta: Option<StdDuration>,
    live_status_changed_at: Option<Instant>,
    live_error: Option<String>,
    confirm_quit: bool,
    confirm_quit_action: ConfirmQuitAction,
    toast: Option<ToastMessage>,
    should_quit: bool,
}

struct ToastMessage {
    message: String,
    created_at: Instant,
    ttl: StdDuration,
}

impl ToastMessage {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            created_at: Instant::now(),
            ttl: StdDuration::from_millis(1500),
        }
    }

    fn expired(&self) -> bool {
        self.created_at.elapsed() >= self.ttl
    }
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
            follow_latest: true,
            events,
            active_tab: 0,
            selected: 0,
            selected_by_tab,
            selected_event_anchor: None,
            details_scroll: 0,
            focus_pane: FocusPane::Timeline,
            raw_details: false,
            metadata_open: true,
            time_display: TimeDisplayMode::Elapsed,
            last_details_visible_text: String::new(),
            root_start_time,
            local_offset,
            search_open: false,
            search_input: String::new(),
            search_query: String::new(),
            search_changed_at: None,
            events_file: Some(options.path.clone()),
            saved_events_file: None,
            save_and_quit_requested: false,
            root_result_token_usage: None,
            workflow_token_usage_by_key: std::collections::HashMap::new(),
            agent_token_usage_by_key: std::collections::HashMap::new(),
            observed_token_usage: None,
            event_rate_bins: std::collections::HashMap::new(),
            latest_event_rate_bin: 0,
            playback: PlaybackState::Paused,
            max_delay: Some(options.max_delay.unwrap_or(DEFAULT_REPLAY_MAX_DELAY)),
            next_due: Some(Instant::now()),
            replay_done_at: None,
            live_status: None,
            live_started_at: None,
            live_finished_delta: None,
            live_status_changed_at: None,
            live_error: None,
            confirm_quit: false,
            confirm_quit_action: ConfirmQuitAction::Stay,
            toast: None,
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
            follow_latest: true,
            events: Vec::new(),
            active_tab: 0,
            selected: 0,
            selected_by_tab: vec![0],
            selected_event_anchor: None,
            details_scroll: 0,
            focus_pane: FocusPane::Timeline,
            raw_details: false,
            metadata_open: true,
            time_display: TimeDisplayMode::Elapsed,
            last_details_visible_text: String::new(),
            root_start_time: None,
            local_offset: UtcOffset::current_local_offset().ok(),
            search_open: false,
            search_input: String::new(),
            search_query: String::new(),
            search_changed_at: None,
            events_file: None,
            saved_events_file: None,
            save_and_quit_requested: false,
            root_result_token_usage: None,
            workflow_token_usage_by_key: std::collections::HashMap::new(),
            agent_token_usage_by_key: std::collections::HashMap::new(),
            observed_token_usage: None,
            event_rate_bins: std::collections::HashMap::new(),
            latest_event_rate_bin: 0,
            playback: PlaybackState::Paused,
            max_delay: None,
            next_due: None,
            replay_done_at: None,
            live_status: Some(LiveStatus::Running),
            live_started_at: Some(Instant::now()),
            live_finished_delta: None,
            live_status_changed_at: Some(Instant::now()),
            live_error: None,
            confirm_quit: false,
            confirm_quit_action: ConfirmQuitAction::Stay,
            toast: None,
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
            .filter(|index| *index < self.events.len())
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
            if matches!(status, LiveStatus::Done | LiveStatus::Failed) {
                if let Some(started_at) = self.live_started_at {
                    self.live_finished_delta
                        .get_or_insert_with(|| started_at.elapsed());
                }
            }
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

    fn active_tab_key(&self) -> Option<(u32, Option<String>)> {
        self.tabs
            .get(self.active_tab)
            .map(|tab| (tab.workflow_depth, tab.parent_step_id.clone()))
    }

    fn request_view_update(&mut self) {
        self.view_generation = self.view_generation.saturating_add(1);
        let active_tab_key = self.active_tab_key();
        let search_query = self.search_query.clone();
        let include_search_text = !search_query.is_empty();
        let events = self
            .events
            .iter()
            .map(|record| TimelineViewEvent {
                event_type: record.event.event_type.clone(),
                metadata: record.event.metadata.clone(),
                timeline_visible: provider::should_show_in_timeline(record),
                search_text: include_search_text.then(|| searchable_event_text(record)),
            })
            .collect();
        let request = TimelineViewRequest {
            generation: self.view_generation,
            events,
            active_tab_key,
            search_query,
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
        if snapshot.generation <= self.applied_view_generation
            || snapshot.generation > self.view_generation
            || snapshot.events_len > self.events.len()
            || snapshot.search_query != self.search_query
            || snapshot.active_tab_key != self.active_tab_key()
        {
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
        } else if let Some(anchor) = self.selected_event_anchor {
            if let Some(row_index) = self
                .timeline_rows
                .iter()
                .position(|row| row.event_index() == anchor && anchor < self.events.len())
            {
                self.selected = row_index;
                self.remember_selection();
            } else {
                self.clamp_selection();
                self.anchor_current_selection();
            }
        } else {
            self.clamp_selection();
        }
    }

    fn anchor_current_selection(&mut self) {
        self.selected_event_anchor = self.selected_event_index();
    }

    fn select_latest_visible(&mut self) {
        let len = self.visible_rows().len();
        if len > 0 {
            self.selected = len - 1;
            self.follow_latest = true;
            self.selected_event_anchor = None;
            self.remember_selection();
        }
    }

    fn push_live_events(&mut self, events: impl IntoIterator<Item = WorkflowEvent>) {
        let mut changed = false;
        for event in events {
            let mut event = event;
            compact_hidden_agent_event_data(&mut event);
            let record = EventRecord {
                event: Arc::new(event),
            };
            if self.root_start_time.is_none() {
                let single = vec![record.clone()];
                self.root_start_time = root_start_time(&single);
            }
            let event_index = self.events.len();
            self.update_header_stats_for_record(&record, event_index);
            self.source_events.push(record.clone());
            self.events.push(record);
            changed = true;
        }
        if changed {
            self.pending_select_latest |=
                self.follow_latest && self.focus_pane == FocusPane::Timeline;
            self.request_view_update();
        }
    }

    fn update_header_stats_for_record(&mut self, record: &EventRecord, event_index: usize) {
        let elapsed_bin = record.event.elapsed_nanos.unwrap_or(0) / EVENT_RATE_BIN_NANOS;
        self.latest_event_rate_bin = self.latest_event_rate_bin.max(elapsed_bin);
        *self.event_rate_bins.entry(elapsed_bin).or_insert(0) += 1;

        match record.event.event_type {
            WorkflowEventType::Result => {
                if let Some(usage) = workflow_token_usage(&record.event.data) {
                    if record
                        .event
                        .metadata
                        .as_ref()
                        .and_then(|metadata| metadata.workflow_depth)
                        .unwrap_or(0)
                        == 0
                    {
                        self.root_result_token_usage = Some(usage);
                    }
                    self.workflow_token_usage_by_key
                        .insert(workflow_usage_key(record, event_index), usage);
                    self.refresh_observed_token_usage();
                }
            }
            WorkflowEventType::AgentEvent => {
                if let Some(usage) = provider_token_usage(&record.event.data) {
                    self.agent_token_usage_by_key
                        .insert(agent_usage_key(record, event_index), usage);
                    self.refresh_observed_token_usage();
                }
            }
            _ => {}
        }
    }

    fn refresh_observed_token_usage(&mut self) {
        self.observed_token_usage = if let Some(usage) = self.root_result_token_usage {
            Some(usage)
        } else {
            let agent_total = sum_token_usage(self.agent_token_usage_by_key.values().copied());
            if agent_total != (0, 0) {
                Some(agent_total)
            } else {
                let workflow_total =
                    sum_token_usage(self.workflow_token_usage_by_key.values().copied());
                (workflow_total != (0, 0)).then_some(workflow_total)
            }
        };
    }

    fn reveal_events_until(&mut self, target_len: usize) {
        let current_len = self.events.len();
        let target_len = target_len.min(self.source_events.len());
        if target_len <= current_len {
            return;
        }
        let revealed = self.source_events[current_len..target_len].to_vec();
        for (offset, record) in revealed.iter().enumerate() {
            self.update_header_stats_for_record(record, current_len + offset);
        }
        self.events.extend(revealed);
        self.pending_select_latest |= self.follow_latest;
        self.mark_replay_done_if_complete();
        self.request_view_update();
    }

    fn schedule_next_due(&mut self, now: Instant) {
        if self.replay_complete() {
            self.next_due = None;
            self.playback = PlaybackState::Paused;
            self.mark_replay_done_if_complete();
            return;
        }
        let next_index = self.events.len();
        let delay = replay_delay(
            self.source_events.get(next_index.saturating_sub(1)),
            self.source_events.get(next_index),
            self.max_delay,
        );
        self.next_due = Some(now + delay);
    }

    fn tick_playback(&mut self) {
        if self.playback != PlaybackState::Playing {
            return;
        }
        let now = Instant::now();
        let Some(mut due) = self.next_due else {
            self.schedule_next_due(now);
            return;
        };
        let mut target_len = self.events.len();
        for _ in 0..TIMED_REPLAY_EVENTS_PER_TICK {
            if now < due || target_len >= self.source_events.len() {
                break;
            }
            target_len = target_len.saturating_add(1);
            if target_len >= self.source_events.len() {
                break;
            }
            due += replay_delay(
                self.source_events.get(target_len.saturating_sub(1)),
                self.source_events.get(target_len),
                self.max_delay,
            );
        }
        if target_len > self.events.len() {
            self.reveal_events_until(target_len);
            if self.replay_complete() {
                self.schedule_next_due(now);
            } else {
                self.next_due = Some(due);
            }
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
            if self.selected == len - 1 {
                self.follow_latest = true;
                self.selected_event_anchor = None;
            } else if self.selected != previous {
                self.follow_latest = false;
                self.anchor_current_selection();
            }
            if self.selected != previous {
                self.reset_details_scroll();
            }
        }
    }

    fn select_previous(&mut self) {
        let previous = self.selected;
        self.selected = self.selected.saturating_sub(1);
        if self.selected != previous {
            self.follow_latest = false;
            self.anchor_current_selection();
            self.reset_details_scroll();
        }
    }

    fn scroll_details_down(&mut self) {
        self.details_scroll = self.details_scroll.saturating_add(1);
    }

    fn scroll_details_up(&mut self) {
        self.details_scroll = self.details_scroll.saturating_sub(1);
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

    fn tick_toast(&mut self) {
        if self.toast.as_ref().is_some_and(ToastMessage::expired) {
            self.toast = None;
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

    fn live_events_unsaved(&self) -> bool {
        self.live_status.is_some() && self.events_file.is_none()
    }

    fn can_save_live_events(&self) -> bool {
        self.live_events_unsaved() && !self.live_is_active()
    }

    fn should_confirm_quit(&self) -> bool {
        self.live_is_active() || self.live_events_unsaved()
    }

    fn open_quit_confirmation(&mut self) {
        self.confirm_quit = true;
        self.confirm_quit_action = if self.can_save_live_events() {
            ConfirmQuitAction::SaveAndQuit
        } else {
            ConfirmQuitAction::Stay
        };
    }

    fn confirm_quit_actions(&self) -> Vec<ConfirmQuitAction> {
        let mut actions = Vec::new();
        if self.can_save_live_events() {
            actions.push(ConfirmQuitAction::SaveAndQuit);
        }
        actions.push(ConfirmQuitAction::Quit);
        actions.push(ConfirmQuitAction::Stay);
        actions
    }

    fn move_confirm_quit_selection(&mut self, offset: isize) {
        let actions = self.confirm_quit_actions();
        let current = actions
            .iter()
            .position(|action| *action == self.confirm_quit_action)
            .unwrap_or_else(|| actions.len().saturating_sub(1));
        let next = if offset.is_negative() {
            current.saturating_sub(offset.unsigned_abs())
        } else {
            current
                .saturating_add(offset as usize)
                .min(actions.len().saturating_sub(1))
        };
        self.confirm_quit_action = actions[next];
    }

    fn activate_confirm_quit_selection(&mut self) {
        match self.confirm_quit_action {
            ConfirmQuitAction::Quit => self.should_quit = true,
            ConfirmQuitAction::SaveAndQuit if self.can_save_live_events() => {
                self.confirm_quit = false;
                self.save_and_quit_requested = true;
            }
            ConfirmQuitAction::Stay | ConfirmQuitAction::SaveAndQuit => self.confirm_quit = false,
        }
    }

    fn save_live_events(&mut self) -> anyhow::Result<PathBuf> {
        let path = live_events_save_path(self);
        let mut file = fs::File::create(&path)?;
        for record in &self.events {
            serde_json::to_writer(&mut file, record.event.as_ref())?;
            file.write_all(b"\n")?;
        }
        self.events_file = Some(path.clone());
        self.saved_events_file = Some(path.clone());
        Ok(path)
    }

    fn handle_key(&mut self, key: KeyEvent) {
        if key.kind == KeyEventKind::Release {
            return;
        }

        if self.confirm_quit {
            match key.code {
                KeyCode::Left | KeyCode::Char('h') | KeyCode::BackTab => {
                    self.move_confirm_quit_selection(-1)
                }
                KeyCode::Right | KeyCode::Char('l') | KeyCode::Tab => {
                    self.move_confirm_quit_selection(1)
                }
                KeyCode::Enter => self.activate_confirm_quit_selection(),
                KeyCode::Char('y')
                | KeyCode::Char('Y')
                | KeyCode::Char('q')
                | KeyCode::Char('Q') => self.should_quit = true,
                KeyCode::Char('s') | KeyCode::Char('S') if self.can_save_live_events() => {
                    self.confirm_quit_action = ConfirmQuitAction::SaveAndQuit;
                    self.activate_confirm_quit_selection();
                }
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
            KeyCode::Char('q') | KeyCode::Esc if self.should_confirm_quit() => {
                self.open_quit_confirmation()
            }
            KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.should_quit = true
            }
            KeyCode::Char(' ') => self.toggle_playback(),
            KeyCode::Down | KeyCode::Char('j') => match self.focus_pane {
                FocusPane::Timeline => self.select_next(),
                FocusPane::Details => self.scroll_details_down(),
            },
            KeyCode::Up | KeyCode::Char('k') => match self.focus_pane {
                FocusPane::Timeline => self.select_previous(),
                FocusPane::Details => self.scroll_details_up(),
            },
            KeyCode::Tab => self.next_tab(),
            KeyCode::BackTab => self.previous_tab(),
            KeyCode::Char('2') => self.focus_pane = FocusPane::Details,
            KeyCode::Char('1') => self.focus_pane = FocusPane::Timeline,
            KeyCode::Char('p') => {
                if self.raw_details {
                    self.raw_details = false;
                    self.reset_details_scroll();
                }
            }
            KeyCode::Char('r') => {
                if !self.raw_details {
                    self.raw_details = true;
                    self.reset_details_scroll();
                }
            }
            KeyCode::Char('t') => {
                self.time_display = match self.time_display {
                    TimeDisplayMode::Elapsed => TimeDisplayMode::LocalTime,
                    TimeDisplayMode::LocalTime => TimeDisplayMode::Elapsed,
                }
            }
            KeyCode::Char('m') => self.metadata_open = !self.metadata_open,
            KeyCode::Char('y') => {
                if !self.last_details_visible_text.trim().is_empty() {
                    match copy_to_clipboard(&self.last_details_visible_text) {
                        Ok(()) => self.toast = Some(ToastMessage::new("content copied")),
                        Err(error) => {
                            self.toast = Some(ToastMessage::new(format!("copy failed: {error}")))
                        }
                    }
                }
            }
            KeyCode::Char('s') if self.can_save_live_events() => match self.save_live_events() {
                Ok(path) => {
                    self.toast = Some(ToastMessage::new(format!("saved {}", path.display())))
                }
                Err(error) => self.toast = Some(ToastMessage::new(format!("save failed: {error}"))),
            },
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

fn compact_hidden_agent_event_data(event: &mut WorkflowEvent) {
    if event.event_type != WorkflowEventType::AgentEvent {
        return;
    }
    let provider = event
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.provider.as_deref());
    if provider != Some("pi") {
        return;
    }
    let provider_event_type = event
        .data
        .get("providerEvent")
        .or(Some(&event.data))
        .and_then(|value| value.get("type"))
        .and_then(Value::as_str);
    if provider_event_type != Some("message_update") {
        return;
    }

    let mut compact = serde_json::Map::new();
    if let Some(provider) = provider {
        compact.insert("provider".to_string(), Value::String(provider.to_string()));
    }
    if let Some(metadata) = event.metadata.as_ref() {
        if let Some(session_id) = metadata.session_id.as_ref() {
            compact.insert("sessionId".to_string(), Value::String(session_id.clone()));
        }
        if let Some(step_id) = metadata.step_id.as_ref() {
            compact.insert("stepId".to_string(), Value::String(step_id.clone()));
        }
    }
    compact.insert(
        "providerEvent".to_string(),
        serde_json::json!({ "type": "message_update", "compacted": true }),
    );
    event.data = Value::Object(compact);
}

fn copy_to_clipboard(text: &str) -> anyhow::Result<()> {
    let commands: &[(&str, &[&str])] = if cfg!(target_os = "macos") {
        &[("pbcopy", &[])]
    } else {
        &[
            ("wl-copy", &[]),
            ("xclip", &["-selection", "clipboard"]),
            ("xsel", &["--clipboard", "--input"]),
        ]
    };

    for (command, args) in commands {
        let mut child = match StdCommand::new(command)
            .args(*args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => child,
            Err(_) => continue,
        };
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(text.as_bytes())?;
        }
        if child.wait()?.success() {
            return Ok(());
        }
    }
    anyhow::bail!("no supported clipboard command found")
}

fn read_event_records(path: &Path) -> anyhow::Result<Vec<EventRecord>> {
    let file = fs::File::open(path)
        .with_context(|| format!("failed to read event stream {}", path.display()))?;
    let reader = io::BufReader::new(file);
    let mut events = Vec::new();
    for (line_index, line) in reader.lines().enumerate() {
        let line = line.with_context(|| {
            format!(
                "failed to read line {} from {}",
                line_index + 1,
                path.display()
            )
        })?;
        if line.trim().is_empty() {
            continue;
        }
        let mut event: WorkflowEvent = serde_json::from_str(&line)
            .with_context(|| format!("invalid workflow event on line {}", line_index + 1))?;
        compact_hidden_agent_event_data(&mut event);
        events.push(EventRecord {
            event: Arc::new(event),
        });
    }
    Ok(events)
}

fn replay_delay(
    previous: Option<&EventRecord>,
    next: Option<&EventRecord>,
    max_delay: Option<StdDuration>,
) -> StdDuration {
    let previous_elapsed = previous
        .and_then(|record| record.event.elapsed_nanos)
        .unwrap_or(0);
    let next_elapsed = next
        .and_then(|record| record.event.elapsed_nanos)
        .unwrap_or(previous_elapsed);
    let nanos = next_elapsed.saturating_sub(previous_elapsed);
    let seconds = nanos as f64 / 1_000_000_000.0;
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
            label: format!("c: {}", short_id4(&parent_step_id)),
            workflow_depth: depth,
            parent_step_id: Some(parent_step_id),
        });
    }

    tabs
}

fn build_scope_tabs_for_view(events: &[TimelineViewEvent]) -> Vec<WorkflowScopeTab> {
    let mut tabs = vec![WorkflowScopeTab {
        label: "root".to_string(),
        workflow_depth: 0,
        parent_step_id: None,
    }];

    for record in events {
        if record.event_type != WorkflowEventType::Started {
            continue;
        }
        let Some(metadata) = record.metadata.as_ref() else {
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
            label: format!("c: {}", short_id4(&parent_step_id)),
            workflow_depth: depth,
            parent_step_id: Some(parent_step_id),
        });
    }

    tabs
}

fn build_timeline_rows_for_view(
    visible_indices: &[usize],
    events: &[TimelineViewEvent],
) -> Vec<TimelineRow> {
    let mut grouped_rows = Vec::<TimelineRow>::new();
    let mut group_row_by_key = std::collections::HashMap::<String, usize>::new();

    for event_index in visible_indices.iter().copied() {
        let record = &events[event_index];
        let Some(group_key) = agent_group_key_for_view(record) else {
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

    expand_grouped_rows(grouped_rows, visible_indices.len())
}

#[cfg(test)]
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

    expand_grouped_rows(grouped_rows, visible_indices.len())
}

fn expand_grouped_rows(grouped_rows: Vec<TimelineRow>, capacity: usize) -> Vec<TimelineRow> {
    let mut rows = Vec::with_capacity(capacity);
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

fn visible_indices_for_view(
    events: &[TimelineViewEvent],
    tab: &WorkflowScopeTab,
    search_query: &str,
) -> Vec<usize> {
    let query = search_query.to_ascii_lowercase();
    events
        .iter()
        .enumerate()
        .filter(|(_, record)| event_in_scope_for_view(record, tab))
        .filter(|(_, record)| record.timeline_visible)
        .filter(|(_, record)| {
            query.is_empty()
                || record
                    .search_text
                    .as_deref()
                    .is_some_and(|text| text.to_ascii_lowercase().contains(&query))
        })
        .map(|(index, _)| index)
        .collect()
}

fn event_in_scope_for_view(record: &TimelineViewEvent, tab: &WorkflowScopeTab) -> bool {
    let metadata = record.metadata.as_ref();
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
    if let Some(path) = app.saved_events_file.as_ref() {
        eprintln!("Events log saved to {}", path.display());
    }
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
        app.tick_toast();
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
        app.tick_toast();
        app.push_live_events(drain_live_events(&event_rx, Some(LIVE_EVENTS_PER_TICK)));
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

        if app.save_and_quit_requested {
            app.push_live_events(drain_live_events(&event_rx, None));
            match app.save_live_events() {
                Ok(_) => app.should_quit = true,
                Err(error) => {
                    app.toast = Some(ToastMessage::new(format!("save failed: {error}")));
                    app.save_and_quit_requested = false;
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

fn drain_live_events(
    event_rx: &mpsc::Receiver<WorkflowEvent>,
    limit: Option<usize>,
) -> Vec<WorkflowEvent> {
    let mut event_batch = Vec::new();
    let limit = limit.unwrap_or(usize::MAX);
    for _ in 0..limit {
        match event_rx.try_recv() {
            Ok(event) => event_batch.push(event),
            Err(mpsc::TryRecvError::Empty) => break,
            Err(mpsc::TryRecvError::Disconnected) => break,
        }
    }
    event_batch
}

fn render(frame: &mut Frame<'_>, app: &mut TuiReplayApp) {
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(6), Constraint::Min(5)])
        .split(frame.area());

    render_header_pane(frame, app, root[0]);

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(root[1]);

    render_timeline(frame, app, body[0]);
    render_details(frame, app, body[1]);

    if app.search_open {
        render_search_overlay(frame, app);
    }
    if app.confirm_quit {
        render_quit_confirmation(frame, app);
    }
    if let Some(toast) = app.toast.as_ref() {
        render_toast(frame, toast);
    }
}

fn render_toast(frame: &mut Frame<'_>, toast: &ToastMessage) {
    let width = u16::try_from(toast.message.chars().count().saturating_add(4))
        .unwrap_or(u16::MAX)
        .min(frame.area().width)
        .max(12);
    let area = ratatui::layout::Rect {
        x: frame
            .area()
            .x
            .saturating_add(frame.area().width.saturating_sub(width)),
        y: frame.area().y,
        width,
        height: 3.min(frame.area().height),
    };
    let paragraph = Paragraph::new(Line::from(vec![
        Span::raw(" "),
        Span::styled(
            toast.message.clone(),
            Style::default()
                .fg(Color::LightGreen)
                .add_modifier(Modifier::BOLD),
        ),
    ]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::Green)),
    );
    frame.render_widget(Clear, area);
    frame.render_widget(paragraph, area);
}

fn render_header_pane(frame: &mut Frame<'_>, app: &TuiReplayApp, area: ratatui::layout::Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::DarkGray));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let sections = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(34), Constraint::Min(0)])
        .split(inner);
    let workflow_tab_rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Min(0),
        ])
        .split(sections[0]);
    render_workflow_status_indicator(frame, app, workflow_tab_rows[0]);
    render_workflow_tabs_section(frame, app, workflow_tab_rows[1]);
    render_workflow_details_section(frame, app, sections[1]);
}

fn render_workflow_status_indicator(
    frame: &mut Frame<'_>,
    app: &TuiReplayApp,
    area: ratatui::layout::Rect,
) {
    let (status, status_style) = header_status(app);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw(" "),
            Span::styled(status, status_style),
        ])),
        area,
    );
}

fn render_workflow_tabs_section(
    frame: &mut Frame<'_>,
    app: &TuiReplayApp,
    area: ratatui::layout::Rect,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(" workflows - tab/shift-tab ")
        .border_style(Style::default().fg(Color::DarkGray));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let visible_range = visible_tab_range(app.active_tab, app.tabs.len(), 3);
    let hidden = app.tabs.len().saturating_sub(visible_range.len());
    let mut spans = vec![Span::raw(" ")];
    for index in visible_range {
        let selected = index == app.active_tab;
        let style = if selected {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White).bg(Color::DarkGray)
        };
        spans.push(Span::styled(format!(" {} ", app.tabs[index].label), style));
        spans.push(Span::raw(" "));
    }
    if hidden > 0 {
        spans.push(Span::styled(
            format!("+{hidden}"),
            Style::default()
                .fg(Color::Gray)
                .add_modifier(Modifier::BOLD),
        ));
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), inner);
}

fn visible_tab_range(active: usize, total: usize, limit: usize) -> std::ops::Range<usize> {
    if total <= limit {
        return 0..total;
    }
    let half = limit / 2;
    let mut start = active.saturating_sub(half);
    start = start.min(total.saturating_sub(limit));
    start..start.saturating_add(limit)
}

fn render_workflow_details_section(
    frame: &mut Frame<'_>,
    app: &TuiReplayApp,
    area: ratatui::layout::Rect,
) {
    let area = pad_content_area(area, 2, 0);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(area);
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(24),
            Constraint::Length(3),
            Constraint::Length(18),
            Constraint::Length(3),
            Constraint::Length(24),
            Constraint::Min(0),
        ])
        .split(rows[0]);
    render_workflow_run_details(frame, app, columns[0]);
    render_event_count_details(frame, app, columns[2]);
    render_token_usage_details(frame, app, columns[4]);
    render_events_file_details(frame, app, rows[1]);
}

fn render_workflow_run_details(
    frame: &mut Frame<'_>,
    app: &TuiReplayApp,
    area: ratatui::layout::Rect,
) {
    let run_id = current_run_id(app);
    let error = app.live_error.as_deref().map(|error| truncate(error, 48));
    let mut lines = vec![
        Line::from(Span::styled(
            "workflow run",
            Style::default()
                .fg(Color::Gray)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(run_id, Style::default().fg(Color::White))),
    ];
    if !app.warnings.is_empty() {
        lines.push(Line::from(Span::styled(
            format!("warnings {}", app.warnings.len()),
            Style::default().fg(Color::Yellow),
        )));
    }
    if let Some(error) = error {
        lines.push(Line::from(Span::styled(
            error,
            Style::default().fg(Color::Red),
        )));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

fn render_event_count_details(
    frame: &mut Frame<'_>,
    app: &TuiReplayApp,
    area: ratatui::layout::Rect,
) {
    let observed = app.events.len();
    let graph_height = 1usize;
    let graph_width = usize::from(area.width)
        .saturating_sub(observed.to_string().len() + 1)
        .min(12)
        .max(1);
    let mut lines = vec![Line::from(Span::styled(
        "events",
        Style::default()
            .fg(Color::Gray)
            .add_modifier(Modifier::BOLD),
    ))];
    lines.extend(event_rate_framegraph_lines(
        app,
        graph_width,
        graph_height,
        observed,
    ));
    frame.render_widget(Paragraph::new(lines), area);
}

fn render_token_usage_details(
    frame: &mut Frame<'_>,
    app: &TuiReplayApp,
    area: ratatui::layout::Rect,
) {
    let (input, output) = app.observed_token_usage.unwrap_or((0, 0));
    let lines = vec![
        Line::from(Span::styled(
            "token usages",
            Style::default()
                .fg(Color::Gray)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(vec![
            Span::styled("+in ", Style::default().fg(Color::Green)),
            Span::styled(format_count(input), Style::default().fg(Color::LightGreen)),
            Span::raw(" "),
            Span::styled("+out ", Style::default().fg(Color::Blue)),
            Span::styled(format_count(output), Style::default().fg(Color::LightBlue)),
        ]),
    ];
    frame.render_widget(Paragraph::new(lines), area);
}

fn render_events_file_details(
    frame: &mut Frame<'_>,
    app: &TuiReplayApp,
    area: ratatui::layout::Rect,
) {
    let mut spans = vec![Span::styled(
        "file: ",
        Style::default()
            .fg(Color::Gray)
            .add_modifier(Modifier::BOLD),
    )];
    if let Some(path) = app.events_file.as_ref() {
        spans.push(Span::styled(
            path.display().to_string(),
            Style::default().fg(Color::DarkGray),
        ));
    } else {
        spans.push(Span::styled(
            "<unsaved>",
            Style::default().fg(Color::DarkGray),
        ));
        if app.can_save_live_events() {
            spans.push(Span::raw(" "));
            spans.push(Span::styled(
                "s",
                Style::default()
                    .fg(Color::LightGreen)
                    .add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled("ave?", Style::default().fg(Color::Gray)));
        }
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn event_rate_framegraph_lines(
    app: &TuiReplayApp,
    width: usize,
    height: usize,
    observed: usize,
) -> Vec<Line<'static>> {
    let width = width.max(1);
    let height = height.max(1);
    let bins = event_rate_bins(app, width.saturating_mul(2));
    let max_count = bins.iter().copied().max().unwrap_or(0).max(1);
    let subcell_height = height.saturating_mul(4).max(1);
    let column_heights = bins
        .iter()
        .map(|count| {
            if *count == 0 {
                1
            } else {
                count
                    .saturating_mul(subcell_height)
                    .saturating_add(max_count - 1)
                    / max_count
            }
        })
        .collect::<Vec<_>>();

    (0..height)
        .map(|row_index| {
            let row_top = height.saturating_sub(row_index).saturating_mul(4);
            let row_bottom = row_top.saturating_sub(4);
            let row = column_heights
                .chunks(2)
                .map(|chunk| {
                    let left = chunk
                        .first()
                        .copied()
                        .unwrap_or(0)
                        .saturating_sub(row_bottom)
                        .min(4);
                    let right = chunk
                        .get(1)
                        .copied()
                        .unwrap_or(0)
                        .saturating_sub(row_bottom)
                        .min(4);
                    event_rate_braille_cell(left, right)
                })
                .collect::<String>();
            let mut spans = vec![Span::styled(row, event_rate_row_style(row_index, height))];
            if row_index == height.saturating_sub(1) {
                spans.push(Span::raw(" "));
                spans.push(Span::styled(
                    observed.to_string(),
                    Style::default().fg(Color::White),
                ));
            }
            Line::from(spans)
        })
        .collect()
}

fn event_rate_braille_cell(left: usize, right: usize) -> char {
    let left_mask = match left {
        0 => 0x00,
        1 => 0x40,
        2 => 0x44,
        3 => 0x46,
        _ => 0x47,
    };
    let right_mask = match right {
        0 => 0x00,
        1 => 0x80,
        2 => 0xa0,
        3 => 0xb0,
        _ => 0xb8,
    };
    char::from_u32(0x2800 + left_mask + right_mask).unwrap_or(' ')
}

fn event_rate_row_style(row_index: usize, height: usize) -> Style {
    let level_from_bottom = height.saturating_sub(row_index);
    let color = match level_from_bottom {
        0 | 1 => Color::Rgb(170, 210, 130),
        2 => Color::Rgb(224, 205, 130),
        3 => Color::Rgb(235, 165, 120),
        _ => Color::Rgb(238, 120, 120),
    };
    Style::default().fg(color)
}

fn event_rate_bins(app: &TuiReplayApp, width: usize) -> Vec<usize> {
    let width = width.max(1);
    let max_bin = app.latest_event_rate_bin;
    let mut bins = vec![0usize; width];
    for (bin, count) in &app.event_rate_bins {
        let distance_from_latest = max_bin.saturating_sub(*bin);
        let Ok(distance_from_latest) = usize::try_from(distance_from_latest) else {
            continue;
        };
        if distance_from_latest >= width {
            continue;
        }
        let index = width.saturating_sub(1).saturating_sub(distance_from_latest);
        if let Some(value) = bins.get_mut(index) {
            *value = *count;
        }
    }
    bins
}

fn render_timeline(frame: &mut Frame<'_>, app: &TuiReplayApp, area: ratatui::layout::Rect) {
    let rows = app.visible_rows();
    let title_suffix = if app.search_query.is_empty() {
        format!(
            "timeline ({}/{}) ",
            app.selected.saturating_add(1),
            rows.len()
        )
    } else {
        format!(
            "timeline ({}/{}) search: {} ",
            app.selected.saturating_add(1),
            rows.len(),
            app.search_query
        )
    };
    let focused = app.focus_pane == FocusPane::Timeline;
    let title_color = if focused { Color::Cyan } else { Color::Blue };
    let title = pane_title("¹", title_suffix, title_color, focused);
    let (_title_area, content_area) =
        render_pane_shell(frame, area, title, title_color, focused, None);
    let content_area = pad_content_area(content_area, 1, 0);
    let (list_area, scroll_indicator_area) = timeline_list_areas(content_area);

    let query = app.search_query.to_ascii_lowercase();
    let height = usize::from(list_area.height).max(1);
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
            if !timeline_row_valid(row, app.events.len()) {
                return ListItem::new(Line::from(""));
            }
            let event_index = row.event_index();
            let summary = timeline_row_summary(app, row);
            let selected = row_index == app.selected;
            let distance = row_index.abs_diff(app.selected);
            let dim_enabled = true;
            let search_match = !query.is_empty() && summary.to_ascii_lowercase().contains(&query);
            let line = Line::from(vec![Span::styled(
                summary,
                timeline_event_style(
                    &app.events[event_index],
                    selected,
                    search_match,
                    distance,
                    dim_enabled,
                ),
            )]);
            ListItem::new(line)
        })
        .collect::<Vec<_>>();
    frame.render_widget(List::new(items), list_area);
    render_timeline_scroll_indicator(
        frame,
        scroll_indicator_area,
        start > 0,
        end < virtual_len,
        focused,
    );
}

fn timeline_list_areas(
    area: ratatui::layout::Rect,
) -> (ratatui::layout::Rect, ratatui::layout::Rect) {
    if area.width == 0 {
        return (area, area);
    }
    let indicator_width = 1;
    let list_area = ratatui::layout::Rect {
        width: area.width.saturating_sub(indicator_width),
        ..area
    };
    let indicator_area = ratatui::layout::Rect {
        x: area.x.saturating_add(list_area.width),
        width: indicator_width,
        ..area
    };
    (list_area, indicator_area)
}

fn render_timeline_scroll_indicator(
    frame: &mut Frame<'_>,
    area: ratatui::layout::Rect,
    can_scroll_up: bool,
    can_scroll_down: bool,
    focused: bool,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let active_style = if focused {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Gray)
    };
    let inactive_style = Style::default().fg(Color::DarkGray);
    frame.render_widget(
        Paragraph::new("↑").style(if can_scroll_up {
            active_style
        } else {
            inactive_style
        }),
        area,
    );
    let bottom = ratatui::layout::Rect {
        y: area.y.saturating_add(area.height.saturating_sub(1)),
        height: 1,
        ..area
    };
    frame.render_widget(
        Paragraph::new("↓").style(if can_scroll_down {
            active_style
        } else {
            inactive_style
        }),
        bottom,
    );
}

fn timeline_row_valid(row: &TimelineRow, events_len: usize) -> bool {
    match row {
        TimelineRow::Event { event_index } | TimelineRow::AgentChild { event_index, .. } => {
            *event_index < events_len
        }
        TimelineRow::AgentGroup { event_indices } => {
            !event_indices.is_empty() && event_indices.iter().all(|index| *index < events_len)
        }
    }
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

fn timeline_event_style(
    record: &EventRecord,
    selected: bool,
    search_match: bool,
    distance_from_selected: usize,
    dim_enabled: bool,
) -> Style {
    if selected {
        return Style::default().fg(Color::Black).bg(Color::Cyan);
    }
    if search_match {
        return Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD);
    }

    let style = event_type_style(&record.event.event_type);
    if !dim_enabled {
        return style;
    }
    match distance_from_selected {
        0..=8 => style,
        _ => style.add_modifier(Modifier::DIM),
    }
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

fn render_details(frame: &mut Frame<'_>, app: &mut TuiReplayApp, area: ratatui::layout::Rect) {
    let focused = app.focus_pane == FocusPane::Details;
    let title_color = if focused { Color::Cyan } else { Color::Blue };
    let title = pane_title("²", "details ".to_string(), title_color, focused);
    let mode_label = details_mode_label(app.raw_details, focused);
    let selected = app.selected_event();
    let lines = selected
        .map(|record| {
            let style = event_type_style(&record.event.event_type);
            if app.raw_details {
                raw_details_lines(record, style)
            } else {
                pretty_details_lines(app, record)
            }
        })
        .unwrap_or_else(|| vec![Line::raw("No event selected")]);
    let metadata_lines = selected
        .map(|record| {
            metadata_details_lines(app, record, event_type_style(&record.event.event_type))
        })
        .unwrap_or_default();
    let (_title_area, content_area) =
        render_pane_shell(frame, area, title, title_color, focused, Some(mode_label));
    let content_area = pad_content_area(content_area, 1, 0);
    let (body_area, scroll_indicator_area) = timeline_list_areas(content_area);
    app.last_details_visible_text = lines
        .iter()
        .map(line_plain_text)
        .collect::<Vec<_>>()
        .join("\n");
    let padded_lines = pad_details_lines(lines);
    let max_scroll = estimated_scroll_overflow(&padded_lines, body_area);
    let effective_scroll = app.details_scroll.min(max_scroll);
    render_details_body_around_metadata(
        frame,
        body_area,
        &padded_lines,
        metadata_lines.len(),
        app.metadata_open,
        effective_scroll,
    );
    render_metadata_overlay(frame, body_area, metadata_lines, focused, app.metadata_open);
    render_timeline_scroll_indicator(
        frame,
        scroll_indicator_area,
        effective_scroll > 0,
        effective_scroll < max_scroll,
        focused,
    );
}

fn render_details_body_around_metadata(
    frame: &mut Frame<'_>,
    body_area: ratatui::layout::Rect,
    lines: &[Line<'static>],
    metadata_line_count: usize,
    metadata_open: bool,
    scroll: usize,
) -> Vec<Line<'static>> {
    if body_area.width == 0 || body_area.height == 0 {
        return Vec::new();
    }
    let metadata_area = metadata_overlay_area(body_area, metadata_line_count, metadata_open);
    let top_height = metadata_area.height.min(body_area.height);
    let top_width = metadata_area
        .x
        .saturating_sub(body_area.x)
        .saturating_sub(1)
        .max(1);
    let full_width = usize::from(body_area.width).max(1);
    let top_width_usize = usize::from(top_width).max(1);
    let viewport_height = usize::from(body_area.height);
    let shaped = shaped_visible_detail_lines(
        lines,
        scroll,
        top_width_usize,
        usize::from(top_height),
        full_width,
        viewport_height,
    );
    let split_at = shaped.len().min(usize::from(top_height));
    let top_lines = shaped[..split_at].to_vec();
    let bottom_lines = shaped[split_at..].to_vec();

    if top_height > 0 {
        let top_area = ratatui::layout::Rect {
            width: top_width,
            height: top_height,
            ..body_area
        };
        frame.render_widget(Paragraph::new(top_lines), top_area);
    }
    if top_height < body_area.height {
        let bottom_area = ratatui::layout::Rect {
            y: body_area.y.saturating_add(top_height),
            height: body_area.height.saturating_sub(top_height),
            ..body_area
        };
        frame.render_widget(Paragraph::new(bottom_lines), bottom_area);
    }
    shaped
}

fn shaped_visible_detail_lines(
    lines: &[Line<'static>],
    scroll: usize,
    top_width: usize,
    top_height: usize,
    full_width: usize,
    viewport_height: usize,
) -> Vec<Line<'static>> {
    let mut full_width_lines = Vec::new();
    for line in lines {
        full_width_lines.extend(wrap_plain_text(&line_plain_text(line), full_width));
    }

    let mut visible = Vec::with_capacity(viewport_height);
    for text in full_width_lines.into_iter().skip(scroll) {
        let width = if visible.len() < top_height {
            top_width
        } else {
            full_width
        };
        for segment in wrap_plain_text(&text, width) {
            if visible.len() >= viewport_height {
                return visible;
            }
            visible.push(Line::raw(segment));
        }
    }
    visible
}

fn line_plain_text(line: &Line<'static>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>()
}

fn wrap_plain_text(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return Vec::new();
    }
    if text.is_empty() {
        return vec![String::new()];
    }

    let leading = text
        .chars()
        .take_while(|ch| ch.is_whitespace() && *ch != '\n')
        .collect::<String>();
    let indent = leading.chars().count().min(width.saturating_sub(1));
    let indent_text = " ".repeat(indent);
    let mut output = Vec::new();
    let mut current = String::new();

    for word in text.split_whitespace() {
        let word_len = word.chars().count();
        if word_len > width {
            if !current.trim().is_empty() {
                output.push(std::mem::take(&mut current));
                current.push_str(&indent_text);
            }
            for chunk in chunk_word(word, width) {
                output.push(chunk);
            }
            continue;
        }

        let separator = if current.trim().is_empty() { 0 } else { 1 };
        let next_len = current.chars().count() + separator + word_len;
        if next_len > width && !current.trim().is_empty() {
            output.push(std::mem::take(&mut current));
            current.push_str(&indent_text);
        }
        if !current.trim().is_empty() {
            current.push(' ');
        } else if current.is_empty() && !output.is_empty() {
            current.push_str(&indent_text);
        } else if current.is_empty() {
            current.push_str(&leading);
        }
        current.push_str(word);
    }

    if !current.is_empty() {
        output.push(current);
    }
    if output.is_empty() {
        output.push(String::new());
    }
    output
}

fn chunk_word(word: &str, width: usize) -> Vec<String> {
    let mut output = Vec::new();
    let mut current = String::new();
    for ch in word.chars() {
        current.push(ch);
        if current.chars().count() >= width {
            output.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        output.push(current);
    }
    output
}

fn estimated_scroll_overflow(lines: &[Line<'static>], area: ratatui::layout::Rect) -> usize {
    let width = usize::from(area.width).max(1);
    let visual_lines = lines
        .iter()
        .map(|line| line.width().saturating_add(width - 1) / width)
        .map(|height| height.max(1))
        .sum::<usize>();
    visual_lines.saturating_sub(usize::from(area.height))
}

fn render_metadata_overlay(
    frame: &mut Frame<'_>,
    content_area: ratatui::layout::Rect,
    lines: Vec<Line<'static>>,
    focused: bool,
    open: bool,
) {
    if lines.is_empty() || content_area.width == 0 || content_area.height == 0 {
        return;
    }
    let area = metadata_overlay_area(content_area, lines.len(), open);
    if area.width == 0 || area.height == 0 {
        return;
    }
    let border_style = if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style)
        .title(Line::from(vec![
            Span::raw(" "),
            Span::styled(
                "m",
                Style::default()
                    .fg(Color::LightBlue)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "etadata ",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
    let inner = block.inner(area);
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    if open {
        frame.render_widget(Paragraph::new(pad_lines(lines, " ")), inner);
    }
}

fn metadata_overlay_area(
    content_area: ratatui::layout::Rect,
    line_count: usize,
    open: bool,
) -> ratatui::layout::Rect {
    let width = content_area.width.min(44).max(content_area.width.min(18));
    let desired_height = if open {
        u16::try_from(line_count.saturating_add(2)).unwrap_or(u16::MAX)
    } else {
        2
    };
    let height = desired_height.min(content_area.height);
    ratatui::layout::Rect {
        x: content_area
            .x
            .saturating_add(content_area.width.saturating_sub(width)),
        y: content_area.y,
        width,
        height,
    }
}

fn details_mode_label(raw_details: bool, focused: bool) -> Line<'static> {
    let active_style = Style::default()
        .fg(if focused {
            Color::LightCyan
        } else {
            Color::Gray
        })
        .add_modifier(Modifier::BOLD);
    let inactive_style = Style::default().fg(Color::DarkGray);
    let shortcut_style = Style::default()
        .fg(Color::LightBlue)
        .add_modifier(Modifier::BOLD);
    let pretty_style = if raw_details {
        inactive_style
    } else {
        active_style
    };
    let raw_style = if raw_details {
        active_style
    } else {
        inactive_style
    };
    Line::from(vec![
        Span::raw(" "),
        Span::styled("p", shortcut_style),
        Span::styled("retty", pretty_style),
        Span::styled("/", Style::default().fg(Color::DarkGray)),
        Span::styled("r", shortcut_style),
        Span::styled("aw", raw_style),
        Span::raw("  cop"),
        Span::styled("y", shortcut_style),
        Span::raw(" content "),
    ])
}

fn pane_title(
    shortcut: &'static str,
    label: String,
    title_bg: Color,
    focused: bool,
) -> Line<'static> {
    let title_style = if focused {
        Style::default().fg(title_bg).add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD)
    };
    let shortcut_style = Style::default()
        .fg(Color::LightBlue)
        .add_modifier(Modifier::BOLD);
    Line::from(vec![
        Span::raw(" "),
        Span::styled(shortcut, shortcut_style),
        Span::styled(label, title_style),
    ])
}

fn render_pane_shell(
    frame: &mut Frame<'_>,
    area: ratatui::layout::Rect,
    title: Line<'static>,
    title_bg: Color,
    focused: bool,
    border_label: Option<Line<'static>>,
) -> (ratatui::layout::Rect, ratatui::layout::Rect) {
    let border_style = if focused {
        Style::default().fg(title_bg)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style)
        .title(title);
    if let Some(border_label) = border_label {
        block = block.title(border_label);
    }
    let inner = block.inner(area);
    frame.render_widget(block, area);
    (area, inner)
}

fn pad_details_lines(lines: Vec<Line<'static>>) -> Vec<Line<'static>> {
    pad_lines(lines, "  ")
}

fn pad_lines(lines: Vec<Line<'static>>, padding: &'static str) -> Vec<Line<'static>> {
    lines
        .into_iter()
        .map(|line| {
            let mut spans = Vec::with_capacity(line.spans.len() + 1);
            spans.push(Span::raw(padding));
            spans.extend(line.spans);
            Line { spans, ..line }
        })
        .collect()
}

fn render_quit_confirmation(frame: &mut Frame<'_>, app: &TuiReplayApp) {
    let area = centered_rect(64, 7, frame.area());
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(" confirm quit ");
    let inner = pad_content_area(block.inner(area), 2, 1);
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);

    let button_line = |actions: &[ConfirmQuitAction]| {
        let mut spans = Vec::new();
        for (index, action) in actions.iter().copied().enumerate() {
            if index > 0 {
                spans.push(Span::raw("  "));
            }
            spans.push(confirm_quit_button(
                action,
                app.confirm_quit_action == action,
            ));
        }
        Line::from(spans)
    };

    let lines = if app.live_is_active() {
        vec![
            Line::from(Span::styled(
                "Quit live workflow TUI?",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(
                "The workflow is still running.",
                Style::default().fg(Color::Gray),
            )),
            Line::from(""),
            button_line(&[ConfirmQuitAction::Quit, ConfirmQuitAction::Stay]),
        ]
    } else if app.live_events_unsaved() {
        vec![
            Line::from(Span::styled(
                "Quit without saving event log?",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(
                "Live events are currently unsaved.",
                Style::default().fg(Color::Gray),
            )),
            Line::from(""),
            button_line(&[
                ConfirmQuitAction::SaveAndQuit,
                ConfirmQuitAction::Quit,
                ConfirmQuitAction::Stay,
            ]),
        ]
    } else {
        vec![
            Line::from(Span::styled(
                "Quit TUI?",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            button_line(&[ConfirmQuitAction::Quit, ConfirmQuitAction::Stay]),
        ]
    };
    frame.render_widget(Paragraph::new(lines), inner);
}

fn confirm_quit_button(action: ConfirmQuitAction, selected: bool) -> Span<'static> {
    let (label, color) = match action {
        ConfirmQuitAction::Quit => ("quit", Color::Red),
        ConfirmQuitAction::SaveAndQuit => ("save & quit", Color::Green),
        ConfirmQuitAction::Stay => ("stay", Color::Blue),
    };
    let label = format!(" {label} ");
    let style = if selected {
        Style::default()
            .fg(Color::Black)
            .bg(color)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(color).add_modifier(Modifier::BOLD)
    };
    Span::styled(label, style)
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

fn current_run_id(app: &TuiReplayApp) -> String {
    app.events
        .first()
        .and_then(|record| record.event.metadata.as_ref())
        .and_then(|metadata| metadata.run_id.as_deref())
        .unwrap_or("<unknown-run>")
        .to_string()
}

fn live_delta_time(app: &TuiReplayApp) -> Option<String> {
    app.live_status?;
    let delta = app
        .live_finished_delta
        .or_else(|| app.live_started_at.map(|started_at| started_at.elapsed()))?;
    Some(format_elapsed_seconds(delta.as_secs()))
}

fn live_events_save_path(app: &TuiReplayApp) -> PathBuf {
    let run_id = sanitize_file_name(&current_run_id(app));
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(format!("{run_id}.jsonl"))
}

fn sanitize_file_name(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    if sanitized.is_empty() || sanitized == "_unknown-run_" {
        "workflow-events".to_string()
    } else {
        sanitized
    }
}

fn header_status(app: &TuiReplayApp) -> (String, Style) {
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
        return match live_status {
            LiveStatus::Running => (
                format!(
                    "{} LIVE RUNNING {}",
                    breathing_light(),
                    live_delta_time(app).unwrap_or_else(|| "+00:00:00".to_string())
                ),
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
            LiveStatus::Cancelling => (
                format!(
                    "{} LIVE CANCELLING {}",
                    breathing_light(),
                    live_delta_time(app).unwrap_or_else(|| "+00:00:00".to_string())
                ),
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
            LiveStatus::Done => (
                format!(
                    "{} LIVE DONE {}",
                    terminal_indicator("✓"),
                    live_delta_time(app).unwrap_or_else(|| "+00:00:00".to_string())
                ),
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            LiveStatus::Failed => (
                format!(
                    "{} LIVE FAILED {}",
                    terminal_indicator("✗"),
                    live_delta_time(app).unwrap_or_else(|| "+00:00:00".to_string())
                ),
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
        };
    }

    match app.playback {
        PlaybackState::Playing => (
            format!("{} REPLAY PLAYING", breathing_light()),
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
            "• REPLAY PAUSED".to_string(),
            Style::default()
                .fg(Color::Rgb(255, 165, 0))
                .add_modifier(Modifier::BOLD),
        ),
    }
}

fn workflow_token_usage(data: &Value) -> Option<(u64, u64)> {
    data.get("tokenUsage")
        .and_then(Value::as_object)
        .and_then(token_usage_from_object)
}

fn provider_token_usage(data: &Value) -> Option<(u64, u64)> {
    let provider_event = data.get("providerEvent").unwrap_or(data);
    find_token_usage_pair(provider_event)
}

fn find_token_usage_pair(value: &Value) -> Option<(u64, u64)> {
    match value {
        Value::Object(object) => {
            if let Some(usage) = object.get("usage").and_then(Value::as_object) {
                if let Some(pair) = token_usage_from_object(usage) {
                    return Some(pair);
                }
            }
            if let Some(tokens) = object.get("tokens").and_then(Value::as_object) {
                if let Some(pair) = token_usage_from_object(tokens) {
                    return Some(pair);
                }
            }
            if let Some(pair) = token_usage_from_object(object) {
                return Some(pair);
            }
            object.values().find_map(find_token_usage_pair)
        }
        Value::Array(items) => items.iter().find_map(find_token_usage_pair),
        _ => None,
    }
}

fn token_usage_from_object(object: &serde_json::Map<String, Value>) -> Option<(u64, u64)> {
    let input = first_u64(
        object,
        &[
            "inputTokens",
            "input_tokens",
            "prompt_tokens",
            "promptTokens",
            "input",
        ],
    )?;
    let output = first_u64(
        object,
        &[
            "outputTokens",
            "output_tokens",
            "completion_tokens",
            "completionTokens",
            "output",
        ],
    )
    .or_else(|| first_u64(object, &["reasoning_output_tokens", "reasoning"]));
    Some((input, output.unwrap_or(0)))
}

fn first_u64(object: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<u64> {
    keys.iter()
        .find_map(|key| object.get(*key).and_then(Value::as_u64))
}

fn workflow_usage_key(record: &EventRecord, index: usize) -> String {
    record
        .event
        .metadata
        .as_ref()
        .and_then(|metadata| {
            metadata
                .parent_step_id
                .as_ref()
                .or(metadata.step_id.as_ref())
        })
        .cloned()
        .unwrap_or_else(|| format!("workflow:{index}"))
}

fn agent_usage_key(record: &EventRecord, index: usize) -> String {
    record
        .event
        .metadata
        .as_ref()
        .and_then(|metadata| {
            metadata
                .step_id
                .as_ref()
                .or(metadata.session_id.as_ref())
                .or(metadata.provider.as_ref())
        })
        .cloned()
        .unwrap_or_else(|| format!("agent:{index}"))
}

fn sum_token_usage(usages: impl IntoIterator<Item = (u64, u64)>) -> (u64, u64) {
    usages
        .into_iter()
        .fold((0, 0), |(total_in, total_out), (input, output)| {
            (
                total_in.saturating_add(input),
                total_out.saturating_add(output),
            )
        })
}

fn format_count(value: u64) -> String {
    value.to_string()
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

fn agent_group_key_for_parts(
    event_type: &WorkflowEventType,
    metadata: Option<&WorkflowEventMetadata>,
) -> Option<String> {
    if !matches!(
        event_type,
        WorkflowEventType::AgentStarted
            | WorkflowEventType::AgentEvent
            | WorkflowEventType::AgentCompleted
            | WorkflowEventType::AgentFailed
    ) {
        return None;
    }
    let metadata = metadata?;
    metadata
        .step_id
        .as_ref()
        .or(metadata.session_id.as_ref())
        .cloned()
}

fn agent_group_key_for_view(record: &TimelineViewEvent) -> Option<String> {
    agent_group_key_for_parts(&record.event_type, record.metadata.as_ref())
}

#[cfg(test)]
fn agent_group_key(record: &EventRecord) -> Option<String> {
    agent_group_key_for_parts(&record.event.event_type, record.event.metadata.as_ref())
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

fn metadata_details_lines(
    app: &TuiReplayApp,
    record: &EventRecord,
    style: Style,
) -> Vec<Line<'static>> {
    let event = &record.event;
    let mut lines = vec![
        Line::from(vec![
            Span::raw("type: "),
            Span::styled(
                event.event_type.to_string(),
                style.add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::styled(
                "t",
                Style::default()
                    .fg(Color::LightBlue)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!("ime: {}", display_time(app, event))),
        ]),
    ];
    if let Some(metadata) = event.metadata.as_ref() {
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
    lines
}

fn raw_details_lines(record: &EventRecord, style: Style) -> Vec<Line<'static>> {
    serde_json::to_string_pretty(record.event.as_ref())
        .unwrap_or_else(|_| "<invalid>".into())
        .lines()
        .map(|line| Line::from(Span::styled(line.to_string(), style)))
        .collect()
}

fn pretty_details_lines(app: &TuiReplayApp, record: &EventRecord) -> Vec<Line<'static>> {
    let event = &record.event;
    let mut lines = Vec::new();
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
            "log {}\n\n{}",
            display_time(app, event),
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
        WorkflowEventType::AgentEvent => return provider::details_lines(record),
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

fn format_elapsed_seconds(seconds: u64) -> String {
    let minutes = seconds / 60;
    let hours = minutes / 60;
    format!("+{:02}:{:02}:{:02}", hours, minutes % 60, seconds % 60)
}

fn short_id(value: &str) -> String {
    let suffix = value.strip_prefix("step_").unwrap_or(value);
    suffix.chars().take(8).collect()
}

fn short_id4(value: &str) -> String {
    let suffix = value.strip_prefix("step_").unwrap_or(value);
    suffix.chars().take(4).collect()
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
            event: Arc::new(event),
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
            event: Arc::new(event),
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
