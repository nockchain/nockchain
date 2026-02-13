//! Bridge TUI - Terminal User Interface for bridge operators.
//!
//! This module provides a comprehensive dashboard for monitoring and
//! managing bridge operations, including:
//!
//! - Peer health monitoring
//! - Network state (chain heights, sync status)
//! - Proposal management (multi-sig coordination)
//! - Transaction activity (deposits/withdrawals)
//! - Alerts and notifications
//!
//! # Module Structure
//!
//! - `types`: Shared data types for all panels
//! - `state`: State management (TuiStatus, LocalUiState)
//! - `panels`: Individual panel components
//! - `widgets`: Reusable widget components
//! - `app`: Main application logic

mod clipboard;
pub mod panels;
pub mod state;
pub mod types;
pub mod widgets;

// Re-export commonly used items
use std::cmp::min;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use chrono::Local;
use clap::ColorChoice;
use clipboard::copy_to_clipboard;
use crossterm::cursor::Show;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use crossterm::terminal::{disable_raw_mode, LeaveAlternateScreen};
use crossterm::ExecutableCommand;
use nockapp::kernel::boot::Cli as BootCli;
use nu_ansi_term::Color;
use panels::{
    AlertPanel, DepositLogPanel, HealthPanel, NetworkStatePanel, ProposalPanel, TransactionPanel,
};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, ListState, Paragraph, TableState};
use ratatui::Frame;
pub use state::{new_log_buffer, BridgeStatus, LocalUiState, LogBuffer, TuiStatus, LOG_CAPACITY};
use tracing::Subscriber;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::writer::BoxMakeWriter;
use tracing_subscriber::fmt::{FmtContext, FormatEvent, FormatFields};
use tracing_subscriber::prelude::*;
use tracing_subscriber::{fmt, EnvFilter};
pub use types::{FocusedPanel, UiMode};
use widgets::{HelpOverlay, StatusBar};

use crate::errors::BridgeError;
use crate::stop::StopHandle;

const TICK_RATE: Duration = Duration::from_millis(500);

