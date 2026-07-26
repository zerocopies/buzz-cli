use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use std::time::Duration;

#[derive(Debug)]
pub enum InputEvent {
    Submit,
    Quit,
    Char(char),
    Backspace,
    Delete,
    CursorLeft,
    CursorRight,
    ScrollUp,
    ScrollDown,
    Help,
}

pub fn poll_event(timeout_ms: u64) -> Result<Option<InputEvent>, Box<dyn std::error::Error>> {
    if event::poll(Duration::from_millis(timeout_ms))? {
        if let Event::Key(key) = event::read()? {
            return Ok(Some(map_key_event(key)));
        }
    }
    Ok(None)
}

fn map_key_event(key: event::KeyEvent) -> InputEvent {
    match key.code {
        KeyCode::Enter => InputEvent::Submit,
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => InputEvent::Quit,
        KeyCode::Up => InputEvent::ScrollUp,
        KeyCode::Down => InputEvent::ScrollDown,
        KeyCode::F(1) => InputEvent::Help,
        KeyCode::Backspace => InputEvent::Backspace,
        KeyCode::Delete => InputEvent::Delete,
        KeyCode::Left => InputEvent::CursorLeft,
        KeyCode::Right => InputEvent::CursorRight,
        KeyCode::Char(ch) => {
            if ch == '/' {
                InputEvent::Help  // / triggers help overlay
            } else {
                InputEvent::Char(ch)
            }
        }
        _ => InputEvent::Submit,
    }
}
