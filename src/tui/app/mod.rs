use crate::core::cost::Cost;
use crate::policy::{Policy, Local};
use crate::providers::RouteProvider;

#[derive(Debug, Clone)]
pub struct Message {
    pub content: String,
    pub provider: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub policy: Policy,
    pub local_model_loaded: bool,
}

#[derive(Debug)]
pub struct App {
    pub messages: Vec<Message>,
    pub input_buffer: String,
    pub scroll_offset: usize,
    pub config: Config,
    pub cost: Cost,
}

impl App {
    pub fn new(policy: Policy) -> Self {
        App {
            messages: Vec::new(),
            input_buffer: String::new(),
            scroll_offset: 0,
            config: Config {
                policy,
                local_model_loaded: false,
            },
            cost: Cost::new_from_config(&policy.cost),
        }
    }

    pub fn add_message(&mut self, content: String, provider: Option<String>) {
        self.messages.push(Message { content, provider });
        self.scroll_offset = self.messages.len().saturating_sub(1);
    }

    pub fn handle_command(&mut self, cmd: &str) {
        match cmd {
            "/help" => {
                self.add_message(
                    "Commands: /help, /config, /stats, /quit".to_string(),
                    None,
                );
            }
            "/config" => {
                self.add_message("Opening config editor... (TUI not implemented yet)".to_string(), None);
            }
            "/stats" => {
                self.add_message(
                    format!("Total spent: ${:.4} / ${:.2}", self.cost.total_spent, self.cost.budget),
                    None,
                );
            }
            "/quit" | "quit" => {
                self.add_message("Exiting...".to_string(), None);
            }
            _ => {
                self.add_message(format!("Unknown command: {}", cmd), None);
            }
        }
    }
}
