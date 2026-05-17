# Architecture

This document describes the POC architecture and the chosen tech stack. It is the source of truth for *how the code is laid out*; product behavior lives in [POC.md](POC.md) and vocabulary in [CONCEPTS.md](CONCEPTS.md).

## High-level diagram (two components, one shared core)

```
                          ┌──────────────────────┐
                          │   GitHub / GitLab    │
                          └──────────┬───────────┘
                                     │  webhook
                                     ▼
                          ┌──────────────────────┐
                          │     Agit Server      │   ◀── DB (sqlite/postgres)
                          │   (control plane)    │       missions, runs, audit
                          │                      │
                          │  • webhook receiver  │
                          │  • mission queue     │
                          │  • runner API        │
                          │  • dashboard         │
                          └──────────┬───────────┘
                                     │  authenticated HTTP
                                     │  (no code, no secrets)
                                     ▼
        ┌─────────────────────────────────────────────────────────┐
        │                     Agit Runner                          │
        │              (self-hosted on customer infra:             │
        │           dev laptop, internal server, k8s,              │
        │           GitHub Actions, dedicated VM, …)               │
        │                                                          │
        │   ┌─────────────────────────────────────────────────┐    │
        │   │ 1. pull Mission                                 │    │
        │   │ 2. clone repo (shallow, isolated workspace)     │    │
        │   │ 3. resolve provider                             │    │
        │   │ 4. invoke Provider ──▶ Claude Code CLI /        │    │
        │   │                       Anthropic / OpenAI /       │    │
        │   │                       Ollama / vLLM / …          │    │
        │   │ 5. policy check (agit-core::policy)              │    │
        │   │ 6. run allowed commands (tests, typecheck)       │    │
        │   │ 7. push branch, open PR                          │    │
        │   │ 8. report status / logs / cost / violations      │    │
        │   └──────────────────────────────────────────────────┘   │
        └──────────────────────────────┬──────────────────────────┘
                                       │
                                       ▼
                          ┌──────────────────────┐
                          │     GitHub PR        │
                          └──────────────────────┘
```

The **server is code-blind**: it never clones repos, never contacts model APIs, never stores model credentials. It accepts webhooks, persists mission metadata, exposes a runner-facing API, exposes a dashboard. That's it.

The **runner is the only component that touches code or secrets**. It runs in the customer's environment — laptop, dedicated server, Kubernetes pod, GitHub Actions runner, whatever. The customer owns its uptime, scaling, network egress.

## Recommended stack

### Shared library — `agit-core` (Rust, sync, no I/O)

The contract every other crate agrees on.

- `serde` + `serde_yml` for the YAML schema.
- Tagged enums (`#[serde(tag = "type", rename_all = "snake_case")]`) for `ProviderConfig`, `TriggerConfig`, `OutputKind`.
- The future `policy` module: pure `check(diff, policy) -> Result<(), PolicyViolation>`.

**Two hard rules**: no `tokio`, no I/O outside the YAML loader. That's what makes it cheap to reuse from CLI + runner + server.

### CLI — `agit-cli` (Rust, sync, clap)

Local config inspector. Never contacts the server. Subcommands:

- `list` — providers + agents.
- `providers` — providers only.
- `show <name>` — full agent + its resolved provider.
- `validate` — schema + cross-references.
- *planned*: `policy-check --diff <file>` — simulate a violation locally.

### Runner — `agit-runner` (Rust, async, self-hosted)

The component that actually does the work.

- **Async**: `tokio`.
- **HTTP client**: `reqwest` (to the server, to OpenAI-compatible providers).
- **Git**: `git2` or shell out to `git`. Shallow clones into per-Run temp dirs, always cleaned up.
- **GitHub App**: `octocrab` for App auth + branch/PR APIs. The runner holds the App's private key.
- **Glob matching**: `globset` (for policy checks).
- **Long-running**: `start --server <url> --token <secret>` long-polls or maintains a WebSocket to the server.

Provider implementations (under `crates/agit-runner/src/providers/`):

- `local_command` — `tokio::process::Command`, piped stdio, configurable timeout.
- `anthropic_api` — `reqwest` to `https://api.anthropic.com`. API key from env var named by the provider config.
- `openai_api` — same shape, OpenAI base URL.
- `openai_compatible` — same shape, custom base URL.

The runner enforces **deny-by-default** policy at two boundaries:

1. **Before commit** — diff inspected through `globset` patterns from `permissions.write`.
2. **Before each command** — command string matched against `permissions.commands.allow`.

### Server — `agit-server` (Rust, async, HTTP)

