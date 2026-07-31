mod providers;
mod theme;

use buzz_core::policy::Config;
use clap::Parser;
use std::error::Error;
use std::io::Write;

use buzz_core::{decide_route, InferenceProvider, ProviderResponse, RouteProvider};
use providers::{
    AnthropicProvider, GeminiProvider, GroqProvider, HuggingFaceProvider, LocalProvider,
};

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

/// A configured key is required for every cloud provider; local needs none.
fn require_key<'a>(key: &'a str, key_name: &str) -> Result<&'a str, Box<dyn Error>> {
    if key.trim().is_empty() {
        Err(
            format!("No API key configured ({key_name}). Run --setup or edit ~/.buzz/config.toml.")
                .into(),
        )
    } else {
        Ok(key)
    }
}

/// Pre-flight budget guard for a cloud call — checked uniformly for every
/// cloud provider (including explicit /provider overrides, not just
/// auto-routed requests) so the budget can't be bypassed just by naming a
/// provider directly. Local is exempt inside `buzz_core::budget::check`.
fn check_budget(
    config: &Config,
    provider: RouteProvider,
    prompt: &str,
) -> Result<(), Box<dyn Error>> {
    let estimated = buzz_core::budget::estimate_cost(prompt, provider);
    let result = buzz_core::budget::check(config, provider, estimated);
    if result.is_ok() {
        Ok(())
    } else {
        Err(result.to_string().into())
    }
}

/// Single dispatch path for every provider, local or cloud — both TUI mode
/// (via `rt_handle.block_on`) and one-shot `--prompt` mode (via a direct
/// `.await`, already inside a runtime) call through here. Local is just
/// another `InferenceProvider` now, not a specially-cased branch: `local`
/// carries the persistent engine/KV-cache state across calls, while cloud
/// providers are cheap to construct fresh each time from the current config
/// (so a `/settings` key change takes effect on the very next message).
async fn dispatch_provider(
    provider: &str,
    prompt: &str,
    config: &Config,
    local: &mut LocalProvider,
    mut on_token: impl FnMut(&str),
) -> Result<ProviderResponse, Box<dyn Error>> {
    match provider {
        "local" => local.generate(prompt, &mut on_token).await,
        "groq" => {
            let key = require_key(&config.providers.groq, "groq_api_key")?;
            check_budget(config, RouteProvider::Groq, prompt)?;
            GroqProvider::new(key.to_string(), None)
                .generate(prompt, &mut on_token)
                .await
        }
        "anthropic" => {
            let key = require_key(&config.providers.anthropic, "anthropic_api_key")?;
            check_budget(config, RouteProvider::Anthropic, prompt)?;
            AnthropicProvider::new(key.to_string(), None)
                .generate(prompt, &mut on_token)
                .await
        }
        "gemini" => {
            let key = require_key(&config.providers.gemini, "gemini_api_key")?;
            check_budget(config, RouteProvider::Gemini, prompt)?;
            GeminiProvider::new(key.to_string(), None)
                .generate(prompt, &mut on_token)
                .await
        }
        "huggingface" | "hf" => {
            let key = require_key(&config.providers.hf, "hf_api_key")?;
            check_budget(config, RouteProvider::HuggingFace, prompt)?;
            HuggingFaceProvider::new(key.to_string(), None)
                .generate(prompt, &mut on_token)
                .await
        }
        other => Err(format!("Unknown provider: {other}").into()),
    }
}

