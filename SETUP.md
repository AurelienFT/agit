# Setup — running Agit on this repo

One-time checklist to start using Agit on this repo. After this, the loop is: open an issue with a label, watch a PR come back.

## Trust model (read this first)

- **No Anthropic API key is stored in GitHub.** No secret to configure.
- **Claude Code runs on your machine**, with whatever authentication you already use (`claude /login`, Bedrock, Vertex — Claude Code's choice, not Agit's).
- Anthropic's terms apply to your local Claude Code usage exactly the same as in any interactive session.
- Agit Cloud (when it exists) and GitHub Actions never see model credentials. The daemon described below runs on **your** machine.

## 1. Push the repo (already done if you're reading this on GitHub)

```bash
gh repo create agit --public --source . --remote origin --push
```

## 2. Install local prerequisites

On the machine where the daemon will run:

| Tool | Why |
|---|---|
| `cargo` (Rust stable) | builds `agit-runner` and `agit-cli` |
| `gh` ([GitHub CLI](https://cli.github.com/)) | reads issues, opens PRs, polls labels |
| `jq` | parses `gh` JSON in `scripts/agit-run` |
| `python3` | runs the post-flight policy check |
| `claude` ([Claude Code](https://docs.anthropic.com/claude-code)) | the actual coding agent |

Authenticate the tools that need it:

```bash
gh auth login           # GitHub
claude /login           # Claude Code (browser-based, one-time)
```

Both keep their credentials on your machine.

## 3. Create the labels (one-time, on the GitHub repo)

Six labels: three for picking the developer agent on issues, three for the reviewer loop on PRs.

```bash
# Issue labels — pick which developer agent runs.
gh label create "agit:test"          --color FBCA04 --description "Triggers test_writer (Rust tests)"
gh label create "agit:doc"           --color 0E8A16 --description "Triggers doc_updater (Markdown docs)"
gh label create "agit:feature"       --color 1D76DB --description "Triggers feature_engineer (small features in crates/)"

# Issue opt-out — keep the reviewer agent off this PR (human review only).
gh label create "agit:human-review"  --color D93F0B --description "On an issue: developer agent will NOT add agit:review to the PR — human review only"

# PR labels — drive the reviewer loop. The agents add these themselves;
# the only one you'd add by hand is agit:review on an existing PR to ask
# the reviewer to look at it.
gh label create "agit:review"        --color 5319E7 --description "On a PR: triggers the reviewer agent"
gh label create "agit:retry"         --color C5DEF5 --description "On a PR: reviewer asked for changes; re-runs the original dev agent"
```

## 4. Launch the daemon

```bash
cargo run --release -p agit-runner -- watch
```

Leave it running. On the first start it:

- Validates `.agit/agents.yaml`.
- Confirms `gh` and `git` are on PATH.
- Looks for `scripts/agit-run` next to it.

Then every 30 seconds it polls GitHub for:

- **Open issues** labeled `agit:test`, `agit:doc`, or `agit:feature` → runs the matching developer agent via `scripts/agit-run`.
- **Open PRs** labeled `agit:review` → runs the `reviewer` agent via `scripts/agit-review`.
- **Open PRs** labeled `agit:retry` → re-runs the original developer agent (resolved from the branch name) via `scripts/agit-retry`.

### Developer flow (issue → PR)

For each new labeled issue, `scripts/agit-run`:

1. Creates branch `agit/<agent>/issue-<n>` on top of the default branch.
2. Builds the prompt from `.agit/prompts/<agent>.md` + issue title/body.
3. Invokes `claude --print --allowedTools <…>` headlessly. Your local Claude Code auth is used.
4. Runs the post-flight policy check (write globs + deny-by-default lockfiles / `.env*` / `.git/**`).
5. Runs the agent's allowed commands (`cargo test`, etc.).
6. Pushes the branch and opens a PR.
7. **Adds `agit:review` to the PR** so the reviewer agent picks it up automatically. The single exception: if the issue carries `agit:human-review`, the developer skips this step and the PR waits for a human reviewer.

### Reviewer flow (PR → merge or retry)

When a PR is labeled `agit:review`, `scripts/agit-review`:

1. Removes the `agit:review` label (idempotency anchor — the label IS the trigger; consuming it prevents a re-trigger mid-run).
2. Clones the PR branch in a temp workspace.
3. Invokes `claude` with read-only tools + `cargo test/check/clippy/fmt --check`.
4. Parses Claude's verdict from the last `AGIT_VERDICT: approve|changes` line of its output.
5. **On approve**: `gh pr review --approve` + `gh pr merge --squash --delete-branch`.
6. **On changes**: `gh pr review --request-changes` + adds `agit:retry` to the PR.

### Retry flow (PR → fixes → review again)

When a PR is labeled `agit:retry`, `scripts/agit-retry`:

1. Removes the `agit:retry` label.
2. Resolves the original developer agent from the branch name (`agit/<slug>/issue-N`).
3. Clones the PR branch.
4. Builds a richer prompt: agent's system prompt + original issue + PR title/body + the reviewer's latest CHANGES_REQUESTED feedback.
5. Same policy check, same allowed commands.
6. Pushes a follow-up commit to the existing branch.
7. **Re-adds `agit:review`** so the reviewer evaluates the new state.

The reviewer ↔ retry cycle can repeat indefinitely until the reviewer approves.

### Idempotency

- **Issue → developer** is anchored by the remote branch existing. If `agit/<slug>/issue-N` is on `origin`, the daemon skips. Delete that branch (or the local in-process cache, by restarting the daemon) to force a re-run.
- **PR → reviewer / retry** is anchored by **label consumption**. The script removes the label as its first action. If the script crashes before that, the daemon retries automatically on the next tick.

### Useful flags

```bash
cargo run --release -p agit-runner -- watch --interval 15        # poll every 15s
cargo run --release -p agit-runner -- watch --dry-run            # log what would run, don't execute
cargo run --release -p agit-runner -- watch -C /path/to/clone    # watch a different checkout
```

## 5. Try it

1. Open an issue on GitHub using one of the templates (`Test request`, `Doc update`, or `Feature request`). The template applies the matching `agit:*` label automatically.
2. Within ~30 seconds, the daemon picks it up and prints something like:
   ```
   agit-runner: → issue #1 [agit:feature] add foo
   agit-runner:   ✓ done
   ```
3. A PR shows up on GitHub, already labeled `agit:review`. On the next tick the daemon hands it to the reviewer:
   ```
   agit-runner: → PR #2 [agit:review] agit(feature_engineer): work on #1 (review)
   agit-runner:   ✓ done
   ```
4. If the reviewer approves, the PR is merged automatically and the branch deleted. If it requests changes, the PR gets `agit:retry`; the next tick re-runs the developer with the review feedback in the prompt, pushes a follow-up, re-adds `agit:review`. Loop continues until merged.

**To opt out of the review loop**: add `agit:human-review` to the issue (the dev agent will then skip the `agit:review` label on its PR, leaving it for you).

## 6. What's wired up vs. what's still stubs

Wired and working today:

- `.agit/agents.yaml` parsed and validated by `agit-cli`.
- `agit-runner watch` is a real polling daemon (this file's main subject).
- `scripts/agit-run` invokes Claude Code locally, runs the policy check, opens the PR.
- `.github/workflows/ci.yml` — `cargo fmt --check`, `cargo clippy`, `cargo test`, `agit validate` on every push and PR.

Still stubs:

- `agit-runner start --server <url>` — the mission-API mode. Today: prints intent and exits. `watch` covers the "no server" use case end-to-end.
- `agit-server serve` — the control-plane HTTP server. Today: prints intent and exits.

Migration path: when `agit-server` is implemented, `agit-runner watch` keeps working for users who want a fully self-hosted, no-server-needed setup. `agit-runner start` becomes the path for orgs that want a managed dashboard.

## 7. Disabling / stopping

Stop the daemon with `Ctrl-C`. To prevent it from running on a given machine, just don't launch it. There is no GitHub-side state.

## Troubleshooting

- **Daemon says `gh issue list failed`** → run `gh auth status` and check the token has `repo` scope.
- **Daemon keeps retrying the same issue** → the agent ran but failed (likely a policy violation or `claude` exited non-zero). Run `scripts/agit-run <N>` manually to see the full output, fix the issue (or the policy), then re-run.
- **`claude --allowedTools` reports "unknown option"** → flag names have varied. Swap for `--allowed-tools` in `scripts/agit-run`, or check `claude --help`.
- **Policy check rejects everything** → run `cargo run -p agit-cli -- show <agent>` to see the agent's allowed write globs. The post-flight check uses the same patterns.
- **`claude` opens a login flow instead of running** → run `claude /login` once to authenticate; the daemon then runs headless.