- **HTTP**: `axum` + `tokio`.
- **DB**: `sqlx` against SQLite for POC; Postgres for Enterprise. Migrations checked in.
- **GitHub App**: `octocrab` for webhook signature verification.
- **Auth**:
  - Runner ↔ server: bearer tokens per runner registration.
  - User ↔ server (dashboard): GitHub OAuth via the App.
- The server publishes a runner-facing API:
  - `GET /missions/next` (long poll, bearer-auth).
  - `POST /missions/{id}/status` (status, logs, cost, optional structured violation).
  - `POST /missions/{id}/done` (terminal status + final PR link).

The server intentionally has no `git2` dependency. No clone code paths exist in this crate.

### Dashboard (TBD — working assumption: Next.js)

The dashboard ships separately, behind the server's HTTP API.

- **Framework**: Next.js (App Router).
- **Styling**: Tailwind.
- **Components**: shadcn/ui.
- **Auth (POC)**: GitHub App install grants access.

No shared types crate across the language boundary in v1 — the contract is an OpenAPI / JSON-schema description exposed by `agit-server`. If the JS/TS surface becomes a maintenance drag, Rust+Leptos / HTMX-on-Axum is a one-week reversal.

### Coding-agent backend (= "providers")

Agit is **provider-agnostic**. The runner doesn't know what Claude is; it knows what `local_command`, `anthropic_api`, `openai_api`, and `openai_compatible` are. Adding a new provider type is a new variant + a new module under `crates/agit-runner/src/providers/`.

Customers configure their own providers and bring their own credentials. **Agit does not sell access to any model.**

## Data model (POC)

Minimum tables, owned by `agit-server`. All times UTC.

```sql
-- Server-side. Mostly metadata; no source code, no model credentials.

runners                              -- one row per self-hosted runner
  id            text primary key
  org_id        text not null
  display_name  text not null
  token_hash    text not null        -- bearer token salt+hash
  last_seen_at  timestamptz
  created_at    timestamptz not null

agents                               -- snapshot at the time of run
  id            text primary key
  repo          text not null        -- "owner/name"
  name          text not null        -- "test_writer"
  config_json   text not null        -- AgentConfig snapshot
  created_at    timestamptz not null
  unique (repo, name, created_at)

missions
  id            text primary key
  repo          text not null
  agent_id      text references agents(id)
  trigger_type  text not null        -- "github_issue_label"
  trigger_ref   text not null        -- "issue:#12"
  status        text not null        -- pending|assigned|completed|failed
  assigned_to   text references runners(id)
  created_at    timestamptz not null
  updated_at    timestamptz not null

runs
  id            text primary key
  mission_id    text not null references missions(id)
  runner_id     text not null references runners(id)
  status        text not null        -- running|policy_violation|tests_failed|pr_opened|failed
  branch_name   text
  pr_number     int
  policy_status text                  -- passed|failed|skipped
  test_status   text                  -- passed|failed|skipped
  cost_usd      numeric(10,4)
  duration_ms   int
  iterations    int
  created_at    timestamptz not null
  updated_at    timestamptz not null

run_steps
  id            text primary key
  run_id        text not null references runs(id)
  step          text not null         -- mission_pulled|repo_cloned|config_loaded|provider_invoked|policy_checked|tests_run|branch_pushed|pr_opened
  status        text not null         -- pending|running|ok|error
  started_at    timestamptz
  finished_at   timestamptz
  log           text                  -- structured JSON or plain text

policy_violations
  id            text primary key
  run_id        text not null references runs(id)
  rule          text not null         -- "write" | "command" | "limit"
  allowed       text not null         -- JSON array
  attempted     text not null         -- offending path or command
  created_at    timestamptz not null
```

The runner stores nothing persistent. Workspaces are ephemeral.

## Module layout

What exists today:

```
agit/
├── Cargo.toml                          workspace
├── crates/
│   ├── agit-core/                      schema + (future) policy + run state
│   │   └── src/
│   │       ├── lib.rs
│   │       └── config.rs               AgitConfig, ProviderConfig, AgentConfig, …
│   ├── agit-cli/                       local config inspector (`agit` binary)
│   │   └── src/
│   │       ├── main.rs
│   │       └── cli.rs                  list / providers / show / validate
│   ├── agit-runner/                    self-hosted runner scaffold
│   │   └── src/main.rs                 start / check (stubs today)
│   └── agit-server/                    control-plane server scaffold
│       └── src/main.rs                 serve / migrate (stubs today)
├── .agit/                              this repo's own Agit config
├── demo-project/                       simplified target project for demos
└── docs/
```