/// Auto-routed dispatch: tries the router's chosen provider, then walks the
/// rest of `cloud_fallback_order` (previously only index [0] was ever
/// consulted — the rest of the list was decorative), then falls back to
/// local as a guaranteed last resort so an auto-routed request never just
/// fails outright. Returns the provider name that actually served the
/// response, which may differ from `initial`.
///
/// Not used for explicit `/provider <name>` overrides — a user who names a
/// provider directly gets exactly that provider (or a clear error), never a
/// silent substitute they didn't ask for.
async fn dispatch_with_fallback(
    initial: &str,
    fallback_order: &[String],
    prompt: &str,
    config: &Config,
    local: &mut LocalProvider,
    mut on_token: impl FnMut(&str),
) -> (String, Result<ProviderResponse, Box<dyn Error>>) {
    let mut attempts: Vec<String> = vec![initial.to_string()];
    for raw in fallback_order {
        let canonical = raw
            .trim()
            .to_lowercase()
            .parse::<RouteProvider>()
            .map(|p| p.as_str().to_string())
            .unwrap_or_else(|_| raw.trim().to_lowercase());
        if !attempts.contains(&canonical) {
            attempts.push(canonical);
        }
    }
    if !attempts.iter().any(|p| p == "local") {
        attempts.push("local".to_string());
    }

    let mut last_result = None;
    for provider in &attempts {
        let result = dispatch_provider(provider, prompt, config, local, &mut on_token).await;
        let succeeded = result.is_ok();
        last_result = Some((provider.clone(), result));
        if succeeded {
            break;
        }
    }
    last_result.expect("attempts always has at least one entry (initial)")
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

/// Restores the terminal on panic, not just on the happy-path teardown at
/// the end of run_tui_mode.
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
    config.providers.groq = input.trim().to_string();

    print!("Anthropic API key (Enter to skip): ");
    input.clear();
    std::io::stdout().flush()?;
    std::io::stdin().read_line(&mut input)?;
    config.providers.anthropic = input.trim().to_string();

    print!("Gemini API key (Enter to skip): ");
    input.clear();
    std::io::stdout().flush()?;
    std::io::stdin().read_line(&mut input)?;
    config.providers.gemini = input.trim().to_string();

    print!("HuggingFace API key (Enter to skip): ");
    input.clear();
    std::io::stdout().flush()?;
    std::io::stdin().read_line(&mut input)?;
    config.providers.hf = input.trim().to_string();

    print!("\nDaily budget USD (default $5.00): ");
    input.clear();
    std::io::stdout().flush()?;
    std::io::stdin().read_line(&mut input)?;
    config.cost.daily_budget_usd = input.trim().parse().unwrap_or(5.0);

    save_config(&config)?;

    println!(
        "\n  {}",
        theme::green("Config saved to ~/.buzz/config.toml")
    );
    println!("  Run: buzz-cli\n");

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
    } else {
        // Always show at least this much — matching the TUI, which prints
        // "[provider] reason" on every turn with no flag needed. Without
        // this, one-shot mode gives zero visibility into why a prompt went
        // local vs. cloud unless you remember to pass --show-routing.
        println!(
            "{}",
            theme::dim(&format!("[{}] {}", route.provider.as_str(), route.reason))
        );
    }

    let bar = "-".repeat(50);
    println!("\n  Executing Request");
    println!("{}", bar);

    let mut local = LocalProvider::new(
        core_config.local.model_path.clone(),
        core_config.local.max_context_size,
    );
    let mut streamed_anything = false;
    let (served_by, result) = dispatch_with_fallback(
        route.provider.as_str(),
        &core_config.routing.cloud_fallback_order,
        prompt,
        &core_config,
        &mut local,
        |piece| {
            streamed_anything = true;
            print!("{piece}");
            let _ = std::io::stdout().flush();
        },
    )
    .await;
    if served_by != route.provider.as_str() {
        println!(
            "\n{}",
            theme::yellow(&format!(
                "[fallback] {served_by} (originally routed to {})",
                route.provider.as_str()
            ))
        );
    }

    match result {
        Ok(resp) => {
            // Providers that stream (local always; Groq/Anthropic/Gemini as
            // of this session) already printed their content incrementally
            // via on_token — printing resp.content here too would duplicate
            // it. Only providers that don't stream (currently HuggingFace)
            // need it printed after the fact.
            if !streamed_anything {
                println!("{}", resp.content);
            }
            println!();
            println!("{}", bar);
            let served_provider = served_by.parse::<RouteProvider>().unwrap_or(route.provider);
            let cost =
                buzz_core::calculate_cost(resp.input_tokens, resp.output_tokens, served_provider);
            let (_, privacy_flags) = buzz_core::analyze_privacy(prompt);
            buzz_core::audit::log_route(
                &core_config.audit,
                &served_by,
                &route.reason,
                &privacy_flags,
                resp.input_tokens,
                resp.output_tokens,
                cost,
            );
            println!(
                "  Tokens: {} | Cost: ${:.6} | {:.1} tok/s | {:.2}s",
                resp.input_tokens + resp.output_tokens,
                cost,
                resp.tokens_per_second(),
                resp.elapsed_ms / 1000.0
            );
        }
        Err(e) => {
            eprintln!("\n  {}", theme::red(&format!("Error: {}", e)));
            std::process::exit(1);
        }
    }

    println!();
    Ok(())
}

