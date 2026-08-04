//! Routing seam (deck slide 04: decide_route is ○ — "scoped / not yet
//! wired"). This is the single most important TODO in this crate.
//!
//! Today, per your own engineering notes, `decide_route` only runs in the
//! one-shot `run_smart_cli` path. The interactive TUI drives provider
//! choice off a manually-set `app.provider_name`, bypassing it entirely.
//! `buzz-gateway` MUST default every request through real `decide_route`
//! (sensitivity × complexity), not a manual override — that's the whole
//! point of "one policy engine, every request" (slide 03).
//!
//! Deck slide 09, Hard Part B: decide_route has never run under
//! concurrent multi-client load. This module's `RouteDecider` trait is a
//! single-request interface on purpose — it forces you to reason about
//! whether the real decide_route implementation is safe to call from N
//! concurrent Tokio tasks (interior mutability? shared budget state
//! needing a lock or atomic?) before this compiles against the real type.

use crate::openai_types::ChatMessage;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteTarget {
    Local,
    Groq,
    Gemini,
    HuggingFace,
    OpenAi,
}

impl RouteTarget {
    pub fn as_str(&self) -> &'static str {
        match self {
            RouteTarget::Local => "local",
            RouteTarget::Groq => "groq",
            RouteTarget::Gemini => "gemini",
            RouteTarget::HuggingFace => "huggingface",
            RouteTarget::OpenAi => "openai",
        }
    }
}

#[derive(Debug)]
pub struct RouteDecision {
    pub target: RouteTarget,
    /// The buzz-core provider `target` was mapped from — kept alongside
    /// `target` so callers can compute cost (`buzz_core::calculate_cost`
    /// takes this enum, not `RouteTarget`) without a fragile reverse
    /// mapping back from the gateway's own 4-variant enum.
    pub provider: buzz_core::RouteProvider,
    /// Human-readable reason decide_route picked this target (e.g.
    /// "sensitive content detected (PII/secrets)") — threaded through to
    /// the budget commit/release call so buzz-core's own audit log
    /// records *why*, not just *what*.
    pub reason: String,
    /// True if sensitivity detection forced this local regardless of
    /// what the caller/model field requested — this is the number slide
    /// 08's "Forced to local model (sensitive): 1,932" comes from.
    pub forced_local_for_sensitivity: bool,
    /// This request's hold on the daily budget cap, already reserved by
    /// the time this decision is returned — `None` for Local, which is
    /// exempt. The caller MUST resolve it exactly once: `budget::commit`
    /// after a successful provider call, `budget::release` on failure —
    /// never just drop it, or its share of the cap leaks for the rest of
    /// the day.
    pub reservation: Option<buzz_core::budget::Reservation>,
}

#[derive(Debug, thiserror::Error)]
pub enum RouteError {
    #[error("budget cap exceeded")]
    BudgetExceeded,
    #[error("no provider available")]
    NoProviderAvailable,
}

/// TODO: this is the seam. Replace `PlaceholderRouter` below with a type
/// that wraps your real `decide_route` + `budget::check` from buzz-core.
/// The trait is `Send + Sync` deliberately — Axum handlers run on a
/// multi-threaded Tokio runtime, so whatever backs this needs to be safe
/// to call from concurrent requests. If your current decide_route/budget
/// implementation isn't yet — e.g. it mutates shared state without a
/// lock — that's exactly the "never run under concurrent load" risk the
/// deck calls out, and it needs to be fixed here, not papered over.
pub trait RouteDecider: Send + Sync {
    fn decide(
        &self,
        messages: &[ChatMessage],
        requested_model: &str,
    ) -> Result<RouteDecision, RouteError>;
}

/// Real decider: wraps `buzz_core::decide_route` + `buzz_core::budget::reserve`,
/// the same functions `buzz-cli`'s one-shot `run_smart_cli` path uses
/// (buzz-cli/src/main.rs), with `reserve` in place of the plain `check`
/// buzz-cli itself still uses. `decide_route`/`analyze_complexity`/
/// `scan_text` are pure functions with no interior mutability, so calling
/// them from concurrent Axum handlers is safe. `budget::reserve` atomically
/// checks *and* holds this request's estimated cost against the daily cap
/// under one lock — the fix for the TOCTOU race a plain `check` has under
/// concurrent callers (two requests reading the same pre-write "spent
/// today" total and both proceeding). The returned reservation MUST be
/// committed or released by whoever dispatches the actual provider call
/// (see `handlers.rs`) — decide() only reserves, it never resolves.
///
/// `decide_route` operates on a single prompt string, not a message list —
/// mirroring the CLI, every message's content is flattened into one string
/// so sensitivity scanning sees the whole conversation, not just the
/// latest turn.
///
/// `RouteProvider` (buzz-core) has 4 variants; `RouteTarget` (this crate)
/// mirrors all 4 (`OpenAi` aside — buzz-core has no such provider, so
/// `decide_route` can never produce it; the variant is dead, kept only
/// because `dispatch` in handlers.rs still matches on it defensively).
/// `Gemini`/`HuggingFace` dispatch through the same buzz-cli provider
/// clients (`GeminiProvider`/`HuggingFaceProvider`) buzz-cli's own
/// `run_smart_cli` path already uses — see `dispatch` in handlers.rs.
/// Loads `~/.buzz/config.toml`, matching the CLI's own convention
/// (buzz-cli/src/main.rs `get_config`). Shared by `RealRouter` and by
/// `main.rs`, which needs its own copy for provider dispatch (API keys,
/// local model path) independent of routing decisions.
///
/// Deliberately distinguishes two very different failure shapes instead
/// of collapsing both into `unwrap_or_default()`:
/// - No file at that path at all — a fresh install with nothing
///   configured yet. Legitimate; falls back to `Config::default()`
///   (local-only, no cloud keys) with no error.
/// - A file exists but doesn't parse (typo'd TOML, truncated write,
///   wrong format). Silently defaulting here would start the gateway
///   successfully on a broken config and let the operator's real
///   settings (provider keys, budget caps) vanish without a trace —
///   they'd only notice when cloud routing mysteriously stopped working.
///   Returned as `Err` so `main.rs` can turn it into a hard startup
///   failure instead.
pub fn load_default_config() -> Result<buzz_core::policy::Config, String> {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let path = std::path::PathBuf::from(format!("{home}/.buzz/config.toml"));
    if !path.exists() {
        return Ok(buzz_core::policy::Config::default());
    }
    buzz_core::Config::load_from_file(&path)
        .map_err(|e| format!("{} exists but could not be parsed: {e}", path.display()))
}