Target layout for the POC:

```
crates/
├── agit-core/
│   ├── config.rs          # YAML schema (here today)
│   ├── policy.rs          # pure: check(diff, policy) → Result<(), PolicyViolation>
│   └── run.rs             # Run, RunStep state machine
├── agit-cli/              # the `agit` binary (here today, growing policy-check)
├── agit-runner/
│   ├── main.rs            # binary entry
│   ├── server_client.rs   # talks to agit-server's runner API
│   ├── workspace.rs       # ephemeral clone management
│   ├── providers/
│   │   ├── local_command.rs
│   │   ├── anthropic_api.rs
│   │   ├── openai_api.rs
│   │   └── openai_compatible.rs
│   └── git.rs             # branch + PR via octocrab
└── agit-server/
    ├── main.rs            # axum bootstrap
    ├── routes/
    │   ├── webhooks.rs    # GitHub App
    │   └── runners.rs     # missions/next, status, done
    ├── db.rs              # sqlx + migrations
    └── dashboard/         # JSON API for the Next.js front (or templates if Rust UI)
```

`agit-core` stays pure: no `tokio`, no async, no HTTP, no clap. That's what makes it safely reusable across CLI + runner + server, and trivially unit-testable in milliseconds.

## Sequence of a Run (with both components)

```
1. GitHub  ────webhook (issues.labeled)────▶ agit-server
                                              │
2. agit-server                                │
   ├── verify webhook signature              │
   ├── dedupe by delivery_id                 │
   ├── fetch .agit/agents.yaml from repo     │
   ├── parse via agit_core::config           │
   ├── match trigger → AgentConfig snapshot  │
   ├── insert Mission(status=pending)        │
   └── enqueue                                │
                                               
3. agit-runner long-poll                       
   ├── GET /missions/next   ──◀──── agit-server returns Mission #42
   ├── status=running
   │
   ├── clone repo (shallow) into temp workspace
   ├── parse .agit/agents.yaml locally (defense in depth)
   ├── resolve providers[agent.provider]
   ├── invoke Provider:
   │     local_command → exec(claude ... )
   │     anthropic_api → reqwest POST
   │     openai_api → reqwest POST
   │     openai_compatible → reqwest POST
   ├── collect diff + commands the agent wanted to run
   │
   ├── agit_core::policy::check(diff, policy)
   │     on failure → POST /missions/42/status (policy_violation)
   │                  STOP, no commit
   │
   ├── execute allowed commands (tests, typecheck)
   │     on failure → status=tests_failed, STOP
   │
   ├── git push branch (App identity)
   ├── octocrab.open_pull_request(...) with the canonical body
   │
   └── POST /missions/42/done (status=pr_opened, pr_number, cost, duration)

4. agit-server
   ├── update Mission + Run rows
   └── dashboard reflects the new state
```

Every step writes a `RunStep` so the dashboard can show progress live.

## Security guardrails

- **Workspaces are ephemeral** on the runner: cloned into a temp dir, deleted at end of Run regardless of outcome. Never reuse across Runs.
- **No network egress** from the agent's process beyond what its provider requires. `local_command` providers run with a deny-by-default firewall hint where possible (out of POC; documented as a future hardening).
- **Provider secrets** live in env vars on the **runner**, never in YAML, never on the server. The server has no field that could hold one.
- **GitHub App scopes** are the minimum: `Contents: R/W`, `Issues: R/W`, `Pull requests: R/W`, `Metadata: R`. Add scopes only when a feature requires them.
- **Deny by default**: lockfiles, `.env*`, `.git/` are blocked even if a glob would technically match.
- **Resource limits**: per-Run timeout, max diff size, max log size, `max_iterations`, `max_cost_usd`. Conservative defaults.
- **Runner ↔ server transport** is HTTPS-only in production. Bearer tokens are hashed at rest, single-use rotation on a `rotate-token` admin endpoint.

## What this architecture deliberately does *not* address (POC)

- Multi-node runners, HA, distributed scheduling (deferred).
- Postgres (use SQLite; migrate when Enterprise is on the table).
- Org-wide policy across repos (deferred — paid tier).
- Run replay / debugger (deferred).
- Marketplace (deferred).
- Pre-built runner images for k8s / GitHub Actions (deferred; the runner is a single binary, deploying it anywhere is the customer's call).

See [BUSINESS_MODEL.md](BUSINESS_MODEL.md) for the deliberate split between CE-now and Enterprise-later capabilities.
