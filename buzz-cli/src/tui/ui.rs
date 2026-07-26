use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect, Alignment},
    style::{Style, Color, Modifier},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Clear, Wrap},
};
use crate::tui::app::{App, MessageRole};

pub fn render(f: &mut Frame, app: &App) {
    let chunks = Layout::new(
        Direction::Vertical,
        [
            Constraint::Min(10),     // Chat history
            Constraint::Length(3),   // Input box
            Constraint::Length(1),   // Status bar
        ]
    ).split(f.size());

    render_chat(f, app, chunks[0]);
    render_input(f, app, chunks[1]);
    render_status(f, app, chunks[2]);

    if app.show_help {
        render_help_overlay(f, chunks[1]);
    }
}

fn render_chat(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(
            " Conversation ",
            Style::default().fg(Color::Gray).add_modifier(Modifier::BOLD)
        ));

    let mut lines: Vec<Line> = Vec::new();
    
    // Apply scroll offset
    let start_idx = app.scroll_offset.min(app.messages.len());
    let visible: Vec<&crate::tui::app::Message> = app.messages.iter().skip(start_idx).collect();

    for msg in &visible {
        match msg.role {
            MessageRole::System => {
                lines.push(Line::from(vec![
                    Span::styled("✓ ", Style::default().fg(Color::DarkGray)),
                    Span::styled("System: ", Style::default().fg(Color::DarkGray).add_modifier(Modifier::BOLD)),
                    Span::styled(&msg.content, Style::default().fg(Color::DarkGray)),
                ]));
            }
            MessageRole::User => {
                lines.push(Line::from(vec![
                    Span::styled("You: ", Style::default().fg(Color::Rgb(196, 168, 130)).add_modifier(Modifier::BOLD)),
                ]));
                for content_line in msg.content.lines() {
                    lines.push(Line::from(Span::styled(
                        format!("  {}", content_line),
                        Style::default().fg(Color::Gray),
                    )));
                }
            }
            MessageRole::Assistant => {
                let provider_str = format!("[{}] ", msg.provider);
                lines.push(Line::from(vec![
                    Span::styled(provider_str, Style::default().fg(Color::Rgb(122, 162, 158)).add_modifier(Modifier::BOLD)),
                ]));
                for content_line in msg.content.lines() {
                    lines.push(Line::from(Span::styled(
                        format!("  {}", content_line),
                        Style::default().fg(Color::Gray),
                    )));
                }
            }
            MessageRole::Error => {
                lines.push(Line::from(vec![
                    Span::styled("✗ ", Style::default().fg(Color::Red)),
                    Span::styled(&msg.content, Style::default().fg(Color::Red)),
                ]));
            }
        }
    }

    if app.is_generating {
        let elapsed = app.gen_start.map(|s| s.elapsed().as_secs()).unwrap_or(0);
        lines.push(Line::from(vec![
            Span::styled("● ", Style::default().fg(Color::Rgb(196, 168, 130))),
            Span::styled(format!("{} thinking... ({}s)", app.provider_name, elapsed), Style::default().fg(Color::Rgb(196, 168, 130))),
        ]));
    }

    if visible.is_empty() && !app.is_generating {
        lines.push(Line::from(Span::styled(
            "  No messages yet. Type below to begin...",
            Style::default().fg(Color::DarkGray),
        )));
    }

    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false });

    f.render_widget(paragraph, area);
}

fn render_input(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(if app.is_generating { Color::Rgb(196, 168, 130) } else { Color::Gray }))
        .title(Span::styled(
            " Message ",
            Style::default().fg(Color::Gray).add_modifier(Modifier::BOLD)
        ));

    let cursor = "▋";
    let text = format!("{}{}", app.input_buffer, cursor);
    
    let paragraph = Paragraph::new(text)
        .block(block)
        .style(Style::default().fg(Color::Gray));

    f.render_widget(paragraph, area);
}

fn render_status(f: &mut Frame, app: &App, area: Rect) {
    let pct = if app.daily_budget_usd > 0.0 {
        (app.current_spend_usd / app.daily_budget_usd * 100.0).min(100.0)
    } else {
        0.0
    };

    let spend_color = if pct > 80.0 { Color::Red } else if pct > 50.0 { Color::Rgb(196, 168, 130) } else { Color::DarkGray };

    let scroll_pos = if app.messages.len() > 0 {
        format!("{}/{}", app.scroll_offset + 1, app.messages.len())
    } else {
        "0/0".to_string()
    };

    let line = Line::from(vec![
        Span::styled(format!(" ${:.6}/${:.2} ", app.current_spend_usd, app.daily_budget_usd), Style::default().fg(spend_color)),
        Span::raw("·"),
        Span::styled(format!(" {} tokens ", app.total_tokens), Style::default().fg(Color::DarkGray)),
        Span::raw("·"),
        Span::styled(format!(" {} ", app.provider_name), Style::default().fg(Color::Rgb(122, 162, 158))),
        Span::raw("·"),
        Span::styled(if app.is_generating { " generating " } else { " ready " }, 
            Style::default().fg(if app.is_generating { Color::Rgb(196, 168, 130) } else { Color::DarkGray })),
        Span::raw("·"),
        Span::styled(format!(" ↑↓:{} ", scroll_pos), Style::default().fg(Color::DarkGray)),
        Span::raw("·"),
        Span::styled("/help · F1 · Ctrl+C quit", Style::default().fg(Color::DarkGray)),
    ]);

    f.render_widget(Paragraph::new(line), area);
}

fn render_help_overlay(f: &mut Frame, input_area: Rect) {
    let help_lines = vec![
        Line::from(vec![
            Span::styled("/provider ", Style::default().fg(Color::Rgb(122, 162, 158))),
            Span::raw("groq|anthropic|gemini|hf|local"),
        ]),
        Line::from(vec![
            Span::styled("/reset  ", Style::default().fg(Color::Rgb(122, 162, 158))),
            Span::styled("/stats  ", Style::default().fg(Color::Rgb(122, 162, 158))),
            Span::styled("/quit", Style::default().fg(Color::Rgb(122, 162, 158))),
        ]),
        Line::from(vec![
            Span::styled("↑/↓", Style::default().fg(Color::DarkGray)),
            Span::raw(" scroll   "),
            Span::styled("F1", Style::default().fg(Color::DarkGray)),
            Span::raw(" toggle help"),
        ]),
        Line::from(Span::styled("/help again to close", Style::default().fg(Color::DarkGray))),
    ];

    let content_width = help_lines.iter()
        .map(|l| l.spans.iter().map(|s| s.content.chars().count()).sum::<usize>())
        .max().unwrap_or(0) as u16;
    let width = (content_width + 4).clamp(20, input_area.width.max(20));
    let height = (help_lines.len() as u16 + 2).min(input_area.y);
    let x = input_area.x;
    let y = input_area.y.saturating_sub(height);
    let popup_area = Rect::new(x, y, width, height);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(
            " Help & Commands ",
            Style::default().fg(Color::Gray).add_modifier(Modifier::BOLD)
        ));

    let paragraph = Paragraph::new(help_lines).block(block);

    f.render_widget(Clear, popup_area);
    f.render_widget(paragraph, popup_area);
}