/// Clean up log files older than retention_days to prevent unbounded disk usage.
///
/// Only removes files matching the pattern `bridge.log*` to avoid accidentally
/// deleting unrelated files in the log directory.
pub fn cleanup_old_logs(log_dir: &Path, retention_days: u64) {
    let retention = Duration::from_secs(retention_days * 24 * 60 * 60);
    let now = match SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
        Ok(_) => SystemTime::now(),
        Err(_) => {
            tracing::warn!("System clock error, skipping log cleanup");
            return;
        }
    };

    let entries = match std::fs::read_dir(log_dir) {
        Ok(entries) => entries,
        Err(e) => {
            tracing::warn!("Failed to read log directory for cleanup: {}", e);
            return;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();

        // Only process bridge log files (bridge.log.YYYY-MM-DD pattern)
        let is_bridge_log = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.starts_with("bridge.log"))
            .unwrap_or(false);

        if !is_bridge_log {
            continue;
        }

        // Check file age
        if let Ok(metadata) = path.metadata() {
            if let Ok(modified) = metadata.modified() {
                if let Ok(age) = now.duration_since(modified) {
                    if age > retention {
                        match std::fs::remove_file(&path) {
                            Ok(()) => {
                                tracing::info!("Removed old log file: {}", path.display());
                            }
                            Err(e) => {
                                tracing::warn!(
                                    "Failed to remove old log file {}: {}",
                                    path.display(),
                                    e
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}

pub fn init_bridge_tracing(
    cli: &BootCli,
    logs: Option<LogBuffer>,
    log_dir: PathBuf,
    log_retention_days: usize,
) -> Result<WorkerGuard, BridgeError> {
    // Create log directory if it doesn't exist
    std::fs::create_dir_all(&log_dir).map_err(|e| {
        BridgeError::Config(format!(
            "Failed to create log directory {}: {}",
            log_dir.display(),
            e
        ))
    })?;

    // Use RUST_LOG env var, default to "info"
    let filter = EnvFilter::new(std::env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string()));

    // File appender with daily rotation using builder pattern (returns Result, doesn't panic)
    let file_appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix("bridge")
        .filename_suffix("log")
        .max_log_files(log_retention_days)
        .build(&log_dir)
        .map_err(|e| BridgeError::Config(format!("Failed to create log file appender: {}", e)))?;
    let (non_blocking_file, guard) = tracing_appender::non_blocking(file_appender);

    // File layer (no ANSI colors, plain format for log aggregation)
    // Use compact format with ISO timestamps for easy parsing
    let file_layer = fmt::layer()
        .with_ansi(false)
        .with_target(true)
        .with_level(true)
        .with_thread_ids(false)
        .with_thread_names(false)
        .with_file(false)
        .with_line_number(false)
        .with_writer(non_blocking_file);

    // Console/TUI layer (existing logic with colors)
    let ansi = matches!(cli.color, ColorChoice::Always | ColorChoice::Auto);
    let console_base_layer = fmt::layer().with_ansi(ansi).event_format(TuiFormatter);
    let console_layer = if let Some(buffer) = logs {
        let writer = TuiLogWriter::new(buffer);
        console_base_layer
            .with_writer(BoxMakeWriter::new(move || writer.clone()))
            .boxed()
    } else {
        console_base_layer
            .with_writer(BoxMakeWriter::new(std::io::stdout))
            .boxed()
    };

    // Combine both layers
    tracing_subscriber::registry()
        .with(filter)
        .with(file_layer)
        .with(console_layer)
        .try_init()
        .map_err(|e| BridgeError::Runtime(format!("failed to init tracing: {}", e)))?;

    Ok(guard)
}

#[derive(Clone)]
struct TuiLogWriter {
    sink: Arc<LogSink>,
}

struct LogSink {
    buffer: Mutex<Vec<u8>>,
    logs: LogBuffer,
}

impl TuiLogWriter {
    fn new(logs: LogBuffer) -> Self {
        Self {
            sink: Arc::new(LogSink {
                buffer: Mutex::new(Vec::new()),
                logs,
            }),
        }
    }

    fn push_line(&self, line: &str) {
        if line.is_empty() {
            return;
        }
        if let Ok(mut guard) = self.sink.logs.write() {
            if guard.len() >= LOG_CAPACITY {
                guard.pop_front();
            }
            guard.push_back(line.to_string());
        }
    }
}

impl Write for TuiLogWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut pending = self
            .sink
            .buffer
            .lock()
            .expect("Mutex poisoned - this should not happen");
        pending.extend_from_slice(buf);
        while let Some(pos) = pending.iter().position(|b| *b == b'\n') {
            let line_bytes: Vec<u8> = pending.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&line_bytes[..line_bytes.len().saturating_sub(1)]);
            self.push_line(line.trim_end());
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        let mut pending = self
            .sink
            .buffer
            .lock()
            .expect("Mutex poisoned - this should not happen");
        if !pending.is_empty() {
            let line = String::from_utf8_lossy(&pending);
            self.push_line(line.trim_end());
            pending.clear();
        }
        Ok(())
    }
}

struct TuiFormatter;

impl<S, N> FormatEvent<S, N> for TuiFormatter
where
    S: Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &tracing::Event<'_>,
    ) -> std::fmt::Result {
        let level = event.metadata().level();
        let use_ansi = writer.has_ansi_escapes();

        // Format level with colors using nu_ansi_term (same as tracing-subscriber internally)
        let level_str = match *level {
            tracing::Level::TRACE => "TRACE",
            tracing::Level::DEBUG => "DEBUG",
            tracing::Level::INFO => " INFO",
            tracing::Level::WARN => " WARN",
            tracing::Level::ERROR => "ERROR",
        };

        let timestamp = Local::now().format("%H:%M:%S").to_string();

        let target = event.metadata().target();
        let simplified = target
            .rsplit("::")
            .take(2)
            .collect::<Vec<&str>>()
            .into_iter()
            .rev()
            .collect::<Vec<&str>>()
            .join("::");

        if use_ansi {
            let colored_timestamp = Color::Fixed(8).paint(&timestamp); // dim gray
            let colored_level = match *level {
                tracing::Level::TRACE => Color::Purple.paint(level_str),
                tracing::Level::DEBUG => Color::Blue.paint(level_str),
                tracing::Level::INFO => Color::Green.paint(level_str),
                tracing::Level::WARN => Color::Yellow.paint(level_str),
                tracing::Level::ERROR => Color::Red.paint(level_str),
            };
            let colored_target = Color::Cyan.paint(&simplified);
            write!(
                writer,
                "{} {} {}: ",
                colored_timestamp, colored_level, colored_target
            )?;
        } else {
            write!(writer, "{} {} {}: ", timestamp, level_str, simplified)?;
        }

        ctx.field_format().format_fields(writer.by_ref(), event)?;
        writeln!(writer)
    }
}

/// Guard that restores terminal state on drop.
///
/// This ensures the terminal is properly restored even if:
/// - The TUI task is cancelled (another task in tokio::select! completes)
/// - A panic occurs in the render loop
/// - An error causes early return
struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = std::io::stdout().execute(LeaveAlternateScreen);
        let _ = std::io::stdout().execute(Show);
    }
}

