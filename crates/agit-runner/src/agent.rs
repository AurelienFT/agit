//! Resolve agents from `.agit/agents.yaml` triggers and conventions.
//!
//! The mapping from a GitHub event (issue with label X, PR with label Y, PR
//! on branch `agit/<slug>/issue-N`) to a concrete [`AgentConfig`] used to
//! live in the bash scripts as hand-written case statements. It now lives
//! here, derived entirely from the YAML.

use agit_core::config::{AgentConfig, AgitConfig, TriggerConfig};
use anyhow::{anyhow, Result};

/// Result of matching an issue's labels to an agent.
pub struct IssueMatch<'a> {
    pub name: &'a str,
    pub agent: &'a AgentConfig,
    pub label: &'a str,
    /// True when the issue also carries `agit:human-review`, which opts out
    /// of the automated reviewer loop. The orchestrator uses this to decide
    /// whether to add `agit:review` to the resulting PR.
    pub opt_out_human_review: bool,
}

/// Pick the agent whose `github_issue_label` trigger matches one of the
/// labels on the issue. Returns `None` if no agent matches (the runner
/// silently skips such issues).
///
/// Labels are scanned in the order they were declared in the YAML — first
/// match wins. `agit:human-review` is never used as a trigger label; if it's
/// on the issue alongside an agit:* trigger, the trigger still fires but the
/// human-review marker is propagated.
pub fn match_issue<'a>(
    config: &'a AgitConfig,
    labels: impl IntoIterator<Item = &'a str>,
) -> Option<IssueMatch<'a>> {
    let labels: Vec<&str> = labels.into_iter().collect();
    let opt_out_human_review = labels.iter().any(|l| *l == "agit:human-review");

    for (name, agent) in &config.agents {
        if let TriggerConfig::GithubIssueLabel { label } = &agent.trigger {
            if labels.iter().any(|l| *l == label.as_str()) {
                return Some(IssueMatch {
                    name,
                    agent,
                    label,
                    opt_out_human_review,
                });
            }
        }
    }
    None
}

/// Pick the agent whose `github_pull_request_label` trigger matches a given
/// PR label. Returns an error when no agent declares that trigger — the
/// runner shouldn't be polling for a label nothing handles.
pub fn match_pr_label<'a>(
    config: &'a AgitConfig,
    label: &str,
) -> Result<(&'a str, &'a AgentConfig)> {
    for (name, agent) in &config.agents {
        if let TriggerConfig::GithubPullRequestLabel { label: trigger_label } = &agent.trigger {
            if trigger_label == label {
                return Ok((name.as_str(), agent));
            }
        }
    }
    Err(anyhow!(
        "no agent declares trigger `github_pull_request_label` for label `{label}`"
    ))
}

/// Resolve which developer agent originally produced a PR by matching the
/// PR's head branch against every agent's `output.branch_prefix`. The
/// retry orchestrator uses this to re-invoke the right agent.
pub fn match_branch_prefix<'a>(
    config: &'a AgitConfig,
    head_branch: &str,
) -> Result<(&'a str, &'a AgentConfig)> {
    for (name, agent) in &config.agents {
        if let Some(prefix) = &agent.output.branch_prefix {
            if head_branch.starts_with(prefix) {
                return Ok((name.as_str(), agent));
            }
        }
    }
    Err(anyhow!(
        "no agent owns branch `{head_branch}` (no matching `output.branch_prefix` in agents.yaml)"
    ))
}

/// Build the `--allowedTools` argument we pass to a Claude-Code-shaped
/// provider, derived from the agent's permissions:
///
///   - `Read` is always allowed.
///   - `Edit` + `Write` when the agent has any write globs (i.e. it's a
///     developer-class agent, not a reviewer).
///   - `Bash(<cmd>:*)` for each entry in `permissions.commands.allow`.
pub fn claude_allowed_tools(agent: &AgentConfig) -> String {
    let mut tools: Vec<String> = vec!["Read".into()];
    if !agent.permissions.write.is_empty() {
        tools.push("Edit".into());
        tools.push("Write".into());
    }
    for cmd in &agent.permissions.commands.allow {
        tools.push(format!("Bash({cmd}:*)"));
    }
    tools.join(",")
}

#[cfg(test)]
mod tests {
    use super::*;
    use agit_core::config::{
        AgentConfig, CommandsPolicy, OutputConfig, OutputKind, PermissionPolicy, ProviderConfig,
        TriggerConfig,
    };
    use std::collections::BTreeMap;

    fn provider() -> ProviderConfig {
        ProviderConfig::LocalCommand {
            command: "claude".into(),
            args: vec![],
            mode: None,
        }
    }

