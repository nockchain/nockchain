//! Deposit log panel - displays recent deposits from the SQLite log.

use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState};
use ratatui::Frame;

use crate::tui::types::DepositLogSnapshot;

/// Deposit log panel for displaying nonce-ordered deposits.
pub struct DepositLogPanel;

impl DepositLogPanel {
    /// Render the deposit log table for the current snapshot.
    ///
    /// The table is newest-first (offset handled by the snapshot builder) and highlights
    /// the row matching `last_deposit_nonce` when available.
    pub fn draw(
        frame: &mut Frame,
        area: Rect,
        snapshot: &DepositLogSnapshot,
        last_deposit_nonce: Option<u64>,
        table_state: &mut TableState,
        is_focused: bool,
    ) {
        // Use a highlight border when the panel is focused.
        let border_style = if is_focused {
            Style::new().light_cyan()
        } else {
            Style::default()
        };

        // Include paging metadata in the title when rows are available.
        let title = if snapshot.total_count == 0 {
            "deposit log".to_string()
        } else {
            let shown = snapshot.rows.len();
            if shown as u64 >= snapshot.total_count {
                format!("deposit log (total {})", snapshot.total_count)
            } else {
                format!(
                    "deposit log (showing {} of {})",
                    shown, snapshot.total_count
                )
            }
        };

        // Draw the outer block and derive the inner render area.
        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(border_style);
        let inner = block.inner(area);
        frame.render_widget(block, area);

        // Show an empty-state hint if the log is empty.
        if snapshot.total_count == 0 {
            let empty =
                Paragraph::new(Line::from("no deposits in log")).style(Style::new().dark_gray());
            frame.render_widget(empty, inner);
            return;
        }

        // Build table rows from the snapshot, truncating long hex fields.
        let rows = snapshot.rows.iter().map(|row| {
            let nonce = row.nonce.to_string();
            let height = row.block_height.to_string();
            let amount = row.amount.to_string();
            let recipient = truncate_middle(&row.recipient_hex, 18);
            let tx_id = truncate_middle(&row.tx_id_base58, 28);

            let mut table_row = Row::new(vec![
                Cell::from(nonce),
                Cell::from(height),
                Cell::from(amount),
                Cell::from(recipient),
                Cell::from(tx_id),
            ]);

            // Highlight the last confirmed nonce for quick visual scanning.
            if Some(row.nonce) == last_deposit_nonce {
                table_row = table_row.style(Style::new().fg(Color::Green).bold());
            }

            table_row
        });

        // Header labels keep the layout stable with narrow numeric columns.
        let header = Row::new(vec![
            "nonce", "height", "amount", "recipient", "tx id (base58)",
        ])
        .style(Style::new().bold());

        // Render a compact table with a wider final column for tx ids.
        let table = Table::new(
            rows,
            [
                Constraint::Length(8),
                Constraint::Length(10),
                Constraint::Length(12),
                Constraint::Length(18),
                Constraint::Min(20),
            ],
        )
        .header(header)
        .column_spacing(1)
        .row_highlight_style(Style::new().reversed())
        .highlight_symbol("> ");

        frame.render_stateful_widget(table, inner, table_state);
    }
}

/// Truncate a long string by keeping both ends with a middle ellipsis.
fn truncate_middle(value: &str, max_len: usize) -> String {
    // Keep short values intact to avoid noisy truncation.
    if value.len() <= max_len {
        return value.to_string();
    }
    // If the budget is tiny, just return the left slice.
    if max_len <= 3 {
        return value[..max_len].to_string();
    }
    // Split the remaining space around a three-character ellipsis.
    let keep = max_len - 3;
    let left = keep / 2;
    let right = keep - left;
    let start = &value[..left];
    let end = &value[value.len().saturating_sub(right)..];
    format!("{}...{}", start, end)
}
