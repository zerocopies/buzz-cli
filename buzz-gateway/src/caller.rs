//! Caller attribution — v1 (deck slide 11).
//!
//! "Caller sends an X-Buzz-Client header. Cheap, works with cooperative
//! tools. Trusted only within the loopback boundary — spoofing it
//! requires local code execution, which is already inside the machine's
//! threat perimeter."
//!
//! v2 (process/binary-level OS attestation) is explicitly out of scope
//! here — "real engineering work, not scoped in detail yet" per the deck.
//! Don't build ahead of that; this module is deliberately this small.

use axum::http::HeaderMap;

pub const CLIENT_HEADER: &str = "x-buzz-client";
pub const UNKNOWN_CALLER: &str = "unknown";

/// Every audit line needs a caller identity (deck slide 11's whole point).
/// Precedence: explicit header first, falling back to the `user` field on
/// the request body (see openai_types::ChatCompletionRequest), falling
/// back to "unknown" — never silently blank, because an audit export with
/// blank caller fields defeats the purpose of slide 08's compliance report.
pub fn identify(headers: &HeaderMap, body_user_field: Option<&str>) -> String {
    if let Some(v) = headers.get(CLIENT_HEADER) {
        if let Ok(s) = v.to_str() {
            if !s.trim().is_empty() {
                return s.trim().to_string();
            }
        }
    }
    if let Some(u) = body_user_field {
        if !u.trim().is_empty() {
            return u.trim().to_string();
        }
    }
    UNKNOWN_CALLER.to_string()
}
