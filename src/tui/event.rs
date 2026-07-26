use std::io;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use std::error::Error;

#[derive(Debug)]
pub enum InputEvent {
    Char(char),
    Backspace,
    Delete,
    CursorLeft,
    CursorRight,
    Submit(String),
    Quit,
    Help,
    Config,
    Stats,
}

pub async fn poll_event(duration: u64) -> Result<Option<InputEvent>, Box<dyn Error>> {
    if event::poll(Duration::from_millis(duration))? {
        if let Event::Key(key) = event::read()? {
            if key.kind == KeyEventKind::Press {
                match key.code {
                    KeyCode::Char(c) => return Ok(Some(InputEvent::Char(c))),
                    KeyCode::Backspace => return Ok(Some(InputEvent::Backspace)),
                    KeyCode::Delete => return Ok(Some(InputEvent::Delete)),
                    KeyCode::Left => return Ok(Some(InputEvent::CursorLeft)),
                    KeyCode::Right => return Ok(Some(InputEvent::CursorRight)),
                    KeyCode::Enter => return Ok(Some(InputEvent::Submit("".to_string()))),
                    KeyCode::Esc => return Ok(Some(InputEvent::Quit)),
                    _ => {}
                }
            }
        }
    }
    Ok(None)
}
