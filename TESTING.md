# Testing

Buzz has a real, passing test suite: **60 tests across the workspace, 0 failures.**

```bash
cargo test --workspace
```

## What's covered

### `buzz-core` — 41 tests

The routing, privacy, budget, cost, audit, and config logic — the pure decision-making core, fully unit tested.

**Privacy detection** (`core::privacy`) — every pattern Buzz uses to decide a message is sensitive, tested both positive and negative:
- SSN, email, phone, credit card, IP address patterns
- Sensitive keywords (password, secret, confidential, etc.)
- Unlabeled API-key/token-shaped strings
- A specific negative case: short alphanumeric words are *not* falsely flagged as keys
- `analyze_privacy` reports the specific flags that triggered, not just a bool

**Routing decisions** (`core::decision`):
- Trivial prompts score low complexity
- Long prompts raise complexity
- Complexity is capped at 10
- Simple queries route local
- Complex prompts route to the first configured cloud fallback
- Complex prompts with an *empty* fallback list stay local (no crash, no silent cloud call)
- Sensitive content always routes local, even if it's also complex — sensitivity check runs first, unconditionally
- Provider names round-trip correctly (enum ↔ string)

**Budget enforcement** (`budget`):
- Local is always allowed regardless of prompt length or config
- Requests within both per-request and daily limits are allowed
- Requests exceeding the per-request estimate are blocked

**Cost calculation** (`core::cost`):
- Local is always free
- Cloud providers charge per token
- Cost scales linearly with token count
- Hugging Face is treated as free tier

**Audit log** (`audit`) — this is the most thoroughly tested module, because it's a hash-chained, tamper-evident log and correctness actually matters here:
- Disabled audit writes nothing
- Enabled audit appends a JSON line per entry
- A fresh log chains and verifies cleanly
- Tampering with a middle entry is detected
- Legacy entries without a `prev_hash` are treated as informational, not broken (backward compatibility)
- Unparseable lines are skipped, not fatal
- An empty log has an empty chain status
- `~` in a configured log path expands against the given home directory
- `recent()` returns newest-first and respects a limit
- `spend_today()` sums only today's entries
- `summarize()` correctly counts local vs. cloud vs. sensitive-flagged messages
- Relative-time formatting produces coarse, human-readable buckets

**Config** (`policy`):
- The default config round-trips through save and load unchanged
- **A config file containing only the fields a user would hand-write still loads correctly** — this is the regression test for a real bug hit during development: an earlier version of config loading required *every* field to be present or it failed the entire parse silently, falling back to defaults with a stale/wrong model path. Every field now has a `#[serde(default = "...")]`, and this test guards against that regressing.

### `buzz-cli` — 19 tests

**SSE stream parsing** (`providers::sse`) — parsing the streaming response format cloud providers send back:
- Extracts data payloads correctly
- Handles a data line split across two chunks (a real streaming edge case)
- Recognizes the `[DONE]` sentinel
- Skips empty data payloads
- Ignores non-data lines (event headers, blank lines)

**Provider name resolution** (`select_provider_name`):
- An explicit override is used and lowercased
- Falls back to the `PROVIDER` environment variable, then to `"groq"`, when nothing else is set (isolated from the real process environment so this test can't be flaky depending on what's exported in whatever shell runs `cargo test`)

**API key validation** (`require_key`):
- An empty key is rejected with a clear, actionable error naming the specific key
- A whitespace-only key is rejected
- A real key is accepted and returned unchanged

**Text sanitization** (`sanitize_terminal_text`) — this exists because model output is untrusted text hitting a terminal, and a stray escape sequence could do more than print garbage:
- Newlines and tabs are preserved
- ANSI escape sequences and other control characters are stripped
- Ordinary text passes through completely unchanged

**Budget pre-flight check** (`check_budget`) — the wrapper that guards every cloud call, including explicit `/provider` overrides, not just auto-routed requests:
- Local is always allowed, regardless of how restrictive the budget config is
- A per-request limit set to zero blocks a normal cloud request
- A normal prompt is allowed under default limits

**Local model discovery** (`list_local_models`):
- Finds only `.gguf` files in a directory, sorted, ignoring other file types
- An empty or nonexistent directory returns an empty list, not an error

**Secret masking** (`mask_secret`):
- Reports whether a key is set and how long it is, without ever including the actual value in output

## What's *not* covered by automated tests

Worth being upfront about this rather than implying full coverage:

- **The interactive chat loop itself** (`run_tui_mode`) — reading stdin, dispatching commands, driving the conversation. This is I/O-bound and stateful in a way that would need a proper CLI-testing harness (spawning the binary, feeding it stdin, asserting on stdout) to test meaningfully. Currently verified by manual testing only.
- **Actual model inference** (the qfz3 engine) — loading a GGUF file and generating tokens is verified by running real models against real prompts during development, not by automated unit tests. This would need a small test model bundled or downloaded in CI, which isn't set up yet.
- **Live API calls to cloud providers** — the SSE *parsing* is tested, but an actual network call to Groq/Anthropic/etc. is not, since that would require live API keys and network access in CI.

## Running a specific subset

```bash
# Just buzz-core (routing, privacy, budget, cost, audit, config)
cargo test -p buzz-core

# Just buzz-cli (SSE parsing, key handling, sanitization, budget check, model discovery)
cargo test -p buzz-cli

# A single test by name
cargo test check_budget_blocks_when_per_request_limit_is_effectively_zero
```

CI runs the full workspace suite on every push (see `.github/workflows/`).