    fn agent(
        write: Vec<&str>,
        commands: Vec<&str>,
        trigger: TriggerConfig,
        branch_prefix: Option<&str>,
    ) -> AgentConfig {
        AgentConfig {
            description: "x".into(),
            provider: "p".into(),
            prompt: None,
            trigger,
            permissions: PermissionPolicy {
                read: vec!["**".into()],
                write: write.into_iter().map(String::from).collect(),
                commands: CommandsPolicy {
                    allow: commands.into_iter().map(String::from).collect(),
                },
            },
            output: OutputConfig {
                kind: OutputKind::PullRequest,
                branch_prefix: branch_prefix.map(String::from),
                require_human_review: true,
            },
            limits: None,
        }
    }

    fn config_with(agents: Vec<(&str, AgentConfig)>) -> AgitConfig {
        let mut providers = BTreeMap::new();
        providers.insert("p".to_string(), provider());
        let mut map = BTreeMap::new();
        for (name, a) in agents {
            map.insert(name.to_string(), a);
        }
        AgitConfig {
            version: "1".into(),
            project: None,
            providers,
            agents: map,
        }
    }

    #[test]
    fn match_issue_returns_first_agent_whose_label_is_present() {
        let cfg = config_with(vec![
            (
                "test_writer",
                agent(
                    vec!["**"],
                    vec![],
                    TriggerConfig::GithubIssueLabel {
                        label: "agit:test".into(),
                    },
                    None,
                ),
            ),
            (
                "doc_updater",
                agent(
                    vec!["**"],
                    vec![],
                    TriggerConfig::GithubIssueLabel {
                        label: "agit:doc".into(),
                    },
                    None,
                ),
            ),
        ]);
        let m = match_issue(&cfg, ["bug", "agit:doc"]).expect("matches");
        assert_eq!(m.name, "doc_updater");
        assert_eq!(m.label, "agit:doc");
        assert!(!m.opt_out_human_review);
    }

    #[test]
    fn match_issue_records_human_review_opt_out() {
        let cfg = config_with(vec![(
            "test_writer",
            agent(
                vec!["**"],
                vec![],
                TriggerConfig::GithubIssueLabel {
                    label: "agit:test".into(),
                },
                None,
            ),
        )]);
        let m = match_issue(&cfg, ["agit:test", "agit:human-review"]).expect("matches");
        assert!(m.opt_out_human_review);
    }

    #[test]
    fn match_issue_returns_none_when_no_label_matches() {
        let cfg = config_with(vec![(
            "test_writer",
            agent(
                vec!["**"],
                vec![],
                TriggerConfig::GithubIssueLabel {
                    label: "agit:test".into(),
                },
                None,
            ),
        )]);
        assert!(match_issue(&cfg, ["bug"]).is_none());
    }

    #[test]
    fn match_pr_label_finds_reviewer() {
        let cfg = config_with(vec![(
            "reviewer",
            agent(
                vec![],
                vec![],
                TriggerConfig::GithubPullRequestLabel {
                    label: "agit:review".into(),
                },
                None,
            ),
        )]);
        let (name, _) = match_pr_label(&cfg, "agit:review").unwrap();
        assert_eq!(name, "reviewer");
    }

    #[test]
    fn match_pr_label_errors_for_unknown_label() {
        let cfg = config_with(vec![]);
        assert!(match_pr_label(&cfg, "agit:review").is_err());
    }

    #[test]
    fn match_branch_prefix_resolves_developer_agent() {
        let cfg = config_with(vec![
            (
                "test_writer",
                agent(
                    vec!["**"],
                    vec![],
                    TriggerConfig::GithubIssueLabel {
                        label: "agit:test".into(),
                    },
                    Some("agit/test-writer/"),
                ),
            ),
            (
                "feature_engineer",
                agent(
                    vec!["**"],
                    vec![],
                    TriggerConfig::GithubIssueLabel {
                        label: "agit:feature".into(),
                    },
                    Some("agit/feature/"),
                ),
            ),
        ]);
        let (name, _) = match_branch_prefix(&cfg, "agit/feature/issue-12").unwrap();
        assert_eq!(name, "feature_engineer");
    }

    #[test]
    fn claude_allowed_tools_includes_edit_write_for_developers() {
        let a = agent(
            vec!["crates/**/src/**"],
            vec!["cargo test", "cargo check"],
            TriggerConfig::Manual,
            None,
        );
        let tools = claude_allowed_tools(&a);
        assert!(tools.contains("Read"));
        assert!(tools.contains("Edit"));
        assert!(tools.contains("Write"));
        assert!(tools.contains("Bash(cargo test:*)"));
        assert!(tools.contains("Bash(cargo check:*)"));
    }

    #[test]
    fn claude_allowed_tools_omits_edit_write_for_reviewer() {
        let a = agent(vec![], vec!["cargo test"], TriggerConfig::Manual, None);
        let tools = claude_allowed_tools(&a);
        assert!(tools.contains("Read"));
        assert!(!tools.contains("Edit"));
        assert!(!tools.contains("Write"));
        assert!(tools.contains("Bash(cargo test:*)"));
    }
}
