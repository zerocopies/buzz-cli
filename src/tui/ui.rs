use ratatui::backend::Backend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Span, Spans};
use ratatui::widgets::{Block, BorderType, Borders, List, ListItem, Paragraph, Wrap};
use ratatui::Frame;

use crate::tui::app::{App, Message};

pub fn render<B: Backend>(frame: &mut Frame<B>, app: &App) {
    let size = frame.size();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(
            [
                Constraint::Min(0),
                Constraint::Length(3),
                Constraint::Length(1),
            ]
            .as_ref(),
        )
        .split(size);

    let items: Vec<ListItem> = app
        .messages
        .iter()
        .map(|msg| {
            let prefix = match msg.provider {
                Some(ref p) => format!("● {}", p),
                None => "● local".to_string(),
            };
            let content = format!("{} {}", prefix, msg.content);
            ListItem::new(Spans::from(content))
        })
        .collect();

    let history_list = List::new(items)
        .block(
            Block::default()
                .title(" Buzz ")
                .border_style(Style::default().fg(Color::Gray))
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded),
        )
        .style(Style::default().fg(Color::White))
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .scroll_offset(app.scroll_offset);

    frame.render_widget(history_list, chunks[0]);

    let input_line = format!("> {}", app.input_buffer);
    let input_paragraph = Paragraph::new(input_line)
        .style(Style::default().fg(Color::Green))
        .alignment(Alignment::Left)
        .wrap(Wrap { trim: true });

    frame.render_widget(input_paragraph, chunks[1]);

    let spend_text = format!(
        "spend: ${:.4}/${:.2} · {}",
        app.cost.total_spent,
        app.cost.budget,
        if app.config.local_model_loaded {
            "local model: loaded"
        } else {
            "local model: downloading..."
        }
    );

    let status_bar = Paragraph::new(spend_text)
        .style(Style::default().fg(Color::Yellow))
        .alignment(Alignment::Center);

    frame.render_widget(status_bar, chunks[2]);
}
