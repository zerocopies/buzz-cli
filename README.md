# Buzz

A privacy-first AI chat CLI. Talk to a local, on-device model by default; bring your own API key for a cloud provider and use it only when you choose to, or when a message is complex enough that Buzz suggests it.

Runs entirely from your terminal — type a message, get a reply, same feel as GitHub Copilot CLI or Gemini CLI. No account required to use the local model.

---

## Why

Most AI chat tools send everything you type to someone else's server. Buzz defaults the other way: your messages stay on your machine unless you explicitly route them to the cloud, or your message contains something Buzz decides is complex enough to be worth it.

Buzz also actively watches for things you probably don't want leaving your device — API keys, SSNs, credit card numbers, emails, phone numbers, IP addresses, and a few sensitive keywords — and forces those messages to stay local automatically, regardless of your provider setting.

---

## Requirements

- Rust 1.75+ (to build)
- A local GGUF model file. Not included in the repository — GGUF files typically run 500MB–5GB, too large for git. See "Getting a model" below.
- Optionally, your own API key for a cloud provider, if you want cloud fallback for complex questions. Buzz currently supports bring-your-own-key for Groq, Gemini, and Hugging Face — use whichever you already have an account with.

---

## Getting a model

Buzz needs one `.gguf` model file on disk before local chat will work. This is a one-time download.

Buzz's engine has been verified working against these architectures — any official (non-uncensored, non-abliterated) instruct model in one of these families should run correctly:

| Family | Tested size | Notes |
|---|---|---|
| **Qwen2.5 / Qwen2.5-Coder** | 1.5B, 3B | Recommended default — smallest, fastest on ordinary laptop hardware |
| **Llama 3.1 / 3.2** | 3B, 8B | Also fully supported |

A good starting point — small, fast, official:

```bash
huggingface-cli download Qwen/Qwen2.5-Coder-1.5B-Instruct-GGUF qwen2.5-coder-1.5b-instruct-q4_k_m.gguf --local-dir ~/.buzz/models
```

If you don't have `huggingface-cli` installed:

```bash
pip install -U huggingface_hub[cli]
```

