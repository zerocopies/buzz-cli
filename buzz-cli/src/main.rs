mod providers;

use clap::Parser;
use std::io::Write;
use buzz_core::policy::Config;
use std::error::Error;

use buzz_core::{decide_route, RouteProvider};
use providers::{AnthropicProvider, GeminiProvider, GroqProvider, HuggingFaceProvider};

#[derive(Parser)]
#[command(
    name = "buzz",
    version = "0.1.0",
    about = "Multi-provider AI CLI with privacy-first local routing"
)]
struct Cli {
    prompt: Option<String>,

    #[arg(long)]
    setup: bool,

    #[arg(short, long)]
    provider: Option<String>,

    #[arg(long)]
    show_routing: bool,
}



/// Run the local qfz3 engine, loading it once on first use and reusing it (and
/// its KV cache / turn counter) for the rest of the session — this is what makes
/// multi-turn memory work and avoids reloading the model per message.
fn run_local(
    prompt: &str,
    engine: &mut Option<qfz3::Engine>,
    model_path: &str,
    mut on_token: impl FnMut(&str),
) -> Result<(String, usize, f64), Box<dyn Error>> {
    if engine.is_none() {
        *engine = Some(qfz3::Engine::load(model_path, 4096, None)?);
    }
    let eng = engine.as_mut().unwrap();
    let out = eng.generate_streaming(prompt, 512, |piece| on_token(piece))
        .map_err(|e| -> Box<dyn Error> { e.into() })?;
    Ok((out.text, out.completion_tokens as usize, 0.0))
}

