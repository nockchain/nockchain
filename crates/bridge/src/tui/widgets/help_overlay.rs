//! Help overlay widget - shows keybindings.

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

/// Help overlay showing all keybindings.
pub struct HelpOverlay;

impl HelpOverlay {
    /// Draw the help overlay centered on screen.
    pub fn draw(frame: &mut Frame) {
        let area = frame.area();

        // Calculate centered overlay area (60% width, 70% height)
        let overlay_width = (area.width as f32 * 0.6) as u16;
        let overlay_height = (area.height as f32 * 0.7) as u16;

        let overlay_area = centered_rect(overlay_width, overlay_height, area);

        // Clear the background
        frame.render_widget(Clear, overlay_area);

        let block = Block::default()
            .title(" Help (press ? or Esc to close) ")
            .borders(Borders::ALL)
            .border_style(Style::new().light_cyan());

        let inner = block.inner(overlay_area);
        frame.render_widget(block, overlay_area);

        let sections = vec![
            (
                "Navigation",
                vec![
                    ("h / ←", "Previous panel"),
                    ("l / →", "Next panel"),
                    ("j / ↓", "Move down / scroll"),
                    ("k / ↑", "Move up / scroll"),
                    ("Tab", "Cycle panels"),
                    ("1-5", "Jump to panel"),
                ],
            ),
            (
                "Panels",
                vec![
                    ("d", "Deposit log"),
                    ("p", "Proposals"),
                    ("t", "Transactions"),
                    ("a", "Alerts"),
                ],
            ),
            (
                "Actions",
                vec![
                    ("r", "Refresh"),
                    ("y", "Copy selected item"),
                    ("Enter", "Expand details"),
                    ("q", "Quit"),
                ],
            ),
        ];

        let mut lines = Vec::new();

        for (section_name, bindings) in sections {
            lines.push(Line::from(vec![Span::styled(
                section_name,
                Style::new().bold().light_yellow(),
            )]));
            lines.push(Line::from(""));

            for (key, desc) in bindings {
                lines.push(Line::from(vec![
                    Span::styled(format!("  {:12}", key), Style::new().light_cyan()),
                    Span::raw(desc),
                ]));
            }

            lines.push(Line::from(""));
        }

        let paragraph = Paragraph::new(lines)
            .wrap(Wrap { trim: true })
            .alignment(Alignment::Left);

        frame.render_widget(paragraph, inner);
    }
}

/// Create a centered rectangle.
fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length((area.height.saturating_sub(height)) / 2),
            Constraint::Length(height),
            Constraint::Min(0),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length((area.width.saturating_sub(width)) / 2),
            Constraint::Length(width),
            Constraint::Min(0),
        ])
        .split(vertical[1])[1]
}
