mod providers;
mod tui;

use clap::Parser;
use std::io::Write;
use buzz_core::policy::Config;
use std::error::Error;
use std::env;
use tokio::sync::mpsc;

use buzz_core::{decide_route, RouteProvider};
use providers::{AnthropicProvider, GeminiProvider, GroqProvider, HuggingFaceProvider};

#[derive(Parser)]
#[command(
    name = "buzz",
    version = "0.1.0",
    about = "Multi-provider AI CLI with privacy-first local routing"
)]
struct Cli {
    #[arg(required_unless_present_any = ["chat", "tui", "setup"])]
    prompt: Option<String>,

    #[arg(long, short)]
    chat: bool,

    #[arg(long)]
    tui: bool,

    #[arg(long)]
    setup: bool,

    #[arg(short, long)]
    provider: Option<String>,

    #[arg(long)]
    show_routing: bool,
}


const LOCAL_MODEL_PATH: &str = "/home/prp/qfz3/ai_playground/qwen2.5-coder-1.5b-q4_K_M.gguf";

/// Run the local qfz3 engine, loading it once on first use and reusing it (and
/// its KV cache / turn counter) for the rest of the session — this is what makes
/// multi-turn memory work and avoids reloading the model per message.
fn run_local(
    prompt: &str,
    engine: &mut Option<qfz3::Engine>,
    mut on_token: impl FnMut(&str),
) -> Result<(String, usize, f64), Box<dyn Error>> {
    if engine.is_none() {
        *engine = Some(qfz3::Engine::load(LOCAL_MODEL_PATH, 4096, None)?);
    }
    let eng = engine.as_mut().unwrap();
    let out = eng.generate_streaming(prompt, 512, |piece| on_token(piece))
        .map_err(|e| -> Box<dyn Error> { e.into() })?;
    Ok((out.text, out.completion_tokens as usize, 0.0))
}

/// Bridge an async cloud provider through the existing runtime; empty key -> error.
fn block_cloud(
    rt: &tokio::runtime::Runtime,
    key: &str,
    key_name: &str,
    call: impl std::future::Future<Output = Result<(String, u64, f64), Box<dyn Error + Send + Sync>>>,
) -> Result<(String, usize, f64), Box<dyn Error>> {
    if key.trim().is_empty() {
        return Err(format!(
            "No API key configured ({key_name}). Run --setup or edit ~/.buzz/config.toml."
        ).into());
    }
    let (content, tokens, cost) = rt.block_on(call).map_err(|e| e.to_string())?;
    Ok((content, tokens as usize, cost))
}

/// Events sent from the background generation thread back to the main loop.
enum StreamEvent {
    Piece(String),
    Done { tokens: usize, cost: f64 },
    Error(String),
}

/// TUI dispatch: local on-device, cloud via runtime. Selected by provider name.
fn execute_provider(
    provider: &str,
    prompt: &str,
    engine: &mut Option<qfz3::Engine>,
    config: &Config,
    rt: &tokio::runtime::Runtime,
) -> Result<(String, usize, f64), Box<dyn Error>> {
    match provider {
        "local" => run_local(prompt, engine, |_| {}),
        "groq" => { let k = config.providers.groq.clone();
            block_cloud(rt, &k, "groq_api_key", GroqProvider::new(k.clone(), None).generate(prompt)) }
        "anthropic" => { let k = config.providers.anthropic.clone();
            block_cloud(rt, &k, "anthropic_api_key", AnthropicProvider::new(k.clone(), None).generate(prompt)) }
        "gemini" => { let k = config.providers.gemini.clone();
            block_cloud(rt, &k, "gemini_api_key", GeminiProvider::new(k.clone(), None).generate(prompt)) }
        "huggingface" | "hf" => { let k = config.providers.hf.clone();
            block_cloud(rt, &k, "hf_api_key", HuggingFaceProvider::new(k.clone(), None).generate(prompt)) }
        other => Err(format!("Unknown provider: {other}").into()),
    }
}

