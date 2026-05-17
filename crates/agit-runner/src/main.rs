//! `agit-runner` — self-hosted runner.
//!
//! The runner is the component that actually executes agents. It lives in the
//! customer's infrastructure (developer laptop, internal server, Kubernetes
//! pod, GitHub Actions, …) and reaches a server (Agit Cloud or a self-hosted
//! `agit-server`) to receive missions.
//!
//! Trust model: the runner is the only piece that ever touches customer code,
//! model credentials, or local CLIs. Neither the code nor the secrets are
//! transmitted to the server beyond opaque status updates and structured logs
//! the operator chose to send back.
//!
//! State today: command surface scaffold only. Real polling, cloning, agent
//! invocation, policy enforcement and PR push come next.

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "agit-runner",
    version,
    about = "Self-hosted runner for Agit missions."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Connect to an Agit server and start consuming missions.
    Start {
        /// Base URL of the Agit server to poll (e.g. https://agit.your-corp.local).
        #[arg(long)]
        server: String,
        /// Bearer token issued by the server for this runner.
        #[arg(long, env = "AGIT_RUNNER_TOKEN")]
        token: String,
        /// Path to the workspace root where repos will be cloned.
        #[arg(long, default_value = "./workspaces")]
        workspaces: std::path::PathBuf,
    },
    /// Diagnostic: load the local .agit/agents.yaml and report which providers
    /// the runner *could* serve (CLI present? env var set? endpoint reachable?).
    /// Does not contact the Agit server.
    Check {
        #[arg(short = 'C', long = "directory", default_value = ".")]
        directory: std::path::PathBuf,
    },
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Start {
            server,
            token,
            workspaces,
        } => {
            // Mask the token in logs — the env-var path is the supported one.
            let _ = token;
            eprintln!("agit-runner: would connect to {server}");
            eprintln!("agit-runner: workspaces at {}", workspaces.display());
            eprintln!("agit-runner: mission loop is not implemented yet.");
            Ok(())
        }
        Command::Check { directory } => check(&directory),
    }
}

fn check(directory: &std::path::Path) -> Result<()> {
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

/// Lightweight, read-only availability hint.
/// Real probing (PATH lookup, env-var check, HTTP HEAD) lands when the runner
/// gains its dependencies; today we only inspect the config shape.
fn probe_provider_availability(provider: &agit_core::config::ProviderConfig) -> &'static str {
    use agit_core::config::ProviderConfig::*;
    match provider {
        LocalCommand { .. } => "declared (PATH check not implemented yet)",
        AnthropicApi { .. } => "declared (env-var check not implemented yet)",
        OpenaiApi { .. } => "declared (env-var check not implemented yet)",
        OpenaiCompatible { .. } => "declared (endpoint check not implemented yet)",
    }
}
