//! `agit-runner` — self-hosted runner.
//!
//! The runner is the component that actually executes agents. It lives in the
//! customer's infrastructure (developer laptop, internal server, Kubernetes
//! pod, GitHub Actions, …) and either polls GitHub directly (this v1) or, in
//! a later iteration, talks to an `agit-server` for missions.
//!
//! Trust model: the runner is the only piece that ever touches customer code,
//! model credentials, or local CLIs. Neither code nor secrets are transmitted
//! to a server beyond status updates the operator chose to send back.

mod history_api;

use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Parser)]
#[command(
    name = "agit-runner",
    version,
    about = "Self-hosted runner for Agit missions."
)]
struct Cli {
    #[command(subcommand)]
    command: CliCommand,
}

#[derive(Subcommand)]
enum CliCommand {
    /// Poll GitHub for labeled issues and run the matching agent for each.
    /// This is the daemon mode — leave it running on your machine.
    Watch {
        /// Project root (a clone of the target repo; defaults to current dir).
        #[arg(short = 'C', long = "directory", default_value = ".")]
        directory: PathBuf,
        /// Poll interval in seconds.
        #[arg(long, default_value_t = 30)]
        interval: u64,
        /// Don't actually run agents — just print what would be done.
        #[arg(long)]
        dry_run: bool,
        /// Address (`host:port`) to expose the read-only history API on.
        /// When unset, the API is not started.
        #[arg(long)]
        history_addr: Option<String>,
        /// Path to a JSON file with history to seed the API on startup.
        /// Optional — when unset (or missing), the API serves an empty store.
        #[arg(long)]
        history_file: Option<PathBuf>,
    },
    /// Serve the read-only history API only — no GitHub polling, no agent
    /// execution. Useful for inspecting historic runs after the fact, or
    /// during development.
    ServeHistory {
        /// Address (`host:port`) to listen on.
        #[arg(long, default_value = "0.0.0.0:8787")]
        addr: String,
        /// Path to a JSON file with history to seed the store from.
        /// Optional — when unset (or missing), starts empty.
        #[arg(long)]
        history_file: Option<PathBuf>,
    },
    /// Connect to an Agit server and consume missions (placeholder).
    Start {
        #[arg(long)]
        server: String,
        #[arg(long, env = "AGIT_RUNNER_TOKEN")]
        token: String,
        #[arg(long, default_value = "./workspaces")]
        workspaces: PathBuf,
    },
    /// Diagnostic: list providers declared in the local `.agit/agents.yaml`.
    Check {
        #[arg(short = 'C', long = "directory", default_value = ".")]
        directory: PathBuf,
    },
}

fn main() -> Result<()> {
    match Cli::parse().command {
        CliCommand::Watch {
            directory,
            interval,
            dry_run,
            history_addr,
            history_file,
        } => {
            // Start the history API alongside the poll loop when an address
            // is configured. The thread runs for the process lifetime — we
            // deliberately don't join it; `watch` is a forever loop.
            if let Some(addr) = history_addr.as_deref() {
                let store = history_api::load_history(history_file.as_deref())?;
                let _handle = history_api::spawn(store, addr)?;
            }
            watch(&directory, interval, dry_run)
        }
        CliCommand::ServeHistory { addr, history_file } => {
            let store = history_api::load_history(history_file.as_deref())?;
            history_api::serve(store, &addr)
        }
        CliCommand::Start {
            server,
            token,
            workspaces,
        } => {
            let _ = token;
            eprintln!("agit-runner: would connect to {server}");
            eprintln!("agit-runner: workspaces at {}", workspaces.display());
            eprintln!(
                "agit-runner: mission-API mode is not implemented yet — use `watch` for now."
            );
            Ok(())
        }
        CliCommand::Check { directory } => check(&directory),
    }
}

// ─── check ────────────────────────────────────────────────────────────────────

fn check(directory: &Path) -> Result<()> {
    let config_path = directory.join(".agit").join("agents.yaml");
    let config = agit_core::config::AgitConfig::load(&config_path)?;

    println!("Config:    {}", config_path.display());
    println!("Providers: {}", config.providers.len());
    for (name, provider) in &config.providers {
        let availability = probe_provider_availability(provider);
        println!(
            "  - {name:20} [{:<18}] {availability}",
            provider.kind_label()
        );
    }
    println!("Agents:    {}", config.agents.len());
    Ok(())
}

