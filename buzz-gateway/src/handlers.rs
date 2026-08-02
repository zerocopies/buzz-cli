//! Handlers for /v1/chat/completions and /healthz.
//!
//! Fail-closed discipline (deck slide 10) applies here concretely:
//! - Bad/missing auth -> 401, immediately, before any routing work.
//! - Routing failure (budget exceeded, no provider) -> a clear OpenAI-
//!   shaped error, never a silent fallback to a cloud provider the
//!   sensitivity check would have blocked.
//! - Provider failure (including mid-stream) -> release the reservation
//!   and a clear error, never a silent reroute to a different provider
//!   than the one decide_route actually chose. That's the compliance
//!   guarantee this whole gateway exists for — see `dispatch` below.
//! - Gateway process dying is handled by the OS (connection refused) —
//!   nothing to do here for that case, which is the point: there's no
//!   "catch the crash and reroute" code to write, on purpose.

use crate::openai_types::{
    ChatCompletionChunk, ChatCompletionRequest, ChatCompletionResponse, ChatMessage, Choice,
    ChunkChoice, Delta, ErrorDetail, ErrorResponse, Usage,
};
use crate::routing::{RouteDecision, RouteError, RouteTarget};
use crate::{caller, AppState};

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use axum::Json;
use buzz_core::InferenceProvider;
use futures::stream::Stream;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

pub async fn healthz() -> &'static str {
    "ok"
}

/// Auth check, shared by streaming and non-streaming paths. Constant-time
/// compare (see auth::verify) — no early-exit string comparison on a
/// secret, ever.
fn check_auth(state: &AppState, headers: &HeaderMap) -> Result<(), Box<Response>> {
    let provided = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));

    match provided {
        Some(token) if auth_ok(state, token) => Ok(()),
        _ => Err(Box::new(error_response(
            StatusCode::UNAUTHORIZED,
            "invalid or missing bearer token",
            "invalid_request_error",
        ))),
    }
}

fn auth_ok(state: &AppState, provided: &str) -> bool {
    crate::auth::verify(&state.token, provided)
}

fn error_response(status: StatusCode, message: &str, error_type: &str) -> Response {
    let body = ErrorResponse {
        error: ErrorDetail {
            message: message.to_string(),
            error_type: error_type.to_string(),
            code: None,
        },
    };
    (status, Json(body)).into_response()
}

/// Every message's content, role-prefixed and newline-joined, into the
/// single prompt string the provider clients expect (they were built for
/// buzz-cli's one-turn-at-a-time usage, not a structured messages array).
/// Distinct from routing.rs's own flattening (content-only, no role
/// labels) — that one only feeds a heuristic sensitivity/complexity scan,
/// this one is what the model actually reads, so preserving who-said-what
/// materially affects response quality on multi-turn conversations.
fn build_prompt(messages: &[ChatMessage]) -> String {
    messages
        .iter()
        .map(|m| format!("{}: {}", m.role, m.content))
        .collect::<Vec<_>>()
        .join("\n")
}

pub async fn chat_completions(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<ChatCompletionRequest>,
) -> Response {
    if let Err(resp) = check_auth(&state, &headers) {
        return *resp;
    }

    let request_id = format!("chatcmpl-{}", Uuid::new_v4());
    let caller_id = caller::identify(&headers, req.user.as_deref());

    // --- THE SEAM: real decide_route call happens here. ---
    // Everything downstream of this line is generic OpenAI-shape plumbing;
    // this line is the actual unproven IP the deck calls out (slide 09,
    // Hard Part B). Every request, streaming or not, goes through it —
    // no manual-override bypass like the TUI has today.
    let decision = match state.router.decide(&req.messages, &req.model) {
        Ok(d) => d,
        Err(RouteError::BudgetExceeded) => {
            // The actual provider decide_route would have picked isn't
            // recoverable here — `RouteError::BudgetExceeded` discards it
            // (see routing.rs's `RealRouter::decide`) — so "n/a" is the
            // honest value, not a guess at which provider it would have been.
            buzz_core::audit::log_rejection(
                &state.config.audit,
                &caller_id,
                &request_id,
                "n/a",
                "budget cap exceeded",
            );
            return error_response(
                StatusCode::TOO_MANY_REQUESTS,
                "budget cap exceeded for this request",
                "budget_exceeded",
            );
        }
        Err(RouteError::NoProviderAvailable) => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "no provider available to handle this request",
                "no_provider_available",
            );
        }
    };

    let prompt = build_prompt(&req.messages);

    if req.stream {
        stream_response(state, request_id, req.model, caller_id, decision, prompt).into_response()
    } else {
        blocking_response(state, request_id, req.model, caller_id, decision, prompt)
            .await
            .into_response()
    }
}

