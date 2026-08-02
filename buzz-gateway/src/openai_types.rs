//! Wire-shape compatibility with `POST /v1/chat/completions`.
//!
//! Goal per the deck: "Any client already built to talk to OpenAI works
//! unmodified — it just points at a different base URL." That means
//! matching field names and JSON shape exactly, including the streaming
//! chunk envelope, not just "close enough."
//!
//! This is intentionally a subset — enough for real clients (VS Code
//! extensions, LangChain-style SDKs, curl) to work, not the full OpenAI
//! surface (no function calling / tool use yet, no logprobs, no `n>1`).
//! Extend as real callers need more.

use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(default)]
    pub stream: bool,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub max_tokens: Option<u32>,

    /// Free-form field the gateway itself reads for caller attribution
    /// (deck slide 11, v1: self-reported). Not part of the OpenAI spec —
    /// clients that don't send it just don't get attributed, they still
    /// work. Prefer the `X-Buzz-Client` header (see caller.rs) over this;
    /// it's kept here only as a fallback for clients that can't set
    /// custom headers.
    #[serde(default)]
    pub user: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChatMessage {
    pub role: String, // "system" | "user" | "assistant"
    pub content: String,
}

#[derive(Debug, Serialize)]
pub struct ChatCompletionResponse {
    pub id: String,
    pub object: &'static str, // "chat.completion"
    pub created: i64,
    pub model: String,
    pub choices: Vec<Choice>,
    pub usage: Usage,
}

#[derive(Debug, Serialize)]
pub struct Choice {
    pub index: u32,
    pub message: ChatMessage,
    pub finish_reason: String, // "stop" | "length" | "content_filter"
}

#[derive(Debug, Serialize)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

// --- Streaming (SSE) shapes ---
// `data: {chunk}\n\n` repeated, terminated by `data: [DONE]\n\n`.
// This is the piece the deck flags as "the hardest unproven piece" —
// get the envelope exactly right or every streaming client silently
// breaks on parse, not on a visible error.

#[derive(Debug, Serialize)]
pub struct ChatCompletionChunk {
    pub id: String,
    pub object: &'static str, // "chat.completion.chunk"
    pub created: i64,
    pub model: String,
    pub choices: Vec<ChunkChoice>,
}

#[derive(Debug, Serialize)]
pub struct ChunkChoice {
    pub index: u32,
    pub delta: Delta,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Default, Serialize)]
pub struct Delta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

/// Standard OpenAI-shaped error body, so client SDKs that already know
/// how to surface OpenAI errors (rate limit, auth, etc.) display something
/// sensible instead of an unparseable blob.
#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: ErrorDetail,
}

#[derive(Debug, Serialize)]
pub struct ErrorDetail {
    pub message: String,
    #[serde(rename = "type")]
    pub error_type: String,
    pub code: Option<String>,
}
