use std::error::Error;

use buzz_core::{InferenceProvider, ProviderResponse};

/// Wraps the local qfz3 engine, loading it once on first use and reusing it
/// (and its KV cache / turn counter) for the rest of the session — this is
/// what makes multi-turn memory work and avoids reloading the model per
/// message. Just another `InferenceProvider`, not a specially-cased branch.
pub struct LocalProvider {
    engine: Option<qfz3::Engine>,
    model_path: String,
    context_size: usize,
}

impl LocalProvider {
    pub fn new(model_path: String, context_size: usize) -> Self {
        Self {
            engine: None,
            model_path,
            context_size,
        }
    }

    /// Clear conversation state (KV cache + turn counter). No-op if the
    /// engine hasn't been loaded yet.
    pub fn reset(&mut self) {
        if let Some(engine) = self.engine.as_mut() {
            engine.reset();
        }
    }

    /// Switch to a different local model. Drops the loaded engine so the
    /// next `generate()` call loads the new one — without this, changing
    /// `model_path` in config has no effect on an already-running session.
    pub fn set_model(&mut self, model_path: String, context_size: usize) {
        if self.model_path != model_path || self.context_size != context_size {
            self.model_path = model_path;
            self.context_size = context_size;
            self.engine = None;
        }
    }
}

impl LocalProvider {
    /// Synchronous form of `generate` — identical behavior, callable from a
    /// plain blocking context (e.g. inside `tokio::task::spawn_blocking`)
    /// with no async machinery involved. There's no real `.await` in
    /// either path — `qfz3::Engine::generate_streaming` is itself fully
    /// synchronous; the trait impl below just wraps this in `async fn` for
    /// uniformity with the cloud providers.
    pub fn generate_blocking(
        &mut self,
        prompt: &str,
        on_token: &mut dyn FnMut(&str),
    ) -> Result<ProviderResponse, Box<dyn Error + Send + Sync>> {
        if self.engine.is_none() {
            self.engine = Some(qfz3::Engine::load(
                &self.model_path,
                self.context_size,
                None,
            )?);
        }
        let engine = self.engine.as_mut().unwrap();
        let out = engine.generate_streaming(prompt, 512, |piece| on_token(piece))?;
        Ok(ProviderResponse {
            content: out.text,
            input_tokens: out.prompt_tokens as u64,
            output_tokens: out.completion_tokens as u64,
            elapsed_ms: out.prompt_ms + out.generate_ms,
        })
    }
}

impl InferenceProvider for LocalProvider {
    async fn generate(
        &mut self,
        prompt: &str,
        on_token: &mut (dyn FnMut(&str) + Send),
    ) -> Result<ProviderResponse, Box<dyn Error + Send + Sync>> {
        self.generate_blocking(prompt, on_token)
    }
}
