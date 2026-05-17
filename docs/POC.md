# POC

The goal of the POC is to prove the **self-hosted control loop** in a 5-minute demo:

> Declare agents + providers in Git → see a mission flow from server to a self-hosted runner → get a controlled PR back, with logs and policy visible in the dashboard.

The differentiator the POC must showcase: **the runner is self-hosted; code and model credentials never leave the customer's infra**. The Agit Server only orchestrates and observes.

The POC must explicitly use an existing coding agent behind the scenes (e.g. Claude Code CLI as a `local_command` provider). Agit's job is to surround that agent with config, policy, observability, and a Git workflow — not to be smart in the agent's place.

## Demo scenario

The POC demo is **two passes** on the same setup. The first proves the loop. The second — the one that actually sells Agit — proves the policy/governance wedge by showing what happens when an agent tries to step out of bounds.

### Setup (once, on a laptop or a single VM)

```bash
# Bring up server + runner side by side.
docker compose up -d         # spins up agit-server, agit-runner, sqlite

# Install the Agit GitHub App on a test repo.
# Configure the runner with the provider it should use:
#   - Mount the user's local Claude Code CLI binary into the runner image, OR
#   - Set ANTHROPIC_API_KEY in the runner's env if anthropic_api is preferred.

# Push a `.agit/agents.yaml` to the test repo with one provider + one agent:
#   provider: claude_code (type: local_command, command: "claude")
#   agent:    test_writer (write: tests/**, commands: pnpm test)
```

At this point:

- Agit Server listens for GitHub webhooks, exposes a dashboard.
- Agit Runner is connected to the server, has the configured provider available locally.
- Nothing about the test repo's code is in the server's storage yet.

### Pass 1 — Happy path (the self-hosted loop works)

1. An issue exists in the test repo:
   ```
   Add tests for the function parseAmount
   ```