/// Bridge an async cloud provider through the existing runtime; empty key -> error.
fn block_cloud(
    rt: &tokio::runtime::Handle,
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

/// TUI dispatch: local on-device, cloud via runtime. Selected by provider name.
fn execute_provider(
    provider: &str,
    prompt: &str,
    engine: &mut Option<qfz3::Engine>,
    config: &Config,
    rt: &tokio::runtime::Handle,
) -> Result<(String, usize, f64), Box<dyn Error>> {
    match provider {
        "local" => run_local(prompt, engine, &config.local.model_path, |_| {}),
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
            let mut eng = qfz3::Engine::load(&core_cfg.local.model_path, 4096, None)?;
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


fn main() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();

    if cli.setup {
        return run_setup_wizard();
    }

    if let Some(prompt) = cli.prompt {
        let rt = tokio::runtime::Runtime::new()?;
        return rt.block_on(run_smart_cli(&prompt, cli.show_routing));
    }

    let provider = select_provider_name(&cli.provider);
    run_tui_mode(&provider, cli.show_routing)
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


fn run_tui_mode(_default_provider: &str, _show_routing: bool) -> Result<(), Box<dyn Error>> {
    install_panic_restore();

    let config = get_config().unwrap_or_default();
    println!("Buzz — type a message, /settings, /provider <name>, /reset, or /quit\n");

    let mut local_engine: Option<qfz3::Engine> = None;
    let mut provider_override: Option<String> = None;
    let mut total_tokens: u64 = 0;
    let mut total_spend: f64 = 0.0;
    // Exact-match reply cache for this session: repeating the same question
    // returns the earlier answer instantly instead of re-generating (or
    // re-paying for) it. Keyed on the literal input text.
    let mut cache: std::collections::HashMap<String, (String, usize, f64)> = std::collections::HashMap::new();

    let rt = tokio::runtime::Runtime::new()?;
    let rt_handle = rt.handle().clone();

    loop {
        print!("> ");
        std::io::stdout().flush()?;
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line)? == 0 {
            break;
        }
        let input = line.trim().to_string();
        if input.is_empty() { continue; }

        if input == "/quit" || input == "/exit" {
            break;
        }
        if input == "/reset" {
            if let Some(eng) = local_engine.as_mut() { eng.reset(); }
            total_tokens = 0;
            total_spend = 0.0;
            cache.clear();
            println!("Conversation reset.\\n");
            continue;
        }
        if input == "/stats" {
            println!("Stats: {total_tokens} tokens | ${total_spend:.6} spent\\n");
            continue;
        }
        if input == "/settings" || input == "/setup" {
            let cur = get_config().unwrap_or_default();
            let mask = |s: &str| if s.is_empty() { "(not set)".to_string() }
                else { format!("set ({} chars)", s.len()) };
            println!(
                "Settings — groq: {} | anthropic: {} | gemini: {} | hf: {} | model: {} | budget: ${:.2}",
                mask(&cur.providers.groq), mask(&cur.providers.anthropic),
                mask(&cur.providers.gemini), mask(&cur.providers.hf),
                cur.local.model_path, cur.cost.daily_budget_usd
            );
            println!("Change with: /settings <groq|anthropic|gemini|hf|model|budget> <value>\\n");
            continue;
        }
        if let Some(rest) = input.strip_prefix("/settings ") {
            let parts: Vec<&str> = rest.splitn(2, char::is_whitespace).collect();
            if parts.len() < 2 {
                println!("Usage: /settings <groq|anthropic|gemini|hf|model|budget> <value>\\n");
                continue;
            }
            let key = parts[0].to_lowercase();
            let value = parts[1].trim();
            let mut cur = get_config().unwrap_or_default();
            let result: Result<String, String> = match key.as_str() {
                "groq" => { cur.providers.groq = value.to_string(); Ok("groq key updated".to_string()) }
                "anthropic" => { cur.providers.anthropic = value.to_string(); Ok("anthropic key updated".to_string()) }
                "gemini" => { cur.providers.gemini = value.to_string(); Ok("gemini key updated".to_string()) }
                "hf" | "huggingface" => { cur.providers.hf = value.to_string(); Ok("huggingface key updated".to_string()) }
                "model" => { cur.local.model_path = value.to_string(); Ok(format!("local model path set to: {value}")) }
                "budget" => match value.parse::<f64>() {
                    Ok(n) => { cur.cost.daily_budget_usd = n; Ok(format!("daily budget set to ${n:.2}")) }
                    Err(_) => Err(format!("invalid budget value: {value}")),
                },
                other => Err(format!("Unknown setting: {other}. Use groq|anthropic|gemini|hf|model|budget")),
            };
            match result {
                Ok(msg) => match save_config(&cur) {
                    Ok(_) => println!("{msg}\\n"),
                    Err(e) => println!("Failed to save settings: {e}\\n"),
                },
                Err(e) => println!("{e}\\n"),
            }
            continue;
        }
        if let Some(rest) = input.strip_prefix("/provider ") {
            let new_p = rest.trim().to_lowercase();
            match new_p.as_str() {
                "local" | "groq" | "anthropic" | "gemini" | "huggingface" | "hf" => {
                    let picked = if new_p == "hf" { "huggingface".to_string() } else { new_p };
                    provider_override = Some(picked.clone());
                    println!("Next message only will use: {picked} (auto-routing resumes after)\\n");
                }
                _ => println!("Unknown provider: {rest}. Use groq|anthropic|gemini|hf|local\\n"),
            }
            continue;
        }

        if let Some((cached_reply, cached_tokens, _)) = cache.get(&input) {
            println!("[cached] repeated question — reusing earlier answer, no cost");
            println!("{cached_reply}");
            println!();
            println!("({cached_tokens} tokens · $0.000000 this reply — served from cache · {total_tokens} tokens · ${total_spend:.6} total)\n");
            continue;
        }

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
        println!("[{provider}] {route_reason}");

        let result = if provider == "local" {
            let mut out = std::io::stdout();
            run_local(&input, &mut local_engine, &config.local.model_path, |piece| {
                print!("{piece}");
                let _ = out.flush();
            })
        } else {
            execute_provider(&provider, &input, &mut local_engine, &config, &rt_handle)
        };

        match result {
            Ok((content, tokens, cost)) => {
                if provider != "local" {
                    println!("{}", sanitize_terminal_text(&content));
                }
                println!();
                total_tokens += tokens as u64;
                total_spend += cost;
                println!("({tokens} tokens · ${cost:.6} this reply · {total_tokens} tokens · ${total_spend:.6} total)\n");
                cache.insert(input.clone(), (sanitize_terminal_text(&content), tokens, cost));
            }
            Err(e) => {
                println!("Error: {}\\n", sanitize_terminal_text(&format!("{e}")));
            }
        }
    }

    println!("\\n  Buzz — Session Summary");
    println!("  Tokens: {total_tokens} | Spent: ${total_spend:.6}\\n");

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
                "model_path" => config.local.model_path = val.trim().trim_matches('"').to_string(),
                _ => {}
            }
        }
    }

    Ok(config)
}