/// Startup-time sanity check on an already-loaded config: not "is this
/// TOML well-formed" (that's `load_default_config`'s job) but "can this
/// config actually serve the requests decide_route is going to send it."
/// Neither gap below is fatal by itself — local-only (no cloud keys) and
/// cloud-only (no local model downloaded yet) are both legitimate
/// deployments — but running with one silently is exactly the "starts
/// fine, fails confusingly on the first real request" failure mode this
/// closes. Every gap is logged as an explicit `tracing::warn!` naming
/// exactly what's missing and what it breaks, so it shows up in startup
/// logs immediately instead of as a mystery 502 later.
pub fn validate_config(config: &buzz_core::policy::Config) {
    match config.routing.cloud_fallback_order.first() {
        None => {
            tracing::warn!(
                "routing.cloud_fallback_order is empty in ~/.buzz/config.toml — every request \
                 will route to local regardless of complexity; cloud routing is fully disabled"
            );
        }
        // Mirrors decide_route's own selection exactly (core/decision.rs):
        // only the *first* fallback entry is ever actually reachable
        // today, so that's the only key worth validating here.
        Some(first) => {
            let (name, key) = match first.to_lowercase().as_str() {
                "gemini" => ("gemini", &config.providers.gemini),
                "huggingface" | "hf" => ("huggingface", &config.providers.hf),
                _ => ("groq", &config.providers.groq),
            };
            if key.trim().is_empty() {
                tracing::warn!(
                    "{name}_api_key not set in ~/.buzz/config.toml — local-only routing will \
                     work, cloud routing will fail closed on any {name}-routed request"
                );
            }
        }
    }

    // Mirrors audit.rs's own tilde expansion convention.
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let model_path = match config.local.model_path.strip_prefix("~/") {
        Some(rest) => std::path::PathBuf::from(&home).join(rest),
        None => std::path::PathBuf::from(&config.local.model_path),
    };
    if !model_path.exists() {
        tracing::warn!(
            "local model not found at {} — any request routed to local (decide_route's default \
             for simple/medium-complexity and sensitivity-forced prompts) will fail closed \
             instead of falling back to cloud",
            model_path.display()
        );
    }
}

pub struct RealRouter {
    config: buzz_core::policy::Config,
}

impl RealRouter {
    pub fn new(config: buzz_core::policy::Config) -> Self {
        Self { config }
    }
}

impl RouteDecider for RealRouter {
    fn decide(
        &self,
        messages: &[ChatMessage],
        _requested_model: &str,
    ) -> Result<RouteDecision, RouteError> {
        let prompt = messages
            .iter()
            .map(|m| m.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        let route = buzz_core::decide_route(&prompt, &self.config.routing);

        // Mirrors decide_route's own first branch condition exactly, so
        // this only reports "forced" when that's the actual reason the
        // decision came back Local — not for every Local decision.
        let forced_local_for_sensitivity =
            self.config.routing.always_local_if_sensitive && buzz_core::scan_text(&prompt);

        let target = match route.provider {
            buzz_core::RouteProvider::Local => RouteTarget::Local,
            buzz_core::RouteProvider::Groq => RouteTarget::Groq,
            buzz_core::RouteProvider::Gemini => RouteTarget::Gemini,
            buzz_core::RouteProvider::HuggingFace => RouteTarget::HuggingFace,
        };

        let estimated_cost = buzz_core::budget::estimate_cost(&prompt, route.provider);
        let reservation = buzz_core::budget::reserve(&self.config, route.provider, estimated_cost)
            .map_err(|_| RouteError::BudgetExceeded)?;

        Ok(RouteDecision {
            target,
            provider: route.provider,
            reason: route.reason,
            forced_local_for_sensitivity,
            reservation,
        })
    }
}