/// One-shot dispatch for --prompt: async, awaited directly (already inside the
/// runtime, so no nested block_on). Local still goes through the sync engine.
async fn execute_provider_oneshot(
    provider: &str,
    prompt: &str,
    core_cfg: &buzz_core::Config,
) -> Result<(String, usize, f64), Box<dyn Error>> {
    async fn cloud(
        key: String, name: &str,
        fut: impl std::future::Future<Output = Result<(String, u64, f64), Box<dyn Error + Send + Sync>>>,
    ) -> Result<(String, usize, f64), Box<dyn Error>> {
        if key.trim().is_empty() {
            return Err(format!("No API key configured ({name}). Run --setup or edit ~/.buzz/config.toml.").into());
        }
        let (c, t, cost) = fut.await.map_err(|e| e.to_string())?;
        Ok((c, t as usize, cost))
    }
    match provider {
        "local" => {
            let mut eng = qfz3::Engine::load(LOCAL_MODEL_PATH, 4096, None)?;
            let (c, t) = eng.generate_sync(prompt, 512)?;
            Ok((c, t as usize, 0.0))
        }
        "groq" => { let k = core_cfg.providers.groq.clone();
            cloud(k.clone(), "groq_api_key", GroqProvider::new(k, None).generate(prompt)).await }
        "anthropic" => { let k = core_cfg.providers.anthropic.clone();
            cloud(k.clone(), "anthropic_api_key", AnthropicProvider::new(k, None).generate(prompt)).await }
        "gemini" => { let k = core_cfg.providers.gemini.clone();
            cloud(k.clone(), "gemini_api_key", GeminiProvider::new(k, None).generate(prompt)).await }
        "huggingface" | "hf" => { let k = core_cfg.providers.hf.clone();
            cloud(k.clone(), "hf_api_key", HuggingFaceProvider::new(k, None).generate(prompt)).await }
        other => Err(format!("Unknown provider: {other}").into()),
    }
}

/// Idempotent terminal restore. Safe to call multiple times; errors ignored.
fn restore_terminal() {
    let _ = crossterm::terminal::disable_raw_mode();
    let _ = crossterm::execute!(
        std::io::stdout(),
        crossterm::terminal::LeaveAlternateScreen,
        crossterm::event::DisableMouseCapture
    );
}

/// Drop guard: restores the terminal on early `?` returns and unwinds,
/// not just the happy-path teardown at the end of run_tui_mode.
struct TerminalGuard;
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore_terminal();
    }
}

fn install_panic_restore() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        prev(info);
    }));
}

/// Strip control chars from untrusted (model/provider) text before it hits
/// the terminal: keeps \n and \t, drops ESC and all other C0 + DEL.
fn sanitize_terminal_text(s: &str) -> String {
    s.chars()
        .filter(|&c| c == '\n' || c == '\t' || (c >= ' ' && c != '\u{7f}'))
        .collect()
}

use crate::tui::ui::render;

fn main() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();

    if cli.setup {
        return run_setup_wizard();
    }

    if cli.tui || cli.chat {
        let provider = select_provider_name(&cli.provider);
        return run_tui_mode(&provider, cli.show_routing);
    }

    if let Some(prompt) = cli.prompt {
        let rt = tokio::runtime::Runtime::new()?;
        return rt.block_on(run_smart_cli(&prompt, cli.show_routing));
    }

    print_banner();
    Ok(())
}

fn print_banner() {
    println!("\n{}", "=".repeat(50));
    println!("  Buzz CLI v0.1.0");
    println!("{}", "=".repeat(50));
    println!("\n  Usage:");
    println!("    buzz \"prompt\"         Send prompt via smart router");
    println!("    buzz --chat            Interactive TUI");
    println!("    buzz --setup           Configure API keys");
    println!("    buzz --help            Show help\n");
}

