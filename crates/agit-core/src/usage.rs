//! Token consumption records and queries.
//!
//! Every agent run produces one or more `TokenUsageRecord`s describing what
//! a model call cost — counted in tokens (and, when the provider returns it,
//! in dollars). The runner is the one that creates these records as it
//! invokes providers; the server is the one that persists them long-term.
//!
//! This module deliberately stays a pure data model:
//!
//! * `TokenUsageRecord` carries enough context (agent, task type, provider,
//!   model, timestamp) to support the filters the product roadmap calls out:
//!   *agent type, task type, timeline*.
//! * `UsageFilter` is the matching surface — a struct of optional bounds.
//!   Any future query frontend (CLI subcommand, dashboard widget, SQL view)
//!   maps onto the same fields.
//! * `TokenUsageStore` is an in-memory `Vec`-backed implementation. It is
//!   intentionally small so it works as the runner's per-process buffer
//!   today and as a reference for whatever the server persists tomorrow.
//!
//! No I/O, no async, no chrono: keep `agit-core` cheap to depend on. Time is
//! represented as a Unix epoch second (`i64`) so callers can format and store
//! it however they like.

use serde::{Deserialize, Serialize};

/// A single accounting entry: the result of one model call (or one logical
/// step) within a run. The runner emits these; the server stores them.
///
/// Fields are flat on purpose: every dimension we may want to filter on later
/// (agent, task type, provider, model, time) is a top-level field, so a
/// future SQL schema can be a direct projection.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct TokenUsageRecord {
    /// Identifier of the run this record belongs to. Free-form on purpose —
    /// the runner picks (e.g. `"agit/test-writer/issue-42"`).
    pub run_id: String,
    /// Agent key as declared in `.agit/agents.yaml` (e.g. `"test_writer"`).
    pub agent: String,
    /// Provider key as declared in `.agit/agents.yaml`
    /// (e.g. `"claude_code"`, `"anthropic_api"`).
    pub provider: String,
    /// Model name, when the provider exposes one (e.g. `"claude-opus-4-7"`).
    /// `None` for `local_command` providers that don't surface this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Short label categorizing the work — typically the trigger slug
    /// (`"test"`, `"doc"`, `"feature"`). Lets us answer "how many tokens
    /// across all `feature` runs this month?".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_type: Option<String>,
    /// Optional mission identifier when the run came from a server-issued
    /// mission. `None` for the no-server `watch` mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mission_id: Option<String>,
    /// Unix epoch seconds. The runner sets this at record creation time.
    pub recorded_at: i64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Cache-read tokens when the provider reports them (Anthropic prompt
    /// caching, OpenAI cached input, …). Zero when not applicable.
    #[serde(default)]
    pub cache_read_tokens: u64,
    /// Cache-write / cache-creation tokens. Zero when not applicable.
    #[serde(default)]
    pub cache_write_tokens: u64,
    /// Dollar cost reported by the provider, if any. `None` when unknown
    /// (e.g. local models).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
}

impl TokenUsageRecord {
    /// Total billable tokens (input + output + cache_write). Cache reads are
    /// typically discounted by providers, so they're excluded here and
    /// surfaced as a separate field.
    pub fn billable_tokens(&self) -> u64 {
        self.input_tokens
            .saturating_add(self.output_tokens)
            .saturating_add(self.cache_write_tokens)
    }

    /// True iff this record matches every constraint set on `filter`.
    /// An empty filter matches every record.
    pub fn matches(&self, filter: &UsageFilter) -> bool {
        if let Some(expected) = &filter.agent {
            if &self.agent != expected {
                return false;
            }
        }
        if let Some(expected) = &filter.provider {
            if &self.provider != expected {
                return false;
            }
        }
        if let Some(expected) = &filter.model {
            if self.model.as_ref() != Some(expected) {
                return false;
            }
        }
        if let Some(expected) = &filter.task_type {
            if self.task_type.as_ref() != Some(expected) {
                return false;
            }
        }
        if let Some(expected) = &filter.run_id {
            if &self.run_id != expected {
                return false;
            }
        }
        if let Some(since) = filter.since {
            if self.recorded_at < since {
                return false;
            }
        }
        if let Some(until) = filter.until {
            if self.recorded_at > until {
                return false;
            }
        }
        true
    }
}