2. A maintainer adds the label `agent:test`.
3. **Agit Server** receives the `issues.labeled` webhook, reads `.agit/agents.yaml` from the repo metadata, matches the `test_writer` agent, creates `Mission #42`.
4. **Agit Runner** (running on the maintainer's infra) pulls the Mission.
5. Runner clones the repo locally, in an isolated workspace.
6. Runner invokes the configured Provider (`claude_code` → exec `claude`).
7. The Provider edits files. The runner only commits paths in `permissions.write` (`tests/**`).
8. The runner only executes commands in `permissions.commands.allow` (`pnpm test`).
9. The runner pushes `agit/test-writer/issue-12` and opens a clean PR.
10. The runner reports the result (status, files changed, command output, cost, duration) back to the server.
11. The Agit dashboard shows:
    ```
    Status:        PR opened
    Agent:         test_writer
    Provider:      claude_code (local_command)
    Issue:         #12
    Files changed: tests/parseAmount.test.ts
    Command run:   pnpm test
    Cost:          $0.18  (from the runner's accounting)
    Result:        success
    ```

Critically: throughout this pass, the **server saw no source code and no model credential**. It only stored the mission metadata and the runner's structured status updates.

### Pass 2 — Sad path (the wedge: governance is real)

This pass is what makes the demo memorable. The policy engine already exists for the happy path; reusing it for the sad path costs almost no engineering.

1. A new issue exists, deliberately framed to make the agent want to edit production code:
   ```
   parseAmount has a bug on negative inputs — fix it and add a regression test
   ```
2. The maintainer adds `agent:test` (the wrong label for a code fix — or simply because someone trusted the agent too much).
3. The runner runs `test_writer` as before.
4. The agent attempts to edit `src/parseAmount.ts` to fix the bug.
5. **Policy check fails** at the pre-commit boundary, *inside the runner*:
   ```
   PolicyViolation
     agent:       test_writer
     rule:        write
     allowed:     ["tests/**"]
     attempted:   src/parseAmount.ts
   ```
6. The Run terminates with status `policy_violation`. No commit, no branch, no PR.
7. The runner reports the violation to the server.
8. The server posts on the issue:
   ```
   Agit blocked this run. test_writer is not allowed to write to src/**.
   See run: https://your-agit/runs/43
   ```
9. The dashboard shows the violation, the attempted diff (provided by the runner), and the rule that fired.

The demo's payoff line: *the agent's intent does not override the team's policy — and the enforcement happens in your infra, not in ours*.

## Scope — in

### 1. `agit-server` (minimum)

- HTTP listener (axum) on port 3000.
- GitHub App webhook receiver: `issues.labeled` first; `pull_request` for `blocking_review` agents later.
- Mission queue (SQLite via `sqlx` for the POC).
- Runner-facing API:
  - `GET  /missions/next` — runner long-polls for work, authenticated by bearer token.
  - `POST /missions/{id}/status` — runner reports status/logs/policy/cost.
- Minimal dashboard: agents list, runs list, run detail.
- Stays code-blind: no clone, no model call, no secret storage.

### 2. `agit-runner` (minimum)

- Long-running process: `agit-runner start --server <url> --token <secret>`.
- Polls the server for missions.
- For each Mission:
  1. Clone repo (shallow, into an isolated workspace).
  2. Parse `.agit/agents.yaml` (via `agit-core::config`).
  3. Resolve the agent's provider.
  4. Invoke the provider:
     - `local_command`: exec the binary, feed the prompt, collect output diff + commands.
     - `anthropic_api` / `openai_api` / `openai_compatible`: planned next; not required for the first demo.
  5. Run policy check on the resulting diff and commands (via the future `agit-core::policy`).
  6. Run allowed test commands.
  7. Push branch, open PR via the GitHub App.
  8. Report back to the server.
- Diagnostic subcommand: `agit-runner check` — already prints declared providers; will grow PATH lookup, env-var presence, and endpoint reachability checks.

### 3. `.agit/agents.yaml` schema (already in code)

- `version`, `project.name`.
- `providers.<name>` (one of `local_command`, `anthropic_api`, `openai_api`, `openai_compatible`).
- `agents.<name>` (`description`, `provider`, `prompt`, `trigger`, `permissions`, `output`, `limits`).
- Cross-validated: every `agent.provider` must reference a declared provider.

### 4. Policy engine (`agit-core::policy`)

Pure, synchronous, globset-backed. `PolicyChecker::from_write_globs(...)` compiles an agent's `permissions.write`, and `check(paths)` returns a `Vec<PolicyViolation>` (empty when the diff is in-policy). A hard deny list (`.env*`, `.git/**`, `**/secrets/**`) is always enforced regardless of the agent's globs.

This is what unlocks the sad-path demo. Wired into `agit-runner` for issue/retry orchestrators today; will be reused by `agit-cli policy-check` and the server's per-Run report.

### 5. Provider implementations in `agit-runner`

Priority order:

1. **`local_command`** — covers Claude Code CLI, Codex CLI, Aider, custom scripts. The widest blast radius for least code.
2. **`openai_compatible`** — covers Ollama / vLLM / LM Studio / OpenRouter without per-vendor code.
3. **`anthropic_api`** — first-class for customers who pay Anthropic directly.
4. **`openai_api`** — same for customers on OpenAI.

The first demo uses **only** `local_command`. The others are scoped in for the post-demo iterations.

## Scope — out

Deliberately deferred:

- Custom HCL-style language.
- Multi-agent pipelines / DAGs.
- GitLab / Bitbucket support.
- Marketplace of agents.
- Complex secret/RBAC permissions.
- Auto-merge.
- Enterprise SSO.
- Kubernetes operator for the runner.
- A pre-built GitHub Actions runner image.
- A no-code agent builder.

## Roadmap (now structured around the two components)

### Step 1 — `agit-cli` for local validation *(done)*

- `agit list`, `agit providers`, `agit show <name>`, `agit validate`.
- Cross-validates `agent.provider` references.

**Validates**: the schema and the `agit-core` library.

### Step 2 — `agit-core::policy` + `agit-cli policy-check`

- Pure synchronous policy engine.
- CLI subcommand: `agit policy-check --diff <diff-file>` simulates a violation against a local diff.

**Validates**: the policy primitives in isolation, before any networking.

### Step 3 — `agit-runner` against a local mission

- `agit-runner run-once --mission ./fixtures/mission.json`.
- Skips the server entirely: reads a mission from disk, clones, invokes a `local_command` provider, runs policy, opens a PR via a personal access token.

**Validates**: the full single-run loop, end-to-end, with zero server.

### Step 4 — `agit-server serve` + runner-facing API

- Webhooks in, missions persisted, runners pull missions, report status.
- Minimal dashboard.
- `docker compose up` spins server + runner + sqlite together.

**Validates**: the two-component loop. This is the demo.

### Step 5 — More providers + sad-path polish

- `openai_compatible`, `anthropic_api`.
- Policy violation surfaces beautifully in the dashboard.
- This is what we show to CTOs.

## Success criteria

The POC passes if a maintainer can:

1. `docker compose up` server + runner locally.
2. Install the Agit GitHub App on a test repo.
3. Push a `.agit/agents.yaml` declaring one `local_command` provider and one agent.
4. Label an issue with `agent:test`.
5. Watch the Run appear in the dashboard while the **runner** (on their machine) does the work.
6. See the agent open a PR that respects `permissions.write` (`tests/**` only).
7. See `pnpm test` was the only command run.
8. Review and merge the PR like any human PR.
9. Trigger a deliberate sad-path scenario and see Agit **block** the agent with a clear `PolicyViolation`, no commit, no PR — and verify that no source code ever appeared in the server's storage.

Items 1–8 prove the loop. Item 9 proves the wedge.

## Tech stack (recap)

See [ARCHITECTURE.md](ARCHITECTURE.md) for the full picture.

- **Shared library**: `agit-core` (Rust, no async, no I/O beyond YAML loader). Schema, policy, run-state types.
- **CLI**: `agit-cli` (`agit` binary). Local-only.
- **Runner**: `agit-runner` (planned deps `tokio`, `reqwest`, `git2`, `globset`, `octocrab`).
- **Server**: `agit-server` (planned deps `axum`, `tokio`, `sqlx` + SQLite).
- **Dashboard**: deferred; working assumption Next.js + Tailwind + shadcn/ui talking to `agit-server` over HTTP.
- **Coding-agent backend**: any local CLI (`claude` first) via `local_command`; APIs and OpenAI-compatible endpoints after.

## PR format produced by Agit

Every PR opened by Agit follows this body template — readability is part of the product:

````md
## Agit run

Agent: `test_writer`
Provider: `claude_code` (local_command — ran on runner host)
Triggered by: Issue #12
Policy: Passed
Tests: Passed

## Summary

Added unit tests for `parseAmount`.

## Files changed

- `tests/parseAmount.test.ts`

## Commands run

```bash
pnpm test
```

## Policy checks

- Agent can write to `tests/**`: passed
- No forbidden files modified: passed
- Human review required: yes

## Notes

This PR was generated by Agit. The runner that produced it ran on
`<runner hostname>`; no source code or model credentials transited
through Agit Cloud. A human review is required before merge.
````