/// One attempt at the provider `decide()` already chose and already
/// reserved budget for. Deliberately not a fallback loop: silently trying
/// a *different* provider than the one decide_route picked (e.g. falling
/// back to a cloud provider after a sensitivity-forced local target
/// failed) would violate the exact compliance guarantee this gateway
/// exists to enforce. On failure here, the caller fails the whole
/// request closed — see `blocking_response`/`stream_response`.
async fn dispatch(
    state: &AppState,
    target: &RouteTarget,
    prompt: &str,
    mut on_token: impl FnMut(&str) + Send + 'static,
) -> Result<buzz_core::ProviderResponse, String> {
    match target {
        RouteTarget::Local => {
            state
                .local_engine
                .generate(prompt.to_string(), on_token)
                .await
        }
        RouteTarget::Groq => {
            let key = state.config.providers.groq.trim();
            if key.is_empty() {
                return Err("no groq_api_key configured".to_string());
            }
            buzz_cli::providers::GroqProvider::new(key.to_string(), None)
                .generate(prompt, &mut on_token)
                .await
                .map_err(|e| e.to_string())
        }
        RouteTarget::Anthropic => {
            let key = state.config.providers.anthropic.trim();
            if key.is_empty() {
                return Err("no anthropic_api_key configured".to_string());
            }
            buzz_cli::providers::AnthropicProvider::new(key.to_string(), None)
                .generate(prompt, &mut on_token)
                .await
                .map_err(|e| e.to_string())
        }
        RouteTarget::Gemini => {
            let key = state.config.providers.gemini.trim();
            if key.is_empty() {
                return Err("no gemini_api_key configured".to_string());
            }
            buzz_cli::providers::GeminiProvider::new(key.to_string(), None)
                .generate(prompt, &mut on_token)
                .await
                .map_err(|e| e.to_string())
        }
        // HuggingFace's InferenceProvider impl ignores on_token entirely
        // (see buzz-cli/src/providers/huggingface.rs) — it has no
        // streaming API, so it returns the full response in one shot
        // just like it does for buzz-cli's own CLI streaming path.
        RouteTarget::HuggingFace => {
            let key = state.config.providers.hf.trim();
            if key.is_empty() {
                return Err("no hf_api_key configured".to_string());
            }
            buzz_cli::providers::HuggingFaceProvider::new(key.to_string(), None)
                .generate(prompt, &mut on_token)
                .await
                .map_err(|e| e.to_string())
        }
        // decide_route never actually produces this — buzz-core's
        // RouteProvider has no OpenAI variant, so routing.rs's mapping
        // only ever constructs Local/Groq/Anthropic. Fail closed rather
        // than silently doing nothing if that ever changes.
        RouteTarget::OpenAi => Err("openai routing target has no provider client wired".into()),
    }
}

