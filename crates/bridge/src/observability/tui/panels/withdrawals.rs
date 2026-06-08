//! Withdrawal frontier and queue panel.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Row, Table};
use ratatui::Frame;

use crate::observability::tui::types::{
    format_nock_from_nicks, WithdrawalFrontierStatus, WithdrawalLifecycleCounts,
    WithdrawalLocalSnapshot, WithdrawalQueueRow, WithdrawalSequencerSnapshot,
    WithdrawalStateSnapshot,
};

pub struct WithdrawalPanel;

impl WithdrawalPanel {
    pub fn draw(
        frame: &mut Frame,
        area: Rect,
        snapshot: &WithdrawalStateSnapshot,
        is_focused: bool,
    ) {
        let border_style = if is_focused {
            Style::new().light_cyan()
        } else {
            Style::default()
        };
        let block = Block::default()
            .title("withdrawals")
            .borders(Borders::ALL)
            .border_style(border_style);
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(14), Constraint::Min(6)])
            .split(inner);
        Self::draw_frontier(frame, chunks[0], snapshot);
        Self::draw_queue(frame, chunks[1], snapshot);
    }

    fn draw_frontier(frame: &mut Frame, area: Rect, snapshot: &WithdrawalStateSnapshot) {
        let mut lines = Vec::new();
        lines.push(Line::from(vec![Span::styled(
            "Sequencer",
            Style::new().bold().underlined(),
        )]));

        let frontier_label = match snapshot.sequencer.frontier_status {
            WithdrawalFrontierStatus::Unknown => "unknown".to_string(),
            WithdrawalFrontierStatus::None => "none".to_string(),
            WithdrawalFrontierStatus::Present => snapshot
                .sequencer
                .frontier_nonce
                .map(|nonce| format!("#{nonce}"))
                .unwrap_or_else(|| "present".to_string()),
        };
        lines.push(Line::from(vec![
            Span::styled("  sequencer: ", Style::new().dark_gray()),
            Span::styled(
                frontier_label,
                status_style(match snapshot.sequencer.frontier_status {
                    WithdrawalFrontierStatus::Present => "present",
                    WithdrawalFrontierStatus::None => "none",
                    WithdrawalFrontierStatus::Unknown => "unknown",
                }),
            ),
            Span::raw("  "),
            Span::styled("state: ", Style::new().dark_gray()),
            Span::styled(
                snapshot
                    .sequencer
                    .frontier_state
                    .as_deref()
                    .unwrap_or("n/a")
                    .to_string(),
                status_style(
                    snapshot
                        .sequencer
                        .frontier_state
                        .as_deref()
                        .unwrap_or("n/a"),
                ),
            ),
            Span::raw("  "),
            Span::styled("epoch: ", Style::new().dark_gray()),
            Span::raw(opt_height(snapshot.sequencer.frontier_epoch)),
        ]));

        lines.push(Line::from(vec![
            Span::styled("  turn: ", Style::new().dark_gray()),
            Span::raw(sequencer_turn_label(&snapshot.sequencer)),
            Span::raw("  "),
            Span::styled("handoff: ", Style::new().dark_gray()),
            Span::raw(block_count_label(snapshot.sequencer.blocks_until_handoff)),
            Span::raw("  "),
            Span::styled("window: ", Style::new().dark_gray()),
            Span::raw(block_count_label(snapshot.sequencer.handoff_window_blocks)),
            Span::raw("  "),
            Span::styled("base: ", Style::new().dark_gray()),
            Span::raw(opt_height(snapshot.sequencer.current_confirmed_base_height)),
        ]));

        if let Some(error) = snapshot.sequencer.last_error.as_deref() {
            lines.push(Line::from(vec![
                Span::styled("  sequencer error: ", Style::new().light_red()),
                Span::styled(truncate(error, 88), Style::new().light_red()),
            ]));
        }

        lines.push(Line::from(vec![Span::styled(
            "Local Node",
            Style::new().bold().underlined(),
        )]));
        lines.push(Line::from(vec![
            Span::styled("  activation: ", Style::new().dark_gray()),
            Span::styled(
                activation_label(&snapshot.local),
                activation_style(&snapshot.local),
            ),
            Span::raw("  "),
            Span::styled("cursor: ", Style::new().dark_gray()),
            Span::raw(format!(
                "base_next={} nock_next={}",
                opt_height(snapshot.local.current_base_next_height),
                opt_height(snapshot.local.current_nock_next_height)
            )),
            Span::raw("  "),
            Span::styled("cutoff: ", Style::new().dark_gray()),
            Span::raw(format!(
                "nock_next={}",
                opt_height(snapshot.local.activation_nock_next_height)
            )),
        ]));

        lines.push(lifecycle_health_line(&snapshot.local.lifecycle));
        lines.push(lifecycle_queue_line(&snapshot.local.lifecycle));

        match snapshot.local.frontier_row.as_ref() {
            Some(row) => {
                lines.push(Line::from(vec![
                    Span::styled("  frontier row: ", Style::new().dark_gray()),
                    Span::styled(row.state.clone(), status_style(&row.state)),
                    Span::raw("  "),
                    Span::styled("epoch: ", Style::new().dark_gray()),
                    Span::raw(row.epoch.to_string()),
                    Span::raw("  "),
                    Span::styled("id: ", Style::new().dark_gray()),
                    Span::styled(truncate(&row.id, 28), Style::new().light_cyan()),
                ]));
                lines.push(Line::from(vec![
                    Span::styled("  amount: ", Style::new().dark_gray()),
                    Span::styled(amount_label(row), Style::new().light_green()),
                    Span::raw("  "),
                    Span::styled("recipient: ", Style::new().dark_gray()),
                    Span::styled(
                        row.recipient
                            .as_deref()
                            .map(|value| truncate(value, 18))
                            .unwrap_or_else(|| "n/a".to_string()),
                        Style::new().light_yellow(),
                    ),
                    Span::raw("  "),
                    Span::styled("base batch: ", Style::new().dark_gray()),
                    Span::raw(opt_height(row.base_batch_end)),
                ]));
                lines.push(Line::from(vec![
                    Span::styled("  turn: ", Style::new().dark_gray()),
                    Span::raw(turn_label(row)),
                    Span::raw("  "),
                    Span::styled("handoff: ", Style::new().dark_gray()),
                    Span::raw(block_count_label(row.blocks_until_handoff)),
                    Span::raw("  "),
                    Span::styled("retry: ", Style::new().dark_gray()),
                    Span::raw(block_count_label(row.blocks_until_retry)),
                ]));
                lines.push(Line::from(vec![
                    Span::styled("  artifacts: ", Style::new().dark_gray()),
                    artifact_span("canonical", row.proposal_hash.is_some()),
                    Span::raw("  "),
                    artifact_span("cert", row.has_commit_certificate),
                    Span::raw("  "),
                    artifact_span("auth", row.has_authorized_transaction),
                    Span::raw("  "),
                    artifact_span("submitted", row.has_submitted_transaction),
                ]));
            }
            None => lines.push(Line::from(vec![Span::styled(
                "  no local row for current frontier",
                Style::new().dark_gray(),
            )])),
        }

        if let Some(error) = snapshot.local.last_error.as_deref() {
            lines.push(Line::from(vec![
                Span::styled("  local error: ", Style::new().light_red()),
                Span::styled(truncate(error, 92), Style::new().light_red()),
            ]));
        }

        frame.render_widget(Paragraph::new(lines), area);
    }

    fn draw_queue(frame: &mut Frame, area: Rect, snapshot: &WithdrawalStateSnapshot) {
        let header = Row::new(["nonce", "state", "epoch", "amount", "to", "queue", "artifacts"])
            .style(Style::new().bold());
        let rows = snapshot.local.queue.iter().map(|row| {
            Row::new([
                row.nonce.to_string(),
                row.state.clone(),
                row.epoch.to_string(),
                amount_label(row),
                row.recipient
                    .as_deref()
                    .map(|value| truncate(value, 12))
                    .unwrap_or_else(|| "n/a".to_string()),
                queue_position_label(row, snapshot.sequencer.frontier_nonce),
                artifact_label(row),
            ])
            .style(status_style(&row.state))
        });
        let table = Table::new(
            rows,
            [
                Constraint::Length(8),
                Constraint::Length(16),
                Constraint::Length(7),
                Constraint::Length(14),
                Constraint::Length(14),
                Constraint::Length(16),
                Constraint::Min(18),
            ],
        )
        .header(header)
        .column_spacing(1);
        frame.render_widget(table, area);
    }
}

