use std::error::Error;

use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use crossterm::{terminal, event, execute, terminal::ClearType};
use std::io;

use crate::tui::app::{App, Config};
use crate::tui::event::poll_event;
use crate::tui::ui::render;
use crate::policy::Policy;

mod core;
mod tui;
mod policy;
mod providers;


/// Restore terminal to a sane state. Idempotent; errors ignored.
/// Defensively disables mouse-reporting modes the app never enables,
/// because *model output* may contain escape sequences that turned
/// them on (untrusted bytes hitting the terminal).
fn restore_terminal() {
    use std::io::Write;
    let _ = terminal::disable_raw_mode();
    let mut out = io::stdout();
    let _ = out.write_all(b"\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b[?1015l\x1b[?25h");
    let _ = out.flush();
}

fn install_panic_restore() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        prev(info);
    }));
}

/// Strip control characters from untrusted (model/provider) text before it
/// reaches the terminal: keeps \n and \t, drops ESC and all other C0 + DEL.
fn sanitize_terminal_text(s: &str) -> String {
    s.chars()
        .filter(|&c| c == '\n' || c == '\t' || (c >= ' ' && c != '\u{7f}'))
        .collect()
}

async fn run() -> Result<(), Box<dyn Error>> {
    install_panic_restore();
    terminal::enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, terminal::Clear(ClearType::All))?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let policy = Policy {
        providers: Providers::default(),
        routing: Routing::default(),
        cost: Cost {
            total_spent_usd: 0.0,
            daily_budget_usd: 5.0,
            max_per_request_usd: 0.01,
        },
        local: Local {
            model_path: Policy::default_model_path(),
        },
        audit: Audit {
            enabled: true,
            log_path: Policy::default_audit_path(),
        },
    };

use qfz3_engine::Engine;

async fn execute_provider(_provider: &str, prompt: &str) -> Result<(String, usize, f64), Box<dyn Error>> {
    let model_path = "/home/prp/qfz3/ai_playground/Qwen2.5-Coder-3B-Instruct-abliterated-Q4_K_M.gguf";
    let mut engine = Engine::load(model_path, 4096, None)?;
    let (content, tokens_i32) = engine.generate_sync(prompt, 512)?;
    let tokens = tokens_i32 as usize;
    let cost = 0.0;
    Ok((content, tokens, cost))
}
    app.config.local_model_loaded = true;

    loop {
        terminal.draw(|f| render(f, &app))?;

        if let Some(event) = poll_event(100).await? {
            match event {
                InputEvent::Char(c) => {
                    app.input_buffer.push(c);
                }
                InputEvent::Backspace => {
                    app.input_buffer.pop();
                }
                InputEvent::Delete => {
                    // Not implemented
                }
                InputEvent::CursorLeft => {
                    if !app.input_buffer.is_empty() {
                        app.input_buffer.pop();
                    }
                }
                InputEvent::CursorRight => {
                    // Not implemented
                }
                InputEvent::Submit(_) => {
                    let input = app.input_buffer.clone();
                    app.input_buffer.clear();

                    if input.starts_with('/') {
                        app.handle_command(&input);
                        if input == "/quit" || input == "quit" {
                            break;
                        }
                    } else {
                        let context = core::decision::DecisionContext::from_config(&app.config.policy).unwrap();
                        let route = context.decide_route(&input);

                        let response = match route {
                            RouteProvider::Local => providers::generate_local_response(&input),
                            RouteProvider::Cloud(provider) => {
                                match provider.as_str() {
                                    "groq" => Groq {}.generate_response(&input),
                                    "anthropic" => Anthropic {}.generate_response(&input),
                                    "gemini" => Gemini {}.generate_response(&input),
                                    _ => "Unknown provider".to_string(),
                                }
                            }
                        };

                        app.add_message(input, Some(route.to_string()));
                        app.add_message(sanitize_terminal_text(&response), None);

                        // Simulate cost: $0.001 per request
                        app.cost.add_spent(0.001);
                    }
                }
                InputEvent::Quit => break,
                InputEvent::Help => {
                    app.handle_command("/help");
                }
                InputEvent::Config => {
                    app.handle_command("/config");
                }
                InputEvent::Stats => {
                    app.handle_command("/stats");
                }
            }
        }
    }

    restore_terminal();
    execute!(io::stdout(), terminal::Clear(ClearType::All))?;

    Ok(())
}

fn main() {
    let res = tokio::runtime::Runtime::new().unwrap().block_on(run());
    restore_terminal();
    if let Err(e) = res {
        eprintln!("Error: {}", e);
    }
}