/// Non-streaming path: dispatch, then resolve the reservation exactly
/// once based on the real outcome — commit with the actual cost on
/// success, release on failure. Never both, never neither.
async fn blocking_response(
    state: Arc<AppState>,
    request_id: String,
    model: String,
    caller_id: String,
    decision: RouteDecision,
    prompt: String,
) -> Response {
    match dispatch(&state, &decision.target, &prompt, |_| {}).await {
        Ok(resp) => {
            let cost =
                buzz_core::calculate_cost(resp.input_tokens, resp.output_tokens, decision.provider);
            buzz_core::budget::commit(
                decision.reservation,
                &state.config,
                &caller_id,
                &request_id,
                decision.target.as_str(),
                &decision.reason,
                &[],
                resp.input_tokens,
                resp.output_tokens,
                cost,
                decision.forced_local_for_sensitivity,
            );

            Json(ChatCompletionResponse {
                id: request_id,
                object: "chat.completion",
                created: chrono::Utc::now().timestamp(),
                model,
                choices: vec![Choice {
                    index: 0,
                    message: ChatMessage {
                        role: "assistant".to_string(),
                        content: resp.content,
                    },
                    finish_reason: "stop".to_string(),
                }],
                usage: Usage {
                    prompt_tokens: resp.input_tokens as u32,
                    completion_tokens: resp.output_tokens as u32,
                    total_tokens: (resp.input_tokens + resp.output_tokens) as u32,
                },
            })
            .into_response()
        }
        Err(e) => {
            buzz_core::budget::release(decision.reservation);
            error_response(
                StatusCode::BAD_GATEWAY,
                &format!("provider failed: {e}"),
                "provider_error",
            )
        }
    }
}