/// Install panic hook that restores terminal before printing panic message.
///
/// This should be called BEFORE ratatui::init() to ensure panics in any
/// thread don't leave the terminal in a corrupted state.
fn install_panic_hook() {
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = disable_raw_mode();
        let _ = std::io::stdout().execute(LeaveAlternateScreen);
        let _ = std::io::stdout().execute(Show);
        original_hook(panic_info);
    }));
}

/// Run the TUI with the shared state.
///
/// The `bridge_status` must be the same instance that background tasks update,
/// otherwise the TUI won't see any data changes.
///
/// The `shutdown` flag allows graceful shutdown when other tasks complete.
/// Set it to `true` to signal the TUI to exit cleanly.
pub async fn run_tui(
    bridge_status: TuiStatus,
    shutdown: Arc<AtomicBool>,
    stop: StopHandle,
) -> Result<(), BridgeError> {
    tokio::task::spawn_blocking(move || render_loop(bridge_status, shutdown, stop))
        .await
        .map_err(|err| BridgeError::Runtime(format!("tui task join error: {}", err)))?
}

fn render_loop(
    state: TuiStatus,
    shutdown: Arc<AtomicBool>,
    stop: StopHandle,
) -> Result<(), BridgeError> {
    install_panic_hook();
    let mut terminal = ratatui::init();
    let _guard = TerminalGuard;

    let mut app = BridgeTuiApp::new(state, stop);
    let mut last_tick = Instant::now();

    loop {
        if shutdown.load(Ordering::Relaxed) {
            break;
        }

        terminal
            .draw(|frame| app.draw(frame))
            .map_err(|err| BridgeError::Runtime(format!("tui draw error: {}", err)))?;

        let timeout = TICK_RATE
            .checked_sub(last_tick.elapsed())
            .unwrap_or_default();
        if event::poll(timeout).map_err(|err| BridgeError::Runtime(err.to_string()))? {
            if let Event::Key(key) = event::read()
                .map_err(|err| BridgeError::Runtime(format!("tui key error: {}", err)))?
            {
                if key.kind == KeyEventKind::Press && app.handle_key(key) {
                    break;
                }
            }
        }
        if last_tick.elapsed() >= TICK_RATE {
            app.tick();
            last_tick = Instant::now();
        }
    }

    Ok(())
}

/// Main TUI application.
struct BridgeTuiApp {
    /// Shared state from background tasks.
    shared: TuiStatus,
    /// Local UI state.
    ui: LocalUiState,
    /// Stop state handle.
    stop: StopHandle,
    /// Health table state.
    health_table: TableState,
    /// Transaction table state.
    tx_table: TableState,
    /// Proposal list state.
    proposal_list: ListState,
    /// Alert table state.
    alert_table: TableState,
    /// Deposit log table state.
    deposit_log_table: TableState,
    /// Transaction detail view (index of expanded transaction).
    tx_detail_view: Option<usize>,
    /// Proposal detail view (index of expanded proposal).
    proposal_detail_view: Option<usize>,
}

