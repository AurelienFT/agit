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

```bash
gh label create "agit:test"    --color FBCA04 --description "Triggers test_writer (Rust tests)"
gh label create "agit:doc"     --color 0E8A16 --description "Triggers doc_updater (Markdown docs)"
gh label create "agit:feature" --color 1D76DB --description "Triggers feature_engineer (small features in crates/)"
```

## 4. Launch the daemon

```bash
cargo run --release -p agit-runner -- watch
```

Leave it running. On the first start it:

- Validates `.agit/agents.yaml`.
- Confirms `gh` and `git` are on PATH.
- Looks for `scripts/agit-run` next to it.

Then it polls GitHub every 30 seconds for open issues labeled with one of:

- `agit:test` → runs the `test_writer` agent
- `agit:doc` → runs the `doc_updater` agent
- `agit:feature` → runs the `feature_engineer` agent

For each new labeled issue, the daemon calls `scripts/agit-run <issue#>` which:

1. Creates branch `agit/<agent>/issue-<n>` on top of the default branch.
2. Builds the prompt from `.agit/prompts/<agent>.md` + the issue title and body.
3. Invokes `claude --print --allowedTools <…>` headlessly. Your local Claude Code auth is used.
4. Runs the post-flight policy check (write globs + deny-by-default lockfiles / `.env*` / `.git/**`).
5. Runs the agent's allowed commands (`cargo test`, etc.).
6. Pushes the branch and opens a PR via `gh`.

Idempotency: the daemon checks whether `agit/<agent>/issue-<n>` already exists on `origin` and skips issues that have already been handled. Re-labeling an issue does not re-trigger the run; delete the branch to force a re-run.

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
3. A PR shows up on GitHub.
4. Review the PR like any other.

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
