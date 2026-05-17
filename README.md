# Agit

> **Agit is a self-hostable control plane for AI coding agents.** Declare agents and providers in `.agit/agents.yaml`, govern them through Git, and execute them in your own infrastructure — your code and model credentials never transit through Agit Cloud.

When the privacy story is the lead message:

> **Run AI agents on your codebase without sending your code or secrets to Agit.**

## Two components, one job: keep humans in the loop and code in your infra

```
GitHub / GitLab
    │  webhook
    ▼
Agit Server      orchestrates: missions, runs, dashboard
    │  (auth'd HTTP — no code, no secrets)
    ▼
Agit Runner      self-hosted on YOUR infra (laptop, server, k8s, GitHub Actions)
    │
    ├── clones the repo
    ├── invokes the agent's Provider (Claude Code, Anthropic, OpenAI, Ollama, …)
    ├── enforces the policy (read / write / commands)
    ├── runs allowed tests
    └── opens a reviewable PR

         ↑
Agit Cloud / self-hosted Agit Server displays the run, never touches the code.
```

`agit-server` orchestrates. `agit-runner` does the work, in your environment. Together they answer the questions every engineering team eventually asks:

- Does my code ever leave my infra? → **No** (the runner is self-hosted).
- Who sees my model credentials? → **Only your runner** (Agit Server never sees them).
- Which agents are allowed to write where? → Declared in `.agit/agents.yaml`, enforced by the runner.
- What did an agent run, on what authority, at what cost? → Audited in the dashboard.
- Can I use my own models? → Yes: any local CLI, any OpenAI-compatible endpoint, the official Anthropic/OpenAI APIs.

## Configuration: `.agit/agents.yaml`

A real-world minimal config:

```yaml
version: "1"

providers:
  claude_code:
    type: local_command
    command: "claude"        # the runner exec's the local Claude Code CLI

agents:
  test_writer:
    description: "Writes tests for issues labeled agent:test"
    provider: claude_code
    trigger:
      type: github_issue_label
      label: "agent:test"
    permissions:
      read:  ["src/**", "tests/**", "package.json"]
      write: ["tests/**"]
      commands:
        allow: ["pnpm test"]
    output:
      type: pull_request
      require_human_review: true
```

Push the file. Label an issue `agent:test`. The Agit server creates a Mission. Your runner picks it up, invokes the local Claude Code CLI, writes tests under `tests/**` only, runs `pnpm test`, opens a clean PR. The dashboard shows the run.

The provider abstraction means swapping `local_command` for `anthropic_api`, `openai_api`, or `openai_compatible` (Ollama / vLLM / LM Studio / OpenRouter / internal proxies) is a YAML edit — no code change.

See [demo-project/.agit/agents.yaml](demo-project/.agit/agents.yaml) for a heavily commented tour of the schema with three different providers and three different agents.

## How it differs from Fast.io and "AI Agent GitOps"

```
Fast.io  =  GitOps for running agents (deployment, runtime, workspaces).
Agit     =  Self-hostable control plane for agents contributing to your codebase
            (issues → missions → branches → PRs → reviews → merge gates).
```

Fast.io provides the agent runtime as a service. Agit lets you bring your runtime (or use our open-source one) and adds policy, observability, and Git workflow on top — without your code ever leaving your infra.

## Status (Rust workspace, pre-MVP)

Four Cargo crates:

| Crate (package) | Binary | Role |
|---|---|---|
| `agit-core` | _(library)_ | Shared schema, (future) policy engine, run state. No I/O, no async. |
| `agit-cli` | `agit` | Local config inspector — `list`, `providers`, `show`, `validate`. Never contacts the server. |
| `agit-runner` | `agit-runner` | Self-hosted runner that consumes missions. Scaffold; `check` reads YAML, `start` is stub. |
| `agit-server` | `agit-server` | Control-plane server (webhooks, missions, dashboard). Scaffold; `serve` is stub. |

Build & explore:

