# Concepts

Vocabulary of Agit. These names are canonical — code, UI, docs, and config keys use them consistently.

## Object diagram

```
              Trigger ──(fires)──▶ Mission (lives on Agit Server)
                                       │
                                       │ pulled by
                                       ▼
                                     Runner ── invokes ──▶ Provider ──▶ model / CLI
                                       │
                                       └── executes ──▶ Run (steps, logs, status, cost)
                                                          │
                                                          ├── governed by ──▶ Policy
                                                          └── produces ─────▶ Output
```

## Provider

How an agent reaches a model. Declared once at the top of `.agit/agents.yaml`, referenced by each agent.

```yaml
providers:
  claude_code:
    type: local_command
    command: "claude"
  anthropic_cloud:
    type: anthropic_api
    api_key_env: "ANTHROPIC_API_KEY"
    model: "claude-sonnet-4.5"
  local_qwen:
    type: openai_compatible
    base_url: "http://localhost:11434/v1"
    model: "qwen2.5-coder"
```

The Provider abstraction is the load-bearing piece that keeps Agit model-agnostic. Variants:

- `local_command` — exec a CLI on the runner host (`claude`, `codex`, `aider`, …). Maximally flexible.
- `anthropic_api` — official Anthropic API. The `api_key_env` names an env var on the runner; the secret never appears in the YAML.
- `openai_api` — official OpenAI API.
- `openai_compatible` — any OpenAI-shaped endpoint: Ollama, vLLM, LM Studio, OpenRouter, internal proxies.

**Trust property**: the runner is the only component that talks to a provider. Provider credentials never leave the runner host; the Agit Server never sees them.

Code type: `ProviderConfig`.

## Agent

A role declared in `.agit/agents.yaml`. An Agent has:

- `description` — human-readable purpose.
- `provider` — key into the top-level `providers:` map. Required.
- `prompt` — optional path to a Markdown system prompt.
- `trigger` — when it activates.
- `permissions` — what it can read, write, and run.
- `output` — what it produces (PR, blocking review, comment, patch).
- `limits` — guardrails (`max_iterations`, `max_files_changed`, `max_cost_usd`, …).

Examples: `test_writer`, `bugfixer`, `security_reviewer`, `dependency_updater`.

Code type: `AgentConfig`.

## Trigger

The event that creates a Mission. Variants:

- `github_issue_label` — label appears on an issue.
- `github_pull_request` — PR opened/updated; optionally scoped by `paths:`.
- `github_comment_command` — `/agit run <agent>` in a comment.
- `manual` — kicked off from the dashboard or CLI.
- *(future)* `schedule` — cron-style recurring runs.

Code type: `TriggerConfig`.

## Mission

A specific tasking instance — "run `test_writer` against issue #12 of repo X". Created by `agit-server` from a trigger; lives in the server's database; pulled by a runner.

One Mission can spawn one or more Runs (e.g., on retry).

Conceptually: `Mission = (Agent, TriggeringEvent, Context)`.

## Run

The concrete execution **on the runner**. A Run records every step from clone to PR and reports status/logs/cost back to the server. It is the unit of observability in the dashboard.

Run fields (minimum):

- `status` — `queued`, `pulled`, `running`, `policy_violation`, `tests_failed`, `pr_opened`, `failed`.
- `branch_name` — pushed branch, if any.
- `pr_number` — opened PR.
- `policy_status`, `test_status`.
- `cost_usd`, `duration_ms`, `iterations`.
- `logs` — structured per `RunStep`.

`RunStep`s: `mission_pulled`, `repo_cloned`, `config_loaded`, `provider_invoked`, `policy_checked`, `tests_run`, `branch_pushed`, `pr_opened`.

Code types: `Run`, `RunStep`.

## Policy

The rules an agent must respect, derived from `permissions` and `limits`. Policy is **deny by default**, enforced by the runner.

```yaml
permissions:
  read:  ["src/**", "tests/**", "package.json"]
  write: ["tests/**"]
  commands:
    allow: ["pnpm test", "pnpm typecheck"]

limits:
  max_iterations: 3
  max_files_changed: 5
  max_cost_usd: 1.00
```

Checked at three boundaries:

1. **Before commit** — every modified path must be in `permissions.write`.
2. **Before command execution** — every command must be in `permissions.commands.allow`.
3. **Per Run** — `max_iterations`, `max_files_changed`, `max_cost_usd` are hard ceilings.

A failed check produces a `PolicyViolation` with the exact rule and offending path/command. Run status becomes `policy_violation`; no commit, no PR.

Forbidden by default, regardless of globs: `.env*`, secrets, `package-lock.json` / `pnpm-lock.yaml` / `Cargo.lock` (unless explicitly allowed), `.git/`.

Code types: `PermissionPolicy`, `PolicyViolation`.

## Output

What a successful Run produces. The POC supports `pull_request`; other variants are scoped in.

```yaml
output:
  type: pull_request
  branch_prefix: "agit/test-writer/"
  require_human_review: true
```

Variants:

- `pull_request` — default. Reviewable, never auto-merged in POC.
- `blocking_review` — runner posts a `REQUEST_CHANGES` review on an existing PR. The PR cannot merge until the review is dismissed.
- `comment` — runner posts on the issue/PR without code changes.
- `patch` — runner emits a patch artifact without opening a PR (CI use).

## Naming summary (config keys ↔ code identifiers)

| Concept     | YAML key             | Code identifier         |
|-------------|----------------------|-------------------------|
| Provider    | `providers.<name>`   | `ProviderConfig`        |
| Agent       | `agents.<name>`      | `AgentConfig`           |
| Trigger     | `trigger`            | `TriggerConfig`         |
| Mission     | —                    | `Mission`               |
| Run         | —                    | `Run`, `RunStep`        |
| Policy      | `permissions`, `limits` | `PermissionPolicy`   |
| Violation   | —                    | `PolicyViolation`       |
| Output      | `output`             | `OutputConfig`          |
