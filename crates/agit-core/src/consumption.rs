//! Agent token-consumption records.
//!
//! Lives in `agit-core` because both the runner (which produces records) and the
//! server / CLI (which read them) need to agree on the schema. The store kept
//! here is an in-memory, synchronous one — durable persistence (file, SQLite,
//! Postgres) is the runner's or server's job, layered on top of these types.
//!
//! Design goals:
//!
//! * **Lossless capture** — every distinct token bucket the providers expose
//!   (prompt / completion / cache read / cache creation) gets its own field, so
//!   later aggregation never has to guess.
//! * **Filter-first** — `ConsumptionFilter` is structured so adding a new
//!   dimension (e.g. cost ceilings, repository) is just a new optional field,
//!   not a new query API.
//! * **Pure** — no I/O, no async; safe to embed in any of the three binaries.

use serde::{Deserialize, Serialize};

/// Token counts reported by a provider for a single agent execution.
///
/// All fields are `u64` and saturating arithmetic is used everywhere, so
/// totals never silently overflow at the aggregation layer.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_read_tokens: u64,
    #[serde(default)]
    pub cache_creation_tokens: u64,
}

impl TokenUsage {
    pub fn total(&self) -> u64 {
        self.input_tokens
            .saturating_add(self.output_tokens)
            .saturating_add(self.cache_read_tokens)
            .saturating_add(self.cache_creation_tokens)
    }

    /// Folds `other` into `self`. Used to build totals over a filter result.
    pub fn add(&mut self, other: &TokenUsage) {
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
        self.cache_read_tokens = self
            .cache_read_tokens
            .saturating_add(other.cache_read_tokens);
        self.cache_creation_tokens = self
            .cache_creation_tokens
            .saturating_add(other.cache_creation_tokens);
    }
}

/// What the agent was doing when it consumed those tokens. Kept as a tagged
/// enum so we can filter on the *shape* of the work (issue vs. PR vs. manual)
/// in addition to the concrete id.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TaskRef {
    GithubIssue { repo: String, number: u64 },
    GithubPullRequest { repo: String, number: u64 },
    Manual { id: String },
    Other { id: String },
}

impl TaskRef {
    pub fn kind(&self) -> TaskKind {
        match self {
            TaskRef::GithubIssue { .. } => TaskKind::GithubIssue,
            TaskRef::GithubPullRequest { .. } => TaskKind::GithubPullRequest,
            TaskRef::Manual { .. } => TaskKind::Manual,
            TaskRef::Other { .. } => TaskKind::Other,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    GithubIssue,
    GithubPullRequest,
    Manual,
    Other,
}

/// What the caller provides when recording a consumption event. The store
/// assigns the `id` on insert.
#[derive(Debug, Clone)]
pub struct ConsumptionEntry {
    /// Concrete agent name from `.agit/agents.yaml` (e.g. `"test_writer"`).
    pub agent: String,
    /// Optional logical kind/role used to group agents across configs
    /// (e.g. `"test_writer"` regardless of the local name). Defaults to None.
    pub agent_kind: Option<String>,
    pub task: TaskRef,
    /// Provider key from `.agit/agents.yaml` (e.g. `"claude_code"`).
    pub provider: String,
    /// Model identifier reported by the provider, if any.
    pub model: Option<String>,
    pub usage: TokenUsage,
    pub cost_usd: Option<f64>,
    /// Unix epoch seconds at which the run produced these tokens.
    /// Owned by the caller so tests and replay are deterministic; the store
    /// does not synthesise timestamps.
    pub recorded_at: u64,
}

/// One immutable row in the consumption store.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ConsumptionRecord {
    pub id: u64,
    pub agent: String,
    #[serde(default)]
    pub agent_kind: Option<String>,
    pub task: TaskRef,
    pub provider: String,
    #[serde(default)]
    pub model: Option<String>,
    pub usage: TokenUsage,
    #[serde(default)]
    pub cost_usd: Option<f64>,
    pub recorded_at: u64,
}

/// Query for `ConsumptionStore::query` and friends. Every field is optional
/// and AND-joined. Adding a new dimension is one more `Option`, not a new
/// query method.
#[derive(Debug, Default, Clone)]
pub struct ConsumptionFilter {
    pub agent: Option<String>,
    pub agent_kind: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub task_kind: Option<TaskKind>,
    /// Inclusive lower bound on `recorded_at` (unix seconds).
    pub since: Option<u64>,
    /// Inclusive upper bound on `recorded_at` (unix seconds).
    pub until: Option<u64>,
}

impl ConsumptionFilter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn agent(mut self, agent: impl Into<String>) -> Self {
        self.agent = Some(agent.into());
        self
    }

    pub fn agent_kind(mut self, kind: impl Into<String>) -> Self {
        self.agent_kind = Some(kind.into());
        self
    }

    pub fn provider(mut self, provider: impl Into<String>) -> Self {
        self.provider = Some(provider.into());
        self
    }

    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    pub fn task_kind(mut self, kind: TaskKind) -> Self {
        self.task_kind = Some(kind);
        self
    }

