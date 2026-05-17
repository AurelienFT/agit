//! Glob-based policy check for the runner.
//!
//! Replicates the Python `fnmatch` block that used to live inside
//! `scripts/agit-run` and `scripts/agit-retry`, but in Rust and driven
//! entirely by the per-agent `permissions.write` declared in
//! `.agit/agents.yaml`. There is no per-agent hard-coding here: the agent's
//! own write globs are the single source of truth.
//!
//! A short deny list is enforced regardless of the agent's globs. Lockfiles
//! are *not* on it: whether they can be touched is driven by the agent's
//! own write globs (e.g. `feature_engineer` lists `Cargo.lock`,
//! `test_writer` does not).

use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::{Deserialize, Serialize};

/// Paths always blocked, regardless of an agent's `permissions.write`. Anything
/// here is considered a security regression if it ever ends up in a diff.
const FORBIDDEN: &[&str] = &[
    ".env",
    ".env.*",
    "**/.env",
    "**/.env.*",
    ".git/**",
    "**/secrets/**",
];

/// A single rejected file in a diff. The dashboard renders these structurally
/// (path + reason), so the reason is an enum, not free text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyViolation {
    pub path: String,
    pub reason: ViolationReason,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViolationReason {
    /// Matched the global deny list (e.g. `.env`, anything under `.git/`).
    ForbiddenByDefault,
    /// Outside the agent's `permissions.write` globs.
    OutsideAllowedWriteGlobs,
}

impl ViolationReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            ViolationReason::ForbiddenByDefault => "forbidden by default",
            ViolationReason::OutsideAllowedWriteGlobs => "outside allowed write globs",
        }
    }
}

/// Compiled allow/deny globs for one agent. Build it once per Run, then call
/// [`PolicyChecker::check`] on the list of paths produced by `git diff`.
pub struct PolicyChecker {
    allow: GlobSet,
    deny: GlobSet,
}

impl PolicyChecker {
    /// Compile an agent's write globs into a checker. The globset semantics
    /// match what users intuitively expect from YAML — `**` is a recursive
    /// match, `*` does not cross `/`, and POSIX-style classes are off.
    pub fn from_write_globs<I, S>(globs: I) -> anyhow::Result<Self>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut allow = GlobSetBuilder::new();
        for raw in globs {
            allow.add(Glob::new(raw.as_ref())?);
        }
        let allow = allow.build()?;

        let mut deny = GlobSetBuilder::new();
        for raw in FORBIDDEN {
            deny.add(Glob::new(raw)?);
        }
        let deny = deny.build()?;

        Ok(Self { allow, deny })
    }

    /// Return a violation for each path that is either denied by default or
    /// outside the agent's allow list. Returns an empty `Vec` when the diff
    /// is fully within policy.
    pub fn check<I, S>(&self, paths: I) -> Vec<PolicyViolation>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut violations = Vec::new();
        for path in paths {
            let p = path.as_ref();
            if self.deny.is_match(p) {
                violations.push(PolicyViolation {
                    path: p.to_owned(),
                    reason: ViolationReason::ForbiddenByDefault,
                });
                continue;
            }
            if !self.allow.is_match(p) {
                violations.push(PolicyViolation {
                    path: p.to_owned(),
                    reason: ViolationReason::OutsideAllowedWriteGlobs,
                });
            }
        }
        violations
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checker(globs: &[&str]) -> PolicyChecker {
        PolicyChecker::from_write_globs(globs.iter().copied()).unwrap()
    }

    #[test]
    fn allows_paths_inside_allow_globs() {
        let c = checker(&["crates/*/tests/**", "crates/*/src/**/*.rs"]);
        assert!(c
            .check(["crates/agit-core/tests/policy.rs"])
            .is_empty());
        assert!(c.check(["crates/agit-core/src/policy.rs"]).is_empty());
    }

    #[test]
    fn rejects_paths_outside_allow_globs() {
        let c = checker(&["crates/*/tests/**"]);
        let v = c.check(["README.md"]);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].reason, ViolationReason::OutsideAllowedWriteGlobs);
        assert_eq!(v[0].path, "README.md");
    }

    #[test]
    fn deny_list_blocks_dot_env_even_when_allow_would_match() {
        // An agent could try to put `**` in its writes; the deny list still wins.
        let c = checker(&["**"]);
        let v = c.check([".env"]);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].reason, ViolationReason::ForbiddenByDefault);
    }

    #[test]
    fn deny_list_blocks_nested_dot_env() {
        let c = checker(&["**"]);
        let v = c.check(["app/.env.local"]);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].reason, ViolationReason::ForbiddenByDefault);
    }

    #[test]
    fn deny_list_blocks_git_dir() {
        let c = checker(&["**"]);
        let v = c.check([".git/config"]);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].reason, ViolationReason::ForbiddenByDefault);
    }

    #[test]
    fn deny_list_blocks_secrets_dir() {
        let c = checker(&["**"]);
        let v = c.check(["infra/secrets/api.key"]);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].reason, ViolationReason::ForbiddenByDefault);
    }

    #[test]
    fn lockfile_is_allowed_when_agent_lists_it() {
        // feature_engineer-style policy: Cargo.lock is allowed explicitly.
        let c = checker(&["Cargo.toml", "Cargo.lock", "crates/**/src/**"]);
        assert!(c.check(["Cargo.lock"]).is_empty());
    }

    #[test]
    fn lockfile_is_rejected_when_agent_does_not_list_it() {
        // test_writer-style policy: Cargo.lock is NOT listed.
        let c = checker(&["crates/*/tests/**", "crates/*/Cargo.toml"]);
        let v = c.check(["Cargo.lock"]);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].reason, ViolationReason::OutsideAllowedWriteGlobs);
    }

    #[test]
    fn empty_write_globs_reject_everything_outside_deny_list() {
        // A reviewer-style agent declares `write: []` — nothing should pass.
        let c = checker(&[]);
        let v = c.check(["README.md", "crates/agit-core/src/lib.rs"]);
        assert_eq!(v.len(), 2);
        assert!(v
            .iter()
            .all(|x| x.reason == ViolationReason::OutsideAllowedWriteGlobs));
    }

    #[test]
    fn invalid_glob_is_an_error() {
        // An unmatched `[` is a glob parse error.
        let err = PolicyChecker::from_write_globs(["src/["]).err();
        assert!(err.is_some(), "expected glob parse error");
    }

    #[test]
    fn ok_diff_returns_empty_vec() {
        let c = checker(&["**/*.md"]);
        assert!(c.check(["README.md", "docs/CONCEPTS.md"]).is_empty());
    }
}
