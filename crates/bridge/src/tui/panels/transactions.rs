//! Transaction activity panel - displays recent deposits and withdrawals.

use ratatui::layout::Constraint;
use ratatui::style::{Color, Style, Stylize};
use ratatui::widgets::{Block, Borders, Cell, Row, Table, TableState};
use ratatui::Frame;

use crate::tui::types::{BridgeTx, TxDirection, TxStatus};

/// Transaction panel for displaying bridge activity.
pub struct TransactionPanel;

impl TransactionPanel {
    /// Draw the transaction activity table.
    pub fn draw(
        frame: &mut Frame,
        area: ratatui::layout::Rect,
        transactions: &[BridgeTx],
        table_state: &mut TableState,
        is_focused: bool,
        detail_view: Option<usize>,
    ) {
        let rows = transactions.iter().map(|tx| {
            let icon = match tx.direction {
                TxDirection::Deposit => "↓",
                TxDirection::Withdrawal => "↑",
            };

            let amount = tx.format_amount();
            let from = truncate_address(&tx.from);
            let to = truncate_address(&tx.to);
            let status = tx.status.display();
            let time = format_relative(tx.timestamp);

            Row::new(vec![
                Cell::from(icon),
                Cell::from(amount),
                Cell::from(from),
                Cell::from(to),
                Cell::from(status),
                Cell::from(time),
            ])
            .style(row_style(&tx.status))
        });

        let border_style = if is_focused {
            Style::new().light_cyan()
        } else {
            Style::default()
        };

        // Check if we should show detail view
        if let Some(detail_idx) = detail_view {
            if let Some(tx) = transactions.get(detail_idx) {
                // Split area: table on left, details on right
                use ratatui::layout::{Constraint, Direction, Layout};
                let chunks = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                    .split(area);

                // Draw table in left half
                let table = Table::new(
                    rows,
                    [
                        Constraint::Length(3),  // Icon
                        Constraint::Length(8),  // Amount (narrower)
                        Constraint::Length(8),  // From (narrower)
                        Constraint::Length(8),  // To (narrower)
                        Constraint::Length(12), // Status (narrower)
                        Constraint::Min(6),     // Time (narrower)
                    ],
                )
                .header(
                    Row::new(vec!["", "nocks", "from", "to", "status", "time"])
                        .style(Style::new().bold()),
                )
                .block(
                    Block::default()
                        .title("transaction activity")
                        .borders(Borders::ALL)
                        .border_style(border_style),
                )
                .row_highlight_style(Style::new().reversed())
                .highlight_symbol("➤ ");

                frame.render_stateful_widget(table, chunks[0], table_state);

                // Draw detail panel in right half
                draw_detail_panel(frame, chunks[1], tx, is_focused);
            } else {
                // Invalid detail index, draw normal table
                let table = Table::new(
                    rows,
                    [
                        Constraint::Length(3),  // Icon
                        Constraint::Length(12), // Amount
                        Constraint::Length(12), // From
                        Constraint::Length(12), // To
                        Constraint::Length(18), // Status
                        Constraint::Min(10),    // Time
                    ],
                )
                .header(
                    Row::new(vec!["", "nocks", "from", "to", "status", "time"])
                        .style(Style::new().bold()),
                )
                .block(
                    Block::default()
                        .title("transaction activity")
                        .borders(Borders::ALL)
                        .border_style(border_style),
                )
                .row_highlight_style(Style::new().reversed())
                .highlight_symbol("➤ ");

                frame.render_stateful_widget(table, area, table_state);
            }
        } else {
            // No detail view, draw normal table
            let table = Table::new(
                rows,
                [
                    Constraint::Length(3),  // Icon
                    Constraint::Length(12), // Amount
                    Constraint::Length(12), // From
                    Constraint::Length(12), // To
                    Constraint::Length(18), // Status
                    Constraint::Min(10),    // Time
                ],
            )
            .header(
                Row::new(vec!["", "nocks", "from", "to", "status", "time"])
                    .style(Style::new().bold()),
            )
            .block(
                Block::default()
                    .title("transaction activity")
                    .borders(Borders::ALL)
                    .border_style(border_style),
            )
            .row_highlight_style(Style::new().reversed())
            .highlight_symbol("➤ ");

            frame.render_stateful_widget(table, area, table_state);
        }
    }
}

