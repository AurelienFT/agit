//! PR-label orchestrator: run the `reviewer` agent on PRs.
//!
//! Rust port of `scripts/agit-review`. The script's hard-coded
//! `--allowedTools 'Read,Bash(cargo test:*),…'` is derived here from the
//! reviewer agent's own `permissions` in `.agit/agents.yaml`, the same way
//! [`crate::run`] does for developer agents.

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

use agit_core::config::{AgentConfig, AgitConfig};
use anyhow::{anyhow, Context, Result};

use crate::agent;
use crate::gh::{self, ReviewOutcome};
use crate::git;
use crate::provider::{self, ProviderInvocation};

const TRIGGER_LABEL: &str = "agit:review";

pub fn handle_review(directory: &Path, config: &AgitConfig, pr_number: u64) -> Result<()> {
    let (name, agent) = agent::match_pr_label(config, TRIGGER_LABEL)?;
    let provider_cfg = config
        .providers
        .get(&agent.provider)
        .ok_or_else(|| anyhow!("agent `{name}` references unknown provider"))?;

    let pr = gh::pr_view(directory, pr_number)?;
    if pr.state != "OPEN" {
        eprintln!("agit-runner: PR #{pr_number} is {} — skipping.", pr.state);
        return Ok(());
    }
    if pr.is_draft {
        eprintln!("agit-runner: PR #{pr_number} is a draft — skipping.");
        return Ok(());
    }
    if !pr
        .labels
        .iter()
        .any(|l| l.name == TRIGGER_LABEL)
    {
        eprintln!("agit-runner: PR #{pr_number} is missing {TRIGGER_LABEL} — skipping.");
        return Ok(());
    }

    // Consume the trigger label up-front so the next poll tick doesn't
    // re-fire while we're still working.
    eprintln!("agit-runner: consuming {TRIGGER_LABEL} on PR #{pr_number}…");
    gh::pr_remove_label(directory, pr_number, TRIGGER_LABEL)?;

    // Clone the PR into an isolated workspace. `tempfile::TempDir` cleans
    // it up on drop — we never leave detritus on disk between runs.
    let workspace = tempfile::Builder::new()
        .prefix("agit-review-")
        .tempdir()
        .context("creating review workspace")?;
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

    let prompt_text = build_review_prompt(&clone_dir, agent, &pr)?;

    let allowed_tools = agent::claude_allowed_tools(agent);
    eprintln!("agit-runner: invoking provider for reviewer agent `{name}`…");
    let output = provider::invoke(
        provider_cfg,
        &ProviderInvocation {
            prompt: &prompt_text,
            allowed_tools: &allowed_tools,
            working_dir: &clone_dir,
        },
    )?;
    // Echo to stderr for operator visibility (the script teed to stdout).
    eprint!("{output}");

    let verdict = parse_verdict(&output).ok_or_else(|| {
        anyhow!(
            "reviewer output did not include an `AGIT_VERDICT:` line — \
             leaving the PR untouched (re-add `{TRIGGER_LABEL}` to retry)"
        )
    })?;

    let body_text = strip_verdict_lines(&output);
    let body_text = if body_text.trim().is_empty() {
        "Automated review by the Agit reviewer agent (no detailed notes).\n".to_owned()
    } else {
        body_text
    };
    let body_file = write_body_file(&body_text, pr_number)?;

    apply_verdict(directory, pr_number, verdict, &body_file.path)?;
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    Approve,
    Changes,
}

/// Trailing `AGIT_VERDICT:` line wins, mirroring the script.
fn parse_verdict(output: &str) -> Option<Verdict> {
    output
        .lines()
        .rev()
        .find(|l| l.starts_with("AGIT_VERDICT:"))
        .and_then(|l| {
            let rest = l.trim_start_matches("AGIT_VERDICT:").trim();
            if rest.eq_ignore_ascii_case("approve") {
                Some(Verdict::Approve)
            } else if rest.eq_ignore_ascii_case("changes") {
                Some(Verdict::Changes)
            } else {
                None
            }
        })
}

