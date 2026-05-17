# Contributing

Two ways to contribute to **agit**:

1. **Through an Agit-managed agent** — open an issue with `agit:test`, `agit:doc`, or `agit:feature`. The runner opens a PR. You review and merge.
2. **The classic way** — branch, PR, review.

Both routes hit the same CI (`.github/workflows/ci.yml`) and the same merge bar.

## Local development

```bash
cargo build
cargo test
cargo run -p agit-cli -- list                      # this repo's own .agit config
cargo run -p agit-cli -- -C demo-project list      # the heavily commented demo
cargo run -p agit-runner -- check -C demo-project  # runner diagnostic (stub today)
```

Required toolchain pinned in `rust-toolchain.toml` (stable + rustfmt + clippy).

## Style and lints

Before pushing:

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

CI re-runs all three with `-D warnings` and fails on any deviation. Agent-opened PRs go through the same gate.

## Architecture rules you must not break

These are load-bearing — the project's whole story depends on them. CI doesn't enforce them today; reviewers do.

1. **`agit-core` stays pure.** No `tokio`, no async, no clap, no HTTP, no I/O beyond the YAML loader. Anything async belongs in `agit-runner` or `agit-server`.
2. **`agit-server` is code-blind.** It must never clone customer repos, never invoke model APIs, never hold model credentials. Cloning, invoking, and credential-handling are the runner's job.
3. **The runner is OSS.** Anything that touches customer code or model credentials must remain in the free CE — never paywall the runner or any provider implementation.
4. **No secrets in YAML.** Provider credentials are referenced via env-var names (`api_key_env: ANTHROPIC_API_KEY`), never literal values.
5. **No auto-merge.** The default output is `pull_request` with `require_human_review: true`. Don't add a code path that merges agent PRs without review.

See `CLAUDE.md` for the longer "what to do / what to avoid" reference.

## Working with an Agit agent

The flow is split deliberately to keep the trust posture clean: **GitHub Actions triggers, your local `claude` does the work**. No Anthropic API key is stored in GitHub.

1. You open an issue with one of the `agit:*` labels.
2. `.github/workflows/agit-runner.yml` runs: validates `.agit/agents.yaml`, confirms the label maps to a declared agent, and **comments on the issue** with the exact command to run locally — something like:
   ```bash
   ./scripts/agit-run <issue-number>
   ```
3. On a machine where Claude Code is installed and authenticated, you run that command. `scripts/agit-run`:
   - Resolves the matching agent from `.agit/agents.yaml`.
   - Creates `agit/<agent>/issue-<n>` off the default branch.
   - Builds the prompt from the agent's `.agit/prompts/<agent>.md` + the issue title and body.
   - Invokes `claude --print --allowedTools <…>` headlessly. **Authentication is whatever your local `claude` has** (Pro/Max OAuth, Bedrock, Vertex, etc.) — same as any interactive session.
   - Runs the post-flight policy check on the diff (mirrors the future `agit-core::policy`). A change to a path outside the agent's `permissions.write` is blocked here, not at review time.
   - Runs the agent's allowed commands (`cargo test`, etc.).
   - Pushes the branch and opens a PR via `gh`.
4. You review the PR like any other.

This is the "GitHub Actions + local runner" deployment shape of the architecture. When the full `agit-runner` binary is implemented, `scripts/agit-run` shrinks to a `agit-runner run-once` wrapper without changing the rest.

### Picking the right label

| Label | Agent | Can write | Can run |
|---|---|---|---|
| `agit:test` | `test_writer` | `crates/*/tests/**`, `crates/*/src/**/*.rs`, `crates/*/Cargo.toml` | `cargo test`, `cargo check`, `cargo fmt --check` |
| `agit:doc` | `doc_updater` | `**/*.md`, `docs/**` | _none_ |
| `agit:feature` | `feature_engineer` | `crates/**/src/**`, `crates/**/tests/**`, `crates/**/Cargo.toml`, `Cargo.toml` | `cargo build`, `cargo test`, `cargo check`, `cargo fmt --check`, `cargo clippy` |

Cross-cutting things `.github/**`, `.agit/**`, `*.yaml`, lockfiles, `.env*` are **never** writable by an agent. Use the classic PR route for those.

### Issue templates

Three are wired up:

- **Test request** → applies `agit:test`.
- **Doc update** → applies `agit:doc`.
- **Feature request** → applies `agit:feature`.

Open issues through the templates rather than blank — they prompt you for the structure the agent needs.

## Setting up Agit on a fresh clone of this repo

See [SETUP.md](SETUP.md) for the one-time checklist (secret, labels, permissions).

## Reporting bugs in Agit itself

Bug reports about `agit-cli`, `agit-runner`, `agit-server`, or `agit-core` belong in classic GitHub issues — *do not* label them `agit:feature`. A human is in the better position to scope a bugfix.