Or download any GGUF file for these architectures directly from your browser (e.g. [huggingface.co/Qwen/Qwen2.5-Coder-1.5B-Instruct-GGUF](https://huggingface.co/Qwen/Qwen2.5-Coder-1.5B-Instruct-GGUF)) and save it anywhere on disk — the exact folder doesn't matter, you'll point Buzz at it during setup.

**Stick to official model repos** (`Qwen/`, `meta-llama/`, or well-known quantizers like `bartowski`/`unsloth` mirroring an official base model) rather than "uncensored" or "abliterated" variants — those are deliberately modified to strip the model's safety training, which isn't what you want for a general-purpose assistant. A `Q4_K_M` quantization is a good balance of size and quality for most machines.

---

## Install

```bash
git clone <this-repo-url>
cd buzz-cli
cargo install --path buzz-cli
```

This builds Buzz and installs it to your Cargo bin directory (usually `~/.cargo/bin`, which should already be on your `$PATH`). Once installed, `buzz-cli` works from any directory.

---

## First run

```bash
buzz-cli --setup
```

Walks you through:
- API keys for any cloud providers you want (press Enter to skip any of them)
- The path to your downloaded local model file
- A daily spending budget (defaults to $5.00, only applies to cloud usage)

You can re-run `--setup` any time, or change individual settings later from inside a chat (see below).

---

## Using it

```bash
buzz-cli
```

Starts an interactive chat. Type a message, press Enter, get a reply.

```
Buzz — type a message, /settings, /provider <name>, /reset, or /quit

> what's a good name for a coffee shop?
[local] simple query (complexity=1)
...reply...
(18 tokens · $0.000000 this reply · 18 tokens · $0.000000 total)
```

The tag before each reply (`[local]`, or the name of whichever provider ran it) shows where that message actually ran, and why. The line after each reply shows token usage and cost — both for that message and running totals for the session.

### One-shot mode

```bash
buzz-cli "what is the capital of France?"
```

Sends a single prompt, prints the answer, exits. No conversation memory. Add `--show-routing` to see the routing decision without opening a full chat.

### In-chat commands

| Command | What it does |
|---|---|
| `/settings` | Show current provider keys (masked), model path, and budget |
| `/settings <provider\|model\|budget> <value>` | Set your own API key for a provider, or change the model path / budget, without leaving the chat |
| `/provider <name>` | Route the *next* message only to a specific provider (`local` or one of your configured cloud providers) — routing then returns to automatic |
| `/reset` | Clear conversation memory and the response cache |
| `/stats` | Show session token and spend totals |
| `/quit` or `/exit` | Leave |

Example — adding your own key for a provider without re-running setup:
```
> /settings groq your_own_api_key_here
groq key updated
```

Buzz currently recognizes `groq`, `gemini`, and `hf` as provider names — use `/settings` (with no arguments) to see the exact list and your current status for each.

---

## How routing works

Every message is checked in this order:

1. **Sensitive content check** — if the message matches a pattern for an SSN, email, phone number, credit card, API key/token, IP address, or a sensitive keyword (password, secret, confidential, etc.), it is forced local. No exceptions, regardless of provider setting.
2. **Manual override** — if you just ran `/provider <name>`, that applies to this one message.
3. **Complexity** — short, simple messages stay local; longer or more complex ones may route to your configured cloud fallback provider, if one is set up.

This is a **default policy**, not a network-level guarantee — Buzz doesn't block your machine from reaching the internet. If you explicitly choose a cloud provider, or a message routes there by the complexity heuristic, that message really is sent to that provider's servers. What Buzz guarantees is that it won't send something *by default* that looks sensitive, and it always shows you which provider handled each message so you can see exactly what happened.

---

## buzz-gateway — HTTP server

If you want to point an existing OpenAI-compatible client (a script, an IDE plugin, anything that speaks the `/v1/chat/completions` shape) at Buzz instead of using the CLI directly, `buzz-gateway` is a loopback-only HTTP server that exposes the same routing, budget, and audit logic as the CLI.

It is **not** a separate credential store or a second install — it reads the same `~/.buzz/config.toml` (provider keys, local model path, daily budget) and writes to the same audit log the CLI does. Anything you've already configured via `buzz-cli --setup` works with the gateway with no extra setup.

### Starting it

```bash
buzz-cli serve --port 8787   # 8787 is the default; --port is optional
```

This execs a sibling `buzz-gateway` binary (built alongside `buzz-cli` by the same `cargo build --release`). On startup it prints the URL and where it wrote a fresh bearer token:

```
✓ listening on http://127.0.0.1:8787
  POST /v1/chat/completions
  Authorization: Bearer <token from /home/you/.buzz/gateway.token>
```

**Loopback-only, hardcoded** — the bind address is `127.0.0.1`, not configurable via flag or env var. It is not reachable from another machine, full stop. Only the port is configurable.

### Pointing a client at it

Read the token from `~/.buzz/gateway.token` (a fresh one is issued on every `serve` restart) and use it as a bearer token against the base URL:

```bash
curl http://127.0.0.1:8787/v1/chat/completions \
  -H "Authorization: Bearer $(cat ~/.buzz/gateway.token)" \
  -H "Content-Type: application/json" \
  -d '{"model": "buzz", "messages": [{"role": "user", "content": "hi"}], "stream": false}'
```

Any OpenAI-compatible client library works the same way: base URL `http://127.0.0.1:8787/v1`, API key = the contents of `~/.buzz/gateway.token`. Both streaming (`"stream": true`, server-sent events) and non-streaming requests are supported.

### What it actually does — and doesn't

- **Routing**: every request goes through the same `decide_route` sensitivity/complexity logic and provider dispatch the CLI uses — sensitive content still forces local, regardless of the `model` field in the request.
- **Budget**: cloud calls still count against the same daily budget cap in `~/.buzz/config.toml`, reserved before the call and committed or released after, same as the CLI.
- **Audit**: every request is written to the same hash-chained, tamper-evident audit log (`buzz-cli audit export` / `audit verify` cover gateway traffic too, not just CLI traffic).
- **Auth**: a random 256-bit token, written to `~/.buzz/gateway.token` with owner-only (`0600`) file permissions, checked in constant time. That's the whole auth model — no user accounts, no scopes, no TLS termination built in.
- **What this is not**: loopback-only bind and token auth are a *default policy*, same honesty standard as the routing section above — this describes what the gateway actually does (bind to 127.0.0.1, require a bearer token, log what it served), not a network-level security guarantee. If you reverse-proxy it, put it behind something that terminates TLS and adds real access control first.
- If the gateway process dies, it does not restart itself — run it under systemd (`buzz-gateway.service`, installed by the release tarball's `install.sh`) with `Restart=on-failure` for anything beyond ad hoc local use.

### Getting it

A tagged `v0.1.0` release exists; pushing a `v*` tag triggers a GitHub Actions workflow that builds `buzz-cli` and `buzz-gateway`, stages both binaries plus the systemd unit and installer into a `buzz-gateway-<version>-linux-<arch>.tar.gz`, and attaches it to the release. Linux-only for now — the packaged systemd unit is Linux-specific.

---

## Compliance & verification (planned)

Everything above is Buzz telling you what it did, in the moment. There's a companion project, `sovereignty-attestor`, that can go further: it produces a signed, tamper-evident report proving — after the fact, independently checkable — that a given session's sensitive messages actually stayed local. It exists as its own crate today but isn't wired into Buzz yet, so this is a documented direction, not a current feature. If provable compliance reporting matters for your use case, treat this as roadmap, not something to rely on yet.

---

## Caching

If you ask the exact same question twice in one session, the second time is instant and free — Buzz reuses the earlier answer instead of generating (or paying for) it again:

```
> hi who are you?
[local] simple query (complexity=1)
...reply... (20 tokens · $0.000000 this reply · 20 tokens · $0.000000 total)

> hi who are you?
[cached] repeated question — reusing earlier answer, no cost
...same reply... (20 tokens · $0.000000 this reply — served from cache · ...)
```

`/reset` clears the cache along with conversation memory.

---

## Project structure

```
buzz-cli/
├── buzz-cli/       — the CLI application (this is what you install and run)
├── buzz-core/      — routing logic, privacy detection, shared config types, audit log
├── buzz-gateway/   — the `buzz-cli serve` HTTP server (see buzz-gateway section above)
└── Cargo.toml      — workspace root
```

Buzz's local inference runs on [qfz3-engine](https://github.com/zerocopies/qfz3), a separate zero-copy inference engine (model weights are memory-mapped, not copied into memory) that Buzz depends on for on-device generation.

---

## Troubleshooting

**"Could not load the local model"** — your configured model path doesn't point to a real file. Run `buzz-cli --setup` or `/settings model <path>` inside a chat to fix it. You need to have already downloaded a `.gguf` file yourself (see Requirements above).

**"No API key configured"** — you tried to use a cloud provider that doesn't have a key set. Use `/settings <provider> <key>` to add one.

---

## License

See [LICENSE](LICENSE).