impl BridgeTuiApp {
    fn new(shared: TuiStatus, stop: StopHandle) -> Self {
        Self {
            shared,
            ui: LocalUiState::new(),
            stop,
            health_table: TableState::default(),
            tx_table: TableState::default(),
            proposal_list: ListState::default(),
            alert_table: TableState::default(),
            deposit_log_table: TableState::default(),
            tx_detail_view: None,
            proposal_detail_view: None,
        }
    }

    fn tick(&mut self) {
        // Check and clear expired status messages
        self.shared.check_and_clear_expired_status();

        if self.ui.focused_panel != self.ui.last_focused_panel {
            if matches!(self.ui.focused_panel, FocusedPanel::DepositLog) {
                self.ui.deposit_log_jump_pending = true;
            }
            self.shared
                .set_deposit_log_active(matches!(self.ui.focused_panel, FocusedPanel::DepositLog));
            self.ui.last_focused_panel = self.ui.focused_panel;
        }

        // Update selection bounds for all panels

        // Health table
        let health_len = self.shared.health_snapshots().len();
        if health_len == 0 {
            self.health_table.select(None);
        } else if self.health_table.selected().is_none() {
            self.health_table.select(Some(0));
        } else if let Some(selected) = self.health_table.selected() {
            self.health_table
                .select(Some(min(selected, health_len.saturating_sub(1))));
        }

        // Transaction table
        let tx_len = self.shared.transactions().transactions.len();
        if tx_len == 0 {
            self.tx_table.select(None);
        } else if self.tx_table.selected().is_none() {
            self.tx_table.select(Some(0));
        } else if let Some(selected) = self.tx_table.selected() {
            self.tx_table
                .select(Some(min(selected, tx_len.saturating_sub(1))));
        }

        // Proposal list
        let proposal_len = self.shared.proposals().history.len();
        if proposal_len == 0 {
            self.proposal_list.select(None);
        } else if self.proposal_list.selected().is_none() {
            self.proposal_list.select(Some(0));
        } else if let Some(selected) = self.proposal_list.selected() {
            self.proposal_list
                .select(Some(min(selected, proposal_len.saturating_sub(1))));
        }

        // Alert table
        let alert_len = self.shared.alerts().alerts.len();
        if alert_len == 0 {
            self.alert_table.select(None);
        } else if self.alert_table.selected().is_none() {
            self.alert_table.select(Some(0));
        } else if let Some(selected) = self.alert_table.selected() {
            self.alert_table
                .select(Some(min(selected, alert_len.saturating_sub(1))));
        }

        // Deposit log table
        let deposit_len = self.shared.deposit_log_snapshot().rows.len();
        if deposit_len == 0 {
            self.deposit_log_table.select(None);
        } else if self.deposit_log_table.selected().is_none() {
            self.deposit_log_table.select(Some(0));
        } else if let Some(selected) = self.deposit_log_table.selected() {
            self.deposit_log_table
                .select(Some(min(selected, deposit_len.saturating_sub(1))));
        }
    }

    fn draw(&mut self, frame: &mut Frame) {
        // Layout: header, body, footer.
        let constraints = [
            Constraint::Length(3), // Header
            Constraint::Min(8),    // Main body
            Constraint::Length(3), // Footer/status bar
        ];

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints(constraints)
            .split(frame.area());

        let header_area = chunks[0];
        let body_area = chunks[1];
        let footer_area = chunks[2];

        // Force clear on layout changes
        if self.ui.force_redraw {
            frame.render_widget(Clear, frame.area());
            self.ui.force_redraw = false;
        }

        // Draw components
        self.draw_header(frame, header_area);
        self.draw_body(frame, body_area);
        self.draw_footer(frame, footer_area);

        // Draw overlays
        if self.ui.mode == UiMode::Help {
            HelpOverlay::draw(frame);
        }
    }

