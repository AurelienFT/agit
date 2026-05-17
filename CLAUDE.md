# CLAUDE.md

Guidance for Claude Code (and other AI coding agents) working on **Agit**.

## The project, in one sentence

> **Agit is a self-hostable control plane for AI coding agents.** It lets engineering teams define agents as code, run them in their own environment, and govern their Git workflow with permissions, policies and audit trails.

Alternative pitch when self-hosted is the lead message:

> **Run AI agents on your codebase without sending your code or secrets to Agit.**

Defensible positioning: Agit is the **policy and workflow layer** for AI-generated software contributions — and it's the layer that *stays in your infra*.

## Two-component architecture (load-bearing)

Internalize this split before touching anything:

```
GitHub / GitLab
    │  webhook
    ▼
Agit Server   ── stores missions, runs, logs; exposes dashboard & UI
    │  (auth'd HTTP, no code, no secrets)
    ▼
Agit Runner   ── self-hosted on customer infra
    │
    ├── clones repo
    ├── invokes the configured Provider (Claude Code CLI, Anthropic API, Ollama, …)
    ├── enforces the policy (read / write / commands)
    ├── runs allowed tests
    ├── pushes branch, opens PR
    │
    ▼
GitHub PR     ── reviewable, never auto-merged by default
```

**The server never touches customer code, never sees model credentials.** The runner does the work, in the customer's environment. The server only orchestrates and observes (via the status/log updates the runner chooses to send back).

This split is the product differentiator. Don't break it: nothing in `agit-server` should ever clone repos, invoke models, or read project secrets.

## Providers (also load-bearing)

Don't couple Agit to one model vendor. Every agent references a **Provider**, declared at the top of `.agit/agents.yaml`. Provider variants (in `agit-core::config::ProviderConfig`):

- `local_command` — exec an existing CLI on the runner host (e.g. `claude`, `codex`, `aider`).
- `anthropic_api` — official Anthropic API; the api key comes from an env var on the runner.
- `openai_api` — official OpenAI API; same pattern.
- `openai_compatible` — any OpenAI-shaped endpoint (Ollama, vLLM, LM Studio, OpenRouter, internal proxies). The way to support local models without writing more code.

Agents reference a provider by key:

```yaml
providers:
  claude_code:
    type: local_command
    command: "claude"

agents:
  test_writer:
    provider: claude_code   # ← key into the providers map
    ...
```

Legal framing: **Agit does not sell access to any model.** Customers configure their own providers and are responsible for their respective terms. Don't write code or docs that imply otherwise.

## What Agit does (reference loop)

```
GitHub event (issue labeled, PR opened, …)
    → server reads .agit/agents.yaml from the repo
    → matches an agent + a trigger, creates a Mission
    → runner pulls the Mission, clones repo into an isolated workspace
    → runner invokes the agent's Provider (local CLI / API / OpenAI-compat)
    → checks permissions (read / write / commands) on the diff and commands
    → runs allowed test/check commands
    → pushes a branch, opens a clean PR
    → runner reports status/cost/logs to the server
    → server surfaces the run in the dashboard
```

The differentiator is **not** "the agent is smart" — it is:

1. **Self-hostable runner** — code and secrets stay in customer infra.
2. **Pluggable providers** — Claude Code, Anthropic, OpenAI, local OpenAI-compatible.
3. **Agents as Code** in `.agit/agents.yaml` (everything lives in the repo).
4. **Permissions** per agent: `read`, `write`, `commands`, glob-scoped.
5. **Git triggers**: issue labels, PR events, comment commands, manual.
6. **Observability**: runs, logs, cost, policy status, all visible in the dashboard.
7. **PR output**: a reviewable PR, never auto-merged by default.

Anything that doesn't strengthen one of these seven is probably out of scope.

## Vocabulary (canonical)

- **Provider** — how an agent reaches a model (`local_command`, `anthropic_api`, …). Declared once at the top of the YAML.
- **Agent** — role declared in the repo (`test_writer`, `bugfixer`, `security_reviewer`). References a provider.
- **Trigger** — event that creates a Mission (issue label, PR opened, …).
- **Mission** — task given to an agent following a trigger. Lives on the server; consumed by a runner.
- **Run** — concrete execution of an agent on a mission. Lives on the runner; reported to the server.
- **Policy** — access and validation rules (read/write/commands permissions, limits).
- **Output** — expected result: `pull_request`, `blocking_review`, `comment`, `patch`.