/// Optional bounds for querying a `TokenUsageStore`. Every field defaults
/// to "no constraint"; combine them as needed.
///
/// Time bounds are inclusive Unix epoch seconds. A `None` bound means open.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct UsageFilter {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<i64>,
}

impl UsageFilter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn agent(mut self, agent: impl Into<String>) -> Self {
        self.agent = Some(agent.into());
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

    pub fn task_type(mut self, task_type: impl Into<String>) -> Self {
        self.task_type = Some(task_type.into());
        self
    }

    pub fn run_id(mut self, run_id: impl Into<String>) -> Self {
        self.run_id = Some(run_id.into());
        self
    }

    pub fn since(mut self, ts: i64) -> Self {
        self.since = Some(ts);
        self
    }

    pub fn until(mut self, ts: i64) -> Self {
        self.until = Some(ts);
        self
    }
}

/// Aggregate counts over a set of records.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct TokenTotals {
    pub records: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
}

impl TokenTotals {
    pub fn add(&mut self, record: &TokenUsageRecord) {
        self.records = self.records.saturating_add(1);
        self.input_tokens = self.input_tokens.saturating_add(record.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(record.output_tokens);
        self.cache_read_tokens = self
            .cache_read_tokens
            .saturating_add(record.cache_read_tokens);
        self.cache_write_tokens = self
            .cache_write_tokens
            .saturating_add(record.cache_write_tokens);
    }

    pub fn billable_tokens(&self) -> u64 {
        self.input_tokens
            .saturating_add(self.output_tokens)
            .saturating_add(self.cache_write_tokens)
    }
}

/// In-memory token-usage store.
///
/// Today this is what the runner uses to buffer records before they're
/// reported back; tomorrow a server-side store will speak the same shape on
/// top of SQLite/Postgres. The query API on this struct is meant to be the
/// canonical reference for both.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct TokenUsageStore {
    records: Vec<TokenUsageRecord>,
}

impl TokenUsageStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&mut self, record: TokenUsageRecord) {
        self.records.push(record);
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    pub fn all(&self) -> &[TokenUsageRecord] {
        &self.records
    }