```bash
cargo build
cargo run -p agit-cli -- list                          # this repo's own .agit config
cargo run -p agit-cli -- -C demo-project list          # the heavily commented demo
cargo run -p agit-runner -- check -C demo-project      # diagnostic on provider declarations
cargo run -p agit-server -- serve                      # placeholder until HTTP lands
```

Full POC plan and roadmap: [docs/POC.md](docs/POC.md).

## Business model

GitLab-style open-core, sharpened by the self-hosted runner:

- **Community Edition** — both `agit-server` and `agit-runner` are OSS and self-hostable. Customer hosts the full stack on their infra. Free.
- **Agit Cloud (paid)** — managed `agit-server`. Customer still runs their own `agit-runner`, so code/secrets stay in their infra. The Cloud sells orchestration UX + reliability, not access to models.
- **Enterprise** — Cloud or self-managed with org-wide governance, audit log, SSO/RBAC, cost analytics, HA/Postgres, support.

**The runner is always OSS.** Anything that touches customer code or secrets is OSS. We don't sell access to any model — customers configure their own providers and are responsible for the relevant terms.

Details: [docs/BUSINESS_MODEL.md](docs/BUSINESS_MODEL.md).

## Developing Agit by labeling issues

This repo is set up to develop itself via Agit. Open a GitHub issue with one of the templates:

| Label | Agent | What it does |
|---|---|---|
| `agit:test` | `test_writer` | Adds Rust tests under `crates/*/tests/**` (or `#[cfg(test)]` inline). |
| `agit:doc` | `doc_updater` | Updates Markdown documentation. |
| `agit:feature` | `feature_engineer` | Implements small features in `crates/**/src`. |

The flow is split between GitHub and your machine to keep the trust posture clean — **no Anthropic API key is stored in GitHub**:

1. **GitHub Actions** (`.github/workflows/agit-runner.yml`) validates `.agit/agents.yaml`, confirms the label maps to a declared agent, and comments on the issue with the exact command to run locally.
2. **Locally**, you run `./scripts/agit-run <issue-number>`. The script:
   - Builds the prompt from `.agit/prompts/<agent>.md` + the issue title and body.
   - Invokes your local `claude` CLI headlessly (`claude --print --allowedTools …`). Authentication is whatever `claude` already has on your machine — Anthropic's terms apply to that session the same way they apply to any interactive Claude Code use.
   - Runs the post-flight policy check (write globs + deny-by-default lockfiles / `.env*` / `.git/**`).
   - Runs the agent's allowed commands (`cargo test`, etc.).
   - Pushes a branch and opens a PR.

This is the *GitHub-Actions trigger + local runner* shape of the architecture. When the `agit-runner` binary is fully implemented, `scripts/agit-run` shrinks to a `agit-runner run-once` wrapper.

One-time setup (push the repo, create labels, install local CLIs): **[SETUP.md](SETUP.md)**. Workflow details + classic contribution path: **[CONTRIBUTING.md](CONTRIBUTING.md)**.

## Docs

- [docs/CONCEPTS.md](docs/CONCEPTS.md) — domain vocabulary (Provider, Agent, Run, Mission, Policy, Trigger, Output).
- [docs/POC.md](docs/POC.md) — POC scope: self-hosted server+runner demo (happy + sad path), roadmap, success criteria.
- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) — system architecture, two-component split, provider abstraction, data model.
- [docs/BUSINESS_MODEL.md](docs/BUSINESS_MODEL.md) — open-core tiers and the self-hosted-runner reinforcement.
- [CLAUDE.md](CLAUDE.md) — guidance for AI coding agents working in this repo.
- [CONTRIBUTING.md](CONTRIBUTING.md) — how to contribute (agent route or classic PR).
- [SETUP.md](SETUP.md) — one-time setup checklist for the Agit-on-Agit workflow.
- [.agit/agents.yaml](.agit/agents.yaml) — this repo's own config (used by `agit list` from the root).
- [demo-project/](demo-project/) — heavily commented schema tour.