fn run_tui_mode(_default_provider: &str, _show_routing: bool) -> Result<(), Box<dyn Error>> {
    install_panic_restore();

    let mut config = get_config().unwrap_or_default();
    println!(
        "{} — type a message, /settings, /provider <name>, /audit, /reset, or /quit\n",
        theme::bold("Buzz")
    );

    let mut local = LocalProvider::new(
        config.local.model_path.clone(),
        config.local.max_context_size,
    );
    let mut provider_override: Option<String> = None;
    let mut total_tokens: u64 = 0;
    let mut total_spend: f64 = 0.0;
    let mut total_elapsed_ms: f64 = 0.0;
    // Exact-match reply cache for this session: repeating the same question
    // returns the earlier answer instantly instead of re-generating (or
    // re-paying for) it. Keyed on the literal input text.
    let mut cache: std::collections::HashMap<String, (String, usize, f64)> =
        std::collections::HashMap::new();

    let rt = tokio::runtime::Runtime::new()?;
    let rt_handle = rt.handle().clone();

    loop {
        print!("{} ", theme::cyan(">"));
        std::io::stdout().flush()?;
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line)? == 0 {
            break;
        }
        let input = line.trim().to_string();
        if input.is_empty() {
            continue;
        }

        if input == "/quit" || input == "/exit" {
            break;
        }
        if input == "/reset" {
            local.reset();
            total_tokens = 0;
            total_spend = 0.0;
            total_elapsed_ms = 0.0;
            cache.clear();
            println!("Conversation reset.\n");
            continue;
        }
        if input == "/stats" {
            let avg_speed = if total_elapsed_ms > 0.0 {
                total_tokens as f64 / (total_elapsed_ms / 1000.0)
            } else {
                0.0
            };
            println!(
                "Stats: {total_tokens} tokens | ${total_spend:.6} spent | {:.2}s total time | {avg_speed:.1} tok/s avg\n",
                total_elapsed_ms / 1000.0
            );
            continue;
        }
        if input == "/audit" {
            print_audit(&config.audit);
            continue;
        }
        if input == "/settings" || input == "/setup" {
            let cur = get_config().unwrap_or_default();
            println!(
                "Settings — groq: {} | anthropic: {} | gemini: {} | hf: {} | model: {} | daily budget: ${:.2} | max per-request: ${:.2}",
                mask_secret(&cur.providers.groq), mask_secret(&cur.providers.anthropic),
                mask_secret(&cur.providers.gemini), mask_secret(&cur.providers.hf),
                cur.local.model_path, cur.cost.daily_budget_usd, cur.cost.max_per_request_usd
            );
            println!(
                "Change with: /settings <groq|anthropic|gemini|hf|model|budget|maxrequest> <value>\n"
            );
            continue;
        }
        if let Some(rest) = input.strip_prefix("/settings ") {
            let parts: Vec<&str> = rest.splitn(2, char::is_whitespace).collect();
            if parts.len() < 2 {
                println!(
                    "Usage: /settings <groq|anthropic|gemini|hf|model|budget|maxrequest> <value>\n"
                );
                continue;
            }
            let key = parts[0].to_lowercase();
            let value = parts[1].trim();
            let mut cur = get_config().unwrap_or_default();
            let result: Result<String, String> = match key.as_str() {
                "groq" => {
                    cur.providers.groq = value.to_string();
                    Ok("groq key updated".to_string())
                }
                "anthropic" => {
                    cur.providers.anthropic = value.to_string();
                    Ok("anthropic key updated".to_string())
                }
                "gemini" => {
                    cur.providers.gemini = value.to_string();
                    Ok("gemini key updated".to_string())
                }
                "hf" | "huggingface" => {
                    cur.providers.hf = value.to_string();
                    Ok("huggingface key updated".to_string())
                }
                "model" => {
                    cur.local.model_path = value.to_string();
                    Ok(format!("local model path set to: {value}"))
                }
                "budget" => match value.parse::<f64>() {
                    Ok(n) => {
                        cur.cost.daily_budget_usd = n;
                        Ok(format!("daily budget set to ${n:.2}"))
                    }
                    Err(_) => Err(format!("invalid budget value: {value}")),
                },
                "maxrequest" | "max_request" | "max-request" => match value.parse::<f64>() {
                    Ok(n) => {
                        cur.cost.max_per_request_usd = n;
                        Ok(format!("max per-request cost set to ${n:.2}"))
                    }
                    Err(_) => Err(format!("invalid value: {value}")),
                },
                other => Err(format!(
                    "Unknown setting: {other}. Use groq|anthropic|gemini|hf|model|budget|maxrequest"
                )),
            };
            match result {
                Ok(msg) => match save_config(&cur) {
                    Ok(_) => {
                        // Apply immediately — without this, a changed key/budget/model
                        // only takes effect after restarting the session.
                        local.set_model(cur.local.model_path.clone(), cur.local.max_context_size);
                        config = cur;
                        println!("{}\n", theme::green(&msg))
                    }
                    Err(e) => {
                        println!("{}\n", theme::red(&format!("Failed to save settings: {e}")))
                    }
                },
                Err(e) => println!("{}\n", theme::red(&e)),
            }
            continue;
        }
        if let Some(rest) = input.strip_prefix("/provider ") {
            let new_p = rest.trim().to_lowercase();
            match new_p.parse::<RouteProvider>() {
                Ok(route_provider) => {
                    let picked = route_provider.as_str().to_string();
                    provider_override = Some(picked.clone());
                    println!("Next message only will use: {picked} (auto-routing resumes after)\n");
                }
                Err(_) => {
                    println!(
                        "{}\n",
                        theme::yellow(&format!(
                            "Unknown provider: {rest}. Use groq|anthropic|gemini|hf|local"
                        ))
                    )
                }
            }
            continue;
        }
        if input == "/provider" {
            println!("1. local");
            println!("2. cloud");
            println!("3. add new (set a key for an unconfigured provider)");
            print!("{} ", theme::cyan(">"));
            std::io::stdout().flush()?;
            let mut choice = String::new();
            if std::io::stdin().read_line(&mut choice)? == 0 {
                continue;
            }
            match choice.trim() {
                "1" => {
                    let models = list_local_models(&config.local.model_path);
                    if models.is_empty() {
                        println!("No .gguf files found next to {}\n", config.local.model_path);
                        continue;
                    }
                    for (i, m) in models.iter().enumerate() {
                        let current = m.to_str() == Some(config.local.model_path.as_str());
                        println!(
                            "  {}. {}{}",
                            i + 1,
                            m.file_name().unwrap_or_default().to_string_lossy(),
                            if current { "  (current)" } else { "" }
                        );
                    }
                    print!("{} ", theme::cyan(">"));
                    std::io::stdout().flush()?;
                    let mut pick = String::new();
                    if std::io::stdin().read_line(&mut pick)? == 0 {
                        continue;
                    }
                    match pick.trim().parse::<usize>() {
                        Ok(n) if n >= 1 && n <= models.len() => {
                            let chosen = &models[n - 1];
                            let mut cur = get_config().unwrap_or_default();
                            cur.local.model_path = chosen.to_string_lossy().to_string();
                            if let Some(stem) = chosen.file_stem() {
                                cur.local.model_name = stem.to_string_lossy().to_string();
                            }
                            match save_config(&cur) {
                                Ok(_) => {
                                    local.set_model(
                                        cur.local.model_path.clone(),
                                        cur.local.max_context_size,
                                    );
                                    println!(
                                        "{}\n",
                                        theme::green(&format!(
                                            "Local model switched to: {}",
                                            cur.local.model_path
                                        ))
                                    );
                                    config = cur;
                                }
                                Err(e) => println!(
                                    "{}\n",
                                    theme::red(&format!("Failed to save settings: {e}"))
                                ),
                            }
                        }
                        _ => println!("{}\n", theme::yellow("Cancelled — no valid selection.")),
                    }
                }
                "2" => {
                    println!("  1. groq        — {}", mask_secret(&config.providers.groq));
                    println!(
                        "  2. anthropic   — {}",
                        mask_secret(&config.providers.anthropic)
                    );
                    println!(
                        "  3. gemini      — {}",
                        mask_secret(&config.providers.gemini)
                    );
                    println!("  4. huggingface — {}", mask_secret(&config.providers.hf));
                    print!("{} ", theme::cyan(">"));
                    std::io::stdout().flush()?;
                    let mut pick = String::new();
                    if std::io::stdin().read_line(&mut pick)? == 0 {
                        continue;
                    }
                    let picked = match pick.trim() {
                        "1" => Some("groq"),
                        "2" => Some("anthropic"),
                        "3" => Some("gemini"),
                        "4" => Some("huggingface"),
                        _ => None,
                    };
                    match picked {
                        Some(p) => {
                            provider_override = Some(p.to_string());
                            println!(
                                "Next message only will use: {p} (auto-routing resumes after)\n"
                            );
                        }
                        None => println!("{}\n", theme::yellow("Cancelled — no valid selection.")),
                    }
                }
                "3" => {
                    print!("Provider to add a key for (groq|anthropic|gemini|hf): ");
                    std::io::stdout().flush()?;
                    let mut name = String::new();
                    if std::io::stdin().read_line(&mut name)? == 0 {
                        continue;
                    }
                    let name = name.trim().to_lowercase();
                    if !matches!(
                        name.as_str(),
                        "groq" | "anthropic" | "gemini" | "hf" | "huggingface"
                    ) {
                        println!(
                            "{}\n",
                            theme::yellow(&format!(
                                "Unknown provider: {name}. Use groq|anthropic|gemini|hf"
                            ))
                        );
                        continue;
                    }
                    print!("API key for {name}: ");
                    std::io::stdout().flush()?;
                    let mut key = String::new();
                    if std::io::stdin().read_line(&mut key)? == 0 {
                        continue;
                    }
                    let key = key.trim().to_string();
                    let mut cur = get_config().unwrap_or_default();
                    match name.as_str() {
                        "groq" => cur.providers.groq = key,
                        "anthropic" => cur.providers.anthropic = key,
                        "gemini" => cur.providers.gemini = key,
                        _ => cur.providers.hf = key,
                    }
                    match save_config(&cur) {
                        Ok(_) => {
                            config = cur;
                            println!("{}\n", theme::green(&format!("{name} key saved.")));
                        }
                        Err(e) => {
                            println!("{}\n", theme::red(&format!("Failed to save settings: {e}")))
                        }
                    }
                }
                other => println!("{}\n", theme::yellow(&format!("Unknown option: {other}"))),
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

        let (provider, route_reason, is_override) = if let Some(p) = provider_override.take() {
            let r = format!("manual override: /provider {p}");
            (p, r, true)
        } else {
            let route = decide_route(&input, &config.routing);
            (route.provider.as_str().to_string(), route.reason, false)
        };
        println!("{}", theme::dim(&format!("[{provider}] {route_reason}")));

        let mut out = std::io::stdout();
        let mut streamed_anything = false;
        let on_token = |piece: &str| {
            streamed_anything = true;
            print!("{piece}");
            let _ = out.flush();
        };
        // An explicit /provider override gets exactly that provider (or a
        // clear error) — never a silent substitute. Auto-routing gets the
        // full fallback chain.
        let (served_by, result) = if is_override {
            let r = rt_handle.block_on(dispatch_provider(
                &provider, &input, &config, &mut local, on_token,
            ));
            (provider.clone(), r)
        } else {
            rt_handle.block_on(dispatch_with_fallback(
                &provider,
                &config.routing.cloud_fallback_order,
                &input,
                &config,
                &mut local,
                on_token,
            ))
        };
        if served_by != provider {
            println!(
                "\n{}",
                theme::yellow(&format!(
                    "[fallback] {served_by} (originally routed to {provider})"
                ))
            );
        }

        match result {
            Ok(resp) => {
                // Providers that stream (local always; Groq/Anthropic/Gemini
                // as of this session) already printed their content
                // incrementally via on_token — printing resp.content here
                // too would duplicate it. Only providers that don't stream
                // (currently HuggingFace) need it printed after the fact.
                if !streamed_anything {
                    println!("{}", sanitize_terminal_text(&resp.content));
                }
                println!();
                let served_provider: RouteProvider = served_by
                    .parse()
                    .expect("served_by always comes from RouteProvider::as_str() or validated /provider input");
                let cost = buzz_core::calculate_cost(
                    resp.input_tokens,
                    resp.output_tokens,
                    served_provider,
                );
                let tokens = resp.input_tokens + resp.output_tokens;
                total_tokens += tokens;
                total_spend += cost;
                total_elapsed_ms += resp.elapsed_ms;
                let (_, privacy_flags) = buzz_core::analyze_privacy(&input);
                buzz_core::audit::log_route(
                    &config.audit,
                    &served_by,
                    &route_reason,
                    &privacy_flags,
                    resp.input_tokens,
                    resp.output_tokens,
                    cost,
                );
                let speed = resp.tokens_per_second();
                let secs = resp.elapsed_ms / 1000.0;
                println!(
                    "({tokens} tokens · ${cost:.6} this reply · {speed:.1} tok/s · {secs:.2}s · {total_tokens} tokens · ${total_spend:.6} total)\n"
                );
                cache.insert(
                    input.clone(),
                    (sanitize_terminal_text(&resp.content), tokens as usize, cost),
                );
            }
            Err(e) => {
                println!(
                    "{}\n",
                    theme::red(&format!(
                        "Error: {}",
                        sanitize_terminal_text(&format!("{e}"))
                    ))
                );
            }
        }
    }

    println!("\n  {}", theme::bold("Buzz — Session Summary"));
    println!(
        "  Tokens: {total_tokens} | Spent: ${total_spend:.6} | Time: {:.2}s\n",
        total_elapsed_ms / 1000.0
    );

    Ok(())
}

fn select_provider_name(override_opt: &Option<String>) -> String {
    override_opt
        .as_ref()
        .map(|s| s.to_lowercase())
        .or_else(|| std::env::var("PROVIDER").ok())
        .unwrap_or_else(|| "groq".to_string())
}

fn mask_secret(s: &str) -> String {
    if s.is_empty() {
        "(not set)".to_string()
    } else {
        format!("set ({} chars)", s.len())
    }
}

/// Prints the audit trail as an actual feature — a summary plus the most
/// recent requests — instead of it only existing as a JSONL file you'd
/// have to know to go find and grep yourself.
fn print_audit(config: &buzz_core::policy::AuditConfig) {
    if !config.enabled {
        println!("Audit logging is disabled (audit.enabled = false in config.toml).\n");
        return;
    }
    let summary = buzz_core::audit::summarize(config);
    if summary.total_requests == 0 {
        println!("No audit entries yet — logged at {}\n", config.log_path);
        return;
    }
    let now = buzz_core::audit::now_unix();
    let span = match (summary.earliest_timestamp, summary.latest_timestamp) {
        (Some(earliest), Some(latest)) => format!(
            "{} — {}",
            buzz_core::audit::relative_time(now, earliest),
            buzz_core::audit::relative_time(now, latest)
        ),
        _ => "unknown".to_string(),
    };
    println!("Audit — {} requests ({})", summary.total_requests, span);
    println!(
        "  Local: {} | Cloud: {} | Sensitive-flagged: {} | Total spend: ${:.6}",
        summary.local_count, summary.cloud_count, summary.sensitive_count, summary.total_cost
    );
    println!("  Log: {}", config.log_path);
    match buzz_core::audit::verify_chain(config) {
        buzz_core::audit::ChainStatus::Verified {
            chained_count,
            legacy_count,
        } => {
            if legacy_count > 0 {
                println!(
                    "  {}",
                    theme::green(&format!(
                        "Integrity: OK — {chained_count} hash-chained entries intact ({legacy_count} older entries predate chaining, unverifiable)"
                    ))
                );
            } else {
                println!(
                    "  {}",
                    theme::green(&format!(
                        "Integrity: OK — all {chained_count} entries hash-chained and intact"
                    ))
                );
            }
        }
        buzz_core::audit::ChainStatus::Broken { at_line, reason } => {
            println!(
                "  {}",
                theme::red(&format!("Integrity: BROKEN — entry {at_line}: {reason}"))
            );
        }
        buzz_core::audit::ChainStatus::Empty => {}
    }
    println!("\n  Recent:");
    for entry in buzz_core::audit::recent(config, 10) {
        let when = buzz_core::audit::relative_time(now, entry.timestamp);
        let flags = if entry.privacy_flags.is_empty() {
            String::new()
        } else {
            format!(" [{}]", entry.privacy_flags.join(", "))
        };
        println!(
            "    {:<8} [{:<10}] {} — ${:.6}{}",
            when, entry.provider, entry.reason, entry.cost_usd, flags
        );
    }
    println!();
}

/// GGUF model files sitting next to the currently configured model, sorted
/// by filename. This is the browsable list `/provider` offers instead of
/// requiring the full path via `/settings model <path>`.
fn list_local_models(current_model_path: &str) -> Vec<std::path::PathBuf> {
    let dir = std::path::Path::new(current_model_path)
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."));
    let mut models: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("gguf"))
        .collect();
    models.sort();
    models
}

