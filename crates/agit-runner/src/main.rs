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
        } => watch(&directory, interval, dry_run),
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

    // Pre-flight: the config must be valid, gh must be available, scripts/agit-run must exist.
    let config_path = directory.join(".agit").join("agents.yaml");
    agit_core::config::AgitConfig::load(&config_path)
        .with_context(|| format!("invalid Agit config at {}", config_path.display()))?;

    require_cli("gh")?;
    require_cli("git")?;

    let script = directory.join("scripts").join("agit-run");
    if !dry_run && !script.exists() {
        return Err(anyhow!(
            "expected runner script at {} — make sure you're in the Agit repo root",
            script.display()
        ));
    }

    eprintln!(
        "agit-runner: watching {} every {}s{}",
        directory.display(),
        interval,
        if dry_run { " (dry-run)" } else { "" }
    );
    eprintln!(
        "agit-runner: reacting to labels: {}",
        LABEL_TO_SLUG
            .iter()
            .map(|(l, _)| *l)
            .collect::<Vec<_>>()
            .join(", ")
    );
    eprintln!("agit-runner: Ctrl-C to stop.");

    // In-process cache to avoid re-checking the same issue every poll.
    let mut seen: HashSet<u64> = HashSet::new();

    loop {
        match poll_once(&directory, &script, dry_run, &mut seen) {
            Ok(processed) => {
                if processed == 0 {
                    eprintln!("agit-runner: idle. sleeping {interval}s…");
                } else {
                    eprintln!(
                        "agit-runner: handled {} issue(s) this tick. sleeping {interval}s…",
                        processed
                    );
                }
            }
            Err(e) => {
                eprintln!("agit-runner: poll error: {e:#}");
            }
        }
        std::thread::sleep(std::time::Duration::from_secs(interval));
    }
}

fn poll_once(
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

        // If we already saw this issue in this process, skip the heavier remote check.
        if seen.contains(&issue.number) {
            continue;
        }

        if remote_branch_exists(directory, &branch)? {
            // Already handled in a previous tick (or by someone else); remember it.
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

        match run_agent(directory, script, issue.number) {
            Ok(()) => {
                eprintln!("agit-runner:   ✓ done");
                seen.insert(issue.number);
                processed += 1;
            }
            Err(e) => {
                eprintln!("agit-runner:   ✗ run failed: {e:#}");
                // Don't add to `seen` — we'll retry next tick. The branch
                // existence check is the durable idempotency anchor.
            }
        }
    }

    Ok(processed)
}

fn fetch_labeled_issues(directory: &Path) -> Result<Vec<GhIssue>> {
    let mut cmd = Command::new("gh");
    cmd.current_dir(directory)
        .arg("issue")
        .arg("list")
        .arg("--state")
        .arg("open")
        .arg("--json")
        .arg("number,title,labels");
    for (label, _) in LABEL_TO_SLUG {
        cmd.arg("--label").arg(label);
    }
    let out = cmd.output().context("running `gh issue list`")?;
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

fn run_agent(directory: &Path, script: &Path, issue: u64) -> Result<()> {
    let status = Command::new(script)
        .current_dir(directory)
        .arg(issue.to_string())
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
