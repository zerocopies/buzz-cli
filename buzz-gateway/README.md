# buzz-gateway

The HTTP surface from the "buzz-cli → buzz-gateway" proposal deck —
`buzz-cli serve` turns one developer's CLI into infrastructure an IT
team deploys machine-wide (deck slide 02).

## What's real in this crate

| Piece | Where |
|---|---|
| Loopback-only bind (127.0.0.1, hardcoded) | `main.rs` |
| Token issuance + persistence, 0600 perms | `auth.rs` |
| Constant-time token verification | `auth.rs` |
| OpenAI-compatible request/response JSON shape, SSE streaming | `openai_types.rs`, `handlers.rs` |
| Caller attribution (v1, `X-Buzz-Client` header) | `caller.rs` |
| Fail-closed error handling (401, 429, 503 — no silent cloud fallback) | `handlers.rs` |
| `decide_route` routing decision + concurrent-safe budget reservation | `routing.rs` (`RealRouter`) |
| Provider dispatch — local engine, Groq, Gemini, HuggingFace | `handlers.rs` (`dispatch`) |
| Budget enforcement (reserve/commit/release, leak-proof via `Drop`) | `buzz-core/src/budget.rs` |
| Real token counts in `Usage` (not hardcoded) | `handlers.rs` |
| Audit hash-chain — every request, real or rejected, logged to `~/.buzz/audit.jsonl` | `buzz-core/src/audit.rs` |
| Signed compliance-report export/verify (deck slide 08) | `buzz-cli audit export`/`audit verify` — see Operations below |

`RouteTarget::OpenAi` (routing.rs) stays as dead code — buzz-core's
`RouteProvider` has no OpenAI variant, so `decide_route` can never
produce it; `dispatch` still matches on it defensively rather than
silently doing nothing if that ever changes.

## Running

The deck's literal example command (slide 02) is the real entry point:

```bash
buzz-cli serve --port 8787
```

(`cargo run --bin buzz-gateway` also still works directly — `buzz-cli
serve` execs the same binary, see the doc comment on `run_serve` in
`buzz-cli/src/main.rs` for why it's implemented as an exec rather than a
direct function call.)

```
✓ listening on http://127.0.0.1:8787
  POST /v1/chat/completions
  Authorization: Bearer <token from ~/.buzz/gateway.token>
```

Any OpenAI-compatible client works by pointing its base URL at
`http://127.0.0.1:8787/v1` with the token from `~/.buzz/gateway.token`
as the bearer credential.

