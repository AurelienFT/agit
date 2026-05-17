use agit_core::config::AgitConfig;
use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(
    name = "agit",
    version,
    about = "Inspect Agit configuration. Local-only tool — does not contact Agit Server or Cloud."
)]
pub struct Cli {
    /// Project root (defaults to the current directory). Agit reads `.agit/agents.yaml` from here.
    #[arg(short = 'C', long = "directory", global = true, default_value = ".")]
    pub directory: PathBuf,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// List declared providers and agents in `.agit/agents.yaml`.
    List,
    /// Print full details of a single agent.
    Show {
        /// Agent name (key under `agents:` in the YAML).
        name: String,
    },
    /// List declared providers only.
    Providers,
    /// Validate `.agit/agents.yaml` against the schema and cross-references.
    Validate,
}

impl Cli {
    pub fn run(self) -> Result<()> {
        let config_path = self.directory.join(".agit").join("agents.yaml");
        match self.command {
            Command::List => list(&config_path),
            Command::Show { name } => show(&config_path, &name),
            Command::Providers => providers(&config_path),
            Command::Validate => validate(&config_path),
        }
    }
}

fn load(path: &Path) -> Result<AgitConfig> {
    AgitConfig::load(path).with_context(|| format!("failed to load {}", path.display()))
}

fn header(config_path: &Path, config: &AgitConfig) {
    let project = config
        .project
        .as_ref()
        .map(|p| p.name.as_str())
        .unwrap_or("(unnamed)");
    println!("Project: {project}");
    println!("Config:  {}", config_path.display());
    println!("Version: {}", config.version);
}

fn list(config_path: &Path) -> Result<()> {
    let config = load(config_path)?;
    header(config_path, &config);

    println!();
    println!("Providers:");
    if config.providers.is_empty() {
        println!("  (none declared)");
    } else {
        for (name, provider) in &config.providers {
            println!(
                "  - {name:20} [{:<18}] {}",
                provider.kind_label(),
                provider.summary()
            );
        }
    }

    println!();
    println!("Agents:");
    if config.agents.is_empty() {
        println!("  (none declared)");
        return Ok(());
    }
    for (name, agent) in &config.agents {
        println!();
        println!("  - {name}");
        println!("      description : {}", agent.description);
        println!("      provider    : {}", agent.provider);
        println!(
            "      trigger     : {} [{}]",
            agent.trigger.kind_label(),
            agent.trigger.detail()
        );
        println!(
            "      write       : {}",
            format_globs(&agent.permissions.write)
        );
        println!(
            "      commands    : {}",
            format_globs(&agent.permissions.commands.allow)
        );
        println!("      output      : {}", agent.output.kind.label());
    }
    Ok(())
}

fn providers(config_path: &Path) -> Result<()> {
    let config = load(config_path)?;
    header(config_path, &config);
    println!();
    if config.providers.is_empty() {
        println!("(no providers declared)");
        return Ok(());
    }
    for (name, provider) in &config.providers {
        println!("- {name}");
        println!("    type    : {}", provider.kind_label());
        println!("    summary : {}", provider.summary());
    }
    Ok(())
}

fn show(config_path: &Path, name: &str) -> Result<()> {
    let config = load(config_path)?;
    let agent = config
        .agents
        .get(name)
        .ok_or_else(|| anyhow!("agent '{name}' not found in {}", config_path.display()))?;
    let yaml = serde_yml::to_string(agent).context("could not re-serialize agent")?;
    println!("# agent: {name}");
    print!("{yaml}");
    // Also surface the resolved provider so the operator sees what will actually run.
    if let Some(provider) = config.providers.get(&agent.provider) {
        println!();
        println!("# resolved provider '{}':", agent.provider);
        let yaml = serde_yml::to_string(provider).context("could not re-serialize provider")?;
        print!("{yaml}");
    }
    Ok(())
}

fn validate(config_path: &Path) -> Result<()> {
    let _config = load(config_path)?;
    println!("ok: {} is valid", config_path.display());
    Ok(())
}

fn format_globs(globs: &[String]) -> String {
    if globs.is_empty() {
        "(none)".into()
    } else {
        globs.join(", ")
    }
}
