//! buzz-gateway — the HTTP surface from deck slide 05.
//!
//! `buzz-cli serve -port 8787` turns one developer's CLI into
//! infrastructure a security team deploys. This binary is that listener.
//!
//! Hard constraints from the deck, non-negotiable:
//! - Loopback-only bind. Never reachable off the machine. This is
//!   hardcoded below, not a config flag — a flag is something that gets
//!   flipped by accident in a script; a hardcoded 127.0.0.1 isn't.
//! - Fails closed (slide 10). If the gateway dies or a route can't be
//!   decided, callers get a clear error — never a silent reroute to a
//!   cloud provider, never a silent hang.
//!
//! ## Crash / restart policy (deck slide 10, formerly an open decision)
//!
//! This process does **not** supervise or restart itself — no watchdog
//! thread, no self-exec-on-panic, nothing. If it crashes, it stays down
//! until something outside it restarts it. That's a deliberate choice,
//! not an oversight: process supervision is a solved problem one layer
//! down (systemd/launchd/a container runtime), and duplicating it in-app
//! only adds a second thing that can be wrong — the classic "who
//! restarts the restarter" problem, plus double-restart races if both
//! layers try at once.
//!
//! The actual policy: run this under systemd with `Restart=on-failure`
//! (see `buzz-gateway.service` in this crate's root, and the README's
//! Operations section). `/healthz` exists for an external liveness probe
//! if one is ever wired up, but the restart trigger itself is systemd
//! watching this process's exit status, not active polling.

mod auth;
mod caller;
mod handlers;
mod local_engine;
mod openai_types;
mod routing;

use auth::default_token_path;
use axum::{routing::post, Router};
use local_engine::LocalEngine;
use routing::RealRouter;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

/// Shared state every handler needs.
pub struct AppState {
    pub token: String,
    pub router: Arc<dyn routing::RouteDecider>,
    /// Provider API keys + local model path, loaded once at startup —
    /// used for dispatch (handlers.rs) and budget commit/release calls.
    /// Same convention as buzz-cli's own config loading.
    pub config: buzz_core::policy::Config,
    /// One dedicated-thread handle for the local qfz3 engine, shared
    /// across every request routed to Local. See local_engine.rs for why
    /// this can't just be a field on AppState directly.
    pub local_engine: Arc<LocalEngine>,
}

const DEFAULT_PORT: u16 = 8787;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // JSON output, not the default human-readable formatter: this runs
    // as a systemd-supervised service (see the crash/restart policy
    // above), not something a developer stares at in a terminal, so logs
    // need to be `jq`/grep-parseable by whatever tails the journal —
    // structured fields (level, target, message) beat scraping formatted
    // text. Deliberately not pulling in a full tracing/OpenTelemetry
    // pipeline here — that's more than "can an ops person parse this."
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "buzz_gateway=info".into()),
        )
        .init();

    // --- Loopback-only, hardcoded. Do not parametrize this. ---
    // The port, unlike the address, is intentionally configurable — via
    // env var rather than a CLI flag on this binary, since `buzz-cli
    // serve --port` (main.rs in buzz-cli) is the real entry point and
    // execs this binary, threading the requested port through this way.
    // Running `buzz-gateway` directly still works with no env var set —
    // falls back to `DEFAULT_PORT`, same as before this existed.
    let port = match std::env::var("BUZZ_GATEWAY_PORT") {
        Ok(val) => val
            .parse::<u16>()
            .map_err(|e| anyhow::anyhow!("invalid BUZZ_GATEWAY_PORT={val:?}: {e}"))?,
        Err(_) => DEFAULT_PORT,
    };
    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);

    // --- Token: issue + persist per slide 05's design intent ---
    let token_path = default_token_path();
    let token = auth::issue_and_persist(&token_path)
        .map_err(|e| anyhow::anyhow!("failed to issue gateway token: {e}"))?;
    tracing::info!("token written to {}", token_path.display());

    // A config file that exists but fails to parse is a hard startup
    // failure, not a silent fall-back to defaults — see
    // `load_default_config`'s doc comment for why. A missing file is
    // fine (fresh install) and returns `Ok` with defaults.
    let config = routing::load_default_config()
        .map_err(|e| anyhow::anyhow!("invalid ~/.buzz/config.toml: {e}"))?;
    // Non-fatal gaps (missing provider keys, no local model downloaded
    // yet) — logs a `tracing::warn!` per gap naming exactly what's
    // missing, rather than starting silently degraded.
    routing::validate_config(&config);

    let local_engine = LocalEngine::new(
        config.local.model_path.clone(),
        config.local.max_context_size,
    );

    let state = Arc::new(AppState {
        token,
        router: Arc::new(RealRouter::new(config.clone())),
        config,
        local_engine,
    });

    let app = Router::new()
        .route("/v1/chat/completions", post(handlers::chat_completions))
        .route("/healthz", axum::routing::get(handlers::healthz))
        .layer(TraceLayer::new_for_http())
        // Fail closed on hung requests too — a stuck request that never
        // times out is its own kind of silent failure.
        .layer(TimeoutLayer::new(Duration::from_secs(120)))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("listening on http://{addr}");
    println!("✓ listening on http://{addr}");
    println!("  POST /v1/chat/completions");
    println!(
        "  Authorization: Bearer <token from {}>",
        token_path.display()
    );

    axum::serve(listener, app).await?;
    Ok(())
}