For a real machine-wide install (deck slide 02: "an IT team deploys it
machine-wide"), see `dist/` and the Packaging section below instead of
running either binary by hand.

## Operations

### Crash / restart policy (deck slide 10)

buzz-gateway does not supervise or restart itself — no in-process
watchdog. That's the decision, not a gap: process supervision belongs
one layer down. Run it under systemd with `Restart=on-failure`; see
`dist/buzz-gateway.service` for the actual unit (a system-wide unit, not
a per-user one — it runs under a dedicated `buzz-gateway` system account
so it comes up on boot without needing a logged-in session, matching
"IT team deploys it machine-wide") and the doc comment on `main()` in
`src/main.rs` for the full rationale. `/healthz` exists for an external
liveness probe if one gets wired up later, but today the restart trigger
is systemd watching the process's exit status, not active polling.

### Config validation on startup

`~/.buzz/config.toml` is checked at startup in two stages:

- **Hard failure**: the file exists but fails to parse (bad TOML). This
  used to silently fall back to `Config::default()`, which meant a
  typo'd config produced a gateway that *started fine* and then behaved
  like an empty config with no explanation. Now it's a startup error
  naming the file and the parse error, and the process exits instead of
  running on a config the operator didn't actually intend.
- **Warnings**: the file is valid but describes a gap decide_route will
  hit at request time — no API key for the cloud provider
  `cloud_fallback_order` would route to, or no local model file at
  `local.model_path`. Neither aborts startup (local-only and cloud-only
  are both legitimate deployments), but each logs an explicit
  `tracing::warn!` naming exactly what's missing and which requests it'll
  affect, so it's visible in startup logs instead of surfacing as a
  confusing 502/503 on the first real request.

A missing config file (fresh install) is neither of these — it's
expected, and just falls back to defaults with no error.

### Logging

Logs are JSON (`tracing_subscriber`'s `fmt().json()`), not the default
human-readable formatter — this runs as a supervised background service,
not something read live in a terminal, so log lines need to be
`jq`/grep-parseable by whatever tails them (`journalctl` if running under
the systemd unit above). This is deliberately just a formatter swap, not
a metrics/tracing pipeline — nothing here needs more than "can an ops
person parse the logs."

### Audit trail & compliance reports (deck slide 08)

Every request the gateway handles — served or budget-rejected — gets one
real, hash-chained entry in `~/.buzz/audit.jsonl` (`buzz-core/src/audit.rs`),
including the caller identity and request ID. There used to be a second,
parallel `AuditSink`/`StderrAuditSink` mechanism in this crate that logged
a differently-shaped entry to stderr on every request — it never wrote to
the real file and had no consumer, so it's been deleted; the hash-chained
file is now the one and only audit trail.

To turn a date range of that log into the signed report deck slide 08
mocks up ("requests audited... forced to local... budget-cap
rejections... hash-chain integrity verified... signed by fleet key
#003"):

```bash
buzz-cli audit export --from 2026-08-01 --to 2026-08-02 --out report.json
buzz-cli audit verify --report report.json
```

`export` refuses to sign if the chain is broken anywhere in the log (not
just the requested range), so a signed report is never built from
already-tampered data. The signing key is an Ed25519 keypair buzz-cli
generates and persists itself on first use (`~/.buzz/audit_signing.key` /
`.pub`, same 0600-on-the-secret-half convention as the gateway's own
bearer token) — **not** a reuse of any pre-existing vault infrastructure;
none exists in this codebase today (verified by exhaustive search, not
assumed). See `buzz-core/src/signing.rs` for the full rationale and the
seam to swap in real vault-backed keys later.

### Packaging

`dist/` holds everything a real machine-wide install needs:

- `buzz-gateway.service` — the system-wide systemd unit (see Crash /
  restart policy above).
- `install.sh` — run as root on the target machine; creates the
  `buzz-gateway` system account, installs both binaries to
  `/usr/local/bin`, installs the unit, and enables + starts it.
  Idempotent — safe to re-run for an upgrade.
- `package.sh` — run on a build machine; builds release binaries and
  stages `buzz-gateway-<version>-linux-<arch>.tar.gz` containing the two
  binaries plus the unit and `install.sh`, ready to copy to a target
  machine.

**Why not qfz3's Tauri bundler** (which already produces `.deb`/`.rpm`/
`.AppImage` for that project — `src-tauri/tauri.conf.json`'s
`bundle.targets: "all"`): ruled out, not just skipped.
- qfz3 and buzz-cli are separate repos/workspaces with no dependency
  between them — routing buzz-gateway's build through qfz3's pipeline
  means one product's release process depends on an unrelated product's
  tree, for no shared benefit.
- Tauri's bundler is built around packaging *one GUI app plus its
  webview frontend* into a desktop-install experience (`.desktop`
  entries, dock/Applications integration, an app identifier like qfz3's
  `com.zerocopies.quantumflow`). buzz-gateway is a headless binary with
  no UI, installed as a background service under a dedicated system
  account — a fundamentally different install shape. Tauri does support
  bundling extra binaries via `bundle.externalBin` ("sidecars"), but
  that mechanism assumes the sidecar rides along with a GUI app it
  supports, not that it *is* the entire product being shipped.
- Forcing the fit would mean giving buzz-gateway a fake GUI identity
  (an app icon, a bundle ID under `com.zerocopies.*`) purely to satisfy
  a packager that doesn't otherwise know what to do with a service
  binary — more accidental complexity than the tarball approach it would
  replace.

A plain tarball + systemd unit + install script has none of that
mismatch, needs no new tooling, and is the standard shape for
distributing a Linux system service.

## Deliberately out of scope for this scaffold

- Process-level caller attestation (deck slide 11, v2) — not scoped in
  detail yet, don't build ahead of the design decision.
- `buzz-cli token rotate` subcommand — v1 rotates on every `serve`
  restart, which already satisfies the deck's design intent. Add the
  subcommand only if restart-to-rotate proves too coarse in practice.
- Windows support — the threat model (loopback boundary, local code
  execution as the spoofing bar) assumes a single-user Unix-like machine.