fn run_setup_wizard() -> Result<(), Box<dyn Error>> {
    println!("\n{}", "=".repeat(50));
    println!("  Buzz CLI Setup");
    println!("{}", "=".repeat(50));

    let mut config = Config::default();

    print!("\nGroq API key (Enter to skip): ");
    std::io::stdout().flush()?;
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    config.groq_api_key = Some(input.trim().to_string());

    print!("Anthropic API key (Enter to skip): ");
    input.clear(); std::io::stdout().flush()?;
    std::io::stdin().read_line(&mut input)?;
    config.anthropic_api_key = Some(input.trim().to_string());

    print!("Gemini API key (Enter to skip): ");
    input.clear(); std::io::stdout().flush()?;
    std::io::stdin().read_line(&mut input)?;
    config.gemini_api_key = Some(input.trim().to_string());

    print!("HuggingFace API key (Enter to skip): ");
    input.clear(); std::io::stdout().flush()?;
    std::io::stdin().read_line(&mut input)?;
    config.hf_api_key = Some(input.trim().to_string());

    print!("\nDaily budget USD (default $5.00): ");
    input.clear(); std::io::stdout().flush()?;
    std::io::stdin().read_line(&mut input)?;
    config.cost.daily_budget_usd = input.trim().parse().unwrap_or(5.0);

    save_config(&config)?;

    println!("\n  Config saved to ~/.buzz/config.toml");
    println!("  Run: buzz --chat\n");

    Ok(())
}

async fn run_smart_cli(prompt: &str, show_routing: bool) -> Result<(), Box<dyn Error>> {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let path = std::path::PathBuf::from(format!("{}/.buzz/config.toml", home));
    let core_config = buzz_core::Config::load_from_file(&path).unwrap_or_default();

    let route = decide_route(prompt, &core_config.routing);

    if show_routing {
        let bar = "-".repeat(50);
        println!("\n  Smart Router Decision");
        println!("{}", bar);
        println!("  Provider: {:?}", route.provider);
        println!("  Confidence: {:.2}", route.confidence);
        println!("  Reason: {}", route.reason);
        println!("{}", bar);
    }

    let bar = "-".repeat(50);
    println!("\n  Executing Request");
    println!("{}", bar);

    let provider_str = match route.provider {
        RouteProvider::Groq => "groq",
        RouteProvider::Anthropic => "anthropic",
        RouteProvider::Gemini => "gemini",
        RouteProvider::HuggingFace => "huggingface",
        RouteProvider::Local => "local",
    };

    let result = execute_provider_oneshot(provider_str, prompt, &core_config).await;

    match result {
        Ok((content, tokens, cost)) => {
            println!("{}\n", content);
            println!("{}", bar);
            println!("  Tokens: {} | Cost: ${:.6}", tokens, cost);
        }
        Err(e) => {
            eprintln!("\n  Error: {}", e);
            std::process::exit(1);
        }
    }

    println!();
    Ok(())
}

enum AsyncEvent {
    GenerationComplete(String, u64, f64),
    GenerationError(String),
}

