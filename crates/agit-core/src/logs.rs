//! Log records and queries for agent runs.
//!
//! Token consumption ([`crate::usage`]) is one half of an agent run's
//! accounting; the textual output the agent produced is the other half. This
//! module captures the latter as structured `LogRecord`s sharing the same
//! filter dimensions as `TokenUsageRecord`, so a single run identifier (or
//! agent / task type / timeline range) pulls back both views.
//!
//! Same constraints as [`crate::usage`]: no I/O, no async, no chrono. Time is
//! represented as a Unix epoch second (`i64`) so callers can format and store
//! it however they like.

use serde::{Deserialize, Serialize};

/// Severity of a log entry. Ordered: `Trace < Debug < Info < Warn < Error`.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

/// A single log line produced during an agent run.
///
/// Mirrors `TokenUsageRecord`'s flat-field layout so that any query expressed
/// against the usage store can also be expressed against the log store.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct LogRecord {
    /// Identifier of the run this entry belongs to. Free-form on purpose —
    /// the runner picks (e.g. `"agit/test-writer/issue-42"`). Must match the
    /// `run_id` used by the matching `TokenUsageRecord`s.
    pub run_id: String,
    /// Agent key as declared in `.agit/agents.yaml`.
    pub agent: String,
    /// Provider key as declared in `.agit/agents.yaml`.
    pub provider: String,
    /// Short label categorizing the work — typically the trigger slug
    /// (`"test"`, `"doc"`, `"feature"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_type: Option<String>,
    /// Optional mission identifier when the run came from a server-issued
    /// mission. `None` for the no-server `watch` mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mission_id: Option<String>,
    /// Unix epoch seconds. The runner sets this at record creation time.
    pub recorded_at: i64,
    pub level: LogLevel,
    pub message: String,
}

impl LogRecord {
    /// True iff this record matches every constraint set on `filter`.
    /// An empty filter matches every record.
    pub fn matches(&self, filter: &LogFilter) -> bool {
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
        if let Some(level) = filter.level {
            if self.level != level {
                return false;
            }
        }
        if let Some(min) = filter.min_level {
            if self.level < min {
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

/// Optional bounds for querying a `LogStore`. Every field defaults to
/// "no constraint"; combine them as needed.
///
/// Time bounds are inclusive Unix epoch seconds. A `None` bound means open.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct LogFilter {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// Match a single level exactly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<LogLevel>,
    /// Match records at or above this level (e.g. `Warn` matches Warn+Error).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_level: Option<LogLevel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<i64>,
}

impl LogFilter {
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

    pub fn task_type(mut self, task_type: impl Into<String>) -> Self {
        self.task_type = Some(task_type.into());
        self
    }

    pub fn run_id(mut self, run_id: impl Into<String>) -> Self {
        self.run_id = Some(run_id.into());
        self
    }

    pub fn level(mut self, level: LogLevel) -> Self {
        self.level = Some(level);
        self
    }

    pub fn min_level(mut self, level: LogLevel) -> Self {
        self.min_level = Some(level);
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

/// In-memory log store.
///
/// Sibling of [`crate::usage::TokenUsageStore`]: the runner buffers per-run
/// log entries here before reporting them back; a server-side store can speak
/// the same shape on top of SQLite/Postgres later.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct LogStore {
    records: Vec<LogRecord>,
}

impl LogStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&mut self, record: LogRecord) {
        self.records.push(record);
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    pub fn all(&self) -> &[LogRecord] {
        &self.records
    }

    /// Borrow every record matching `filter`, in insertion order.
    pub fn query<'a>(&'a self, filter: &LogFilter) -> Vec<&'a LogRecord> {
        self.records.iter().filter(|r| r.matches(filter)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(
        run_id: &str,
        agent: &str,
        task_type: Option<&str>,
        provider: &str,
        recorded_at: i64,
        level: LogLevel,
        message: &str,
    ) -> LogRecord {
        LogRecord {
            run_id: run_id.into(),
            agent: agent.into(),
            provider: provider.into(),
            task_type: task_type.map(Into::into),
            mission_id: None,
            recorded_at,
            level,
            message: message.into(),
        }
    }

    fn seeded_store() -> LogStore {
        let mut store = LogStore::new();
        store.record(entry(
            "run-1",
            "test_writer",
            Some("test"),
            "claude_code",
            100,
            LogLevel::Info,
            "starting test_writer",
        ));
        store.record(entry(
            "run-1",
            "test_writer",
            Some("test"),
            "claude_code",
            110,
            LogLevel::Warn,
            "cargo test reported a flaky case",
        ));
        store.record(entry(
            "run-2",
            "doc_updater",
            Some("doc"),
            "anthropic_api",
            300,
            LogLevel::Error,
            "doc_updater failed to push branch",
        ));
        store.record(entry(
            "run-3",
            "feature_engineer",
            Some("feature"),
            "claude_code",
            400,
            LogLevel::Debug,
            "diff is 12 lines",
        ));
        store
    }

    #[test]
    fn empty_filter_matches_everything() {
        let store = seeded_store();
        assert_eq!(store.query(&LogFilter::new()).len(), 4);
    }

    #[test]
    fn filter_by_run_id_returns_just_that_run() {
        let store = seeded_store();
        let hits = store.query(&LogFilter::new().run_id("run-1"));
        assert_eq!(hits.len(), 2);
        assert!(hits.iter().all(|r| r.run_id == "run-1"));
    }

    #[test]
    fn filter_by_agent_and_task_type() {
        let store = seeded_store();
        let hits = store.query(&LogFilter::new().agent("test_writer").task_type("test"));
        assert_eq!(hits.len(), 2);
    }

    #[test]
    fn filter_by_exact_level() {
        let store = seeded_store();
        let hits = store.query(&LogFilter::new().level(LogLevel::Error));
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].run_id, "run-2");
    }

    #[test]
    fn filter_by_min_level_is_inclusive_upward() {
        let store = seeded_store();
        // min_level=Warn → Warn + Error, but not Info or Debug.
        let hits = store.query(&LogFilter::new().min_level(LogLevel::Warn));
        assert_eq!(hits.len(), 2);
        assert!(hits.iter().all(|r| r.level >= LogLevel::Warn));
    }

    #[test]
    fn filter_by_timeline_is_inclusive() {
        let store = seeded_store();
        let hits = store.query(&LogFilter::new().since(110).until(300));
        assert_eq!(hits.len(), 2);
        assert!(hits.iter().all(|r| (110..=300).contains(&r.recorded_at)));
    }

    #[test]
    fn record_serde_roundtrip() {
        let original = entry(
            "run-1",
            "test_writer",
            Some("test"),
            "claude_code",
            123,
            LogLevel::Info,
            "hello",
        );
        let yaml = serde_yml::to_string(&original).expect("serialize");
        let back: LogRecord = serde_yml::from_str(&yaml).expect("deserialize");
        assert_eq!(original, back);
    }
}
