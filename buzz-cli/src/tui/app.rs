use std::time::Instant;

#[derive(Debug, Clone)]
pub struct Message {
    pub role: MessageRole,
    pub provider: String,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MessageRole {
    System,
    User,
    Assistant,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    Insert,
    Normal,
}

pub struct App {
    pub messages: Vec<Message>,
    pub input_buffer: String,
    pub cursor_position: usize,
    pub current_spend_usd: f64,
    pub daily_budget_usd: f64,
    pub is_running: bool,
    pub input_mode: InputMode,
    pub scroll_offset: usize,
    pub provider_name: String,
    pub is_generating: bool,
    pub gen_start: Option<Instant>,
    pub total_tokens: u64,
    pub show_help: bool,
    /// Set while a background-threaded generation is streaming tokens back.
    /// Drained every frame in the main loop; cleared when the thread finishes.
    pub stream_rx: Option<std::sync::mpsc::Receiver<String>>,
}

impl Default for App {
    fn default() -> Self {
        Self {
            messages: Vec::new(),
            input_buffer: String::new(),
            cursor_position: 0,
            current_spend_usd: 0.0,
            daily_budget_usd: 5.0,
            is_running: true,
            input_mode: InputMode::Insert,
            scroll_offset: 0,
            provider_name: "groq".to_string(),
            is_generating: false,
            gen_start: None,
            total_tokens: 0,
            show_help: false,
            stream_rx: None,
        }
    }
}

impl App {
    pub fn new(daily_budget: f64, provider: &str) -> Self {
        Self {
            daily_budget_usd: daily_budget,
            provider_name: provider.to_string(),
            ..Default::default()
        }
    }

    pub fn add_system(&mut self, content: &str) {
        self.messages.push(Message {
            role: MessageRole::System,
            provider: "system".to_string(),
            content: content.to_string(),
        });
    }

    pub fn add_user(&mut self, content: &str) {
        self.messages.push(Message {
            role: MessageRole::User,
            provider: self.provider_name.clone(),
            content: content.to_string(),
        });
    }

    pub fn add_assistant(&mut self, provider: &str, content: &str) {
        self.messages.push(Message {
            role: MessageRole::Assistant,
            provider: provider.to_string(),
            content: content.to_string(),
        });
    }

    /// Append a streamed piece to the last message (must be an in-progress
    /// Assistant message pushed by add_assistant before streaming began).
    pub fn append_to_last_assistant(&mut self, piece: &str) {
        if let Some(last) = self.messages.last_mut() {
            last.content.push_str(piece);
        }
    }

    pub fn add_error(&mut self, content: &str) {
        self.messages.push(Message {
            role: MessageRole::Error,
            provider: self.provider_name.clone(),
            content: content.to_string(),
        });
    }

    pub fn start_generation(&mut self) {
        self.is_generating = true;
        self.gen_start = Some(Instant::now());
    }

    pub fn end_generation(&mut self, tokens: u64, cost: f64) {
        self.is_generating = false;
        self.gen_start = None;
        self.total_tokens += tokens;
        self.current_spend_usd += cost;
    }

    pub fn scroll_up(&mut self) {
        if self.scroll_offset > 0 {
            self.scroll_offset -= 1;
        }
    }

    pub fn scroll_down(&mut self) {
        self.scroll_offset += 1;
    }

    pub fn input_chars(&self) -> usize {
        self.input_buffer.chars().count()
    }

    fn byte_index(&self) -> usize {
        self.input_buffer
            .char_indices()
            .nth(self.cursor_position)
            .map(|(i, _)| i)
            .unwrap_or_else(|| self.input_buffer.len())
    }

    pub fn push_char(&mut self, ch: char) {
        let idx = self.byte_index();
        self.input_buffer.insert(idx, ch);
        self.cursor_position += 1;
    }

    pub fn backspace(&mut self) {
        if self.cursor_position > 0 {
            self.cursor_position -= 1;
            let idx = self.byte_index();
            self.input_buffer.remove(idx);
        }
    }

    pub fn delete_char(&mut self) {
        if self.cursor_position < self.input_chars() {
            let idx = self.byte_index();
            self.input_buffer.remove(idx);
        }
    }

    pub fn clear_input(&mut self) {
        self.input_buffer.clear();
        self.cursor_position = 0;
    }

    pub fn quit(&mut self) {
        self.is_running = false;
    }
}