fn run_tui_mode(_default_provider: &str, _show_routing: bool) -> Result<(), Box<dyn Error>> {
    use crossterm::{
        event::{DisableMouseCapture, EnableMouseCapture},
        execute,
        terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    };
    use ratatui::backend::CrosstermBackend;
    use ratatui::Terminal;
    use tui::event::{poll_event, InputEvent};
    use tui::app::App;

    install_panic_restore();
    // Rust does not run Drop on a signal exit — SIGINT (Ctrl+C) kills the
    // process immediately, bypassing TerminalGuard. Restore explicitly here.
    let _ = ctrlc::set_handler(|| {
        restore_terminal();
        std::process::exit(130);
    });
    enable_raw_mode()?;
    let _guard = TerminalGuard;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let config = get_config().unwrap_or_default();
    let mut app = App::new(config.cost.daily_budget_usd, "local");

    app.add_system("Smart router and providers initialized. Type a message to begin.");

    // Persistent local engine: built on first local request, reused thereafter.
    let mut local_engine: Option<qfz3::Engine> = None;
    // Set by /provider <name>; consumed by the next message only, then cleared
    // so auto-routing (Sentinel/decide_route) resumes on the message after.
    let mut provider_override: Option<String> = None;

    let rt = tokio::runtime::Runtime::new()?;

    loop {
        terminal.draw(|frame| {
            tui::ui::render(frame, &app);
        })?;

        if let Some(event) = poll_event(50)? {
            match event {
                InputEvent::Char(ch) => {
                    if app.is_generating { continue; }
                    if app.input_buffer.is_empty() && ch == '/' {
                        app.push_char('/');
                        continue;
                    }
                    app.push_char(ch);
                }
                InputEvent::Backspace => {
                    if !app.is_generating { app.backspace(); }
                }
                InputEvent::Delete => {
                    if !app.is_generating && app.cursor_position < app.input_chars() {
                        app.input_buffer.remove(app.cursor_position);
                    }
                }
                InputEvent::CursorLeft => {
                    if app.cursor_position > 0 { app.cursor_position -= 1; }
                }
                InputEvent::CursorRight => {
                    if app.cursor_position < app.input_chars() { app.cursor_position += 1; }
                }
                InputEvent::ScrollUp => {
                    if !app.is_generating && app.scroll_offset > 0 {
                        app.scroll_offset -= 1;
                    }
                }
                InputEvent::ScrollDown => {
                    if !app.is_generating && app.scroll_offset < app.messages.len().saturating_sub(1) {
                        app.scroll_offset += 1;
                    }
                }
                InputEvent::Help => {
                    if app.input_buffer.is_empty() {
                        app.show_help = !app.show_help;
                    } else {
                        app.push_char('/');
                    }
                }
                InputEvent::Submit => {
                    let input = app.input_buffer.clone();
                    if input.is_empty() || app.is_generating { continue; }

                    if input.trim().starts_with('/') {
                        let cmd = input.trim();
                        app.clear_input();

                        if cmd == "/quit" || cmd == "/exit" {
                            app.quit();
                            continue;
                        }
                        if cmd == "/help" {
                            app.show_help = !app.show_help;
                            continue;
                        }
                        if cmd == "/reset" {
                            if let Some(eng) = local_engine.as_mut() { eng.reset(); }
                            app.messages.clear();
                            app.current_spend_usd = 0.0;
                            app.total_tokens = 0;
                            app.scroll_offset = 0;
                            app.add_system("Conversation reset.");
                            continue;
                        }
                        if cmd == "/stats" {
                            app.add_system(&format!(
                                "Stats: {} messages | {} tokens | ${:.6} spent",
                                app.messages.len(),
                                app.total_tokens,
                                app.current_spend_usd
                            ));
                            continue;
                        }
                        if cmd.starts_with("/provider ") {
                            let parts: Vec<&str> = cmd.split_whitespace().collect();
                            if parts.len() > 1 {
                                let new_p = parts[1].to_lowercase();
                                match new_p.as_str() {
                                    "local" | "groq" | "anthropic" | "gemini" | "huggingface" | "hf" => {
                                        let picked = if new_p == "hf" { "huggingface".to_string() } else { new_p };
                                        app.provider_name = picked.clone();
                                        provider_override = Some(picked.clone());
                                        app.add_system(&format!("Next message only will use: {} (auto-routing resumes after)", picked));
                                    }
                                    _ => {
                                        app.add_error(&format!("Unknown provider: {}. Use groq|anthropic|gemini|hf", parts[1]));
                                    }
                                }
                            }
                            continue;
                        }

                        app.add_error(&format!("Unknown command: {}", cmd));
                        continue;
                    }

                    // Regular prompt
                    app.add_user(&input);
                    app.clear_input();
                    app.start_generation();

                    let (provider, route_reason) = if let Some(p) = provider_override.take() {
                        let r = format!("manual override: /provider {p}");
                        (p, r)
                    } else {
                        let route = decide_route(&input, &config.routing);
                        let p = match route.provider {
                            RouteProvider::Groq => "groq",
                            RouteProvider::Anthropic => "anthropic",
                            RouteProvider::Gemini => "gemini",
                            RouteProvider::HuggingFace => "huggingface",
                            RouteProvider::Local => "local",
                        }.to_string();
                        (p, route.reason)
                    };
                    
                    // Spawn async task to avoid blocking TUI
                    terminal.draw(|f| render(f, &app))?;
                    let result = execute_provider(&provider, &input, &mut local_engine, &config, &rt);

                    match result {
                        Ok((content, tokens, cost)) => {
                            app.end_generation(tokens as u64, cost);
                            app.add_system(&format!("[{}] {}", provider, route_reason));
                            app.add_assistant(&provider, &sanitize_terminal_text(&content));
                        }
                        Err(e) => {
                            app.is_generating = false;
                            app.gen_start = None;
                            app.add_error(&sanitize_terminal_text(&format!("{}", e)));
                        }
                    }

                    app.scroll_offset = app.messages.len().saturating_sub(1);
                }
                InputEvent::Quit => app.quit(),
            }
        }

        if !app.is_running {
            break;
        }
    }

    terminal.clear()?;
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
    terminal.show_cursor()?;

    println!("\n  Buzz CLI — Session Summary");
    println!("  Tokens: {} | Spent: ${:.6}\n", app.total_tokens, app.current_spend_usd);

    Ok(())
}