    pub fn since(mut self, ts: u64) -> Self {
        self.since = Some(ts);
        self
    }

    pub fn until(mut self, ts: u64) -> Self {
        self.until = Some(ts);
        self
    }

    pub fn matches(&self, record: &ConsumptionRecord) -> bool {
        if let Some(agent) = &self.agent {
            if record.agent != *agent {
                return false;
            }
        }
        if let Some(kind) = &self.agent_kind {
            if record.agent_kind.as_deref() != Some(kind.as_str()) {
                return false;
            }
        }
        if let Some(provider) = &self.provider {
            if record.provider != *provider {
                return false;
            }
        }
        if let Some(model) = &self.model {
            if record.model.as_deref() != Some(model.as_str()) {
                return false;
            }
        }
        if let Some(task_kind) = self.task_kind {
            if record.task.kind() != task_kind {
                return false;
            }
        }
        if let Some(since) = self.since {
            if record.recorded_at < since {
                return false;
            }
        }
        if let Some(until) = self.until {
            if record.recorded_at > until {
                return false;
            }
        }
        true
    }
}

/// In-memory, append-only store of consumption records.
///
/// The runner and server will compose this with their own persistence layer
/// (JSONL file, SQLite, …). The store itself is intentionally I/O-free so it
/// can stay in `agit-core`.
#[derive(Debug, Default)]
pub struct ConsumptionStore {
    records: Vec<ConsumptionRecord>,
    next_id: u64,
}