    /// Borrow every record matching `filter`, in insertion order.
    pub fn query<'a>(&'a self, filter: &UsageFilter) -> Vec<&'a TokenUsageRecord> {
        self.records.iter().filter(|r| r.matches(filter)).collect()
    }

    /// Aggregate the records matching `filter`.
    pub fn totals(&self, filter: &UsageFilter) -> TokenTotals {
        let mut totals = TokenTotals::default();
        for record in self.records.iter().filter(|r| r.matches(filter)) {
            totals.add(record);
        }
        totals
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(
        agent: &str,
        task_type: Option<&str>,
        provider: &str,
        model: Option<&str>,
        recorded_at: i64,
        input: u64,
        output: u64,
    ) -> TokenUsageRecord {
        TokenUsageRecord {
            run_id: format!("run-{recorded_at}"),
            agent: agent.into(),
            provider: provider.into(),
            model: model.map(Into::into),
            task_type: task_type.map(Into::into),
            mission_id: None,
            recorded_at,
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            cost_usd: None,
        }
    }

    fn seeded_store() -> TokenUsageStore {
        let mut store = TokenUsageStore::new();
        store.record(rec(
            "test_writer",
            Some("test"),
            "claude_code",
            Some("claude-opus-4-7"),
            100,
            1_000,
            500,
        ));
        store.record(rec(
            "test_writer",
            Some("test"),
            "claude_code",
            Some("claude-opus-4-7"),
            200,
            2_000,
            800,
        ));
        store.record(rec(
            "doc_updater",
            Some("doc"),
            "anthropic_api",
            Some("claude-sonnet-4-6"),
            300,
            300,
            100,
        ));
        store.record(rec(
            "feature_engineer",
            Some("feature"),
            "claude_code",
            Some("claude-opus-4-7"),
            400,
            5_000,
            2_000,
        ));
        store
    }

    #[test]
    fn empty_filter_matches_everything() {
        let store = seeded_store();
        let filter = UsageFilter::new();
        assert_eq!(store.query(&filter).len(), 4);
        let totals = store.totals(&filter);
        assert_eq!(totals.records, 4);
        assert_eq!(totals.input_tokens, 1_000 + 2_000 + 300 + 5_000);
        assert_eq!(totals.output_tokens, 500 + 800 + 100 + 2_000);
    }

    #[test]
    fn filter_by_agent() {
        let store = seeded_store();
        let filter = UsageFilter::new().agent("test_writer");
        let hits = store.query(&filter);
        assert_eq!(hits.len(), 2);
        assert!(hits.iter().all(|r| r.agent == "test_writer"));
    }

    #[test]
    fn filter_by_task_type_and_provider() {
        let store = seeded_store();
        let filter = UsageFilter::new().task_type("test").provider("claude_code");
        let totals = store.totals(&filter);
        assert_eq!(totals.records, 2);
        assert_eq!(totals.input_tokens, 3_000);
        assert_eq!(totals.output_tokens, 1_300);
    }

    #[test]
    fn filter_by_timeline_is_inclusive() {
        let store = seeded_store();
        // 200..=300 should grab records at t=200 and t=300, skip t=100 and t=400.
        let filter = UsageFilter::new().since(200).until(300);
        let hits = store.query(&filter);
        assert_eq!(hits.len(), 2);
        assert!(hits.iter().all(|r| (200..=300).contains(&r.recorded_at)));
    }

    #[test]
    fn filter_by_model_skips_records_without_one() {
        let mut store = seeded_store();
        // A local_command provider that didn't report a model.
        store.record(rec(
            "feature_engineer",
            Some("feature"),
            "claude_code",
            None,
            500,
            10,
            10,
        ));
        let filter = UsageFilter::new().model("claude-opus-4-7");
        let hits = store.query(&filter);
        assert_eq!(hits.len(), 3, "the model-less record must be excluded");
    }

    #[test]
    fn billable_tokens_excludes_cache_reads() {
        let mut r = rec("a", None, "p", None, 0, 100, 50);
        r.cache_read_tokens = 1_000;
        r.cache_write_tokens = 10;
        // 100 + 50 + 10 = 160; cache_read is not counted.
        assert_eq!(r.billable_tokens(), 160);
    }

    #[test]
    fn totals_billable_matches_record_sum() {
        let store = seeded_store();
        let totals = store.totals(&UsageFilter::new());
        let by_hand: u64 = store.all().iter().map(|r| r.billable_tokens()).sum();
        assert_eq!(totals.billable_tokens(), by_hand);
    }

    #[test]
    fn record_serde_roundtrip() {
        let original = rec(
            "test_writer",
            Some("test"),
            "claude_code",
            Some("claude-opus-4-7"),
            123,
            10,
            20,
        );
        let yaml = serde_yml::to_string(&original).expect("serialize");
        let back: TokenUsageRecord = serde_yml::from_str(&yaml).expect("deserialize");
        assert_eq!(original, back);
    }

    #[test]
    fn filter_combines_constraints_with_and_semantics() {
        let store = seeded_store();
        // task_type=test AND agent=doc_updater → no record matches.
        let filter = UsageFilter::new().task_type("test").agent("doc_updater");
        assert!(store.query(&filter).is_empty());
    }
}