Code identifiers: `ProviderConfig`, `AgentConfig`, `TriggerConfig`, `PermissionPolicy`, `Run`, `RunStep`, `PolicyViolation`.

Full definitions: [docs/CONCEPTS.md](docs/CONCEPTS.md).

## Repo state

Rust Cargo workspace at the repo root. Four crates today:

```
agit/
├── Cargo.toml                  workspace
├── crates/
│   ├── agit-core/              shared library — schema, (future) policy, run state
│   │   └── src/
│   │       ├── lib.rs
│   │       └── config.rs       AgitConfig, ProviderConfig, AgentConfig, …
│   ├── agit-cli/               local config inspector — produces `agit` binary
│   │   └── src/{main,cli}.rs   subcommands: list / providers / show / validate
│   ├── agit-runner/            self-hosted runner — produces `agit-runner` binary
│   │   └── src/main.rs         subcommands: start --server <url> --token <t> / check
│   └── agit-server/            control-plane server — produces `agit-server` binary
│       └── src/main.rs         subcommands: serve --port <p> / migrate
├── .agit/
│   ├── agents.yaml             this repo's OWN config (one provider: claude_code, one agent)
│   └── prompts/test_writer.md
├── demo-project/               separate target project for demos
│   ├── .agit/agents.yaml       heavily commented schema tour (3 providers, 3 agents)
│   └── README.md, src/, tests/
└── docs/                       CONCEPTS, POC, ARCHITECTURE, BUSINESS_MODEL
```

**Three binaries, one shared core**:

| Crate (package) | Binary | Role |
|---|---|---|
| `agit-cli` | `agit` | Local CLI: inspect/validate `.agit/agents.yaml`. Does NOT contact the server. |
| `agit-runner` | `agit-runner` | Long-running process on customer infra. Polls the server, executes missions. |
| `agit-server` | `agit-server` | HTTP control plane: webhooks, missions, runs, dashboard. Never touches code. |

Run them with `cargo run -p <package> -- <args>` (e.g. `cargo run -p agit-cli -- list`).

**Currently implemented**:

- `agit list` / `agit providers` / `agit show <name>` / `agit validate` — read the YAML, validate the schema, cross-validate `agent.provider` references.
- `agit-runner check` — load the YAML and emit declared providers (placeholder until real probing lands).
- `agit-runner start` and `agit-server serve` — flag scaffolds only, print "not implemented yet".

**Not implemented yet**: HTTP listener and webhook receiver in `agit-server`; mission polling, repo clone, provider invocation, policy engine, PR push in `agit-runner`. Scope ahead in [docs/POC.md](docs/POC.md).

## Target stack

- **Workspace**: Rust 2021, stable toolchain (`rust-toolchain.toml`).
- **agit-core**: `serde` + `serde_yml` for the schema. **No I/O outside the YAML loader, no async, no clap, no HTTP.** Pure-domain crate; that's what makes it safely shared by CLI + runner + server.
- **agit-cli**: `clap` derive API. Local-only.
- **agit-runner** (planned deps): `tokio`, `reqwest` (talk to server + OpenAI-compatible providers), `git2` (or shell out), `globset` (policy), `octocrab` (push PRs).
- **agit-server** (planned deps): `axum` + `tokio`, `sqlx` against SQLite (Postgres in Enterprise), `octocrab` for App auth + webhook verification.
- **Dashboard**: deferred. Working assumption is Next.js + Tailwind + shadcn/ui talking to `agit-server` over JSON HTTP, but a Rust+Leptos/HTMX alternative stays on the table.
- **Coding-agent backends**: Claude Code CLI is the default `local_command`; everything else is the Provider abstraction's job.

Full picture: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## POC scope (do not exceed)

The POC demonstrates the **self-hosted loop**, run twice:

