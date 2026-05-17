# Setup — running Agit on this repo

This document is a one-time checklist to get the **Agit-on-Agit** workflow live: opening a GitHub issue with the right label tells you exactly which command to run locally, and a single command opens a PR back at you.

## Trust model (read this first)

- **No Anthropic API key is stored in GitHub.** GitHub Actions never sees one.
- **Claude Code runs on your machine**, with whatever authentication you already use (`/login` against your Pro/Max subscription, or Bedrock / Vertex / your own setup — Claude Code's choice, not Agit's).
- Anthropic's terms apply to your local Claude Code usage exactly the same as in any interactive session. Agit does not wrap or proxy that — it just hands a prompt to your `claude` binary.

GitHub Actions plays a small role: it validates `.agit/agents.yaml` on each label and posts a comment with the exact command to run locally. That's it.

## 1. Push the repo

```bash
git init -b main           # if not already initialized
git add -A
git commit -m "agit: initial commit"
gh repo create agit --public --source . --remote origin --push
# or: git remote add origin git@github.com:<you>/agit.git && git push -u origin main
```

## 2. Install local prerequisites

On the machine where you'll actually run agents (your laptop, an internal server, anywhere):

| Tool | Why |
|---|---|
| `cargo` (Rust stable) | builds `agit-cli` and runs the test suite |
| `gh` ([GitHub CLI](https://cli.github.com/)) | reads issues, opens PRs |
| `jq` | parses `gh` JSON output |
| `python3` | runs the post-flight policy check |
| `claude` ([Claude Code](https://docs.anthropic.com/claude-code)) | the actual coding agent |

Then authenticate the tools you need:

```bash
gh auth login           # GitHub
claude /login           # Claude Code (one-time, opens a browser)
```

Both keep their credentials on your machine. Neither is stored in GitHub.

## 3. Create the labels (one-time, on the repo)

```bash
gh label create "agit:test"    --color FBCA04 --description "Triggers test_writer (Rust tests)"
gh label create "agit:doc"     --color 0E8A16 --description "Triggers doc_updater (Markdown docs)"
gh label create "agit:feature" --color 1D76DB --description "Triggers feature_engineer (small features in crates/)"
```

Reference: [.github/labels.yml](.github/labels.yml).

## 4. (Optional) Allow the workflow to push branches/PRs from `agit-runner`

Even though the workflow no longer invokes a model, it still posts comments. `GITHUB_TOKEN` covers that by default. The actual PRs are opened by **you** running `gh pr create` from `scripts/agit-run`, using your own GitHub auth.

If you later evolve the workflow to open PRs server-side, enable:

- **Settings → Actions → General → Workflow permissions** → "Read and write permissions" + "Allow GitHub Actions to create and approve pull requests".

## 5. Try it

1. Open an issue using one of the three templates (`Test request`, `Doc update`, or `Feature request`). The template applies the right `agit:*` label automatically.
2. Watch **Actions → agit-runner**. The workflow runs `agit validate`, confirms the agent exists, and **comments on your issue** with the exact command:
   ```bash
   ./scripts/agit-run <issue-number>
   ```
3. On your local clone, run that command. The script:
   - Pulls the latest default branch.
   - Creates `agit/<agent>/issue-<n>`.
   - Builds the prompt from `.agit/prompts/<agent>.md` + the issue title + body.
   - Invokes `claude --print --allowedTools <…>` headlessly.
   - Runs the post-flight policy check (allowed write globs + deny-by-default lockfiles / `.env*` / `.git/**`).
   - Runs the agent's allowed commands (e.g. `cargo test`).
   - Pushes the branch and opens a PR via `gh`.
4. Review the PR like any other.

To pick an issue interactively:

```bash
./scripts/agit-run --list      # list open agit-labeled issues
./scripts/agit-run 42          # run against issue #42
./scripts/agit-run --help
```

## 6. What's wired up vs. what's still stubs

Wired and working today:

- `.agit/agents.yaml` parsed and validated by `agit-cli` (`agit validate`).
- `.github/workflows/agit-runner.yml` — validates config and tells you what to run locally. **No API key, no model invocation in CI.**
- `.github/workflows/ci.yml` — `cargo fmt --check`, `cargo clippy`, `cargo test`, `agit validate` on every push and PR.
- `scripts/agit-run` — local runner. Hands the prompt to your `claude` CLI, runs the policy check, opens a PR.

Stubs (Rust binaries that compile but don't yet do their full job):

- `agit-runner start` — would long-poll an `agit-server` for missions. Today: prints intent and exits. `scripts/agit-run` plays its role for now.
- `agit-server serve` — would expose webhooks + dashboard. Today: prints intent and exits.

Migration path: when `agit-runner` is fully implemented, `scripts/agit-run` shrinks to a `agit-runner run-once` wrapper, and the same workflow can either be retired or kept as a "comment with run instructions" trigger.

## 7. Cost & safety knobs

Each agent has `limits` in `.agit/agents.yaml`:

```yaml
limits:
  max_iterations: 10
  max_files_changed: 20
  max_cost_usd: 5.00
```

These will be enforced by the runner once it's implemented. Today, the budget is whatever your local Claude Code subscription or quota allows — same as any interactive Claude Code session.

## 8. Disabling the workflow

```bash
gh workflow disable agit-runner.yml
# re-enable with: gh workflow enable agit-runner.yml
```

`scripts/agit-run` keeps working regardless — you can just run agents locally without ever triggering the workflow.

## Troubleshooting

- **Workflow doesn't run on a label** → check the label spelling matches exactly (`agit:test`, with a colon, no space).
- **`claude --allowedTools` reports "unknown option"** → flag names have varied across Claude Code releases. Swap `--allowedTools` for `--allowed-tools` in `scripts/agit-run`, or check `claude --help`.
- **Policy check rejects everything** → run `cargo run -p agit-cli -- show <agent>` to see the agent's allowed write globs. The post-flight check uses the same patterns.
- **PR isn't created** → confirm `gh auth status` is happy and your token has `repo` scope.
- **`claude` opens a login flow instead of running** → run `claude /login` once to authenticate; the script then runs headless.
