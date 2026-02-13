//! Network state panel - displays chain heights and sync status.
//!
//! Shows Base chain height, Nockchain height, pending operations,
//! and batch processing status.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::tui::types::{BatchStatus, NetworkState, NockchainApiStatus};

/// Network state panel for displaying chain sync status.
pub struct NetworkStatePanel;

impl NetworkStatePanel {
    /// Draw the network state panel.
    pub fn draw(
        frame: &mut Frame,
        area: Rect,
        state: &NetworkState,
        is_focused: bool,
        is_stopped: bool,
    ) {
        let border_style = if is_focused {
            Style::new().light_cyan()
        } else {
            Style::default()
        };

        let block = Block::default()
            .title("network state")
            .borders(Borders::ALL)
            .border_style(border_style);

        let inner = block.inner(area);
        frame.render_widget(block, area);

        // Split into two columns: chains and operations
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(inner);

        Self::draw_chains(frame, columns[0], state);
        Self::draw_operations(frame, columns[1], state, is_stopped);
    }

    fn draw_chains(frame: &mut Frame, area: Rect, state: &NetworkState) {
        let mut lines = Vec::new();

        // Base chain
        lines.push(Line::from(vec![Span::styled(
            "Base Chain",
            Style::new().bold(),
        )]));
        lines.push(Line::from(vec![
            Span::raw("  height: "),
            Span::styled(format!("{}", state.base.height), Style::new().light_green()),
            if state.base.is_syncing {
                Span::styled(" (syncing)", Style::new().light_yellow())
            } else {
                Span::raw("")
            },
        ]));
        if !state.base.tip_hash.is_empty() {
            let truncated = truncate_hash(&state.base.tip_hash, 16);
            lines.push(Line::from(vec![
                Span::raw("  tip: "),
                Span::styled(truncated, Style::new().light_cyan()),
            ]));
        }
        if state.base.confirmations > 0 {
            lines.push(Line::from(vec![
                Span::raw("  confirmations: "),
                Span::styled(
                    format!("{}", state.base.confirmations),
                    Style::new().light_cyan(),
                ),
            ]));
        }
        if let Some(updated) = state.base.last_updated {
            lines.push(Line::from(vec![
                Span::raw("  updated: "),
                Span::styled(format_relative(updated), Style::new().dark_gray()),
            ]));
        }

        lines.push(Line::from(""));

        // Nockchain
        lines.push(Line::from(vec![Span::styled(
            "Nockchain",
            Style::new().bold(),
        )]));
        lines.push(Line::from(vec![
            Span::raw("  height: "),
            Span::styled(
                format!("{}", state.nockchain.height),
                Style::new().light_green(),
            ),
            if state.nockchain.is_syncing {
                Span::styled(" (syncing)", Style::new().light_yellow())
            } else {
                Span::raw("")
            },
        ]));
        if !state.nockchain.tip_hash.is_empty() {
            let truncated = truncate_hash(&state.nockchain.tip_hash, 16);
            lines.push(Line::from(vec![
                Span::raw("  tip: "),
                Span::styled(truncated, Style::new().light_cyan()),
            ]));
        }
        if let Some(updated) = state.nockchain.last_updated {
            lines.push(Line::from(vec![
                Span::raw("  updated: "),
                Span::styled(format_relative(updated), Style::new().dark_gray()),
            ]));
        }

        // Nockchain API status
        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            "Nockchain API",
            Style::new().bold(),
        )]));

        let (status_indicator, status_text, status_style) = match &state.nockchain_api_status {
            NockchainApiStatus::Connected { .. } => ("●", "connected", Style::new().light_green()),
            NockchainApiStatus::Connecting { attempt, .. } if *attempt == 0 => {
                ("◐", "connecting...", Style::new().light_yellow())
            }
            NockchainApiStatus::Connecting { attempt, .. } => (
                "◐",
                &format!("reconnecting (attempt {})", attempt) as &str,
                Style::new().light_yellow(),
            ),
            NockchainApiStatus::Disconnected { .. } => {
                ("○", "disconnected", Style::new().light_red())
            }
        };

        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(status_indicator, status_style),
            Span::raw(" "),
            Span::styled(status_text.to_string(), status_style),
        ]));

        // Show duration for connected/disconnected states
        let duration = state.nockchain_api_status.duration();
        if duration.as_secs() > 0 {
            let duration_text = if duration.as_secs() >= 3600 {
                format!(
                    "{}h {}m",
                    duration.as_secs() / 3600,
                    (duration.as_secs() % 3600) / 60
                )
            } else if duration.as_secs() >= 60 {
                format!("{}m {}s", duration.as_secs() / 60, duration.as_secs() % 60)
            } else {
                format!("{}s", duration.as_secs())
            };
            lines.push(Line::from(vec![
                Span::raw("    for "),
                Span::styled(duration_text, Style::new().dark_gray()),
            ]));
        }

        // Show error if available
        if let Some(error) = state.nockchain_api_status.last_error() {
            let truncated = if error.len() > 30 {
                &error[..30]
            } else {
                error
            };
            lines.push(Line::from(vec![
                Span::raw("    "),
                Span::styled(truncated.to_string(), Style::new().light_red().italic()),
            ]));
        }

        let paragraph = Paragraph::new(lines);
        frame.render_widget(paragraph, area);
    }

    fn draw_operations(frame: &mut Frame, area: Rect, state: &NetworkState, is_stopped: bool) {
        let mut lines = Vec::new();

        // Status indicator (stopped/hold/running)
        let status = status_kind(state, is_stopped);
        let hold_details = hold_status_details(state);
        let (status_label, status_style, status_detail) = match status {
            BridgeStatus::Stopped => (
                "STOPPED".to_string(),
                Style::new().light_red().bold(),
                if hold_details.is_empty() {
                    None
                } else {
                    Some(hold_details.join(", "))
                },
            ),
            BridgeStatus::BaseHold => {
                let height = hold_height_label(state.base_hold_height);
                (
                    "BASE HOLD".to_string(),
                    Style::new().light_red().bold(),
                    Some(format!("nock@{height}")),
                )
            }
            BridgeStatus::NockHold => {
                let height = hold_height_label(state.nock_hold_height);
                (
                    "NOCK HOLD".to_string(),
                    Style::new().light_red().bold(),
                    Some(format!("base@{height}")),
                )
            }
            BridgeStatus::Running => (
                "RUNNING".to_string(),
                Style::new().light_green().bold(),
                None,
            ),
        };

        lines.push(Line::from(vec![Span::styled(
            "Status",
            Style::new().bold(),
        )]));
        let mut status_spans = vec![Span::raw("  "), Span::styled(status_label, status_style)];
        if let Some(detail) = status_detail {
            let separator = if status == BridgeStatus::Stopped {
                ", "
            } else {
                " "
            };
            status_spans.push(Span::raw(separator));
            status_spans.push(Span::styled(detail, Style::new().dark_gray()));
        }
        lines.push(Line::from(status_spans));
        lines.push(Line::from(""));

        // Kernel state counts (detailed breakdown)
        lines.push(Line::from(vec![Span::styled(
            "Kernel State",
            Style::new().bold(),
        )]));

        // Deposits breakdown
        let unsettled_dep_style = if state.unsettled_deposit_count > 0 {
            Style::new().light_yellow()
        } else {
            Style::new().dark_gray()
        };
        lines.push(Line::from(vec![
            Span::raw("  deposits: "),
            Span::styled(
                format!("{}", state.unsettled_deposit_count),
                unsettled_dep_style,
            ),
            Span::styled(" unsettled", Style::new().dark_gray()),
        ]));

        // Withdrawals breakdown
        let unsettled_wd_style = if state.unsettled_withdrawal_count > 0 {
            Style::new().light_yellow()
        } else {
            Style::new().dark_gray()
        };
        lines.push(Line::from(vec![
            Span::raw("  withdrawals: "),
            Span::styled(
                format!("{}", state.unsettled_withdrawal_count),
                unsettled_wd_style,
            ),
            Span::styled(" unsettled", Style::new().dark_gray()),
        ]));

        lines.push(Line::from(""));

        // Batch status
        lines.push(Line::from(vec![Span::styled(
            "Batch Status",
            Style::new().bold(),
        )]));

        let (status_text, status_style) = match &state.batch_status {
            BatchStatus::Idle => ("idle".to_string(), Style::new().dark_gray()),
            BatchStatus::Processing {
                batch_id,
                progress_pct,
            } => (
                format!("processing #{} ({}%)", batch_id, progress_pct),
                Style::new().light_cyan(),
            ),
            BatchStatus::AwaitingSignatures {
                batch_id,
                collected,
                required,
            } => (
                format!("#{}: {}/{} signatures", batch_id, collected, required),
                Style::new().light_yellow(),
            ),
            BatchStatus::Submitting { batch_id } => (
                format!("submitting #{}", batch_id),
                Style::new().light_green(),
            ),
        };

        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(status_text, status_style),
        ]));

        // Progress bar for signature collection
        if let BatchStatus::AwaitingSignatures {
            collected,
            required,
            ..
        } = &state.batch_status
        {
            let progress = *collected as f64 / *required as f64;
            let bar_width = (area.width as usize).saturating_sub(4);
            let filled = (progress * bar_width as f64) as usize;
            let empty = bar_width.saturating_sub(filled);

            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled("█".repeat(filled), Style::new().light_green()),
                Span::styled("░".repeat(empty), Style::new().dark_gray()),
            ]));
        }

        let paragraph = Paragraph::new(lines);
        frame.render_widget(paragraph, area);
    }

    /// Draw a compact version for the header area.
    pub fn draw_compact(frame: &mut Frame, area: Rect, state: &NetworkState, is_stopped: bool) {
        let mut spans = vec![
            Span::styled("base: ", Style::new().dark_gray()),
            Span::styled(format!("{}", state.base.height), Style::new().light_cyan()),
        ];

        // Show hold indicator for base
        if state.base_hold {
            spans.push(Span::styled(" ⏸", Style::new().light_red()));
        }

        spans.push(Span::raw("  "));
        spans.push(Span::styled("nock: ", Style::new().dark_gray()));
        spans.push(Span::styled(
            format!("{}", state.nockchain.height),
            Style::new().light_cyan(),
        ));

        // Show hold indicator for nock
        if state.nock_hold {
            spans.push(Span::styled(" ⏸", Style::new().light_red()));
        }

        // Show nockchain API status indicator
        spans.push(Span::raw(" "));
        let (api_indicator, api_style) = match &state.nockchain_api_status {
            NockchainApiStatus::Connected { .. } => ("●", Style::new().light_green()),
            NockchainApiStatus::Connecting { .. } => ("◐", Style::new().light_yellow()),
            NockchainApiStatus::Disconnected { .. } => ("○", Style::new().light_red()),
        };
        spans.push(Span::styled(api_indicator, api_style));

        spans.push(Span::raw("  "));
        spans.push(Span::styled("pending: ", Style::new().dark_gray()));
        spans.push(Span::styled(
            format!("{}↓ {}↑", state.pending_deposits, state.pending_withdrawals),
            if state.pending_deposits > 0 || state.pending_withdrawals > 0 {
                Style::new().light_yellow()
            } else {
                Style::new().dark_gray()
            },
        ));
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            state.batch_status.display(),
            batch_status_style(&state.batch_status),
        ));
        spans.push(Span::raw("  "));

        let status = status_kind(state, is_stopped);
        let hold_details = hold_status_details(state);
        let (status_text, status_style) = match status {
            BridgeStatus::Stopped => {
                let text = if hold_details.is_empty() {
                    "STOPPED".to_string()
                } else {
                    format!("STOPPED, {}", hold_details.join(", "))
                };
                (text, Style::new().light_red().bold())
            }
            BridgeStatus::BaseHold => {
                let height = hold_height_label(state.base_hold_height);
                (
                    format!("BASE HOLD nock@{height}"),
                    Style::new().light_red().bold(),
                )
            }
            BridgeStatus::NockHold => {
                let height = hold_height_label(state.nock_hold_height);
                (
                    format!("NOCK HOLD base@{height}"),
                    Style::new().light_red().bold(),
                )
            }
            BridgeStatus::Running => ("RUNNING".to_string(), Style::new().light_green().bold()),
        };
        spans.push(Span::styled(format!("status: {status_text}"), status_style));

        let line = Line::from(spans);
        let paragraph = Paragraph::new(line);
        frame.render_widget(paragraph, area);
    }
}

