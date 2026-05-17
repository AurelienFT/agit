# demo-project

A minimal target repository used to **demonstrate** what Agit does. No Agit code lives here — just a tiny TypeScript project and a heavily commented `.agit/agents.yaml`.

The `.agit/agents.yaml` in this folder is the one to read first if you want to learn the schema. It walks through **three providers** (showing that Agit is model-agnostic) and **three agents** (each demonstrating a different combination of trigger and output):

### Providers declared here

| Provider | Type | Demonstrates |
|---|---|---|
| `claude_code` | `local_command` (`claude`) | The runner exec's a local CLI — the most flexible provider. |
| `anthropic_cloud` | `anthropic_api` | Official API; api key from a runner-side env var (`ANTHROPIC_API_KEY`). |
| `local_qwen` | `openai_compatible` | Any OpenAI-shaped endpoint — Ollama, vLLM, LM Studio, OpenRouter, etc. |

### Agents declared here

| Agent | Provider | Trigger | Output | Demonstrates |
|---|---|---|---|---|
| `test_writer` | `claude_code` | `github_issue_label` (`agent:test`) | `pull_request` | Reference loop: label → run → policy → PR. |
| `security_reviewer` | `anthropic_cloud` | `github_pull_request` on `src/auth/**`, `src/payments/**` | `blocking_review` | Read-only agent gating sensitive paths. |
| `readme_polisher` | `local_qwen` | `manual` | `comment` | Advice-only agent invoked on demand, running on a local model. |

## Running the CLI against this project

From the repo root (where `Cargo.toml` lives):

```bash
cargo run -p agit-cli -- -C demo-project list             # providers + agents
cargo run -p agit-cli -- -C demo-project providers        # providers only
cargo run -p agit-cli -- -C demo-project show security_reviewer
cargo run -p agit-cli -- -C demo-project validate         # schema + reference checks
```

`agit list` summarizes each provider and agent. `show <name>` prints the full agent definition in YAML **plus its resolved provider**. `validate` exits non-zero with a clear error if the file fails to parse or if any `agent.provider` references a missing provider.

## Diagnostic from the runner

The self-hosted runner has a read-only diagnostic that inspects the same config:

```bash
cargo run -p agit-runner -- check -C demo-project
```

This is what a customer would run on a runner host to see which declared providers are available (PATH for `local_command`, env vars for API providers, endpoint reachability for `openai_compatible`). Today it lists the providers; the actual availability probing is the next implementation step.

## What's in this folder

- `.agit/agents.yaml` — the commented tour of the schema (3 providers, 3 agents).
- `.agit/prompts/test_writer.md` — example system prompt referenced by `test_writer`.
- `src/parseAmount.ts` — the tiny function from the POC demo scenario.
- `tests/parseAmount.test.ts` — placeholder test the agent will expand.
- `package.json` — stub scripts so `pnpm test` etc. have something to call.

## Where the *full* (minimal) demo lives

The repo root **is itself** a working Agit project — `../.agit/agents.yaml` is the realistic minimal config (one provider, one agent). Use this folder (`demo-project/`) to see the schema's breadth; use the root to see the schema applied honestly.
