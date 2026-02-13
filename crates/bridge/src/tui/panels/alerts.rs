//! Alerts panel - displays recent alerts.
//!
//! Shows a list of alerts with severity indicators and timestamps.

use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Style, Stylize};
use ratatui::widgets::{Block, Borders, Cell, Row, Table, TableState};
use ratatui::Frame;

use crate::tui::types::{Alert, AlertSeverity, AlertState};

/// Alerts panel for displaying alerts.
pub struct AlertPanel;

impl AlertPanel {
    /// Draw the alerts panel showing alerts.
    pub fn draw(
        frame: &mut Frame,
        area: Rect,
        alert_state: &AlertState,
        table_state: &mut TableState,
        is_focused: bool,
    ) {
        let mut rows = Vec::new();

        for alert in &alert_state.alerts {
            rows.push(Self::make_row(alert));
        }

        let border_style = if is_focused {
            Style::new().light_cyan()
        } else {
            Style::default()
        };

        let title = format!("alerts ({})", alert_state.alerts.len());

        let table = Table::new(
            rows,
            [
                Constraint::Length(3),  // severity icon
                Constraint::Length(22), // title
                Constraint::Min(28),    // message
                Constraint::Length(15), // timestamp
                Constraint::Length(10), // source
            ],
        )
        .header(Row::new(vec!["", "title", "message", "time", "source"]).style(Style::new().bold()))
        .block(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(border_style),
        )
        .row_highlight_style(Style::new().reversed())
        .highlight_symbol("➤ ");

        frame.render_stateful_widget(table, area, table_state);
    }

    fn make_row(alert: &Alert) -> Row<'static> {
        let severity_style = match alert.severity {
            AlertSeverity::Critical => Style::new().light_red().bold(),
            AlertSeverity::Error => Style::new().light_red(),
            AlertSeverity::Warning => Style::new().light_yellow(),
            AlertSeverity::Info => Style::new().light_cyan(),
        };

        let severity_cell = Cell::from(alert.severity.symbol()).style(severity_style);

        let title_cell = Cell::from(alert.title.clone()).style(severity_style);

        let message_cell = Cell::from(alert.message.clone());

        let time_str = Self::format_timestamp(alert.timestamp);
        let time_cell = Cell::from(time_str).style(Style::new().dark_gray());

        let source_cell = Cell::from(alert.source.clone()).style(Style::new().dark_gray());

        Row::new(vec![
            severity_cell, title_cell, message_cell, time_cell, source_cell,
        ])
        .style(severity_style)
    }

    fn format_timestamp(time: std::time::SystemTime) -> String {
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
}