fn lifecycle_health_line(counts: &WithdrawalLifecycleCounts) -> Line<'static> {
    Line::from(vec![
        Span::styled("  health: ", Style::new().dark_gray()),
        count_span(
            "blocking",
            counts.ordering_blocking_count,
            Style::new().light_yellow(),
        ),
        Span::raw("  "),
        count_span(
            "mempool",
            counts.mempool_accepted_count,
            Style::new().light_magenta(),
        ),
        Span::raw("  "),
        count_span(
            "confirmed",
            counts.confirmed_count,
            Style::new().light_green(),
        ),
        Span::raw("  "),
        count_span("unconfirmed", counts.live_count, Style::default()),
    ])
}

fn lifecycle_queue_line(counts: &WithdrawalLifecycleCounts) -> Line<'static> {
    Line::from(vec![
        Span::styled("  queue: ", Style::new().dark_gray()),
        count_span(
            "below",
            counts.below_frontier_count,
            Style::new().dark_gray(),
        ),
        Span::raw("  "),
        count_span(
            "future",
            counts.above_frontier_count,
            Style::new().light_cyan(),
        ),
        Span::raw("  "),
        count_span("pending", counts.pending_count, Style::new().light_yellow()),
        Span::raw("  "),
        count_span(
            "authorized",
            counts.authorized_count,
            Style::new().light_green(),
        ),
    ])
}

