//! Issue-label orchestrator: turn `agit:<kind>` labels into PRs.
//!
//! This is the Rust port of `scripts/agit-run`. The mapping from label to
//! agent now comes from `.agit/agents.yaml` (via `agent::match_issue`), the
//! policy check is `agit_core::policy`, and the model invocation goes
//! through `provider::invoke`. No external scripts involved.

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use agit_core::config::{AgentConfig, AgitConfig};
use agit_core::policy::PolicyChecker;
use anyhow::{anyhow, Context, Result};

use crate::agent::{self, IssueMatch};
use crate::gh;
use crate::git;
use crate::provider::{self, ProviderInvocation};

/// Entry point called from `watch()`. Returns Ok(()) on success or any
/// orchestrator failure that should be logged but not crash the watcher.
pub fn handle_issue(directory: &Path, config: &AgitConfig, issue_number: u64) -> Result<()> {
    let issue = gh::issue_view(directory, issue_number)?;
    let labels: Vec<&str> = issue.labels.iter().map(|l| l.name.as_str()).collect();

    let Some(m) = agent::match_issue(config, labels.iter().copied()) else {
        return Err(anyhow!(
            "issue #{issue_number} has no agit:* label that maps to an agent"
        ));
    };

    let provider_cfg = config
        .providers
        .get(&m.agent.provider)
        .ok_or_else(|| anyhow!("agent `{}` references unknown provider", m.name))?;
    let branch_prefix = m
        .agent
        .output
        .branch_prefix
        .as_deref()
        .ok_or_else(|| anyhow!("agent `{}` has no `output.branch_prefix`", m.name))?;

    let default_branch = gh::default_branch(directory)?;
    let branch = format!("{branch_prefix}issue-{issue_number}");

    if !git::working_tree_is_clean(directory)? {
        return Err(anyhow!(
            "working tree is not clean — commit/stash before running"
        ));
    }

    eprintln!("agit-runner: checking out {default_branch} and pulling…");
    git::fetch(directory)?;
    git::checkout(directory, &default_branch)?;
    git::pull_ff_only(directory)?;
    git::checkout_new_branch(directory, &branch)?;

    // From here on, any failure should reset the working tree so the next
    // poll tick can run cleanly. Guard runs on Drop unless we disarm it.
    let cleanup = ResetGuard::armed(directory, &default_branch);

    let result = invoke_and_open_pr(directory, config, &m, &issue, provider_cfg, &branch, &default_branch);

    if result.is_ok() {
        cleanup.disarm();
    }
    result
}

#[allow(clippy::too_many_arguments)]
fn invoke_and_open_pr(
    directory: &Path,
    _config: &AgitConfig,
    m: &IssueMatch<'_>,
    issue: &gh::Issue,
    provider_cfg: &agit_core::config::ProviderConfig,
    branch: &str,
    default_branch: &str,
) -> Result<()> {
    let prompt_path = m
        .agent
        .prompt
        .as_deref()
        .map(|p| directory.join(p))
        .ok_or_else(|| anyhow!("agent `{}` has no `prompt:` set", m.name))?;
    let prompt_text =
        build_prompt_with_issue(&prompt_path, &issue.title, &issue.body)?;

    let allowed_tools = agent::claude_allowed_tools(m.agent);

    eprintln!(
        "agit-runner: invoking provider for agent `{}` (label `{}`)…",
        m.name, m.label
    );
    provider::invoke(
        provider_cfg,
        &ProviderInvocation {
            prompt: &prompt_text,
            allowed_tools: &allowed_tools,
            working_dir: directory,
        },
    )?;

    // Policy check on whatever the agent produced.
    let changed = git::diff_changed_paths(directory)?;
    if changed.is_empty() {
        return Err(anyhow!(
            "the agent produced no changes — leaving the branch for inspection"
        ));
    }
    let checker = PolicyChecker::from_write_globs(m.agent.permissions.write.iter())?;
    let violations = checker.check(changed.iter().map(|s| s.as_str()));
    if !violations.is_empty() {
        let mut msg = String::from("PolicyViolation: the following changes are not permitted:\n");
        for v in &violations {
            msg.push_str(&format!("  - {}  ({})\n", v.path, v.reason.as_str()));
        }
        msg.push_str(&format!(
            "\nAllowed write globs: {:?}",
            m.agent.permissions.write
        ));
        return Err(anyhow!(msg));
    }
    eprintln!(
        "agit-runner: policy OK — {} changed file(s) within agent globs",
        changed.len()
    );

    // Run the agent's allowed commands as a sanity gate.
    run_allowed_commands(directory, &m.agent.permissions.commands.allow)?;

    // Commit, push, open PR.
    eprintln!("agit-runner: committing & pushing {branch}…");
    git::add_all(directory)?;
    git::commit(directory, &format!("agit({}): work on issue #{}", m.name, issue.number))?;
    git::push_set_upstream(directory, branch)?;

    let body_file =
        write_pr_body_file(m.name, m.agent, &provider_cfg_summary(provider_cfg), issue.number)?;
    eprintln!("agit-runner: opening PR…");
    gh::pr_create(
        directory,
        &format!("agit({}): work on #{}", m.name, issue.number),
        &body_file.path,
        default_branch,
        branch,
    )?;
    let pr = gh::pr_number_for_head(directory, branch)?;

    // Tag the PR for the reviewer agent unless the operator opted out.
    if m.opt_out_human_review {
        eprintln!("agit-runner: issue carries agit:human-review — leaving PR for human review.");
        gh::issue_comment(
            directory,
            issue.number,
            &format!(
                "Agit ran agent `{agent}`. PR #{pr} opened. The issue carried `agit:human-review`, so the reviewer agent will NOT touch this PR — please review it yourself.",
                agent = m.name
            ),
        )?;
    } else {
        eprintln!("agit-runner: adding agit:review label to PR #{pr}");
        gh::pr_add_label(directory, pr, "agit:review")?;
        gh::issue_comment(
            directory,
            issue.number,
            &format!(
                "Agit ran agent `{agent}`. PR #{pr} opened and handed off to the `reviewer` agent (label `agit:review`).",
                agent = m.name
            ),
        )?;
    }

    Ok(())
}

