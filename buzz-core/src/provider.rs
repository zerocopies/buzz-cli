/// Response from any inference provider, local or cloud. Token counts are
/// kept split (not pre-priced) so cost is always computed in exactly one
/// place — `crate::core::cost::calculate_cost` — instead of each provider
/// hardcoding its own per-token rate.
#[derive(Debug, Clone)]
pub struct ProviderResponse {
    pub content: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Total wall-clock time the request took, in milliseconds — local
    /// reports the engine's own real prefill+generate timing; cloud
    /// providers measure the HTTP round trip.
    pub elapsed_ms: f64,
}

impl ProviderResponse {
    /// Output tokens per second. 0.0 if elapsed_ms is 0 (e.g. a mocked or
    /// instant response) rather than dividing by zero.
    pub fn tokens_per_second(&self) -> f64 {
        if self.elapsed_ms <= 0.0 {
            0.0
        } else {
            self.output_tokens as f64 / (self.elapsed_ms / 1000.0)
        }
    }
}

/// Common contract for every inference backend — the local on-device engine
/// and every cloud provider implement the same trait, so callers dispatch
/// through one code path instead of branching on "is this local or cloud."
///
/// `on_token` is called for each incrementally decoded piece, for providers
/// that support streaming (currently only the local engine); providers that
/// only return a full response at once simply never call it.
///
/// No `Send` bound: the local engine wraps ggml raw pointers and is not
/// Send/Sync, and every caller runs on a single thread (block_on or a
/// direct .await, never tokio::spawn), so requiring Send would be pure
/// friction with no benefit here.
#[allow(async_fn_in_trait)]
pub trait InferenceProvider {
    async fn generate(
        &mut self,
        prompt: &str,
        on_token: &mut dyn FnMut(&str),
    ) -> Result<ProviderResponse, Box<dyn std::error::Error>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(output_tokens: u64, elapsed_ms: f64) -> ProviderResponse {
        ProviderResponse {
            content: String::new(),
            input_tokens: 0,
            output_tokens,
            elapsed_ms,
        }
    }

    #[test]
    fn tokens_per_second_computes_rate() {
        let resp = response(100, 2000.0); // 100 tokens in 2 seconds
        assert!((resp.tokens_per_second() - 50.0).abs() < 1e-9);
    }

    #[test]
    fn tokens_per_second_is_zero_when_elapsed_is_zero() {
        assert_eq!(response(100, 0.0).tokens_per_second(), 0.0);
        assert_eq!(response(100, -5.0).tokens_per_second(), 0.0);
    }
}
