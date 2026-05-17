//! Schema and loader for `.agit/agents.yaml`.
//!
//! The structs here are the canonical Rust representation of an Agit project
//! configuration. They are shared by the CLI (`agit-cli`), the runner
//! (`agit-runner`) and the server (`agit-server`), so they intentionally avoid
//! clap / HTTP / runner concerns.

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Deserialize, Serialize)]
pub struct AgitConfig {
    pub version: String,
    #[serde(default)]
    pub project: Option<Project>,
    /// Top-level providers: how each agent reaches a model / coding agent.
    /// Agents reference a provider by key.
    #[serde(default)]
    pub providers: BTreeMap<String, ProviderConfig>,
    pub agents: BTreeMap<String, AgentConfig>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Project {
    pub name: String,
}

// ─── Providers ───────────────────────────────────────────────────────────────

/// How a coding agent is reached. Each variant maps to a different
/// `AgentProvider` implementation in `agit-runner`.
///
/// Self-hosted by design: the runner contacts these providers from the
/// customer's own infra, so neither the code nor the model credentials ever
/// transit through Agit Cloud.
#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProviderConfig {
    /// Run an existing CLI on the runner host (e.g. `claude`, `codex`, `aider`).
    /// The most flexible option: anything the runner can exec can be a provider.
    LocalCommand {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        /// Optional working-mode hint passed to the runner integration.
        /// Examples: "workspace", "single_file".
        #[serde(default)]
        mode: Option<String>,
    },
    /// Talk to the official Anthropic API.
    AnthropicApi {
        /// Env var (on the runner) that holds the API key. The value never
        /// leaves the runner host.
        api_key_env: String,
        model: String,
    },
    /// Talk to the official OpenAI API.
    OpenaiApi { api_key_env: String, model: String },
    /// Talk to any OpenAI-compatible endpoint (Ollama, vLLM, LM Studio,
    /// OpenRouter, internal proxies, …).
    OpenaiCompatible {
        base_url: String,
        model: String,
        #[serde(default)]
        api_key_env: Option<String>,
    },
}

impl ProviderConfig {
    pub fn kind_label(&self) -> &'static str {
        match self {
            ProviderConfig::LocalCommand { .. } => "local_command",
            ProviderConfig::AnthropicApi { .. } => "anthropic_api",
            ProviderConfig::OpenaiApi { .. } => "openai_api",
            ProviderConfig::OpenaiCompatible { .. } => "openai_compatible",
        }
    }

    pub fn summary(&self) -> String {
        match self {
            ProviderConfig::LocalCommand { command, args, .. } => {
                if args.is_empty() {
                    format!("`{command}`")
                } else {
                    format!("`{command} {}`", args.join(" "))
                }
            }
            ProviderConfig::AnthropicApi { model, .. } => format!("anthropic / {model}"),
            ProviderConfig::OpenaiApi { model, .. } => format!("openai / {model}"),
            ProviderConfig::OpenaiCompatible {
                base_url, model, ..
            } => format!("{model} @ {base_url}"),
        }
    }
}

// ─── Agents ──────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Serialize)]
pub struct AgentConfig {
    pub description: String,
    /// Key into the top-level `providers:` map. Must resolve at load time.
    pub provider: String,
    #[serde(default)]
    pub prompt: Option<String>,
    pub trigger: TriggerConfig,
    pub permissions: PermissionPolicy,
    pub output: OutputConfig,
    #[serde(default)]
    pub limits: Option<Limits>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TriggerConfig {
    GithubIssueLabel {
        label: String,
    },
    /// Fires when a label is added to a pull request. Used by reviewer-class
    /// agents and any agent that re-acts on a PR (e.g. retry).
    GithubPullRequestLabel {
        label: String,
    },
    GithubPullRequest {
        #[serde(default)]
        paths: Vec<String>,
    },
    GithubCommentCommand {
        command: String,
    },
    Manual,
}

impl TriggerConfig {
    pub fn kind_label(&self) -> &'static str {
        match self {
            TriggerConfig::GithubIssueLabel { .. } => "github_issue_label",
            TriggerConfig::GithubPullRequestLabel { .. } => "github_pull_request_label",
            TriggerConfig::GithubPullRequest { .. } => "github_pull_request",
            TriggerConfig::GithubCommentCommand { .. } => "github_comment_command",
            TriggerConfig::Manual => "manual",
        }
    }

