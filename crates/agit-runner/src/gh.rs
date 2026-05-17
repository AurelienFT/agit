//! Thin wrappers over the `gh` CLI.
//!
//! We shell out to `gh` rather than calling the GitHub REST API directly:
//! it reuses whatever auth the operator already has on the runner host
//! (PAT, OAuth, Codespaces token, gh-app token, …), and matches the
//! trust-model promise — credentials never leave the runner.

use std::path::Path;
use std::process::Command;

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Issue {
    pub number: u64,
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub labels: Vec<Label>,
}

#[derive(Debug, Deserialize)]
pub struct Pr {
    pub number: u64,
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default, rename = "headRefName")]
    pub head_ref_name: String,
    #[serde(default, rename = "baseRefName")]
    pub base_ref_name: String,
    #[serde(default)]
    pub labels: Vec<Label>,
    #[serde(default)]
    pub state: String,
    #[serde(default, rename = "isDraft")]
    pub is_draft: bool,
    #[serde(default)]
    pub reviews: Vec<Review>,
}

#[derive(Debug, Deserialize)]
pub struct Label {
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct Review {
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub body: String,
}

/// Fetch a single issue. Returns the parsed JSON view; callers pick fields.
pub fn issue_view(directory: &Path, number: u64) -> Result<Issue> {
    let out = run_gh(
        directory,
        &[
            "issue",
            "view",
            &number.to_string(),
            "--json",
            "number,title,body,labels",
        ],
    )?;
    serde_json::from_slice(&out).context("parsing `gh issue view` JSON")
}

pub fn pr_view(directory: &Path, number: u64) -> Result<Pr> {
    let out = run_gh(
        directory,
        &[
            "pr",
            "view",
            &number.to_string(),
            "--json",
            "number,title,body,headRefName,baseRefName,labels,state,isDraft,reviews",
        ],
    )?;
    serde_json::from_slice(&out).context("parsing `gh pr view` JSON")
}

pub fn default_branch(directory: &Path) -> Result<String> {
    let out = run_gh(
        directory,
        &[
            "repo",
            "view",
            "--json",
            "defaultBranchRef",
            "--jq",
            ".defaultBranchRef.name",
        ],
    )?;
    Ok(String::from_utf8_lossy(&out).trim().to_owned())
}

pub fn repo_name_with_owner(directory: &Path) -> Result<String> {
    let out = run_gh(
        directory,
        &["repo", "view", "--json", "nameWithOwner", "--jq", ".nameWithOwner"],
    )?;
    Ok(String::from_utf8_lossy(&out).trim().to_owned())
}

/// `gh repo clone <repo> <dest> -- --depth=100 --no-single-branch`
pub fn clone_shallow(directory: &Path, repo: &str, dest: &Path) -> Result<()> {
    run_gh(
        directory,
        &[
            "repo",
            "clone",
            repo,
            &dest.display().to_string(),
            "--",
            "--depth=100",
            "--no-single-branch",
        ],
    )?;
    Ok(())
}

pub fn pr_create(
    directory: &Path,
    title: &str,
    body_file: &Path,
    base: &str,
    head: &str,
) -> Result<()> {
    run_gh(
        directory,
        &[
            "pr",
            "create",
            "--title",
            title,
            "--body-file",
            &body_file.display().to_string(),
            "--base",
            base,
            "--head",
            head,
        ],
    )?;
    Ok(())
}

pub fn pr_number_for_head(directory: &Path, head: &str) -> Result<u64> {
    let out = run_gh(
        directory,
        &["pr", "view", head, "--json", "number", "--jq", ".number"],
    )?;
    let s = String::from_utf8_lossy(&out);
    s.trim()
        .parse::<u64>()
        .with_context(|| format!("parsing PR number from `{}`", s.trim()))
}

pub fn pr_add_label(directory: &Path, pr: u64, label: &str) -> Result<()> {
    run_gh(
        directory,
        &["pr", "edit", &pr.to_string(), "--add-label", label],
    )?;
    Ok(())
}

pub fn pr_remove_label(directory: &Path, pr: u64, label: &str) -> Result<()> {
    run_gh(
        directory,
        &["pr", "edit", &pr.to_string(), "--remove-label", label],
    )?;
    Ok(())
}

pub fn issue_comment(directory: &Path, issue: u64, body: &str) -> Result<()> {
    run_gh(
        directory,
        &["issue", "comment", &issue.to_string(), "--body", body],
    )?;
    Ok(())
}

pub fn pr_comment(directory: &Path, pr: u64, body_file: &Path) -> Result<()> {
    run_gh(
        directory,
        &[
            "pr",
            "comment",
            &pr.to_string(),
            "--body-file",
            &body_file.display().to_string(),
        ],
    )?;
    Ok(())
}

pub fn pr_comment_text(directory: &Path, pr: u64, body: &str) -> Result<()> {
    run_gh(
        directory,
        &["pr", "comment", &pr.to_string(), "--body", body],
    )?;
    Ok(())
}

/// Outcome of an approve/request-changes attempt. GitHub forbids reviewing
/// your own PR, so callers fall back to a plain comment when that's why we
/// failed (instead of bubbling the error up).
pub enum ReviewOutcome {
    Posted,
    SelfReviewRejected,
}

pub fn pr_review_approve(directory: &Path, pr: u64, body_file: &Path) -> Result<ReviewOutcome> {
    pr_review(directory, pr, "--approve", body_file)
}

pub fn pr_review_request_changes(
    directory: &Path,
    pr: u64,
    body_file: &Path,
) -> Result<ReviewOutcome> {
    pr_review(directory, pr, "--request-changes", body_file)
}

fn pr_review(
    directory: &Path,
    pr: u64,
    verdict_flag: &str,
    body_file: &Path,
) -> Result<ReviewOutcome> {
    let out = Command::new("gh")
        .current_dir(directory)
        .args([
            "pr",
            "review",
            &pr.to_string(),
            verdict_flag,
            "--body-file",
            &body_file.display().to_string(),
        ])
        .output()
        .context("running `gh pr review`")?;
    if out.status.success() {
        return Ok(ReviewOutcome::Posted);
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    if stderr.contains("Can not approve your own pull request")
        || stderr.contains("Can not request changes on your own pull request")
    {
        return Ok(ReviewOutcome::SelfReviewRejected);
    }
    Err(anyhow!("gh pr review failed: {}", stderr.trim()))
}

/// `gh pr merge --squash --delete-branch [--auto]`. Tries `--auto` first
/// (works when branch protections require checks); on failure, falls back
/// to an immediate merge. Returns Ok(false) when even the fallback fails
/// so the caller can leave a comment for human merge.
pub fn pr_merge_squash(directory: &Path, pr: u64) -> Result<bool> {
    let auto = Command::new("gh")
        .current_dir(directory)
        .args([
            "pr",
            "merge",
            &pr.to_string(),
            "--squash",
            "--delete-branch",
            "--auto",
        ])
        .status();
    if let Ok(s) = auto {
        if s.success() {
            return Ok(true);
        }
    }
    let direct = Command::new("gh")
        .current_dir(directory)
        .args([
            "pr",
            "merge",
            &pr.to_string(),
            "--squash",
            "--delete-branch",
        ])
        .status()
        .context("running `gh pr merge`")?;
    Ok(direct.success())
}

fn run_gh(directory: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let out = Command::new("gh")
        .current_dir(directory)
        .args(args)
        .output()
        .with_context(|| format!("running `gh {}`", args.join(" ")))?;
    if !out.status.success() {
        return Err(anyhow!(
            "`gh {}` failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(out.stdout)
}