fn truncate_hash(hash: &str, max_len: usize) -> String {
    if hash.len() <= max_len {
        return hash.to_string();
    }
    let prefix_len = max_len / 2 - 2;
    let suffix_len = max_len / 2 - 1;
    format!(
        "{}...{}",
        &hash[..prefix_len],
        &hash[hash.len() - suffix_len..]
    )
}

fn format_relative(time: std::time::SystemTime) -> String {
    match time.elapsed() {
        Ok(duration) if duration.as_secs() > 0 => format!("{}s ago", duration.as_secs()),
        Ok(_) => "just now".into(),
        Err(_) => "–".into(),
    }
}

fn batch_status_style(status: &BatchStatus) -> Style {
    match status {
        BatchStatus::Idle => Style::new().dark_gray(),
        BatchStatus::Processing { .. } => Style::new().light_cyan(),
        BatchStatus::AwaitingSignatures { .. } => Style::new().light_yellow(),
        BatchStatus::Submitting { .. } => Style::new().light_green(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BridgeStatus {
    Stopped,
    BaseHold,
    NockHold,
    Running,
}

fn status_kind(state: &NetworkState, is_stopped: bool) -> BridgeStatus {
    if is_stopped {
        BridgeStatus::Stopped
    } else if state.base_hold {
        BridgeStatus::BaseHold
    } else if state.nock_hold {
        BridgeStatus::NockHold
    } else {
        BridgeStatus::Running
    }
}

fn hold_status_details(state: &NetworkState) -> Vec<String> {
    let mut details = Vec::new();
    if state.base_hold {
        let height = hold_height_label(state.base_hold_height);
        details.push(format!("BASE HOLD nock@{height}"));
    }
    if state.nock_hold {
        let height = hold_height_label(state.nock_hold_height);
        details.push(format!("NOCK HOLD base@{height}"));
    }
    details
}

fn hold_height_label(height: Option<u64>) -> String {
    height
        .map(|value| value.to_string())
        .unwrap_or_else(|| "unknown".to_string())
}