fn probe_provider_availability(provider: &agit_core::config::ProviderConfig) -> &'static str {
    use agit_core::config::ProviderConfig::*;
    match provider {
        LocalCommand { .. } => "declared (PATH check not implemented yet)",
        AnthropicApi { .. } => "declared (env-var check not implemented yet)",
        OpenaiApi { .. } => "declared (env-var check not implemented yet)",
        OpenaiCompatible { .. } => "declared (endpoint check not implemented yet)",
    }
}

// ─── watch ────────────────────────────────────────────────────────────────────

/// Which Agit labels we react to, and how each maps to a branch-name slug.
const LABEL_TO_SLUG: &[(&str, &str)] = &[
    ("agit:test", "test-writer"),
    ("agit:doc", "doc-updater"),
    ("agit:feature", "feature"),
];

#[derive(Debug, serde::Deserialize)]
struct GhLabel {
    name: String,
}

#[derive(Debug, serde::Deserialize)]
struct GhIssue {
    number: u64,
    title: String,
    labels: Vec<GhLabel>,
}

fn watch(directory: &Path, interval: u64, dry_run: bool) -> Result<()> {
    let directory = directory
        .canonicalize()
        .with_context(|| format!("could not resolve {}", directory.display()))?;

    // Pre-flight: the config must be valid, gh must be available, all three scripts must exist.
    let config_path = directory.join(".agit").join("agents.yaml");
    agit_core::config::AgitConfig::load(&config_path)
        .with_context(|| format!("invalid Agit config at {}", config_path.display()))?;

    require_cli("gh")?;
    require_cli("git")?;

    let run_script = directory.join("scripts").join("agit-run");
    let review_script = directory.join("scripts").join("agit-review");
    let retry_script = directory.join("scripts").join("agit-retry");
    if !dry_run {
        for s in [&run_script, &review_script, &retry_script] {
            if !s.exists() {
                return Err(anyhow!(
                    "expected runner script at {} — make sure you're in the Agit repo root",
                    s.display()
                ));
            }
        }
    }

    eprintln!(
        "agit-runner: watching {} every {}s{}",
        directory.display(),
        interval,
        if dry_run { " (dry-run)" } else { "" }
    );
    eprintln!(
        "agit-runner: issue labels: {}  |  PR labels: agit:review, agit:retry",
        LABEL_TO_SLUG
            .iter()
            .map(|(l, _)| *l)
            .collect::<Vec<_>>()
            .join(", "),
    );
    eprintln!("agit-runner: Ctrl-C to stop.");

    // In-process cache for issue runs (branch existence is the durable anchor).
    // PR runs don't need it: the label is consumed by the script on entry.
    let mut seen_issues: HashSet<u64> = HashSet::new();

    loop {
        let mut processed = 0usize;
        match poll_issues(&directory, &run_script, dry_run, &mut seen_issues) {
            Ok(n) => processed += n,
            Err(e) => eprintln!("agit-runner: issue poll error: {e:#}"),
        }
        match poll_pr_label(&directory, &review_script, "agit:review", "review", dry_run) {
            Ok(n) => processed += n,
            Err(e) => eprintln!("agit-runner: review poll error: {e:#}"),
        }
        match poll_pr_label(&directory, &retry_script, "agit:retry", "retry", dry_run) {
            Ok(n) => processed += n,
            Err(e) => eprintln!("agit-runner: retry poll error: {e:#}"),
        }

        if processed == 0 {
            eprintln!("agit-runner: idle. sleeping {interval}s…");
        } else {
            eprintln!("agit-runner: handled {processed} item(s) this tick. sleeping {interval}s…");
        }
        std::thread::sleep(std::time::Duration::from_secs(interval));
    }
}

fn poll_issues(
    directory: &Path,
    script: &Path,
    dry_run: bool,
    seen: &mut HashSet<u64>,
) -> Result<usize> {
    let issues = fetch_labeled_issues(directory)?;
    let mut processed = 0usize;

    for issue in issues {
        let Some((label, slug)) = pick_agit_label(&issue) else {
            continue;
        };

        let branch = format!("agit/{slug}/issue-{}", issue.number);

        if seen.contains(&issue.number) {
            continue;
        }

        if remote_branch_exists(directory, &branch)? {
            seen.insert(issue.number);
            continue;
        }

        eprintln!(
            "agit-runner: → issue #{} [{}] {}",
            issue.number, label, issue.title
        );

        if dry_run {
            eprintln!(
                "agit-runner:   (dry-run) would run: {} {}",
                script.display(),
                issue.number
            );
            seen.insert(issue.number);
            processed += 1;
            continue;
        }

        match run_script_with_number(directory, script, issue.number) {
            Ok(()) => {
                eprintln!("agit-runner:   ✓ done");
                seen.insert(issue.number);
                processed += 1;
            }
            Err(e) => {
                eprintln!("agit-runner:   ✗ run failed: {e:#}");
            }
        }
    }

    Ok(processed)
}