fn count_span(label: &'static str, value: u64, active_style: Style) -> Span<'static> {
    let style = if value > 0 {
        active_style
    } else {
        Style::new().dark_gray()
    };
    Span::styled(format!("{label}={value}"), style)
}

fn activation_label(snapshot: &WithdrawalLocalSnapshot) -> String {
    match snapshot.activation_ready {
        Some(true) => "ready".to_string(),
        Some(false) => "waiting".to_string(),
        None => "unknown".to_string(),
    }
}

fn activation_style(snapshot: &WithdrawalLocalSnapshot) -> Style {
    match snapshot.activation_ready {
        Some(true) => Style::new().light_green(),
        Some(false) => Style::new().light_yellow(),
        None => Style::new().dark_gray(),
    }
}

fn status_style(status: &str) -> Style {
    match status {
        "pending" => Style::new().light_yellow(),
        "assembling" | "prepared" | "peer_canonical" => Style::new().light_cyan(),
        "authorized" => Style::new().light_green(),
        "mempool_accepted" => Style::new().light_magenta(),
        "confirmed" => Style::new().dark_gray(),
        "present" => Style::new().light_green(),
        "none" => Style::new().dark_gray(),
        "unknown" => Style::new().light_yellow(),
        _ => Style::default(),
    }
}

fn amount_label(row: &WithdrawalQueueRow) -> String {
    row.amount
        .map(|amount| format_nock_from_nicks(u128::from(amount)))
        .unwrap_or_else(|| "n/a".to_string())
}

fn turn_label(row: &WithdrawalQueueRow) -> String {
    match row.current_responsible_node {
        Some(node) if row.is_my_turn => format!("node {node} (me)"),
        Some(node) => format!("node {node}"),
        None => "n/a".to_string(),
    }
}

fn sequencer_turn_label(snapshot: &WithdrawalSequencerSnapshot) -> String {
    match snapshot.current_responsible_node {
        Some(node) if snapshot.is_my_turn => format!("node {node} (me)"),
        Some(node) => format!("node {node}"),
        None => "n/a".to_string(),
    }
}

fn queue_position_label(row: &WithdrawalQueueRow, frontier_nonce: Option<u64>) -> String {
    match frontier_nonce {
        Some(frontier) if row.nonce < frontier => "below frontier".to_string(),
        Some(frontier) if row.nonce == frontier => "frontier".to_string(),
        Some(_) => "future".to_string(),
        None => "passive".to_string(),
    }
}

fn artifact_label(row: &WithdrawalQueueRow) -> String {
    let canonical = if row.proposal_hash.is_some() {
        "canon"
    } else {
        "-"
    };
    let cert = if row.has_commit_certificate {
        "cert"
    } else {
        "-"
    };
    let auth = if row.has_authorized_transaction {
        "auth"
    } else {
        "-"
    };
    let submitted = if row.has_submitted_transaction {
        "sent"
    } else {
        "-"
    };
    format!("{canonical}/{cert}/{auth}/{submitted}")
}

fn artifact_span(label: &'static str, present: bool) -> Span<'static> {
    let text = if present {
        format!("{label}=yes")
    } else {
        format!("{label}=no")
    };
    Span::styled(
        text,
        if present {
            Style::new().light_green()
        } else {
            Style::new().dark_gray()
        },
    )
}

fn opt_height(value: Option<u64>) -> String {
    value
        .map(|height| height.to_string())
        .unwrap_or_else(|| "n/a".to_string())
}

fn block_count_label(value: Option<u64>) -> String {
    value
        .map(|blocks| format!("{blocks} blocks"))
        .unwrap_or_else(|| "n/a".to_string())
}

fn truncate(value: &str, max: usize) -> String {
    if value.len() <= max {
        value.to_string()
    } else {
        format!("{}...", &value[..max.saturating_sub(3)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(nonce: u64) -> WithdrawalQueueRow {
        WithdrawalQueueRow {
            nonce,
            ..WithdrawalQueueRow::default()
        }
    }

    #[test]
    fn queue_position_labels_are_passive_context_only() {
        assert_eq!(queue_position_label(&row(1), Some(2)), "below frontier");
        assert_eq!(queue_position_label(&row(2), Some(2)), "frontier");
        assert_eq!(queue_position_label(&row(3), Some(2)), "future");
        assert_eq!(queue_position_label(&row(3), None), "passive");
    }
}