/// Draw the transaction detail panel.
fn draw_detail_panel(
    frame: &mut Frame,
    area: ratatui::layout::Rect,
    tx: &BridgeTx,
    is_focused: bool,
) {
    use ratatui::text::{Line, Span};
    use ratatui::widgets::Paragraph;

    let border_style = if is_focused {
        Style::new().light_cyan()
    } else {
        Style::default()
    };

    let block = Block::default()
        .title("transaction details")
        .borders(Borders::ALL)
        .border_style(border_style);

    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Build detail lines
    let mut lines = Vec::new();

    // Transaction hash
    lines.push(Line::from(vec![
        Span::styled("Hash: ", Style::new().bold()),
        Span::raw(&tx.tx_hash),
    ]));

    // Direction
    let direction_str = match tx.direction {
        TxDirection::Deposit => "Deposit ↓",
        TxDirection::Withdrawal => "Withdrawal ↑",
    };
    lines.push(Line::from(vec![
        Span::styled("Direction: ", Style::new().bold()),
        Span::raw(direction_str),
    ]));

    // Amount
    lines.push(Line::from(vec![
        Span::styled("Amount: ", Style::new().bold()),
        Span::raw(format!("{} NOCK", tx.format_amount())),
    ]));

    // From address
    lines.push(Line::from(vec![
        Span::styled("From: ", Style::new().bold()),
        Span::raw(&tx.from),
    ]));

    // To address
    lines.push(Line::from(vec![
        Span::styled("To: ", Style::new().bold()),
        Span::raw(&tx.to),
    ]));

    // Status
    lines.push(Line::from(vec![
        Span::styled("Status: ", Style::new().bold()),
        Span::styled(tx.status.display(), row_style(&tx.status)),
    ]));

    // Timestamp
    lines.push(Line::from(vec![
        Span::styled("Time: ", Style::new().bold()),
        Span::raw(format_relative(tx.timestamp)),
    ]));

    // Block numbers (if available)
    if let Some(block) = tx.base_block {
        lines.push(Line::from(vec![
            Span::styled("Base Block: ", Style::new().bold()),
            Span::raw(format!("{}", block)),
        ]));
    }

    if let Some(height) = tx.nock_height {
        lines.push(Line::from(vec![
            Span::styled("Nock Height: ", Style::new().bold()),
            Span::raw(format!("{}", height)),
        ]));
    }

    // Basescan link (for deposits)
    if let Some(url) = tx.basescan_url() {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            "View on Basescan:",
            Style::new().bold(),
        )]));
        lines.push(Line::from(vec![Span::styled(
            url,
            Style::new().light_blue(),
        )]));
    }

    // Instructions
    lines.push(Line::from(""));
    lines.push(Line::from(vec![Span::styled(
        "Press Enter to close | Press 'y' to copy hash",
        Style::new().dark_gray(),
    )]));

    let paragraph = Paragraph::new(lines).wrap(ratatui::widgets::Wrap { trim: false });
    frame.render_widget(paragraph, inner);
}

/// Truncate an address for display (0x1234...5678).
fn truncate_address(addr: &str) -> String {
    if addr.len() > 12 {
        let prefix = &addr[..6];
        let suffix = &addr[addr.len() - 4..];
        format!("{}...{}", prefix, suffix)
    } else {
        addr.to_string()
    }
}

/// Get row style based on transaction status.
fn row_style(status: &TxStatus) -> Style {
    match status {
        TxStatus::Completed => Style::new().fg(Color::Green),
        TxStatus::Pending | TxStatus::Confirming { .. } | TxStatus::Processing => {
            Style::new().fg(Color::Yellow)
        }
        TxStatus::Failed { .. } => Style::new().fg(Color::Red),
    }
}

/// Format a timestamp as relative time.
fn format_relative(time: std::time::SystemTime) -> String {
    match time.elapsed() {
        Ok(duration) => {
            let secs = duration.as_secs();
            if secs == 0 {
                "just now".into()
            } else if secs < 60 {
                format!("{}s ago", secs)
            } else if secs < 3600 {
                format!("{}m ago", secs / 60)
            } else if secs < 86400 {
                format!("{}h ago", secs / 3600)
            } else {
                format!("{}d ago", secs / 86400)
            }
        }
        Err(_) => "–".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_address() {
        let addr = "0x1234567890abcdef1234567890abcdef12345678";
        assert_eq!(truncate_address(addr), "0x1234...5678");

        let short_addr = "0x1234";
        assert_eq!(truncate_address(short_addr), "0x1234");
    }
}
