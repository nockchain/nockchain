//! Proposal management panel.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Gauge, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use crate::tui::types::{format_nock_from_nicks, Proposal, ProposalState, ProposalStatus};

pub struct ProposalPanel;

impl ProposalPanel {
    pub fn draw(
        frame: &mut Frame,
        area: Rect,
        state: &ProposalState,
        list_state: &mut ListState,
        is_focused: bool,
    ) {
        Self::draw_with_detail(frame, area, state, list_state, is_focused, None);
    }

    pub fn draw_with_detail(
        frame: &mut Frame,
        area: Rect,
        state: &ProposalState,
        list_state: &mut ListState,
        is_focused: bool,
        detail_index: Option<usize>,
    ) {
        // If detail view is active, show detail instead of normal view
        if let Some(idx) = detail_index {
            if let Some(proposal) = state.history.get(idx) {
                Self::draw_detail_view(frame, area, proposal, is_focused);
                return;
            }
        }

        // Normal view (original draw logic)
        let border_style = if is_focused {
            Style::new().light_cyan()
        } else {
            Style::default()
        };

        let block = Block::default()
            .title("proposals")
            .borders(Borders::ALL)
            .border_style(border_style);

        let inner = block.inner(area);
        frame.render_widget(block, area);

        // Split layout: top section for active proposals, bottom for history
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(12),        // Active proposals section
                Constraint::Percentage(50), // History section
            ])
            .split(inner);

        Self::draw_active(frame, layout[0], state);
        Self::draw_history(frame, layout[1], state, list_state, is_focused);
    }

    fn draw_active(frame: &mut Frame, area: Rect, state: &ProposalState) {
        // Split into two sections: last submitted and pending inbound
        let sections = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(6), // Last submitted
                Constraint::Min(6),    // Pending inbound
            ])
            .split(area);

        Self::draw_last_submitted(frame, sections[0], &state.last_submitted);
        Self::draw_pending_inbound(frame, sections[1], &state.pending_inbound);
    }

    fn draw_last_submitted(frame: &mut Frame, area: Rect, proposal: &Option<Proposal>) {
        let mut lines = Vec::new();

        lines.push(Line::from(vec![Span::styled(
            "Last Submitted",
            Style::new().bold().underlined(),
        )]));

        if let Some(p) = proposal {
            // Proposal ID and status
            lines.push(Line::from(vec![
                Span::styled("  id: ", Style::new().dark_gray()),
                Span::styled(truncate_hash(&p.id, 16), Style::new().light_cyan()),
                Span::raw("  "),
                Span::styled(p.status.display(), status_style(&p.status)),
            ]));

            // Deposit details: amount, recipient, source block
            if p.amount.is_some() || p.recipient.is_some() {
                let mut deposit_line = vec![Span::raw("  ")];

                if let Some(amount) = p.amount {
                    deposit_line.push(Span::styled("amt: ", Style::new().dark_gray()));
                    deposit_line.push(Span::styled(
                        format_nock_amount(amount),
                        Style::new().light_green(),
                    ));
                }

                if let Some(ref recipient) = p.recipient {
                    deposit_line.push(Span::raw("  "));
                    deposit_line.push(Span::styled("to: ", Style::new().dark_gray()));
                    deposit_line.push(Span::styled(
                        truncate_hash(recipient, 14),
                        Style::new().light_yellow(),
                    ));
                }

                if let Some(nonce) = p.nonce {
                    deposit_line.push(Span::raw("  "));
                    deposit_line.push(Span::styled("nonce: ", Style::new().dark_gray()));
                    deposit_line.push(Span::styled(format!("{}", nonce), Style::new().white()));
                }

                lines.push(Line::from(deposit_line));
            }

            // Source block (where deposit was detected)
            if let Some(source_block) = p.source_block {
                lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled("src block: ", Style::new().dark_gray()),
                    Span::styled(format!("{}", source_block), Style::new().light_blue()),
                ]));
            }

            // Signature progress and timing info
            let mut sig_line = vec![
                Span::styled("  sigs: ", Style::new().dark_gray()),
                Span::styled(
                    format!("{}/{}", p.signatures_collected, p.signatures_required),
                    if p.is_ready() {
                        Style::new().light_green()
                    } else {
                        Style::new().light_yellow()
                    },
                ),
                Span::raw("  "),
                Span::styled("signers: ", Style::new().dark_gray()),
                Span::styled(format_signers(&p.signers), Style::new().light_cyan()),
            ];

            // Add time-to-submit if available
            if let Some(ms) = p.time_to_submit_ms {
                sig_line.push(Span::raw("  "));
                sig_line.push(Span::styled(
                    format!("latency: {}ms", ms),
                    Style::new().light_magenta(),
                ));
            }

            lines.push(Line::from(sig_line));

            // Turn state: show proposer and turn status
            if let Some(proposer) = p.current_proposer {
                let mut turn_line = vec![
                    Span::styled("  proposer: ", Style::new().dark_gray()),
                    Span::styled(format!("#{}", proposer), Style::new().light_cyan()),
                ];

                if p.is_my_turn {
                    turn_line.push(Span::raw("  "));
                    turn_line.push(Span::styled(
                        "⚡ YOUR TURN TO POST",
                        Style::new().light_green().bold(),
                    ));
                } else {
                    turn_line.push(Span::raw("  "));
                    turn_line.push(Span::styled("waiting", Style::new().dark_gray()));
                }

                // Show time until takeover if applicable
                if let Some(duration) = p.time_until_takeover {
                    let secs = duration.as_secs();
                    turn_line.push(Span::raw("  "));
                    turn_line.push(Span::styled("takeover: ", Style::new().dark_gray()));
                    turn_line.push(Span::styled(
                        format_duration(duration),
                        if secs < 30 {
                            Style::new().light_yellow().bold()
                        } else {
                            Style::new().light_blue()
                        },
                    ));
                }

                lines.push(Line::from(turn_line));
            }

            // Submission block and tx hash (if submitted/executed)
            if p.submitted_at_block.is_some() || p.tx_hash.is_some() {
                let mut block_line = vec![Span::raw("  ")];

                if let Some(block) = p.submitted_at_block {
                    block_line.push(Span::styled("submit block: ", Style::new().dark_gray()));
                    block_line.push(Span::styled(
                        format!("{}", block),
                        Style::new().light_green(),
                    ));
                }

                if let Some(ref tx_hash) = p.tx_hash {
                    if p.submitted_at_block.is_some() {
                        block_line.push(Span::raw("  "));
                    }
                    block_line.push(Span::styled("tx: ", Style::new().dark_gray()));
                    block_line.push(Span::styled(
                        truncate_hash(tx_hash, 18),
                        Style::new().light_cyan(),
                    ));
                }

                lines.push(Line::from(block_line));
            }

            // Draw progress bar
            let progress = p.signature_progress();
            let gauge = Gauge::default()
                .gauge_style(gauge_style_for_status(&p.status))
                .ratio(progress)
                .label(format!(
                    "{}/{} {}",
                    p.signatures_collected,
                    p.signatures_required,
                    p.status.display()
                ));

            // Adjust gauge position based on whether we have block info
            let gauge_y_offset = if p.submitted_at_block.is_some() || p.tx_hash.is_some() {
                5
            } else {
                4
            };

            let gauge_area = Rect {
                x: area.x + 2,
                y: area.y + gauge_y_offset,
                width: area.width.saturating_sub(4),
                height: 1,
            };

            let paragraph = Paragraph::new(lines);
            frame.render_widget(paragraph, area);
            frame.render_widget(gauge, gauge_area);
        } else {
            lines.push(Line::from(vec![Span::styled(
                "  (none)",
                Style::new().dark_gray(),
            )]));
            let paragraph = Paragraph::new(lines);
            frame.render_widget(paragraph, area);
        }
    }

    fn draw_pending_inbound(frame: &mut Frame, area: Rect, proposals: &[Proposal]) {
        let mut lines = Vec::new();

        lines.push(Line::from(vec![
            Span::styled("Pending Inbound", Style::new().bold().underlined()),
            Span::raw("  "),
            Span::styled(
                format!("({})", proposals.len()),
                if proposals.is_empty() {
                    Style::new().dark_gray()
                } else {
                    Style::new().light_yellow()
                },
            ),
        ]));

        if proposals.is_empty() {
            lines.push(Line::from(vec![Span::styled(
                "  (none)",
                Style::new().dark_gray(),
            )]));
        } else {
            // Show up to 3 pending proposals
            for (idx, p) in proposals.iter().take(3).enumerate() {
                if idx > 0 {
                    lines.push(Line::from(""));
                }

                // Line 1: index, truncated ID
                lines.push(Line::from(vec![
                    Span::styled(format!("  [{}] ", idx + 1), Style::new().dark_gray()),
                    Span::styled(truncate_hash(&p.id, 16), Style::new().light_cyan()),
                ]));

                // Line 2: deposit details (amount, recipient, source block)
                if p.amount.is_some() || p.source_block.is_some() {
                    let mut deposit_parts = vec![Span::raw("    ")];

                    if let Some(amount) = p.amount {
                        deposit_parts.push(Span::styled(
                            format_nock_amount(amount),
                            Style::new().light_green(),
                        ));
                    }

                    if let Some(ref recipient) = p.recipient {
                        deposit_parts.push(Span::styled(" → ", Style::new().dark_gray()));
                        deposit_parts.push(Span::styled(
                            truncate_hash(recipient, 14),
                            Style::new().light_yellow(),
                        ));
                    }

                    if let Some(source_block) = p.source_block {
                        deposit_parts.push(Span::raw("  "));
                        deposit_parts.push(Span::styled("src: ", Style::new().dark_gray()));
                        deposit_parts.push(Span::styled(
                            format!("{}", source_block),
                            Style::new().light_blue(),
                        ));
                    }

                    lines.push(Line::from(deposit_parts));
                }

                // Line 3: signature progress
                lines.push(Line::from(vec![
                    Span::raw("    "),
                    Span::styled("sigs: ", Style::new().dark_gray()),
                    Span::styled(
                        format!("{}/{}", p.signatures_collected, p.signatures_required),
                        if p.is_ready() {
                            Style::new().light_green()
                        } else {
                            Style::new().light_yellow()
                        },
                    ),
                    Span::raw("  "),
                    Span::styled("signers: ", Style::new().dark_gray()),
                    Span::styled(format_signers(&p.signers), Style::new().light_cyan()),
                ]));

                // Line 4: Turn state (if ready and proposer assigned)
                if p.is_ready() {
                    if let Some(proposer) = p.current_proposer {
                        let mut turn_parts = vec![
                            Span::raw("    "),
                            Span::styled("proposer: ", Style::new().dark_gray()),
                            Span::styled(format!("#{}", proposer), Style::new().light_cyan()),
                        ];

                        if p.is_my_turn {
                            turn_parts.push(Span::raw("  "));
                            turn_parts.push(Span::styled(
                                "⚡ POST NOW",
                                Style::new().light_green().bold(),
                            ));
                        }

                        if let Some(duration) = p.time_until_takeover {
                            turn_parts.push(Span::raw("  "));
                            turn_parts.push(Span::styled(
                                format!("takeover: {}", format_duration(duration)),
                                Style::new().light_blue(),
                            ));
                        }

                        lines.push(Line::from(turn_parts));
                    }
                }
            }

            if proposals.len() > 3 {
                lines.push(Line::from(""));
                lines.push(Line::from(vec![Span::styled(
                    format!("  ... and {} more", proposals.len() - 3),
                    Style::new().dark_gray().italic(),
                )]));
            }
        }

        let paragraph = Paragraph::new(lines);
        frame.render_widget(paragraph, area);
    }

    fn draw_history(
        frame: &mut Frame,
        area: Rect,
        state: &ProposalState,
        list_state: &mut ListState,
        is_focused: bool,
    ) {
        let block = Block::default()
            .title("history")
            .borders(Borders::TOP)
            .border_style(if is_focused {
                Style::new().light_cyan()
            } else {
                Style::default()
            });

        if state.history.is_empty() {
            let paragraph = Paragraph::new(vec![Line::from(vec![Span::styled(
                "  (no history)",
                Style::new().dark_gray(),
            )])])
            .block(block);
            frame.render_widget(paragraph, area);
            return;
        }

        let items: Vec<ListItem> = state
            .history
            .iter()
            .map(|p| {
                let status_style = status_style(&p.status);
                let status_indicator = status_indicator(&p.status);

                // Line 1: status indicator, truncated ID, status
                let line1 = Line::from(vec![
                    Span::styled(status_indicator, status_style),
                    Span::raw(" "),
                    Span::styled(truncate_hash(&p.id, 16), Style::new().light_cyan()),
                    Span::raw("  "),
                    Span::styled(p.status.display(), status_style),
                    Span::raw("  "),
                    Span::styled(format_relative(p.created_at), Style::new().dark_gray()),
                ]);

                let mut lines = vec![line1];

                // Line 2: deposit details (amount, recipient, nonce, source block)
                if p.amount.is_some() || p.source_block.is_some() {
                    let mut deposit_parts = vec![Span::raw("  ")];

                    if let Some(amount) = p.amount {
                        deposit_parts.push(Span::styled(
                            format_nock_amount(amount),
                            Style::new().light_green(),
                        ));
                    }

                    if let Some(ref recipient) = p.recipient {
                        deposit_parts.push(Span::styled(" → ", Style::new().dark_gray()));
                        deposit_parts.push(Span::styled(
                            truncate_hash(recipient, 14),
                            Style::new().light_yellow(),
                        ));
                    }

                    if let Some(source_block) = p.source_block {
                        deposit_parts.push(Span::raw("  "));
                        deposit_parts.push(Span::styled("src: ", Style::new().dark_gray()));
                        deposit_parts.push(Span::styled(
                            format!("{}", source_block),
                            Style::new().light_blue(),
                        ));
                    }

                    if let Some(nonce) = p.nonce {
                        deposit_parts.push(Span::raw("  "));
                        deposit_parts.push(Span::styled("n:", Style::new().dark_gray()));
                        deposit_parts
                            .push(Span::styled(format!("{}", nonce), Style::new().white()));
                    }

                    lines.push(Line::from(deposit_parts));
                }

                // Line 3: signature info
                let mut sig_parts = vec![
                    Span::raw("  "),
                    Span::styled(
                        format!("{}/{} sigs", p.signatures_collected, p.signatures_required),
                        if p.is_ready() {
                            Style::new().light_green()
                        } else {
                            Style::new().light_yellow()
                        },
                    ),
                    Span::raw("  "),
                    Span::styled("signers: ", Style::new().dark_gray()),
                    Span::styled(format_signers(&p.signers), Style::new().light_cyan()),
                ];

                // Add latency if available
                if let Some(ms) = p.time_to_submit_ms {
                    sig_parts.push(Span::raw("  "));
                    sig_parts.push(Span::styled(
                        format!("{}ms", ms),
                        Style::new().light_magenta(),
                    ));
                }

                lines.push(Line::from(sig_parts));

                // Line 4: submission block and tx hash (if submitted/executed)
                if p.submitted_at_block.is_some() || p.tx_hash.is_some() {
                    let mut submit_parts = vec![Span::raw("  ")];

                    if let Some(block) = p.submitted_at_block {
                        submit_parts.push(Span::styled("submit: ", Style::new().dark_gray()));
                        submit_parts.push(Span::styled(
                            format!("{}", block),
                            Style::new().light_green(),
                        ));
                    }

                    if let Some(ref tx_hash) = p.tx_hash {
                        if p.submitted_at_block.is_some() {
                            submit_parts.push(Span::raw("  "));
                        }
                        submit_parts.push(Span::styled("tx: ", Style::new().dark_gray()));
                        submit_parts.push(Span::styled(
                            truncate_hash(tx_hash, 18),
                            Style::new().light_cyan(),
                        ));
                    }

                    lines.push(Line::from(submit_parts));
                }

                ListItem::new(lines)
            })
            .collect();

        let list = List::new(items)
            .block(block)
            .highlight_style(Style::new().reversed())
            .highlight_symbol("➤ ");

        frame.render_stateful_widget(list, area, list_state);
    }

    /// Draw detailed view of a single proposal.
    #[allow(clippy::vec_init_then_push)]
    fn draw_detail_view(frame: &mut Frame, area: Rect, proposal: &Proposal, is_focused: bool) {
        let border_style = if is_focused {
            Style::new().light_cyan()
        } else {
            Style::default()
        };

        let block = Block::default()
            .title("proposal details (press Enter to close)")
            .borders(Borders::ALL)
            .border_style(border_style);

        let inner = block.inner(area);
        frame.render_widget(block, area);

        let mut lines = Vec::new();

        // Header: ID and status
        lines.push(Line::from(vec![
            Span::styled("Proposal ID: ", Style::new().bold()),
            Span::styled(&proposal.id, Style::new().light_cyan()),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Status: ", Style::new().bold()),
            Span::styled(proposal.status.display(), status_style(&proposal.status)),
        ]));
        lines.push(Line::from(""));

        // Type and description
        lines.push(Line::from(vec![
            Span::styled("Type: ", Style::new().bold()),
            Span::styled(&proposal.proposal_type, Style::new().light_yellow()),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Description: ", Style::new().bold()),
            Span::raw(&proposal.description),
        ]));
        lines.push(Line::from(""));

        // Deposit details (if applicable)
        if proposal.amount.is_some()
            || proposal.recipient.is_some()
            || proposal.source_block.is_some()
        {
            lines.push(Line::from(Span::styled(
                "Deposit Details:",
                Style::new().bold().underlined(),
            )));

            if let Some(amount) = proposal.amount {
                lines.push(Line::from(vec![
                    Span::raw("  Amount: "),
                    Span::styled(format_nock_amount(amount), Style::new().light_green()),
                ]));
            }

            if let Some(ref recipient) = proposal.recipient {
                lines.push(Line::from(vec![
                    Span::raw("  Recipient: "),
                    Span::styled(recipient, Style::new().light_yellow()),
                ]));
            }

            if let Some(nonce) = proposal.nonce {
                lines.push(Line::from(vec![
                    Span::raw("  Nonce: "),
                    Span::styled(format!("{}", nonce), Style::new().white()),
                ]));
            }

            if let Some(source_block) = proposal.source_block {
                lines.push(Line::from(vec![
                    Span::raw("  Source Block: "),
                    Span::styled(format!("{}", source_block), Style::new().light_blue()),
                ]));
            }

            if let Some(ref source_tx_id) = proposal.source_tx_id {
                lines.push(Line::from(vec![
                    Span::raw("  Source Tx ID: "),
                    Span::styled(source_tx_id, Style::new().light_cyan()),
                ]));
            }

            lines.push(Line::from(""));
        }

        // Signature information
        lines.push(Line::from(Span::styled(
            "Signature Information:",
            Style::new().bold().underlined(),
        )));
        lines.push(Line::from(vec![
            Span::raw("  Collected: "),
            Span::styled(
                format!("{}", proposal.signatures_collected),
                if proposal.is_ready() {
                    Style::new().light_green()
                } else {
                    Style::new().light_yellow()
                },
            ),
            Span::raw(" / "),
            Span::styled(
                format!("{}", proposal.signatures_required),
                Style::new().white(),
            ),
        ]));

        lines.push(Line::from(vec![
            Span::raw("  Signers: "),
            Span::styled(format_signers(&proposal.signers), Style::new().light_cyan()),
        ]));

        // Detailed signer list if we have them
        if !proposal.signers.is_empty() {
            lines.push(Line::from("  Signer IDs:"));
            for signer in &proposal.signers {
                lines.push(Line::from(vec![
                    Span::raw("    • Node #"),
                    Span::styled(format!("{}", signer), Style::new().light_cyan()),
                ]));
            }
        }

        lines.push(Line::from(""));

        // Turn-based proposal state
        if proposal.current_proposer.is_some() || proposal.is_my_turn {
            lines.push(Line::from(Span::styled(
                "Turn State:",
                Style::new().bold().underlined(),
            )));

            if let Some(proposer) = proposal.current_proposer {
                lines.push(Line::from(vec![
                    Span::raw("  Current Proposer: Node #"),
                    Span::styled(format!("{}", proposer), Style::new().light_cyan()),
                ]));
            }

            if proposal.is_my_turn {
                lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(
                        "⚡ YOUR TURN TO POST THIS PROPOSAL",
                        Style::new().light_green().bold(),
                    ),
                ]));
            }

            if let Some(duration) = proposal.time_until_takeover {
                lines.push(Line::from(vec![
                    Span::raw("  Time Until Takeover: "),
                    Span::styled(
                        format_duration(duration),
                        if duration.as_secs() < 30 {
                            Style::new().light_yellow().bold()
                        } else {
                            Style::new().light_blue()
                        },
                    ),
                ]));
            }

            lines.push(Line::from(""));
        }

        // Submission details
        if proposal.submitted_at_block.is_some()
            || proposal.tx_hash.is_some()
            || proposal.executed_at_block.is_some()
        {
            lines.push(Line::from(Span::styled(
                "Submission Details:",
                Style::new().bold().underlined(),
            )));

            if let Some(block) = proposal.submitted_at_block {
                lines.push(Line::from(vec![
                    Span::raw("  Submitted at Block: "),
                    Span::styled(format!("{}", block), Style::new().light_green()),
                ]));
            }

            if let Some(ref tx_hash) = proposal.tx_hash {
                lines.push(Line::from(vec![
                    Span::raw("  Transaction Hash: "),
                    Span::styled(tx_hash, Style::new().light_cyan()),
                ]));
            }

            if let Some(block) = proposal.executed_at_block {
                lines.push(Line::from(vec![
                    Span::raw("  Executed at Block: "),
                    Span::styled(format!("{}", block), Style::new().light_green()),
                ]));
            }

            lines.push(Line::from(""));
        }

        // Timing information
        lines.push(Line::from(Span::styled(
            "Timing:",
            Style::new().bold().underlined(),
        )));
        lines.push(Line::from(vec![
            Span::raw("  Created: "),
            Span::styled(
                format_relative(proposal.created_at),
                Style::new().dark_gray(),
            ),
        ]));

        if let Some(ref submitted_at) = proposal.submitted_at {
            lines.push(Line::from(vec![
                Span::raw("  Submitted: "),
                Span::styled(format_relative(*submitted_at), Style::new().dark_gray()),
            ]));
        }

        if let Some(ms) = proposal.time_to_submit_ms {
            lines.push(Line::from(vec![
                Span::raw("  Latency: "),
                Span::styled(format!("{}ms", ms), Style::new().light_magenta()),
            ]));
        }

        lines.push(Line::from(""));

        // Data hash
        lines.push(Line::from(Span::styled(
            "Data Hash:",
            Style::new().bold().underlined(),
        )));
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(&proposal.data_hash, Style::new().light_cyan()),
        ]));

        // Render the paragraph
        let paragraph = Paragraph::new(lines);
        frame.render_widget(paragraph, inner);
    }
}