impl ConsumptionStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Rebuild a store from previously persisted records. The store's
    /// id-allocator advances past the highest id present so subsequent
    /// `record` calls don't collide.
    pub fn from_records(records: Vec<ConsumptionRecord>) -> Self {
        let next_id = records
            .iter()
            .map(|r| r.id)
            .max()
            .map(|m| m + 1)
            .unwrap_or(0);
        Self { records, next_id }
    }

    /// Append a new record. Returns the assigned id.
    pub fn record(&mut self, entry: ConsumptionEntry) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.records.push(ConsumptionRecord {
            id,
            agent: entry.agent,
            agent_kind: entry.agent_kind,
            task: entry.task,
            provider: entry.provider,
            model: entry.model,
            usage: entry.usage,
            cost_usd: entry.cost_usd,
            recorded_at: entry.recorded_at,
        });
        id
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    pub fn all(&self) -> &[ConsumptionRecord] {
        &self.records
    }

    pub fn get(&self, id: u64) -> Option<&ConsumptionRecord> {
        self.records.iter().find(|r| r.id == id)
    }

    /// Iterate over records matching `filter`, in insertion order.
    pub fn query<'a>(
        &'a self,
        filter: &'a ConsumptionFilter,
    ) -> impl Iterator<Item = &'a ConsumptionRecord> + 'a {
        self.records.iter().filter(move |r| filter.matches(r))
    }

    /// Sum token usage over records matching `filter`.
    pub fn totals(&self, filter: &ConsumptionFilter) -> TokenUsage {
        let mut sum = TokenUsage::default();
        for r in self.query(filter) {
            sum.add(&r.usage);
        }
        sum
    }

    /// Sum cost (USD) over records matching `filter`. Records with `cost_usd =
    /// None` contribute zero; the returned `Option` is `None` only when no
    /// record at all matched the filter, so callers can distinguish "no data"
    /// from "data, but no cost reported".
    pub fn cost_total(&self, filter: &ConsumptionFilter) -> Option<f64> {
        let mut matched = false;
        let mut sum = 0.0;
        for r in self.query(filter) {
            matched = true;
            if let Some(c) = r.cost_usd {
                sum += c;
            }
        }
        matched.then_some(sum)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(agent: &str, recorded_at: u64, usage: TokenUsage) -> ConsumptionEntry {
        ConsumptionEntry {
            agent: agent.into(),
            agent_kind: Some(agent.into()),
            task: TaskRef::GithubIssue {
                repo: "acme/repo".into(),
                number: 1,
            },
            provider: "claude_code".into(),
            model: Some("claude-opus-4-7".into()),
            usage,
            cost_usd: Some(0.12),
            recorded_at,
        }
    }

    #[test]
    fn record_assigns_monotonic_ids() {
        let mut store = ConsumptionStore::new();
        let a = store.record(entry("test_writer", 100, TokenUsage::default()));
        let b = store.record(entry("test_writer", 101, TokenUsage::default()));
        assert_eq!(a, 0);
        assert_eq!(b, 1);
        assert_eq!(store.len(), 2);
        assert!(store.get(a).is_some());
    }

    #[test]
    fn token_usage_total_and_add_are_saturating() {
        let mut a = TokenUsage {
            input_tokens: u64::MAX - 1,
            output_tokens: 5,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
        };
        let b = TokenUsage {
            input_tokens: 10,
            output_tokens: 1,
            cache_read_tokens: 7,
            cache_creation_tokens: 3,
        };
        a.add(&b);
        assert_eq!(a.input_tokens, u64::MAX);
        assert_eq!(a.output_tokens, 6);
        assert_eq!(a.cache_read_tokens, 7);
        assert_eq!(a.cache_creation_tokens, 3);
        // total saturates too
        assert_eq!(a.total(), u64::MAX);
    }

    #[test]
    fn filter_by_agent_provider_and_task_kind() {
        let mut store = ConsumptionStore::new();
        store.record(entry(
            "test_writer",
            100,
            TokenUsage {
                input_tokens: 10,
                output_tokens: 20,
                ..Default::default()
            },
        ));
        let mut feature = entry(
            "feature_engineer",
            105,
            TokenUsage {
                input_tokens: 1,
                output_tokens: 2,
                ..Default::default()
            },
        );
        feature.provider = "openai_proxy".into();
        feature.task = TaskRef::GithubPullRequest {
            repo: "acme/repo".into(),
            number: 42,
        };
        store.record(feature);

        let by_agent = store
            .query(&ConsumptionFilter::new().agent("test_writer"))
            .count();
        assert_eq!(by_agent, 1);

        let by_provider = store
            .query(&ConsumptionFilter::new().provider("openai_proxy"))
            .count();
        assert_eq!(by_provider, 1);

        let by_task = store
            .query(&ConsumptionFilter::new().task_kind(TaskKind::GithubPullRequest))
            .count();
        assert_eq!(by_task, 1);
    }

    #[test]
    fn filter_by_time_window_is_inclusive() {
        let mut store = ConsumptionStore::new();
        for ts in [100, 110, 120, 130] {
            store.record(entry("test_writer", ts, TokenUsage::default()));
        }
        let inside = store
            .query(&ConsumptionFilter::new().since(110).until(120))
            .count();
        assert_eq!(inside, 2);
        let from_only = store.query(&ConsumptionFilter::new().since(115)).count();
        assert_eq!(from_only, 2);
        let to_only = store.query(&ConsumptionFilter::new().until(110)).count();
        assert_eq!(to_only, 2);
    }

    #[test]
    fn totals_and_cost_aggregate_filtered_records() {
        let mut store = ConsumptionStore::new();
        store.record(entry(
            "test_writer",
            100,
            TokenUsage {
                input_tokens: 10,
                output_tokens: 5,
                cache_read_tokens: 3,
                cache_creation_tokens: 1,
            },
        ));
        store.record(entry(
            "test_writer",
            200,
            TokenUsage {
                input_tokens: 1,
                output_tokens: 2,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
            },
        ));
        store.record(entry(
            "doc_updater",
            300,
            TokenUsage {
                input_tokens: 999,
                output_tokens: 999,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
            },
        ));

        let totals = store.totals(&ConsumptionFilter::new().agent("test_writer"));
        assert_eq!(totals.input_tokens, 11);
        assert_eq!(totals.output_tokens, 7);
        assert_eq!(totals.cache_read_tokens, 3);
        assert_eq!(totals.cache_creation_tokens, 1);
        assert_eq!(totals.total(), 22);

        let cost = store
            .cost_total(&ConsumptionFilter::new().agent("test_writer"))
            .expect("two test_writer records exist");
        assert!((cost - 0.24).abs() < 1e-9, "got {cost}");

        // No matching records => None, so callers can distinguish empty filter
        // results from "matched but no cost".
        assert_eq!(
            store.cost_total(&ConsumptionFilter::new().agent("nobody")),
            None
        );
    }

    #[test]
    fn from_records_advances_id_allocator() {
        let mut seeded = ConsumptionStore::from_records(vec![ConsumptionRecord {
            id: 41,
            agent: "test_writer".into(),
            agent_kind: None,
            task: TaskRef::Manual { id: "x".into() },
            provider: "claude_code".into(),
            model: None,
            usage: TokenUsage::default(),
            cost_usd: None,
            recorded_at: 0,
        }]);
        let id = seeded.record(entry("test_writer", 1, TokenUsage::default()));
        assert_eq!(id, 42);
    }

    #[test]
    fn records_roundtrip_through_yaml() {
        let original = ConsumptionRecord {
            id: 7,
            agent: "test_writer".into(),
            agent_kind: Some("test_writer".into()),
            task: TaskRef::GithubIssue {
                repo: "acme/repo".into(),
                number: 12,
            },
            provider: "claude_code".into(),
            model: Some("claude-opus-4-7".into()),
            usage: TokenUsage {
                input_tokens: 100,
                output_tokens: 50,
                cache_read_tokens: 10,
                cache_creation_tokens: 5,
            },
            cost_usd: Some(0.5),
            recorded_at: 1_700_000_000,
        };
        let yaml = serde_yml::to_string(&original).expect("serialize");
        let back: ConsumptionRecord = serde_yml::from_str(&yaml).expect("deserialize");
        assert_eq!(back.id, original.id);
        assert_eq!(back.agent, original.agent);
        assert_eq!(back.task, original.task);
        assert_eq!(back.usage, original.usage);
        assert_eq!(back.cost_usd, original.cost_usd);
    }
}