> Spin up `agit-server` + one local `agit-runner` (`docker compose up`). Install the GitHub App on a test repo. Label an issue `agent:test` → the server creates a Mission → the local runner picks it up, invokes the configured provider (Claude Code CLI), writes tests under `tests/**`, opens a PR. The dashboard surfaces the run.
>
> Then a deliberate sad-path run shows the runner **blocking** the agent when it tries to write outside `tests/**`, with a clear `PolicyViolation` visible in the dashboard.

The pivot from the previous POC: it's now **explicitly two components** (server + runner), not a single backend. The runner being self-hosted is the lead value prop, not an afterthought.

**Out of POC scope**: custom HCL language, multi-agent pipelines, GitLab/Bitbucket, marketplace, RBAC/SSO, auto-merge, no-code agent builder, GitHub Actions runner image, Kubernetes operator.

## Business model

**Open-core, GitLab-style** — see [docs/BUSINESS_MODEL.md](docs/BUSINESS_MODEL.md).

The self-hosted runner reshapes the open-core story for the better:

- **Community Edition** — both `agit-server` and `agit-runner` are OSS and self-hostable. Customer hosts the full stack on their infra; nothing leaves it. Free.
- **Agit Cloud (paid)** — managed `agit-server`. Customer still runs their own `agit-runner`, so code/secrets stay in their infra. Cloud sells the orchestration UI + reliability, not access to models.
- **Enterprise** — Cloud or self-managed with org-wide governance, audit log, SSO/RBAC, cost analytics, Postgres/HA, support. Same product; different buyer.

Implication for POC code: **the runner is always OSS**, never paywalled. Anything that requires touching code or secrets is OSS. The control-plane UX is what we charge for in the hosted/enterprise tier.

## Competitors and boundary

```
Fast.io = GitOps for running agents (deployment, runtime, persistent workspaces).
Agit    = Self-hostable control plane for agents contributing to code
          (issue → mission → branch → PR → reviews → merge gates).
```

The self-hosted runner sharpens the boundary further: Fast.io provides the agent runtime as a service. Agit lets you bring your runtime (or our open-source one) and adds policy, observability and Git workflow on top.

## Code conventions

- **Rust 2021**, stable toolchain. One workspace, one `Cargo.lock`. Shared deps in `[workspace.dependencies]`, pulled via `*.workspace = true`.
- `serde` + `serde_yml`. Tagged enums use `#[serde(tag = "type", rename_all = "snake_case")]` so the YAML stays human-friendly (`type: local_command`, etc.).
- `clap` derive API. Global flags (`-C/--directory`) declared once on the root `Cli`.
- `anyhow::Result` at the binary/HTTP boundary; library code in `agit-core` uses concrete error types.
- **agit-core stays pure**: no `tokio`, no I/O beyond the YAML loader, no async. If you're tempted to add HTTP or async there, it belongs in a different crate.
- **No secrets in YAML.** Provider credentials live in env vars (`api_key_env: ANTHROPIC_API_KEY`), never as literal values. Adding a literal-secret field would be a security regression.
- **Glob matching** (planned): `globset`. One crate, used in `agit-core::policy` (when it lands) and reused by every other crate.
- **No auto-merge** — default output is `pull_request` with `require_human_review: true`.
- **Everything goes through policy** — no write or command path may bypass the check.
- **Structured logs** per `RunStep`, structured violations as `PolicyViolation`. The dashboard reads those, not free-text strings.
- **Deny by default**: `.env*`, lockfiles, `.git/` are blocked even if the glob would technically match.

## POC pitch (keep handy)

> Agit is a self-hostable control plane for AI coding agents. Declare agents and providers in `.agit/agents.yaml`. The Agit server orchestrates missions from GitHub events; a self-hosted Agit runner — in your infra, with your credentials, talking to your provider of choice (Claude Code, Anthropic, OpenAI, Ollama, …) — executes them, enforces policy, and opens reviewable PRs. Your code and your model credentials never transit through Agit Cloud.

## Source of inspiration

Original conversation (FR): *Contrôle d'agents pour Devs*. The self-hosted pivot tightens the original wedge: same governance + workflow value, plus the privacy/trust posture that B2B engineering teams ask for first.
