//! Health panel - displays peer node health status.
//!
//! This is a migration of the existing health table from tui.rs
//! into the new modular panel structure.

use ratatui::layout::Constraint;
use ratatui::style::{Color, Style, Stylize};
use ratatui::widgets::{Block, Borders, Cell, Row, Table, TableState};
use ratatui::Frame;

use crate::health::{NodeHealthSnapshot, NodeHealthStatus};

/// Health panel for displaying peer status.
pub struct HealthPanel;

impl HealthPanel {
    /// Draw the health table.
    pub fn draw(
        frame: &mut Frame,
        area: ratatui::layout::Rect,
        snapshots: &[NodeHealthSnapshot],
        table_state: &mut TableState,
        is_focused: bool,
    ) {
        let rows = snapshots.iter().map(|snapshot| {
            let status_cell = match &snapshot.status {
                NodeHealthStatus::Healthy => {
                    Cell::from("healthy").style(Style::new().light_green())
                }
                NodeHealthStatus::Unreachable { error } => {
                    Cell::from(error.clone()).style(Style::new().light_red())
                }
            };
            let latency = snapshot
                .latency_ms
                .map(|ms| format!("{ms} ms"))
                .unwrap_or_else(|| "—".into());
            let uptime = snapshot
                .peer_uptime_ms
                .map(|ms| format!("{:.1}s", ms as f64 / 1000.0))
                .unwrap_or_else(|| "—".into());
            let updated = format_relative(snapshot.last_updated);

            Row::new(vec![
                Cell::from(snapshot.node_id.to_string()),
                Cell::from(snapshot.address.clone()),
                status_cell,
                Cell::from(latency),
                Cell::from(uptime),
                Cell::from(updated),
            ])
            .style(row_style(&snapshot.status))
        });

        let border_style = if is_focused {
            Style::new().light_cyan()
        } else {
            Style::default()
        };

        let table = Table::new(
            rows,
            [
                Constraint::Length(8),
                Constraint::Length(22),
                Constraint::Length(18),
                Constraint::Length(12),
                Constraint::Length(12),
                Constraint::Min(10),
            ],
        )
        .header(
            Row::new(vec![
                "node", "address", "status", "latency", "uptime", "updated",
            ])
            .style(Style::new().bold()),
        )
        .block(
            Block::default()
                .title("peer status")
                .borders(Borders::ALL)
                .border_style(border_style),
        )
        .row_highlight_style(Style::new().reversed())
        .highlight_symbol("➤ ");

        frame.render_stateful_widget(table, area, table_state);
    }
}

fn row_style(status: &NodeHealthStatus) -> Style {
    match status {
        NodeHealthStatus::Healthy => Style::new().fg(Color::Green),
        NodeHealthStatus::Unreachable { .. } => Style::new().fg(Color::Red),
    }
}

fn format_relative(time: std::time::SystemTime) -> String {
    match time.elapsed() {
        Ok(duration) if duration.as_secs() > 0 => format!("{}s ago", duration.as_secs()),
        Ok(_) => "just now".into(),
        Err(_) => "–".into(),
    }
}
