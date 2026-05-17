//! PR-label orchestrator: re-run the original developer agent on a PR
//! that the reviewer asked for changes on.
//!
//! Rust port of `scripts/agit-retry`. The original agent is identified by
//! matching the PR head branch against every agent's `output.branch_prefix`
//! in `agents.yaml` — the script's hard-coded case statement is gone.

use std::path::Path;

use agit_core::config::AgitConfig;
use agit_core::policy::PolicyChecker;
use anyhow::{anyhow, Context, Result};

use crate::agent;
use crate::gh;
use crate::git;
use crate::provider::{self, ProviderInvocation};
use crate::run;

const TRIGGER_LABEL: &str = "agit:retry";

pub fn handle_retry(directory: &Path, config: &AgitConfig, pr_number: u64) -> Result<()> {
    let pr = gh::pr_view(directory, pr_number)?;
    if pr.state != "OPEN" {
        eprintln!("agit-runner: PR #{pr_number} is {} — nothing to retry.", pr.state);
        return Ok(());
    }
    if !pr.labels.iter().any(|l| l.name == TRIGGER_LABEL) {
        eprintln!("agit-runner: PR #{pr_number} is missing {TRIGGER_LABEL} — skipping.");
        return Ok(());
    }

    // Consume the trigger label up-front so we never re-fire mid-flight.
    eprintln!("agit-runner: consuming {TRIGGER_LABEL} on PR #{pr_number}…");
    gh::pr_remove_label(directory, pr_number, TRIGGER_LABEL)?;

    let (name, agent) = agent::match_branch_prefix(config, &pr.head_ref_name)?;
    let provider_cfg = config
        .providers
        .get(&agent.provider)
        .ok_or_else(|| anyhow!("agent `{name}` references unknown provider"))?;

    let review_feedback = extract_review_feedback(&pr);

    // Try to recover the original issue context so the retry agent sees the
    // original intent, not just the reviewer's notes. Best-effort: the run
    // orchestrator writes "Triggered by: Issue #N" in the PR body, so we
    // scan for it.
    let issue = extract_issue_number(&pr.body)
        .and_then(|n| gh::issue_view(directory, n).ok());

    // Clone the PR head into an isolated workspace.
    let workspace = tempfile::Builder::new()
        .prefix("agit-retry-")
        .tempdir()
        .context("creating retry workspace")?;
    let clone_dir = workspace.path().join("repo");

    let repo = gh::repo_name_with_owner(directory)?;
    eprintln!(
        "agit-runner: cloning {repo} @ {} into {}…",
        pr.head_ref_name,
        clone_dir.display()
    );
    gh::clone_shallow(directory, &repo, &clone_dir)?;
    git::fetch_refs(&clone_dir, &[&pr.head_ref_name, &pr.base_ref_name])?;
    git::checkout(&clone_dir, &pr.head_ref_name)?;
    git::set_user(
        &clone_dir,
        "agit-runner[bot]",
        "agit-runner@users.noreply.github.com",
    )?;

    let prompt_text = build_retry_prompt(
        &clone_dir,
        agent,
        &pr,
        issue.as_ref(),
        &review_feedback,
    )?;

    let allowed_tools = agent::claude_allowed_tools(agent);
    eprintln!(
        "agit-runner: invoking provider for agent `{name}` (retry on PR #{pr_number})…"
    );
    provider::invoke(
        provider_cfg,
        &ProviderInvocation {
            prompt: &prompt_text,
            allowed_tools: &allowed_tools,
            working_dir: &clone_dir,
        },
    )?;

    let changed = git::diff_changed_paths(&clone_dir)?;
    if changed.is_empty() {
        eprintln!(
            "agit-runner: no changes produced — re-adding agit:review so the reviewer can re-evaluate."
        );
        gh::pr_add_label(directory, pr_number, "agit:review")?;
        return Ok(());
    }

    let checker = PolicyChecker::from_write_globs(agent.permissions.write.iter())?;
    let violations = checker.check(changed.iter().map(|s| s.as_str()));
    if !violations.is_empty() {
        let mut msg = String::from("PolicyViolation: the following changes are not permitted:\n");
        for v in &violations {
            msg.push_str(&format!("  - {}  ({})\n", v.path, v.reason.as_str()));
        }
        msg.push_str(&format!(
            "\nAllowed write globs: {:?}",
            agent.permissions.write
        ));
        return Err(anyhow!(msg));
    }
    eprintln!(
        "agit-runner: policy OK — {} changed file(s) within agent globs",
        changed.len()
    );

    run::run_allowed_commands(&clone_dir, &agent.permissions.commands.allow)?;

    eprintln!("agit-runner: committing & pushing follow-up to {}…", pr.head_ref_name);
    git::add_all(&clone_dir)?;
    git::commit(
        &clone_dir,
        &format!("agit({name}): retry on PR #{pr_number} — address reviewer feedback"),
    )?;
    git::push(&clone_dir, &pr.head_ref_name)?;

    eprintln!("agit-runner: re-adding agit:review on PR #{pr_number}");
    gh::pr_add_label(directory, pr_number, "agit:review")?;
    gh::pr_comment_text(
        directory,
        pr_number,
        &format!(
            "Agit `{name}` pushed a follow-up addressing the previous review. Handing back to `reviewer` via label `agit:review`."
        ),
    )?;

    Ok(())
}

