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

A single local daemon (`agit-runner watch`) drives the whole loop; you only label things on GitHub.

```
issue agit:test/doc/feature  ──▶  dev agent (scripts/agit-run)
                                     │ creates branch + opens PR
                                     │ adds  agit:review  to the PR
                                     │ (unless issue has agit:human-review)
                                     ▼
                            PR agit:review  ──▶  reviewer (scripts/agit-review)
                                     │
                          ┌──────────┴──────────┐
                          │                     │
                       approve               changes
                          │                     │
                  gh pr review --approve   gh pr review --request-changes
                  gh pr merge --squash     + adds  agit:retry  to the PR
                          │                     │
                        merged                  ▼
                                       PR agit:retry  ──▶  original dev (scripts/agit-retry)
                                                              │ pushes follow-up
                                                              │ re-adds agit:review
                                                              └─────────── loops
```

All agents share the same trust model: `claude` runs on your machine under your own auth (no API key, no GitHub secret, no public URL). The watcher consumes the `agit:review` / `agit:retry` label as its first action so a mid-run crash auto-retries without double-firing.

To **opt out of the agentic review** on a given issue, add the `agit:human-review` label to it before triggering. The developer agent will then open the PR but not add `agit:review` — a human reviewer is expected to take over.

When the full `agit-server` is implemented, `agit-runner watch` keeps working for users who don't want a managed dashboard; `agit-runner start` becomes the path for orgs that do.

### Picking the right label

Issue-level labels:

| Label | Agent | Can write | Can run |
|---|---|---|---|
| `agit:test` | `test_writer` | `crates/*/tests/**`, `crates/*/src/**/*.rs`, `crates/*/Cargo.toml` | `cargo test`, `cargo check`, `cargo fmt --check` |
| `agit:doc` | `doc_updater` | `**/*.md`, `docs/**` | _none_ |
| `agit:feature` | `feature_engineer` | `crates/**/src/**`, `crates/**/tests/**`, `crates/**/Cargo.toml`, `Cargo.toml` | `cargo build`, `cargo test`, `cargo check`, `cargo fmt --check`, `cargo clippy` |
| `agit:human-review` | — | (opt-out marker; suppresses `agit:review` on the resulting PR) | — |

PR-level labels (you rarely add these by hand — the agents do):

| Label | Agent | Action |
|---|---|---|
| `agit:review` | `reviewer` | Reads PR diff, runs cargo checks, posts approve or request_changes, may merge. |
| `agit:retry` | _original developer_ | Pulls the latest reviewer feedback + original issue, pushes a follow-up commit on the same branch, hands back to `agit:review`. |

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
