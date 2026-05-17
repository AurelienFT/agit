//! `agit-server` — the Agit control plane.
//!
//! Owns the dashboard, the GitHub/GitLab webhook receiver, the missions queue,
//! and the run history. Can run as Agit Cloud (managed SaaS) or self-hosted
//! on customer infra.
//!
//! Crucially, the server NEVER clones customer code, never contacts model
//! providers, and never sees model credentials. That work is delegated to
//! `agit-runner` instances connected to the server.
//!
//! State today: command surface scaffold only. The HTTP listener, the webhook
//! verification, the runner-facing API, and the dashboard come next — see
//! docs/ARCHITECTURE.md for the target shape.

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "agit-server",
    version,
    about = "Agit control-plane server (self-hostable)."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Start the HTTP server.
    Serve {
        #[arg(long, default_value_t = 3000)]
        port: u16,
    },
    /// Run pending database migrations.
    /// Placeholder until `sqlx` lands in this crate.
    Migrate,
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Serve { port } => {
            eprintln!("agit-server: would listen on 0.0.0.0:{port}");
            eprintln!("agit-server: HTTP routing is not implemented yet.");
            eprintln!(
                "agit-server: when implemented, this process exposes the dashboard, the GitHub \
                 webhook receiver, and the runner-facing mission API. It never touches \
                 customer code directly."
            );
            Ok(())
        }
        Command::Migrate => {
            eprintln!("agit-server: migrations not implemented yet.");
            Ok(())
        }
    }
}
