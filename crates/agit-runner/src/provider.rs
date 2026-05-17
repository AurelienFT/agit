//! Provider dispatch: how the runner reaches a model.
//!
//! Each `ProviderConfig` variant in `agit-core::config` maps to a different
//! way of executing an agent turn. For now only `local_command` is wired up
//! — that's the path the orchestrators (run / review / retry) use today.
//! The API-backed variants are kept as a clear "not yet implemented" error
//! so the dispatch shape is in place when we land them.
//!
//! Trust model reminder: every provider call happens on the runner host,
//! using whatever auth the operator has there (Claude Code OAuth, API keys
//! in env vars, etc.). The server never sees the prompt or the response.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use agit_core::config::ProviderConfig;
use anyhow::{anyhow, Context, Result};

/// One agent turn against a provider.
pub struct ProviderInvocation<'a> {
    /// Full prompt sent to the provider on stdin.
    pub prompt: &'a str,
    /// Comma-separated tool allowlist passed to Claude Code via
    /// `--allowedTools`. Other providers may ignore this for now.
    pub allowed_tools: &'a str,
    /// Working directory the provider executes in. The CWD matters for
    /// `local_command` providers like `claude`, which scope file access to
    /// the directory they were launched from.
    pub working_dir: &'a Path,
}

/// Run one agent turn. Returns the provider's stdout as a string so callers
/// can parse a verdict line, save logs, etc.
pub fn invoke(provider: &ProviderConfig, call: &ProviderInvocation<'_>) -> Result<String> {
    match provider {
        ProviderConfig::LocalCommand { command, args, .. } => invoke_local_command(
            command,
            args,
            call.allowed_tools,
            call.prompt,
            call.working_dir,
        ),
        ProviderConfig::AnthropicApi { .. } => Err(anyhow!(
            "provider `anthropic_api` is not yet implemented in agit-runner. \
             Use a `local_command` provider (e.g. claude) for now."
        )),
        ProviderConfig::OpenaiApi { .. } => Err(anyhow!(
            "provider `openai_api` is not yet implemented in agit-runner. \
             Use a `local_command` provider for now."
        )),
        ProviderConfig::OpenaiCompatible { .. } => Err(anyhow!(
            "provider `openai_compatible` is not yet implemented in agit-runner. \
             Use a `local_command` provider for now."
        )),
    }
}

fn invoke_local_command(
    command: &str,
    extra_args: &[String],
    allowed_tools: &str,
    prompt: &str,
    working_dir: &Path,
) -> Result<String> {
    // We follow the shape Claude Code expects (--print + --allowedTools, prompt
    // on stdin). For non-Claude local CLIs the operator can already wire the
    // flags they need via the provider's `args:` list — those come *after*
    // ours, so any final positional argument from the agent config wins.
    let mut cmd = Command::new(command);
    cmd.arg("--print")
        .arg("--allowedTools")
        .arg(allowed_tools)
        .args(extra_args)
        .current_dir(working_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());

    let mut child = cmd
        .spawn()
        .with_context(|| format!("spawning `{command}` provider"))?;

    {
        let stdin = child
            .stdin
            .as_mut()
            .ok_or_else(|| anyhow!("provider stdin was not captured"))?;
        stdin
            .write_all(prompt.as_bytes())
            .context("writing prompt to provider stdin")?;
    }

    let output = child
        .wait_with_output()
        .with_context(|| format!("waiting on `{command}` provider"))?;

    if !output.status.success() {
        return Err(anyhow!(
            "`{command}` exited with status {}",
            output.status.code().unwrap_or(-1)
        ));
    }

    String::from_utf8(output.stdout).context("provider stdout was not valid UTF-8")
}

#[cfg(test)]
mod tests {
    use super::*;
    use agit_core::config::ProviderConfig;

    #[test]
    fn unimplemented_providers_return_a_clear_error() {
        let tmp = std::env::temp_dir();
        let call = ProviderInvocation {
            prompt: "hi",
            allowed_tools: "Read",
            working_dir: &tmp,
        };

        let anthropic = ProviderConfig::AnthropicApi {
            api_key_env: "ANTHROPIC_API_KEY".into(),
            model: "claude-opus-4-7".into(),
        };
        let err = invoke(&anthropic, &call).expect_err("must error");
        assert!(format!("{err}").contains("anthropic_api"));

        let openai = ProviderConfig::OpenaiApi {
            api_key_env: "OPENAI_API_KEY".into(),
            model: "gpt-5".into(),
        };
        let err = invoke(&openai, &call).expect_err("must error");
        assert!(format!("{err}").contains("openai_api"));

        let compat = ProviderConfig::OpenaiCompatible {
            base_url: "http://localhost:11434/v1".into(),
            model: "llama3".into(),
            api_key_env: None,
        };
        let err = invoke(&compat, &call).expect_err("must error");
        assert!(format!("{err}").contains("openai_compatible"));
    }

    // The success path for `LocalCommand` invokes the configured CLI with
    // a Claude-Code-shaped argv (`--print --allowedTools …`) and the prompt
    // on stdin. A meaningful unit test would need a fake binary that mimics
    // that contract — easier exercised by the run/review/retry orchestrators
    // against a real `claude` install than asserted here.
}