/// Remove every `AGIT_VERDICT:` line so the body posted to GitHub stays clean.
fn strip_verdict_lines(output: &str) -> String {
    let mut out = String::with_capacity(output.len());
    for line in output.lines() {
        if line.starts_with("AGIT_VERDICT:") {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

fn apply_verdict(
    directory: &Path,
    pr_number: u64,
    verdict: Verdict,
    body_file: &Path,
) -> Result<()> {
    match verdict {
        Verdict::Approve => {
            eprintln!("agit-runner: verdict APPROVE — posting review and attempting merge.");
            match gh::pr_review_approve(directory, pr_number, body_file)? {
                ReviewOutcome::Posted => {}
                ReviewOutcome::SelfReviewRejected => {
                    eprintln!(
                        "agit-runner: cannot approve own PR; posting verdict as a comment."
                    );
                    gh::pr_comment(directory, pr_number, body_file)?;
                }
            }
            if !gh::pr_merge_squash(directory, pr_number)? {
                eprintln!(
                    "agit-runner: merge failed (branch protection? failing checks?) — leaving for human merge."
                );
                gh::pr_comment_text(
                    directory,
                    pr_number,
                    "Agit reviewer approved this PR, but the auto-merge attempt failed. Please merge manually.",
                )?;
            }
        }
        Verdict::Changes => {
            eprintln!(
                "agit-runner: verdict CHANGES — posting review and handing off to agit:retry."
            );
            match gh::pr_review_request_changes(directory, pr_number, body_file)? {
                ReviewOutcome::Posted => {}
                ReviewOutcome::SelfReviewRejected => {
                    eprintln!(
                        "agit-runner: cannot request changes on own PR; posting verdict as a comment."
                    );
                    gh::pr_comment(directory, pr_number, body_file)?;
                }
            }
            gh::pr_add_label(directory, pr_number, "agit:retry")?;
        }
    }
    Ok(())
}

fn build_review_prompt(clone_dir: &Path, agent: &AgentConfig, pr: &gh::Pr) -> Result<String> {
    let prompt_rel = agent
        .prompt
        .as_deref()
        .ok_or_else(|| anyhow!("reviewer agent has no `prompt:` set"))?;
    let prompt_path = clone_dir.join(prompt_rel);
    let base = std::fs::read_to_string(&prompt_path)
        .with_context(|| format!("reading reviewer prompt {}", prompt_path.display()))?;

    let base_ref = format!("origin/{}", pr.base_ref_name);
    let diff_stat = git::diff_stat_vs(clone_dir, &base_ref)?;
    let full_diff = git::diff_vs(clone_dir, &base_ref)?;

    let mut out = base;
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str("\n---\n");
    out.push_str(&format!("PR #{}: {}\n\n", pr.number, pr.title));
    out.push_str(&format!("PR body:\n{}\n\n", pr.body));
    out.push_str(&format!("Diff stat:\n{diff_stat}\n\n"));
    out.push_str(&format!("Full diff:\n{full_diff}\n"));
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

fn write_body_file(body: &str, pr_number: u64) -> Result<TempBodyFile> {
    let path = std::env::temp_dir().join(format!(
        "agit-review-body-{}-{pr_number}.md",
        std::process::id()
    ));
    let mut f = File::create(&path)
        .with_context(|| format!("opening review body file {}", path.display()))?;
    f.write_all(body.as_bytes())?;
    Ok(TempBodyFile { path })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_verdict_picks_last_approve() {
        let out = "AGIT_VERDICT: changes\nsome chatter\nAGIT_VERDICT: approve\n";
        assert_eq!(parse_verdict(out), Some(Verdict::Approve));
    }

    #[test]
    fn parse_verdict_picks_last_changes() {
        let out = "AGIT_VERDICT: approve\nbut wait...\nAGIT_VERDICT: changes\n";
        assert_eq!(parse_verdict(out), Some(Verdict::Changes));
    }

    #[test]
    fn parse_verdict_is_case_insensitive() {
        assert_eq!(parse_verdict("AGIT_VERDICT: APPROVE"), Some(Verdict::Approve));
        assert_eq!(parse_verdict("AGIT_VERDICT: Changes"), Some(Verdict::Changes));
    }

    #[test]
    fn parse_verdict_returns_none_when_absent() {
        assert_eq!(parse_verdict("looks good to me\n"), None);
    }

    #[test]
    fn parse_verdict_returns_none_when_value_is_unknown() {
        assert_eq!(parse_verdict("AGIT_VERDICT: maybe\n"), None);
    }

    #[test]
    fn strip_verdict_lines_removes_all_occurrences() {
        let out = "first line\nAGIT_VERDICT: approve\nsecond line\nAGIT_VERDICT: changes\n";
        let stripped = strip_verdict_lines(out);
        assert_eq!(stripped, "first line\nsecond line\n");
    }

    #[test]
    fn strip_verdict_lines_leaves_chatter_intact() {
        let stripped = strip_verdict_lines("noisy review\nwith details\n");
        assert_eq!(stripped, "noisy review\nwith details\n");
    }
}