/// Poll open PRs carrying a given Agit label and dispatch to a script.
/// No in-process cache: the script consumes (removes) the label on entry, so
/// the next poll won't see it.
fn poll_pr_label(
    directory: &Path,
    script: &Path,
    label: &str,
    kind: &str,
    dry_run: bool,
) -> Result<usize> {
    let prs = fetch_prs_with_label(directory, label)?;
    let mut processed = 0usize;

    for pr in prs {
        eprintln!(
            "agit-runner: → PR #{} [{}] {} ({kind})",
            pr.number, label, pr.title
        );

        if dry_run {
            eprintln!(
                "agit-runner:   (dry-run) would run: {} {}",
                script.display(),
                pr.number
            );
            processed += 1;
            continue;
        }

        match run_script_with_number(directory, script, pr.number) {
            Ok(()) => {
                eprintln!("agit-runner:   ✓ done");
                processed += 1;
            }
            Err(e) => {
                eprintln!("agit-runner:   ✗ run failed: {e:#}");
            }
        }
    }

    Ok(processed)
}

fn fetch_labeled_issues(directory: &Path) -> Result<Vec<GhIssue>> {
    // NOTE: `gh issue list --label A --label B` is AND-joined, not OR.
    // We want OR semantics (any agit:* label), so we fetch open issues and
    // filter client-side via `pick_agit_label`. --limit 200 is plenty for
    // any sane backlog; bump when needed.
    let out = Command::new("gh")
        .current_dir(directory)
        .args([
            "issue",
            "list",
            "--state",
            "open",
            "--limit",
            "200",
            "--json",
            "number,title,labels",
        ])
        .output()
        .context("running `gh issue list`")?;
    if !out.status.success() {
        return Err(anyhow!(
            "gh issue list failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let issues: Vec<GhIssue> =
        serde_json::from_slice(&out.stdout).context("parsing gh JSON output")?;
    Ok(issues)
}

/// Fetch open PRs carrying a specific label. Single label here is safe to use
/// directly — `gh pr list --label X` doesn't have the OR/AND ambiguity since
/// we're filtering on exactly one label.
fn fetch_prs_with_label(directory: &Path, label: &str) -> Result<Vec<GhIssue>> {
    let out = Command::new("gh")
        .current_dir(directory)
        .args([
            "pr",
            "list",
            "--state",
            "open",
            "--limit",
            "200",
            "--label",
            label,
            "--json",
            "number,title,labels",
        ])
        .output()
        .context("running `gh pr list`")?;
    if !out.status.success() {
        return Err(anyhow!(
            "gh pr list failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let prs: Vec<GhIssue> =
        serde_json::from_slice(&out.stdout).context("parsing gh JSON output")?;
    Ok(prs)
}

fn pick_agit_label(issue: &GhIssue) -> Option<(&'static str, &'static str)> {
    for label in &issue.labels {
        for (l, slug) in LABEL_TO_SLUG {
            if label.name == *l {
                return Some((l, slug));
            }
        }
    }
    None
}

fn remote_branch_exists(directory: &Path, branch: &str) -> Result<bool> {
    let status = Command::new("git")
        .current_dir(directory)
        .args(["ls-remote", "--exit-code", "--heads", "origin", branch])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .context("running `git ls-remote`")?;
    Ok(status.success())
}

fn run_script_with_number(directory: &Path, script: &Path, number: u64) -> Result<()> {
    let status = Command::new(script)
        .current_dir(directory)
        .arg(number.to_string())
        .status()
        .with_context(|| format!("executing {}", script.display()))?;
    if !status.success() {
        return Err(anyhow!(
            "{} exited with status {}",
            script.display(),
            status.code().unwrap_or(-1)
        ));
    }
    Ok(())
}

fn require_cli(name: &str) -> Result<()> {
    let ok = Command::new(name)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok {
        return Err(anyhow!(
            "required CLI `{name}` is not on PATH. Install it and re-run."
        ));
    }
    Ok(())
}