fn save_config(config: &Config) -> Result<(), Box<dyn Error>> {
    // Always round-trip through serde (Config::save_to_file), never hand-build
    // the TOML text — a hand-rolled writer here previously dropped fields
    // (max_context_size, max_per_request_usd, log_path) that strict parsing
    // then required, which is exactly what caused the local-model-path bug.
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let path = std::path::PathBuf::from(format!("{}/.buzz/config.toml", home));
    config.save_to_file(&path)
}

fn get_config() -> Result<Config, Box<dyn Error>> {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let path = std::path::PathBuf::from(format!("{}/.buzz/config.toml", home));
    match Config::load_from_file(&path) {
        Ok(cfg) => Ok(cfg),
        Err(e) => {
            eprintln!(
                "[buzz] warning: could not load config ({}): {}",
                path.display(),
                e
            );
            Ok(Config::default())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_secret_reports_presence_without_leaking_value() {
        assert_eq!(mask_secret(""), "(not set)");
        let masked = mask_secret("sk-supersecret");
        assert_eq!(masked, "set (14 chars)");
        assert!(!masked.contains("supersecret"));
    }

    #[test]
    fn list_local_models_finds_only_gguf_files_sorted() {
        let dir = std::env::temp_dir().join(format!(
            "buzz-model-list-test-{:?}",
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("z-model.gguf"), b"").unwrap();
        std::fs::write(dir.join("a-model.gguf"), b"").unwrap();
        std::fs::write(dir.join("readme.txt"), b"").unwrap();

        let current = dir.join("z-model.gguf");
        let models = list_local_models(current.to_str().unwrap());

        assert_eq!(models.len(), 2);
        assert!(models[0].to_string_lossy().contains("a-model.gguf"));
        assert!(models[1].to_string_lossy().contains("z-model.gguf"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_local_models_empty_dir_returns_empty() {
        let dir = std::env::temp_dir().join(format!(
            "buzz-model-list-empty-test-{:?}",
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let current = dir.join("nonexistent.gguf");
        let models = list_local_models(current.to_str().unwrap());
        assert!(models.is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn require_key_rejects_empty_string() {
        let err = require_key("", "groq_api_key").unwrap_err();
        assert!(err.to_string().contains("No API key configured"));
        assert!(err.to_string().contains("groq_api_key"));
    }

    #[test]
    fn require_key_rejects_whitespace_only() {
        assert!(require_key("   ", "anthropic_api_key").is_err());
    }

    #[test]
    fn require_key_accepts_and_returns_a_real_key() {
        let key = require_key("sk-real-value", "hf_api_key").unwrap();
        assert_eq!(key, "sk-real-value");
    }

    #[test]
    fn sanitize_terminal_text_keeps_newlines_and_tabs() {
        let out = sanitize_terminal_text("line one\nline\ttwo");
        assert_eq!(out, "line one\nline\ttwo");
    }

    #[test]
    fn sanitize_terminal_text_strips_escape_and_control_chars() {
        let out = sanitize_terminal_text("hello\x1b[31mworld\x07");
        assert_eq!(out, "hello[31mworld");
    }

    #[test]
    fn sanitize_terminal_text_leaves_plain_text_untouched() {
        let out = sanitize_terminal_text("The capital of France is Paris.");
        assert_eq!(out, "The capital of France is Paris.");
    }

    #[test]
    fn select_provider_name_uses_override_when_present() {
        let picked = select_provider_name(&Some("Anthropic".to_string()));
        assert_eq!(picked, "anthropic");
    }

    #[test]
    fn select_provider_name_falls_back_to_groq_when_nothing_set() {
        let prior = std::env::var("PROVIDER").ok();
        std::env::remove_var("PROVIDER");
        let picked = select_provider_name(&None);
        assert_eq!(picked, "groq");
        if let Some(v) = prior { std::env::set_var("PROVIDER", v); }
    }

    #[test]
    fn check_budget_allows_local_regardless_of_config() {
        let mut config = Config::default();
        config.cost.max_per_request_usd = 0.0;
        config.cost.daily_budget_usd = 0.0;
        assert!(check_budget(&config, RouteProvider::Local, "anything at all").is_ok());
    }

    #[test]
    fn check_budget_blocks_when_per_request_limit_is_effectively_zero() {
        let mut config = Config::default();
        config.cost.max_per_request_usd = 0.0;
        let result = check_budget(&config, RouteProvider::Groq, "a normal length prompt");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().len() > 0);
    }

    #[test]
    fn check_budget_allows_a_normal_prompt_under_default_limits() {
        let config = Config::default();
        assert!(check_budget(&config, RouteProvider::Groq, "hi").is_ok());
    }
}