fn gauge_style_for_status(status: &ProposalStatus) -> Style {
    match status {
        ProposalStatus::Pending => Style::new().light_yellow(),
        ProposalStatus::Ready => Style::new().light_green(),
        ProposalStatus::Submitted => Style::new().light_cyan(),
        ProposalStatus::Executed => Style::new().light_green(),
        ProposalStatus::Expired => Style::new().dark_gray(),
        ProposalStatus::Failed { .. } => Style::new().light_red(),
    }
}

fn status_style(status: &ProposalStatus) -> Style {
    match status {
        ProposalStatus::Pending => Style::new().light_yellow(),
        ProposalStatus::Ready => Style::new().light_green(),
        ProposalStatus::Submitted => Style::new().light_cyan(),
        ProposalStatus::Executed => Style::new().light_green(),
        ProposalStatus::Expired => Style::new().dark_gray(),
        ProposalStatus::Failed { .. } => Style::new().light_red(),
    }
}

fn status_indicator(status: &ProposalStatus) -> &'static str {
    match status {
        ProposalStatus::Pending => "◌",
        ProposalStatus::Ready => "◉",
        ProposalStatus::Submitted => "⏳",
        ProposalStatus::Executed => "✓",
        ProposalStatus::Expired => "⊗",
        ProposalStatus::Failed { .. } => "✗",
    }
}

fn format_signers(signers: &[u64]) -> String {
    if signers.is_empty() {
        return "none".to_string();
    }
    if signers.len() <= 3 {
        signers
            .iter()
            .map(|id| id.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    } else {
        format!(
            "{}, ... +{}",
            signers[..3]
                .iter()
                .map(|id| id.to_string())
                .collect::<Vec<_>>()
                .join(", "),
            signers.len() - 3
        )
    }
}

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

fn truncate_hash(hash: &str, max_len: usize) -> String {
    if hash.len() <= max_len {
        return hash.to_string();
    }
    let prefix_len = (max_len - 3) / 2;
    let suffix_len = max_len - 3 - prefix_len;
    format!(
        "{}...{}",
        &hash[..prefix_len],
        &hash[hash.len() - suffix_len..]
    )
}

fn format_nock_amount(amount: u128) -> String {
    format!("{} NOCK", format_nock_from_nicks(amount))
}

fn format_duration(duration: std::time::Duration) -> String {
    let secs = duration.as_secs();
    if secs == 0 {
        "now".into()
    } else if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    }
}