fn select_provider_name(override_opt: &Option<String>) -> String {
    override_opt.as_ref()
        .map(|s| s.to_lowercase())
        .or_else(|| std::env::var("PROVIDER").ok())
        .unwrap_or_else(|| "groq".to_string())
}

fn save_config(config: &Config) -> Result<(), Box<dyn Error>> {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let path = format!("{}/.buzz/config.toml", home);
    
    if let Some(parent) = std::path::Path::new(&path).parent() {
        std::fs::create_dir_all(parent)?;
    }

    let content = format!(
        "# Buzz CLI Configuration\n\
        [providers]\ngroq = \"{}\"\nanthropic = \"{}\"\ngemini = \"{}\"\nhf = \"{}\"\n\n\
        [routing]\nalways_local_if_sensitive = true\ncloud_fallback_order = [\"groq\"]\n\n\
        [local]\nmodel_path = \"{}\"\nmodel_name = \"{}\"\n\n\
        [cost]\ndaily_budget_usd = {}\n\n\
        [audit]\nenabled = true\n",
        config.providers.groq,
        config.providers.anthropic,
        config.providers.gemini,
        config.providers.hf,
        config.local.model_path,
        config.local.model_name,
        config.cost.daily_budget_usd
    );

    std::fs::write(&path, &content)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }

    Ok(())
}

fn get_config() -> Result<Config, Box<dyn Error>> {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let path = format!("{}/.buzz/config.toml", home);
    let content = std::fs::read_to_string(&path).unwrap_or_default();

    let mut config = Config::default();
    for line in content.lines() {
        if let Some((key, val)) = line.split_once('=') {
            match key.trim() {
                "groq_api_key" => config.providers.groq = val.trim().trim_matches('"').to_string(),
                "anthropic_api_key" => config.providers.anthropic = val.trim().trim_matches('"').to_string(),
                "gemini_api_key" => config.providers.gemini = val.trim().trim_matches('"').to_string(),
                "hf_api_key" => config.providers.hf = val.trim().trim_matches('"').to_string(),
                "huggingface_api_key" => config.providers.hf = val.trim().trim_matches('"').to_string(),
                "daily_budget_usd" => config.cost.daily_budget_usd = val.trim().parse().unwrap_or(5.0),
                _ => {}
            }
        }
    }

    Ok(config)
}