    fn draw_header(&self, frame: &mut Frame, area: Rect) {
        let snapshots = self.shared.health_snapshots();
        let healthy = snapshots
            .iter()
            .filter(|s| matches!(s.status, crate::health::NodeHealthStatus::Healthy))
            .count();

        let network = self.shared.network();
        let last_nonce = self.shared.last_deposit_nonce();
        let alerts = self.shared.alerts();
        let alert_count = alerts.alerts.len();

        // Build title with alert indicator if there are alerts (no count).
        let title = if alert_count > 0 {
            let severity = alerts
                .highest_severity()
                .unwrap_or(crate::tui::types::AlertSeverity::Info);
            let (indicator, style) = match severity {
                crate::tui::types::AlertSeverity::Critical => {
                    ("◉", Style::new().light_red().bold())
                }
                crate::tui::types::AlertSeverity::Error => ("●", Style::new().light_red()),
                crate::tui::types::AlertSeverity::Warning => ("●", Style::new().light_yellow()),
                crate::tui::types::AlertSeverity::Info => ("●", Style::new().light_blue()),
            };
            Line::from(vec![
                Span::raw("bridge health  "),
                Span::styled(indicator, style),
            ])
        } else {
            Line::from("bridge health")
        };

        let block = Block::default().title(title).borders(Borders::ALL);
        let inner = block.inner(area);
        frame.render_widget(block, area);

        // Draw network mode indicator in top-right corner
        self.draw_network_mode_indicator(frame, area, &network);

        // Check for degradation warning
        let has_degradation = network.degradation_warning.is_some() || healthy < 4;

        // Determine the info area and optionally draw degradation banner
        let info_area = if has_degradation {
            // Split header: degradation banner (top), then health/network info (bottom)
            let header_layout = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(1), // Degradation banner
                    Constraint::Min(1),    // Health summary and network state
                ])
                .split(inner);

            // Draw degradation banner
            let warning_msg = network
                .degradation_warning
                .as_deref()
                .unwrap_or("⚠ DEGRADED MODE: <4 healthy nodes - proposals may be delayed");
            let banner = Paragraph::new(Line::from(vec![Span::styled(
                warning_msg,
                Style::new().light_red().bold(),
            )]));
            frame.render_widget(banner, header_layout[0]);