fn run_allowed_commands(directory: &Path, commands: &[String]) -> Result<()> {
    if commands.is_empty() {
        return Ok(());
    }
    eprintln!("agit-runner: running allowed commands…");
    for cmd in commands {
        eprintln!("  $ {cmd}");
        // We split on whitespace deliberately: the YAML schema treats command
        // strings as plain argv lines (no shell features). Operators who need
        // pipes can write a script that runs them and list that script here.
        let mut parts = cmd.split_whitespace();
        let program = parts
            .next()
            .ok_or_else(|| anyhow!("empty allowed command in agents.yaml"))?;
        let args: Vec<&str> = parts.collect();
        let status = Command::new(program)
            .args(&args)
            .current_dir(directory)
            .status()
            .with_context(|| format!("running allowed command `{cmd}`"))?;
        if !status.success() {
            return Err(anyhow!(
                "allowed command `{cmd}` exited with status {}",
                status.code().unwrap_or(-1)
            ));
        }
    }
    Ok(())
}

fn provider_cfg_summary(provider_cfg: &agit_core::config::ProviderConfig) -> String {
    format!(
        "{} ({})",
        provider_cfg.summary(),
        provider_cfg.kind_label()
    )
}

fn build_prompt_with_issue(prompt_path: &Path, title: &str, body: &str) -> Result<String> {
    let base = std::fs::read_to_string(prompt_path)
        .with_context(|| format!("reading prompt file {}", prompt_path.display()))?;
    let mut out = base;
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str("\n---\nIssue context:\n\n");
    out.push_str(&format!("Title: {title}\n\nBody:\n{body}\n"));
    Ok(out)
}

struct TempBodyFile {
    path: PathBuf,
}

impl Drop for TempBodyFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn write_pr_body_file(
    agent_name: &str,
    _agent: &AgentConfig,
    provider_summary: &str,
    issue_number: u64,
) -> Result<TempBodyFile> {
    let path = std::env::temp_dir().join(format!(
        "agit-pr-body-{}-{}.md",
        std::process::id(),
        issue_number
    ));
    let mut f = File::create(&path)
        .with_context(|| format!("opening PR body file {}", path.display()))?;
    let hostname = hostname_lossy();
    write!(
        f,
        concat!(
            "## Agit run\n\n",
            "Agent: `{agent}`\n",
            "Provider: {provider} — ran on `{host}`\n",
            "Triggered by: Issue #{issue}\n",
            "Policy: Passed (write globs + deny-by-default)\n\n",
            "---\n\n",
            "This PR was generated by a **local** Agit run.\n",
            "No model credential transited through Agit Cloud or GitHub Actions; the\n",
            "provider was invoked on the runner host using whatever authentication\n",
            "that host had.\n\n",
            "A human review is required before merge.\n",
        ),
        agent = agent_name,
        provider = provider_summary,
        host = hostname,
        issue = issue_number,
    )?;
    Ok(TempBodyFile { path })
}

fn hostname_lossy() -> String {
    Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_owned())
        .unwrap_or_else(|| "unknown".into())
}

/// On Drop, resets the working tree and checks out the default branch.
/// Disarmed when the orchestrator succeeds, so successful runs don't lose
/// the freshly pushed branch reference locally.
struct ResetGuard<'a> {
    directory: &'a Path,
    default_branch: &'a str,
    armed: bool,
}

impl<'a> ResetGuard<'a> {
    fn armed(directory: &'a Path, default_branch: &'a str) -> Self {
        Self {
            directory,
            default_branch,
            armed: true,
        }
    }

    fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for ResetGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            eprintln!(
                "agit-runner: run failed — resetting working tree to {}.",
                self.default_branch
            );
            git::best_effort_reset(self.directory, self.default_branch);
        }
    }
}
