//! Thread-safe handle around the local qfz3 engine for use from Axum's
//! multi-threaded runtime.
//!
//! `qfz3::Engine` (wrapped inside `buzz_cli::providers::LocalProvider`)
//! holds raw ggml pointers and is deliberately not `Send`/`Sync` — see the
//! safety note on `qfz3_engine::engine::Engine`. That note also states the
//! sanctioned way around it: "ggml contexts are not thread-affine, so
//! mutex-serialized access is sound" as long as nothing ever touches the
//! engine without holding that mutex. `LocalEngine` is exactly that lock,
//! plus the `unsafe impl Send/Sync` the note says is safe to assert once
//! serialization is guaranteed.
//!
//! Every call runs on a dedicated blocking-pool thread via
//! `spawn_blocking`, never on a Tokio worker — generation can take
//! seconds, and blocking a worker thread for that long would stall other
//! requests' async I/O. `LocalProvider::generate_blocking` is a plain
//! synchronous function (there is no real `.await` anywhere in the local
//! generation path), so nothing here needs an inner async runtime.

use buzz_cli::providers::LocalProvider;
use buzz_core::ProviderResponse;
use std::sync::{Arc, Mutex};

pub struct LocalEngine(Mutex<LocalProvider>);

// SAFETY: see module doc — qfz3::Engine's raw pointers are not
// thread-affine, and `generate` below is the only way to reach the inner
// `LocalProvider`, always through the mutex, never released mid-use.
unsafe impl Send for LocalEngine {}
unsafe impl Sync for LocalEngine {}

impl LocalEngine {
    /// Expands a leading `~/` against $HOME before handing the path to
    /// LocalProvider — mirrors audit.rs's expand_tilde convention.
    /// Without this, a raw "~/..." string only resolves correctly under
    /// an interactive shell; under systemd (no shell, no tilde
    /// expansion) it's passed through literally and the model file load
    /// fails with a confusing "No such file or directory" even when the
    /// file exists at the real, expanded path.
    fn expand_tilde(path: &str) -> String {
        match path.strip_prefix("~/") {
            Some(rest) => {
                let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
                std::path::PathBuf::from(home)
                    .join(rest)
                    .to_string_lossy()
                    .to_string()
            }
            None => path.to_string(),
        }
    }

    pub fn new(model_path: String, context_size: usize) -> Arc<Self> {
        Arc::new(Self(Mutex::new(LocalProvider::new(
            Self::expand_tilde(&model_path),
            context_size,
        ))))
    }

    /// Runs one generation to completion, serialized against every other
    /// concurrent call via the inner lock — the engine (one physical model
    /// instance, one KV cache) can only ever serve one request at a time,
    /// regardless of how many gateway requests are in flight.
    ///
    /// Resets the KV cache before generating: unlike the CLI's single
    /// long-lived conversation, every gateway request already carries its
    /// full message history (OpenAI's stateless wire format) — carrying
    /// KV-cache state across *different* HTTP callers would leak one
    /// caller's conversation context into another's completion, which is
    /// both wrong and a real cross-tenant privacy issue for a gateway
    /// meant to enforce sensitivity-based routing.
    pub async fn generate(
        self: &Arc<Self>,
        prompt: String,
        mut on_token: impl FnMut(&str) + Send + 'static,
    ) -> Result<ProviderResponse, String> {
        let this = Arc::clone(self);
        tokio::task::spawn_blocking(move || {
            let mut local = this.0.lock().unwrap_or_else(|e| e.into_inner());
            local.reset();
            local
                .generate_blocking(&prompt, &mut on_token)
                .map_err(|e| e.to_string())
        })
        .await
        .unwrap_or_else(|join_err| Err(format!("local engine task panicked: {join_err}")))
    }
}