            header_layout[1]
        } else {
            inner
        };

        // Split header into health summary and network state
        let info_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
            .split(info_area);

        // Health summary
        let last_nonce_text = last_nonce
            .map(|n| n.to_string())
            .unwrap_or_else(|| "n/a".to_string());
        let health_line = Line::from(vec![
            Span::styled(
                format!("nodes: {}", snapshots.len()),
                Style::new().light_cyan(),
            ),
            Span::raw("  "),
            Span::styled(format!("healthy: {}", healthy), Style::new().light_green()),
            Span::raw("  "),
            Span::styled(
                format!("last nonce: {}", last_nonce_text),
                Style::new().light_cyan(),
            ),
        ]);
        frame.render_widget(Paragraph::new(health_line), info_chunks[0]);

        // Network state compact view
        let is_stopped = self.stop.is_stopped() || network.kernel_stopped;
        NetworkStatePanel::draw_compact(frame, info_chunks[1], &network, is_stopped);
    }

    /// Draw network mode indicator (mainnet/fakenet) in top-right corner of header.
    fn draw_network_mode_indicator(
        &self,
        frame: &mut Frame,
        header_area: Rect,
        network: &types::NetworkState,
    ) {
        let (label, bg_color) = match network.is_mainnet {
            Some(true) => (" MAINNET ", ratatui::style::Color::Green),
            Some(false) => (" FAKENET ", ratatui::style::Color::Rgb(255, 165, 0)), // Orange
            None => (" ??? ", ratatui::style::Color::DarkGray),
        };

        let indicator_width = label.len() as u16;
        // Position in top-right corner, inside the border (offset by 2 for border + padding)
        let x = header_area
            .x
            .saturating_add(header_area.width)
            .saturating_sub(indicator_width + 2);
        let y = header_area.y;

        let indicator_area = Rect::new(x, y, indicator_width, 1);

        let indicator = Paragraph::new(Span::styled(
            label,
            Style::new().fg(ratatui::style::Color::Black).bg(bg_color),
        ));
        frame.render_widget(indicator, indicator_area);
    }

    fn draw_body(&mut self, frame: &mut Frame, area: Rect) {
        // Single panel view based on focused_panel
        match self.ui.focused_panel {
            FocusedPanel::Health => {
                self.draw_health_and_network(frame, area, true);
            }
            FocusedPanel::Proposals => {
                let proposal_state = self.shared.proposals();
                ProposalPanel::draw_with_detail(
                    frame, area, &proposal_state, &mut self.proposal_list, true,
                    self.proposal_detail_view,
                );
            }
            FocusedPanel::Transactions => {
                let tx_state = self.shared.transactions();
                // Convert VecDeque to slice for drawing
                let txs: Vec<_> = tx_state.transactions.iter().cloned().collect();
                TransactionPanel::draw(
                    frame, area, &txs, &mut self.tx_table, true, self.tx_detail_view,
                );
            }
            FocusedPanel::Alerts => {
                let alerts = self.shared.alerts();
                AlertPanel::draw(frame, area, &alerts, &mut self.alert_table, true);
            }
            FocusedPanel::DepositLog => {
                self.update_deposit_log_limit(area);
                let snapshot = self.shared.deposit_log_snapshot();
                let last_nonce = self.shared.last_deposit_nonce();
                self.maybe_jump_to_last_nonce(&snapshot, last_nonce);
                DepositLogPanel::draw(
                    frame, area, &snapshot, last_nonce, &mut self.deposit_log_table, true,
                );
            }
        }
    }

    fn draw_health_and_network(&mut self, frame: &mut Frame, area: Rect, is_focused: bool) {
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
            .split(area);

        let snapshots = self.shared.health_snapshots();
        HealthPanel::draw(
            frame, chunks[0], &snapshots, &mut self.health_table, is_focused,
        );

        let network = self.shared.network();
        let is_stopped = self.stop.is_stopped() || network.kernel_stopped;
        NetworkStatePanel::draw(frame, chunks[1], &network, is_focused, is_stopped);
    }

    fn draw_footer(&self, frame: &mut Frame, area: Rect) {
        let status_msg = self.shared.status_message().map(|(text, _)| text);
        StatusBar::draw_full_with_status(
            frame, area, self.ui.mode, self.ui.focused_panel, status_msg,
        );
    }

    fn handle_key(&mut self, key: KeyEvent) -> bool {
        // Handle mode-specific keys first
        if self.ui.mode == UiMode::Help {
            // Any key closes help
            self.ui.mode = UiMode::Normal;
            self.ui.force_redraw = true;
            return false;
        }

        // Normal mode keys
        match key.code {
            KeyCode::Char('q') => true,
            KeyCode::Esc => {
                // Close detail view if open
                if self.tx_detail_view.is_some() {
                    self.tx_detail_view = None;
                } else if self.proposal_detail_view.is_some() {
                    self.proposal_detail_view = None;
                }
                false
            }
            KeyCode::Char('?') => {
                self.ui.toggle_help();
                false
            }
            KeyCode::Char('r') => {
                self.tick();
                false
            }
            // Panel shortcuts
            KeyCode::Char('p') => {
                self.ui.focused_panel = FocusedPanel::Proposals;
                false
            }
            KeyCode::Char('t') => {
                self.ui.focused_panel = FocusedPanel::Transactions;
                false
            }
            KeyCode::Char('a') => {
                self.ui.focused_panel = FocusedPanel::Alerts;
                false
            }
            KeyCode::Char('d') => {
                self.ui.focused_panel = FocusedPanel::DepositLog;
                self.ui.deposit_log_jump_pending = true;
                false
            }
            // Number shortcuts for direct panel access
            KeyCode::Char('1') => {
                self.ui.focused_panel = FocusedPanel::Health;
                false
            }
            KeyCode::Char('2') => {
                self.ui.focused_panel = FocusedPanel::DepositLog;
                self.ui.deposit_log_jump_pending = true;
                false
            }
            KeyCode::Char('3') => {
                self.ui.focused_panel = FocusedPanel::Proposals;
                false
            }
            KeyCode::Char('4') => {
                self.ui.focused_panel = FocusedPanel::Transactions;
                false
            }
            KeyCode::Char('5') => {
                self.ui.focused_panel = FocusedPanel::Alerts;
                false
            }
            // Panel-specific actions
            KeyCode::Enter => {
                if matches!(self.ui.focused_panel, FocusedPanel::Transactions) {
                    self.toggle_tx_detail();
                } else if matches!(self.ui.focused_panel, FocusedPanel::Proposals) {
                    self.toggle_proposal_detail();
                }
                false
            }
            KeyCode::Char('y') => {
                if matches!(self.ui.focused_panel, FocusedPanel::Transactions) {
                    self.copy_selected_tx_hash();
                } else if matches!(self.ui.focused_panel, FocusedPanel::DepositLog) {
                    self.copy_current_deposit_log_entry();
                }
                false
            }
            // Navigation
            KeyCode::Tab => {
                self.ui.focus_next();
                false
            }
            KeyCode::BackTab => {
                self.ui.focus_prev();
                false
            }
            KeyCode::Left | KeyCode::Char('h') => {
                self.ui.focus_prev();
                false
            }
            KeyCode::Right | KeyCode::Char('l') => {
                self.ui.focus_next();
                false
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.handle_down();
                false
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.handle_up();
                false
            }
            KeyCode::Char('J') => {
                if matches!(self.ui.focused_panel, FocusedPanel::DepositLog) {
                    for _ in 0..5 {
                        self.move_deposit_log_selection(1);
                    }
                }
                false
            }
            KeyCode::Char('K') => {
                if matches!(self.ui.focused_panel, FocusedPanel::DepositLog) {
                    for _ in 0..5 {
                        self.move_deposit_log_selection(-1);
                    }
                }
                false
            }
            _ => false,
        }
    }

    fn handle_down(&mut self) {
        match self.ui.focused_panel {
            FocusedPanel::Health => {
                let len = self.shared.health_snapshots().len();
                if len == 0 {
                    self.health_table.select(None);
                } else {
                    let next = self
                        .health_table
                        .selected()
                        .map(|idx| min(idx + 1, len.saturating_sub(1)))
                        .unwrap_or(0);
                    self.health_table.select(Some(next));
                }
            }
            FocusedPanel::Transactions => {
                let len = self.shared.transactions().transactions.len();
                if len == 0 {
                    self.tx_table.select(None);
                } else {
                    let next = self
                        .tx_table
                        .selected()
                        .map(|idx| min(idx + 1, len.saturating_sub(1)))
                        .unwrap_or(0);
                    self.tx_table.select(Some(next));
                }
            }
            FocusedPanel::Proposals => {
                let len = self.shared.proposals().history.len();
                if len == 0 {
                    self.proposal_list.select(None);
                } else {
                    let next = self
                        .proposal_list
                        .selected()
                        .map(|idx| min(idx + 1, len.saturating_sub(1)))
                        .unwrap_or(0);
                    self.proposal_list.select(Some(next));
                }
            }
            FocusedPanel::Alerts => {
                let len = self.shared.alerts().alerts.len();
                if len == 0 {
                    self.alert_table.select(None);
                } else {
                    let next = self
                        .alert_table
                        .selected()
                        .map(|idx| min(idx + 1, len.saturating_sub(1)))
                        .unwrap_or(0);
                    self.alert_table.select(Some(next));
                }
            }
            FocusedPanel::DepositLog => {
                self.move_deposit_log_selection(1);
            }
        }
    }

    fn handle_up(&mut self) {
        match self.ui.focused_panel {
            FocusedPanel::Health => {
                if let Some(idx) = self.health_table.selected() {
                    self.health_table.select(Some(idx.saturating_sub(1)));
                }
            }
            FocusedPanel::Transactions => {
                if let Some(idx) = self.tx_table.selected() {
                    self.tx_table.select(Some(idx.saturating_sub(1)));
                }
            }
            FocusedPanel::Proposals => {
                if let Some(idx) = self.proposal_list.selected() {
                    self.proposal_list.select(Some(idx.saturating_sub(1)));
                }
            }
            FocusedPanel::Alerts => {
                if let Some(idx) = self.alert_table.selected() {
                    self.alert_table.select(Some(idx.saturating_sub(1)));
                }
            }
            FocusedPanel::DepositLog => {
                self.move_deposit_log_selection(-1);
            }
        }
    }

    fn maybe_jump_to_last_nonce(
        &mut self,
        _snapshot: &crate::tui::types::DepositLogSnapshot,
        _last_nonce: Option<u64>,
    ) {
        if self.ui.deposit_log_jump_pending {
            self.ui.deposit_log_jump_pending = false;
        }
    }

    fn toggle_proposal_detail(&mut self) {
        // Toggle detail view for selected proposal
        if let Some(idx) = self.proposal_list.selected() {
            if self.proposal_detail_view == Some(idx) {
                // Close detail view if already viewing this proposal
                self.proposal_detail_view = None;
            } else {
                // Open detail view for this proposal
                self.proposal_detail_view = Some(idx);
            }
        }
    }

    fn toggle_tx_detail(&mut self) {
        // Toggle detail view for selected transaction
        if let Some(idx) = self.tx_table.selected() {
            if self.tx_detail_view == Some(idx) {
                // Close detail view if already viewing this transaction
                self.tx_detail_view = None;
            } else {
                // Open detail view for this transaction
                self.tx_detail_view = Some(idx);
            }
        }
    }

    fn copy_current_deposit_log_entry(&mut self) {
        let snapshot = self.shared.deposit_log_snapshot();
        let Some(selected) = self.deposit_log_table.selected() else {
            return;
        };
        let Some(row) = snapshot.rows.get(selected) else {
            return;
        };
        let entry = format!(
            "nonce={} height={} amount={} recipient={} tx_id={}",
            row.nonce, row.block_height, row.amount, row.recipient_hex, row.tx_id_base58
        );
        if let Err(e) = copy_to_clipboard(&entry) {
            tracing::warn!("Failed to copy deposit log entry: {}", e);
            self.shared
                .set_status_message("Failed to copy to clipboard".to_string(), 3);
        } else {
            tracing::debug!("Copied deposit log entry: {}", entry);
            self.shared
                .set_status_message("Copied to clipboard!".to_string(), 3);
        }
    }

    fn copy_selected_tx_hash(&mut self) {
        // Copy selected transaction hash to clipboard using OSC 52
        if let Some(idx) = self.tx_table.selected() {
            let txs = self.shared.transactions().transactions;
            if let Some(tx) = txs.get(idx) {
                // Use OSC 52 escape sequence (terminal-agnostic, works over SSH/tmux)
                if let Err(e) = copy_to_clipboard(&tx.tx_hash) {
                    tracing::warn!("Failed to copy tx hash to clipboard: {}", e);
                    // Show error status message
                    self.shared
                        .set_status_message("Failed to copy to clipboard".to_string(), 3);
                } else {
                    tracing::debug!("Copied tx hash to clipboard: {}", tx.tx_hash);
                    // Show success status message (3 seconds)
                    self.shared
                        .set_status_message("Copied to clipboard!".to_string(), 3);
                }
            }
        }
    }

    fn move_deposit_log_selection(&mut self, delta: i32) {
        let snapshot = self.shared.deposit_log_snapshot();
        let row_count = snapshot.rows.len();
        if row_count == 0 {
            self.deposit_log_table.select(None);
            return;
        }

        let selected = self.deposit_log_table.selected().unwrap_or(0);
        if delta.is_positive() {
            if selected + 1 < row_count {
                self.deposit_log_table.select(Some(selected + 1));
            } else {
                self.deposit_log_table
                    .select(Some(row_count.saturating_sub(1)));
            }
        } else if delta.is_negative() {
            if selected > 0 {
                self.deposit_log_table.select(Some(selected - 1));
            } else {
                self.deposit_log_table.select(Some(0));
            }
        }
    }

    fn update_deposit_log_limit(&self, area: Rect) {
        let inner_height = area.height.saturating_sub(2);
        let limit = inner_height.saturating_sub(1) as usize;
        let mut view = self.shared.deposit_log_view();
        if view.limit != limit || view.offset != 0 {
            view.limit = limit;
            view.offset = 0;
            self.shared.set_deposit_log_view(view);
        }
    }
}
