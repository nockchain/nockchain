//! Status bar widget - shows mode and panel indicators.

use ratatui::layout::Rect;
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crate::tui::types::{FocusedPanel, UiMode};

/// Status bar showing current mode and panel.
pub struct StatusBar;

impl StatusBar {
    /// Draw the status bar.
    pub fn draw(frame: &mut Frame, area: Rect, mode: UiMode, focused_panel: FocusedPanel) {
        let mut spans = Vec::new();

        // Mode indicator
        let (mode_text, mode_style) = match mode {
            UiMode::Normal => ("NORMAL", Style::new().light_green()),
            UiMode::Help => ("HELP", Style::new().light_cyan()),
        };
        spans.push(Span::styled(
            format!(" {} ", mode_text),
            mode_style.reversed(),
        ));
        spans.push(Span::raw("  "));

        // Panel indicator
        spans.push(Span::styled("panel: ", Style::new().dark_gray()));
        spans.push(Span::styled(
            focused_panel.display(),
            Style::new().light_cyan(),
        ));
        spans.push(Span::raw("  "));

        // Help hint
        spans.push(Span::styled("? help", Style::new().dark_gray()));

        let line = Line::from(spans);
        let paragraph = Paragraph::new(line)
            .wrap(Wrap { trim: true })
            .block(Block::default().borders(Borders::ALL));

        frame.render_widget(paragraph, area);
    }

    /// Draw combined status bar with mode indicator, panel, shortcuts, AND optional status message.
    ///
    /// If `status_message` is provided, it replaces the shortcuts section with the message
    /// displayed prominently. This is used for temporary feedback like "Copied to clipboard!".
    pub fn draw_full(frame: &mut Frame, area: Rect, mode: UiMode, focused_panel: FocusedPanel) {
        Self::draw_full_with_status(frame, area, mode, focused_panel, None);
    }

    /// Draw combined status bar with optional status message.
    ///
    /// When `status_message` is Some, it replaces the shortcuts with the message.
    pub fn draw_full_with_status(
        frame: &mut Frame,
        area: Rect,
        mode: UiMode,
        focused_panel: FocusedPanel,
        status_message: Option<String>,
    ) {
        let mut spans = Vec::new();

        // Mode indicator
        let (mode_text, mode_style) = match mode {
            UiMode::Normal => ("NORMAL", Style::new().light_green()),
            UiMode::Help => ("HELP", Style::new().light_cyan()),
        };
        spans.push(Span::styled(
            format!(" {} ", mode_text),
            mode_style.reversed(),
        ));
        spans.push(Span::raw(" "));

        // Panel indicator
        spans.push(Span::styled(
            focused_panel.display(),
            Style::new().light_cyan(),
        ));
        spans.push(Span::raw("  "));

        // Separator
        spans.push(Span::styled("│", Style::new().dark_gray()));
        spans.push(Span::raw(" "));

        // Status message OR shortcuts
        if let Some(msg) = status_message {
            // Show status message prominently
            spans.push(Span::styled(msg, Style::new().light_green().bold()));
        } else {
            // Show shortcuts for current panel
            let shortcuts = Self::shortcuts_for_panel(focused_panel);
            for (idx, (key, desc)) in shortcuts.iter().enumerate() {
                if idx > 0 {
                    spans.push(Span::raw("  "));
                }
                spans.push(Span::styled(*key, Style::new().light_yellow()));
                spans.push(Span::raw(" "));
                spans.push(Span::styled(*desc, Style::new().dark_gray()));
            }
        }

        let line = Line::from(spans);
        let paragraph = Paragraph::new(line)
            .wrap(Wrap { trim: true })
            .block(Block::default().borders(Borders::ALL));

        frame.render_widget(paragraph, area);
    }

    /// Get shortcuts for a panel.
    fn shortcuts_for_panel(focused_panel: FocusedPanel) -> Vec<(&'static str, &'static str)> {
        match focused_panel {
            FocusedPanel::Health => {
                vec![("↑↓", "nav"), ("r", "refresh"), ("?", "help")]
            }
            FocusedPanel::Proposals => {
                vec![("↑↓", "nav"), ("Enter", "details"), ("?", "help")]
            }
            FocusedPanel::Transactions => {
                vec![("↑↓", "nav"), ("y", "copy"), ("Enter", "details"), ("?", "help")]
            }
            FocusedPanel::Alerts => {
                vec![("↑↓", "nav"), ("?", "help")]
            }
            FocusedPanel::DepositLog => {
                vec![("↑↓", "scroll"), ("J/K", "scroll"), ("y", "copy"), ("?", "help")]
            }
        }
    }

    /// Draw keyboard shortcuts for the current panel (legacy, use draw_full instead).
    pub fn draw_shortcuts(frame: &mut Frame, area: Rect, focused_panel: FocusedPanel) {
        let shortcuts = Self::shortcuts_for_panel(focused_panel);

        let mut spans = Vec::new();
        for (idx, (key, desc)) in shortcuts.iter().enumerate() {
            if idx > 0 {
                spans.push(Span::raw("   "));
            }
            spans.push(Span::styled(*key, Style::new().light_yellow()));
            spans.push(Span::raw(" "));
            spans.push(Span::styled(*desc, Style::new().dark_gray()));
        }

        let line = Line::from(spans);
        let paragraph = Paragraph::new(line)
            .wrap(Wrap { trim: true })
            .block(Block::default().borders(Borders::ALL));

        frame.render_widget(paragraph, area);
    }
}