    pub fn detail(&self) -> String {
        match self {
            TriggerConfig::GithubIssueLabel { label } => format!("label = {label}"),
            TriggerConfig::GithubPullRequestLabel { label } => format!("pr label = {label}"),
            TriggerConfig::GithubPullRequest { paths } if paths.is_empty() => "paths = any".into(),
            TriggerConfig::GithubPullRequest { paths } => {
                format!("paths = [{}]", paths.join(", "))
            }
            TriggerConfig::GithubCommentCommand { command } => format!("command = {command}"),
            TriggerConfig::Manual => "-".into(),
        }
    }
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct PermissionPolicy {
    #[serde(default)]
    pub read: Vec<String>,
    #[serde(default)]
    pub write: Vec<String>,
    #[serde(default)]
    pub commands: CommandsPolicy,
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct CommandsPolicy {
    #[serde(default)]
    pub allow: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct OutputConfig {
    #[serde(rename = "type")]
    pub kind: OutputKind,
    #[serde(default)]
    pub branch_prefix: Option<String>,
    #[serde(default)]
    pub require_human_review: bool,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputKind {
    PullRequest,
    BlockingReview,
    Comment,
    Patch,
}

impl OutputKind {
    pub fn label(&self) -> &'static str {
        match self {
            OutputKind::PullRequest => "pull_request",
            OutputKind::BlockingReview => "blocking_review",
            OutputKind::Comment => "comment",
            OutputKind::Patch => "patch",
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Limits {
    /// Hard ceiling on agentic iterations (turns) for a single Run.
    #[serde(default)]
    pub max_iterations: Option<u32>,
    #[serde(default)]
    pub max_files_changed: Option<u32>,
    #[serde(default)]
    pub max_cost_usd: Option<f64>,
}

// ─── Loader + validation ─────────────────────────────────────────────────────

impl AgitConfig {
    /// Load and parse an Agit config from disk, then cross-validate that every
    /// agent's `provider:` references an existing provider.
    pub fn load(path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("could not read {}", path.display()))?;
        let config =
            Self::from_yaml(&raw).with_context(|| format!("invalid YAML at {}", path.display()))?;
        config
            .validate_references()
            .with_context(|| format!("invalid references in {}", path.display()))?;
        Ok(config)
    }

    /// Parse an Agit config from a YAML string (no reference validation).
    pub fn from_yaml(yaml: &str) -> Result<Self> {
        serde_yml::from_str(yaml).context("could not parse Agit YAML")
    }

    /// Check that every `agent.provider` resolves to an entry in `providers`.
    pub fn validate_references(&self) -> Result<()> {
        for (name, agent) in &self.agents {
            if !self.providers.contains_key(&agent.provider) {
                return Err(anyhow!(
                    "agent '{name}' references provider '{}' which is not declared under `providers:`",
                    agent.provider
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn workspace_root() -> PathBuf {
        // CARGO_MANIFEST_DIR is crates/agit-core; the workspace root is two up.
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("workspace root")
            .to_path_buf()
    }

    #[test]
    fn parses_root_agit_config() {
        let path = workspace_root().join(".agit").join("agents.yaml");
        let config = AgitConfig::load(&path).expect("root config must parse + validate");
        assert!(config.agents.contains_key("test_writer"));
        assert!(
            !config.providers.is_empty(),
            "root config must declare at least one provider"
        );
    }

    #[test]
    fn parses_demo_project_agit_config() {
        let path = workspace_root()
            .join("demo-project")
            .join(".agit")
            .join("agents.yaml");
        let config = AgitConfig::load(&path).expect("demo-project config must parse + validate");
        assert!(!config.agents.is_empty());
        assert!(!config.providers.is_empty());
    }

    #[test]
    fn unknown_provider_reference_is_rejected() {
        let yaml = r#"
version: "1"
providers:
  claude_code:
    type: local_command
    command: claude
agents:
  test_writer:
    description: x
    provider: does_not_exist
    trigger: { type: manual }
    permissions: {}
    output: { type: pull_request }
"#;
        let config = AgitConfig::from_yaml(yaml).expect("YAML parses");
        let err = config
            .validate_references()
            .expect_err("reference validation must fail");
        let msg = format!("{err}");
        assert!(msg.contains("does_not_exist"), "got: {msg}");
    }
}