/// Streaming path. The envelope (id/object/created/model per chunk, final
/// chunk with finish_reason set and empty delta, then a literal `[DONE]`)
/// is the exact shape OpenAI clients expect — get this wrong and clients
/// fail silently on parse, which is worse than a visible error.
///
/// Real generation runs on a separate task (`tokio::spawn`) so it can make
/// progress concurrently with this generator draining tokens off a
/// channel and yielding SSE chunks as they arrive — without a second
/// task, nothing would ever poll the generation future while this
/// generator sits parked in `rx.recv().await`, and no tokens would ever
/// actually stream.
fn stream_response(
    state: Arc<AppState>,
    request_id: String,
    model: String,
    caller_id: String,
    decision: RouteDecision,
    prompt: String,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let stream = async_stream::stream! {
        let created = chrono::Utc::now().timestamp();

        // First chunk: role marker, per OpenAI convention.
        let first = ChatCompletionChunk {
            id: request_id.clone(),
            object: "chat.completion.chunk",
            created,
            model: model.clone(),
            choices: vec![ChunkChoice {
                index: 0,
                delta: Delta { role: Some("assistant".into()), content: None },
                finish_reason: None,
            }],
        };
        yield Ok(Event::default().data(serde_json::to_string(&first).unwrap()));

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let gen_task = {
            let state = Arc::clone(&state);
            let target = decision.target.clone();
            let prompt = prompt.clone();
            tokio::spawn(async move {
                dispatch(&state, &target, &prompt, move |piece: &str| {
                    let _ = tx.send(piece.to_string());
                })
                .await
            })
        };

        while let Some(piece) = rx.recv().await {
            let chunk = ChatCompletionChunk {
                id: request_id.clone(),
                object: "chat.completion.chunk",
                created,
                model: model.clone(),
                choices: vec![ChunkChoice {
                    index: 0,
                    delta: Delta { role: None, content: Some(piece) },
                    finish_reason: None,
                }],
            };
            yield Ok(Event::default().data(serde_json::to_string(&chunk).unwrap()));
        }

        match gen_task.await {
            Ok(Ok(resp)) => {
                let cost = buzz_core::calculate_cost(
                    resp.input_tokens,
                    resp.output_tokens,
                    decision.provider,
                );
                buzz_core::budget::commit(
                    decision.reservation,
                    &state.config,
                    &caller_id,
                    &request_id,
                    decision.target.as_str(),
                    &decision.reason,
                    &[],
                    resp.input_tokens,
                    resp.output_tokens,
                    cost,
                    decision.forced_local_for_sensitivity,
                );

                let final_chunk = ChatCompletionChunk {
                    id: request_id.clone(),
                    object: "chat.completion.chunk",
                    created,
                    model: model.clone(),
                    choices: vec![ChunkChoice {
                        index: 0,
                        delta: Delta::default(),
                        finish_reason: Some("stop".into()),
                    }],
                };
                yield Ok(Event::default().data(serde_json::to_string(&final_chunk).unwrap()));
                yield Ok(Event::default().data("[DONE]"));
            }
            Ok(Err(e)) => {
                // Fail closed mid-stream: the client already received some
                // content chunks (or none), but there is no clean SSE
                // "abort" — emit a clear error payload and stop, rather
                // than a bogus finish_reason:"stop" that would claim
                // success, and never silently switch to a different
                // provider than the one decide_route chose.
                buzz_core::budget::release(decision.reservation);
                yield Ok(Event::default().data(format!(
                    "{{\"error\":{{\"message\":\"provider failed: {}\",\"type\":\"provider_error\"}}}}",
                    e.replace('"', "'")
                )));
            }
            Err(join_err) => {
                buzz_core::budget::release(decision.reservation);
                yield Ok(Event::default().data(format!(
                    "{{\"error\":{{\"message\":\"internal error: {}\",\"type\":\"internal_error\"}}}}",
                    join_err.to_string().replace('"', "'")
                )));
            }
        }
    };

    Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::RealRouter;
    use buzz_core::policy::{AuditConfig, Config, CostConfig};
    use buzz_core::RouteProvider;

    fn test_state(groq_key: &str, daily_budget_usd: f64) -> Arc<AppState> {
        let path = std::env::temp_dir().join(format!(
            "buzz-gateway-handlers-test-{:?}.jsonl",
            std::thread::current().id()
        ));
        let _ = std::fs::remove_file(&path);

        let mut config = Config::default();
        config.providers.groq = groq_key.to_string();
        config.cost = CostConfig {
            max_per_request_usd: 10.0,
            daily_budget_usd,
        };
        config.audit = AuditConfig {
            enabled: true,
            log_path: path.to_string_lossy().to_string(),
        };

        Arc::new(AppState {
            token: "test-token".to_string(),
            router: Arc::new(RealRouter::new(config.clone())),
            local_engine: crate::local_engine::LocalEngine::new(
                config.local.model_path.clone(),
                config.local.max_context_size,
            ),
            config,
        })
    }

    /// Regression test for the reservation lifecycle this task wires in:
    /// a provider failure must release its hold on the daily budget, not
    /// leak it. No live provider key or network access needed — an empty
    /// API key forces a deterministic failure in `dispatch`'s own
    /// pre-flight check, before it ever reaches Groq. This exercises the
    /// real `blocking_response` path, not a reimplementation of it.
    #[tokio::test]
    async fn provider_failure_releases_the_reservation_instead_of_leaking_it() {
        const COST: f64 = 1.0;
        // Exactly enough budget for two $1 reservations — proves the
        // first one was actually released back, not just that an empty
        // log starts with room to spare.
        let state = test_state("", COST * 2.0);

        let reservation = buzz_core::budget::reserve(&state.config, RouteProvider::Groq, COST)
            .expect("first reservation should fit under the cap");
        let decision = RouteDecision {
            target: RouteTarget::Groq,
            provider: RouteProvider::Groq,
            reason: "test".to_string(),
            forced_local_for_sensitivity: false,
            reservation,
        };

        let response = blocking_response(
            Arc::clone(&state),
            "req-1".to_string(),
            "test-model".to_string(),
            "test-caller".to_string(),
            decision,
            "hello".to_string(),
        )
        .await;

        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);

        // If the first reservation leaked, this is rejected: two $1
        // reservations don't fit under a $2 cap unless the first one was
        // actually released.
        let second = buzz_core::budget::reserve(&state.config, RouteProvider::Groq, COST);
        assert!(
            second.is_ok(),
            "second reservation was rejected — the first one leaked instead of being released"
        );
        buzz_core::budget::release(second.unwrap());

        let _ = std::fs::remove_file(&state.config.audit.log_path);
    }
}
