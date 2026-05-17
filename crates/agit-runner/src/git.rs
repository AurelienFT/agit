//! Thin wrappers over `git` for the orchestrators.
//!
//! Same rationale as `gh.rs`: shell out so the operator's existing config
//! (user.email, signing, credential helpers, …) is respected as-is.

// Same rationale as gh.rs: several helpers serve review/retry, which land
// in the commits that follow this one.
#![allow(dead_code)]

use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{anyhow, Context, Result};

pub fn working_tree_is_clean(directory: &Path) -> Result<bool> {
    let unstaged = Command::new("git")
        .current_dir(directory)
        .args(["diff", "--quiet"])
        .status()
        .context("running `git diff --quiet`")?;
    let staged = Command::new("git")
        .current_dir(directory)
        .args(["diff", "--cached", "--quiet"])
        .status()
        .context("running `git diff --cached --quiet`")?;
    Ok(unstaged.success() && staged.success())
}

pub fn fetch(directory: &Path) -> Result<()> {
    expect_status(directory, &["fetch", "--quiet"])
}

pub fn fetch_refs(directory: &Path, refs: &[&str]) -> Result<()> {
    let mut args = vec!["fetch", "--quiet", "origin"];
    args.extend_from_slice(refs);
    expect_status(directory, &args)
}

pub fn checkout(directory: &Path, branch: &str) -> Result<()> {
    expect_status(directory, &["checkout", "--quiet", branch])
}

pub fn checkout_new_branch(directory: &Path, branch: &str) -> Result<()> {
    expect_status(directory, &["checkout", "--quiet", "-B", branch])
}

pub fn pull_ff_only(directory: &Path) -> Result<()> {
    expect_status(directory, &["pull", "--quiet", "--ff-only"])
}

pub fn add_all(directory: &Path) -> Result<()> {
    expect_status(directory, &["add", "-A"])
}

pub fn commit(directory: &Path, message: &str) -> Result<()> {
    expect_status(directory, &["commit", "-q", "-m", message])
}

pub fn push_set_upstream(directory: &Path, branch: &str) -> Result<()> {
    expect_status(directory, &["push", "-q", "-u", "origin", branch])
}

pub fn push(directory: &Path, branch: &str) -> Result<()> {
    expect_status(directory, &["push", "-q", "origin", branch])
}

/// Paths in the current diff (unstaged + uncommitted changes from HEAD).
pub fn diff_changed_paths(directory: &Path) -> Result<Vec<String>> {
    let out = Command::new("git")
        .current_dir(directory)
        .args(["diff", "--name-only"])
        .output()
        .context("running `git diff --name-only`")?;
    if !out.status.success() {
        return Err(anyhow!(
            "git diff failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_owned())
        .collect())
}

pub fn diff_stat_vs(directory: &Path, base_ref: &str) -> Result<String> {
    let out = Command::new("git")
        .current_dir(directory)
        .args(["diff", "--stat", &format!("{base_ref}...HEAD")])
        .output()
        .context("running `git diff --stat`")?;
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

pub fn diff_vs(directory: &Path, base_ref: &str) -> Result<String> {
    let out = Command::new("git")
        .current_dir(directory)
        .args(["diff", &format!("{base_ref}...HEAD")])
        .output()
        .context("running `git diff`")?;
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

pub fn remote_branch_exists(directory: &Path, branch: &str) -> Result<bool> {
    let status = Command::new("git")
        .current_dir(directory)
        .args(["ls-remote", "--exit-code", "--heads", "origin", branch])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("running `git ls-remote`")?;
    Ok(status.success())
}

pub fn set_user(directory: &Path, name: &str, email: &str) -> Result<()> {
    expect_status(directory, &["config", "user.name", name])?;
    expect_status(directory, &["config", "user.email", email])
}

/// Best-effort cleanup after a failed run so the working tree is usable
/// for the next iteration. Never errors — failures during cleanup are
/// printed to stderr and swallowed.
pub fn best_effort_reset(directory: &Path, default_branch: &str) {
    let _ = Command::new("git")
        .current_dir(directory)
        .args(["reset", "--hard", "--quiet"])
        .status();
    let _ = Command::new("git")
        .current_dir(directory)
        .args(["clean", "-fd", "--quiet"])
        .status();
    let _ = Command::new("git")
        .current_dir(directory)
        .args(["checkout", "--quiet", default_branch])
        .status();
}

fn expect_status(directory: &Path, args: &[&str]) -> Result<()> {
    let status = Command::new("git")
        .current_dir(directory)
        .args(args)
        .status()
        .with_context(|| format!("running `git {}`", args.join(" ")))?;
    if !status.success() {
        return Err(anyhow!(
            "`git {}` exited with status {}",
            args.join(" "),
            status.code().unwrap_or(-1)
        ));
    }
    Ok(())
}