/// Pick the body of the most recent `CHANGES_REQUESTED` review, falling
/// back to the body of the latest review of any kind. Empty string if
/// no usable review body is present — the caller substitutes a notice.
fn extract_review_feedback(pr: &gh::Pr) -> String {
    let last_changes_requested = pr
        .reviews
        .iter()
        .rev()
        .find(|r| r.state == "CHANGES_REQUESTED");
    if let Some(r) = last_changes_requested {
        if !r.body.is_empty() {
            return r.body.clone();
        }
    }
    pr.reviews
        .last()
        .map(|r| r.body.clone())
        .unwrap_or_default()
}

/// Find the first `Issue #N` reference in the PR body, mirroring the
/// "Triggered by: Issue #N" line the run orchestrator writes.
fn extract_issue_number(body: &str) -> Option<u64> {
    let needle = "Issue #";
    let start = body.find(needle)?;
    let after = &body[start + needle.len()..];
    let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

fn build_retry_prompt(
    clone_dir: &Path,
    agent: &agit_core::config::AgentConfig,
    pr: &gh::Pr,
    issue: Option<&gh::Issue>,
    review_feedback: &str,
) -> Result<String> {
    let prompt_rel = agent
        .prompt
        .as_deref()
        .ok_or_else(|| anyhow!("agent has no `prompt:` set"))?;
    let prompt_path = clone_dir.join(prompt_rel);
    let base = std::fs::read_to_string(&prompt_path)
        .with_context(|| format!("reading agent prompt {}", prompt_path.display()))?;

    let mut out = base;
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str("\n---\n");
    out.push_str(&format!(
        "This is a RETRY pass on an existing PR (#{}).\n",
        pr.number
    ));
    out.push_str("The reviewer agent requested changes; address them and push fixes.\n\n");
    if let Some(issue) = issue {
        out.push_str(&format!(
            "Original issue #{}:\nTitle: {}\n\nBody:\n{}\n\n",
            issue.number, issue.title, issue.body
        ));
    }
    out.push_str(&format!("Current PR title: {}\n\n", pr.title));
    out.push_str(&format!("Current PR body:\n{}\n\n", pr.body));
    let feedback = if review_feedback.trim().is_empty() {
        "(no explicit feedback found; please re-check the PR scope)"
    } else {
        review_feedback
    };
    out.push_str(&format!(
        "Latest reviewer feedback (this is what you must address):\n{feedback}\n"
    ));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr_with_reviews(reviews: Vec<gh::Review>) -> gh::Pr {
        gh::Pr {
            number: 1,
            title: "t".into(),
            body: "b".into(),
            head_ref_name: "agit/test-writer/issue-1".into(),
            base_ref_name: "main".into(),
            labels: vec![],
            state: "OPEN".into(),
            is_draft: false,
            reviews,
        }
    }

    #[test]
    fn extract_review_feedback_picks_last_changes_requested() {
        let pr = pr_with_reviews(vec![
            gh::Review {
                state: "APPROVED".into(),
                body: "approve early".into(),
            },
            gh::Review {
                state: "CHANGES_REQUESTED".into(),
                body: "first revision".into(),
            },
            gh::Review {
                state: "COMMENTED".into(),
                body: "later comment".into(),
            },
            gh::Review {
                state: "CHANGES_REQUESTED".into(),
                body: "second revision".into(),
            },
        ]);
        assert_eq!(extract_review_feedback(&pr), "second revision");
    }

    #[test]
    fn extract_review_feedback_falls_back_to_last_review() {
        let pr = pr_with_reviews(vec![
            gh::Review {
                state: "COMMENTED".into(),
                body: "first".into(),
            },
            gh::Review {
                state: "COMMENTED".into(),
                body: "last".into(),
            },
        ]);
        assert_eq!(extract_review_feedback(&pr), "last");
    }

    #[test]
    fn extract_review_feedback_returns_empty_when_no_reviews() {
        let pr = pr_with_reviews(vec![]);
        assert_eq!(extract_review_feedback(&pr), "");
    }

    #[test]
    fn extract_issue_number_finds_triggered_by_line() {
        let body = "Some text.\nTriggered by: Issue #42\nMore text.\n";
        assert_eq!(extract_issue_number(body), Some(42));
    }

    #[test]
    fn extract_issue_number_picks_first_match() {
        let body = "Issue #7 introduced this. See also Issue #99.";
        assert_eq!(extract_issue_number(body), Some(7));
    }

    #[test]
    fn extract_issue_number_returns_none_when_absent() {
        assert_eq!(extract_issue_number("nothing here"), None);
    }
}
