//! Aggregated history view for the runner's read-only API.
//!
//! Bundles [`crate::usage::TokenUsageStore`] and [`crate::logs::LogStore`] and
//! exposes the queries the runner's history API surfaces: list past runs,
//! fetch a single run's full trace, filter usage records, and produce totals
//! bucketed along the dimensions called out by the product roadmap
//! (*agent type, model, task, timeline*).
//!
//! Same constraints as the rest of `agit-core`: pure data, no I/O, no async,
//! no chrono. Persistence (reading and writing a `HistoryStore` to disk) lives
//! in the runner.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::logs::{LogFilter, LogRecord, LogStore};
use crate::trace::RunTrace;
use crate::usage::{TokenTotals, TokenUsageRecord, TokenUsageStore, UsageFilter};

/// Combined token-usage + log store. The `run_id` field is the shared join
/// key — every [`TokenUsageRecord`] and [`LogRecord`] for a given run carry
/// the same value.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct HistoryStore {
    #[serde(default)]
    pub usage: TokenUsageStore,
    #[serde(default)]
    pub logs: LogStore,
}

impl HistoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// One [`RunSummary`] per run that has at least one usage record or one
    /// log entry. Returned sorted by `started_at` ascending, then by run id —
    /// stable ordering matters for the dashboard's list view.
    pub fn list_runs(&self) -> Vec<RunSummary> {
        let mut by_id: BTreeMap<String, RunSummary> = BTreeMap::new();

        for record in self.usage.all() {
            by_id
                .entry(record.run_id.clone())
                .or_insert_with(|| RunSummary::seed_from_usage(record))
                .merge_usage(record);
        }
        for record in self.logs.all() {
            by_id
                .entry(record.run_id.clone())
                .or_insert_with(|| RunSummary::seed_from_log(record))
                .merge_log(record);
        }

        let mut summaries: Vec<RunSummary> = by_id.into_values().collect();
        summaries.sort_by(|a, b| {
            a.started_at
                .cmp(&b.started_at)
                .then_with(|| a.run_id.cmp(&b.run_id))
        });
        summaries
    }

    /// Headline for a single run. `None` when the run has neither usage nor
    /// log records.
    pub fn run_summary(&self, run_id: &str) -> Option<RunSummary> {
        self.list_runs().into_iter().find(|s| s.run_id == run_id)
    }

    /// Full trace for a single run: borrowed usage records and log entries.
    /// `None` when the run is unknown.
    pub fn run_trace(&self, run_id: &str) -> Option<RunTrace<'_>> {
        let trace = RunTrace::collect(&self.usage, &self.logs, run_id);
        if trace.is_empty() {
            None
        } else {
            Some(trace)
        }
    }

    pub fn usage_records(&self, filter: &UsageFilter) -> Vec<&TokenUsageRecord> {
        self.usage.query(filter)
    }

    pub fn log_records(&self, filter: &LogFilter) -> Vec<&LogRecord> {
        self.logs.query(filter)
    }

    pub fn totals(&self, filter: &UsageFilter) -> TokenTotals {
        self.usage.totals(filter)
    }

    /// Totals bucketed along one dimension. Keys are sorted lexicographically
    /// — for [`GroupBy::Day`] that happens to be chronological order.
    pub fn totals_grouped(&self, filter: &UsageFilter, by: GroupBy) -> Vec<GroupedTotals> {
        let mut groups: BTreeMap<String, TokenTotals> = BTreeMap::new();
        for record in self.usage.all().iter().filter(|r| r.matches(filter)) {
            let key = match by {
                GroupBy::Agent => record.agent.clone(),
                GroupBy::Provider => record.provider.clone(),
                GroupBy::Model => record.model.clone().unwrap_or_else(|| UNKNOWN_KEY.into()),
                GroupBy::TaskType => record
                    .task_type
                    .clone()
                    .unwrap_or_else(|| UNKNOWN_KEY.into()),
                GroupBy::Day => unix_day_key(record.recorded_at),
            };
            groups.entry(key).or_default().add(record);
        }
        groups
            .into_iter()
            .map(|(key, totals)| GroupedTotals { key, totals })
            .collect()
    }
}

const UNKNOWN_KEY: &str = "(unknown)";

/// Dimension along which to bucket token totals.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum GroupBy {
    Agent,
    Provider,
    Model,
    TaskType,
    /// One bucket per UTC day, key formatted as `YYYY-MM-DD`.
    Day,
}

impl GroupBy {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "agent" => Some(Self::Agent),
            "provider" => Some(Self::Provider),
            "model" => Some(Self::Model),
            "task_type" => Some(Self::TaskType),
            "day" => Some(Self::Day),
            _ => None,
        }
    }
}

/// One token-totals bucket from [`HistoryStore::totals_grouped`].
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct GroupedTotals {
    pub key: String,
    pub totals: TokenTotals,
}

/// One-line headline for a run: enough to render a row in the dashboard
/// without fetching the full trace.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct RunSummary {
    pub run_id: String,
    pub agent: String,
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mission_id: Option<String>,
    /// Earliest `recorded_at` across all of this run's usage and log records.
    pub started_at: i64,
    /// Latest `recorded_at` across all of this run's usage and log records.
    pub ended_at: i64,
    pub totals: TokenTotals,
    pub log_count: usize,
}

impl RunSummary {
    fn seed_from_usage(record: &TokenUsageRecord) -> Self {
        Self {
            run_id: record.run_id.clone(),
            agent: record.agent.clone(),
            provider: record.provider.clone(),
            model: record.model.clone(),
            task_type: record.task_type.clone(),
            mission_id: record.mission_id.clone(),
            started_at: record.recorded_at,
            ended_at: record.recorded_at,
            totals: TokenTotals::default(),
            log_count: 0,
        }
    }

    fn seed_from_log(record: &LogRecord) -> Self {
        Self {
            run_id: record.run_id.clone(),
            agent: record.agent.clone(),
            provider: record.provider.clone(),
            model: None,
            task_type: record.task_type.clone(),
            mission_id: record.mission_id.clone(),
            started_at: record.recorded_at,
            ended_at: record.recorded_at,
            totals: TokenTotals::default(),
            log_count: 0,
        }
    }

    fn merge_usage(&mut self, record: &TokenUsageRecord) {
        self.totals.add(record);
        self.started_at = self.started_at.min(record.recorded_at);
        self.ended_at = self.ended_at.max(record.recorded_at);
        if self.model.is_none() {
            self.model = record.model.clone();
        }
        if self.task_type.is_none() {
            self.task_type = record.task_type.clone();
        }
        if self.mission_id.is_none() {
            self.mission_id = record.mission_id.clone();
        }
    }

    fn merge_log(&mut self, record: &LogRecord) {
        self.log_count = self.log_count.saturating_add(1);
        self.started_at = self.started_at.min(record.recorded_at);
        self.ended_at = self.ended_at.max(record.recorded_at);
    }
}

/// Format a Unix-epoch timestamp as a `YYYY-MM-DD` UTC date key.
///
/// Inlined to keep `agit-core` chrono-free. The arithmetic is Howard
/// Hinnant's `civil_from_days` — proleptic Gregorian, battle-tested.
fn unix_day_key(unix_seconds: i64) -> String {
    let days = unix_seconds.div_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}")
}

fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u32, d as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logs::LogLevel;

    #[allow(clippy::too_many_arguments)]
    fn usage(
        run_id: &str,
        agent: &str,
        provider: &str,
        model: Option<&str>,
        task_type: Option<&str>,
        recorded_at: i64,
        input: u64,
        output: u64,
    ) -> TokenUsageRecord {
        TokenUsageRecord {
            run_id: run_id.into(),
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

    fn logrec(
        run_id: &str,
        agent: &str,
        provider: &str,
        task_type: Option<&str>,
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

    fn seeded() -> HistoryStore {
        let mut store = HistoryStore::new();
        // Two records for run-A on 2023-11-14.
        store.usage.record(usage(
            "run-A",
            "test_writer",
            "claude_code",
            Some("claude-opus-4-7"),
            Some("test"),
            1_700_000_000,
            1_000,
            500,
        ));
        store.usage.record(usage(
            "run-A",
            "test_writer",
            "claude_code",
            Some("claude-opus-4-7"),
            Some("test"),
            1_700_000_100,
            2_000,
            800,
        ));
        // One record for run-B on 2023-11-16.
        store.usage.record(usage(
            "run-B",
            "doc_updater",
            "anthropic_api",
            Some("claude-sonnet-4-6"),
            Some("doc"),
            1_700_100_000,
            300,
            100,
        ));
        store.logs.record(logrec(
            "run-A",
            "test_writer",
            "claude_code",
            Some("test"),
            1_700_000_050,
            LogLevel::Info,
            "starting",
        ));
        store.logs.record(logrec(
            "run-A",
            "test_writer",
            "claude_code",
            Some("test"),
            1_700_000_200,
            LogLevel::Info,
            "done",
        ));
        store.logs.record(logrec(
            "run-B",
            "doc_updater",
            "anthropic_api",
            Some("doc"),
            1_700_100_050,
            LogLevel::Warn,
            "doc warning",
        ));
        store
    }

    #[test]
    fn list_runs_one_summary_per_run_id() {
        let store = seeded();
        let runs = store.list_runs();
        assert_eq!(runs.len(), 2);
        let a = runs.iter().find(|s| s.run_id == "run-A").unwrap();
        let b = runs.iter().find(|s| s.run_id == "run-B").unwrap();
        assert_eq!(a.totals.records, 2);
        assert_eq!(a.totals.input_tokens, 3_000);
        assert_eq!(a.log_count, 2);
        assert_eq!(a.started_at, 1_700_000_000);
        assert_eq!(a.ended_at, 1_700_000_200);
        assert_eq!(b.totals.records, 1);
        assert_eq!(b.log_count, 1);
    }

    #[test]
    fn list_runs_includes_runs_with_only_logs() {
        let mut store = HistoryStore::new();
        store
            .logs
            .record(logrec("log-only", "x", "y", None, 10, LogLevel::Info, "hi"));
        let runs = store.list_runs();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].run_id, "log-only");
        assert_eq!(runs[0].totals.records, 0);
        assert_eq!(runs[0].log_count, 1);
    }

    #[test]
    fn list_runs_orders_by_started_at_ascending() {
        let store = seeded();
        let runs = store.list_runs();
        assert!(runs[0].started_at <= runs[1].started_at);
        assert_eq!(runs[0].run_id, "run-A");
        assert_eq!(runs[1].run_id, "run-B");
    }

    #[test]
    fn run_summary_returns_none_for_unknown_run() {
        let store = seeded();
        assert!(store.run_summary("unknown").is_none());
    }

    #[test]
    fn run_trace_returns_some_for_known_run() {
        let store = seeded();
        let trace = store.run_trace("run-A").unwrap();
        assert_eq!(trace.usage.len(), 2);
        assert_eq!(trace.logs.len(), 2);
    }

    #[test]
    fn run_trace_returns_none_for_unknown_run() {
        let store = seeded();
        assert!(store.run_trace("nope").is_none());
    }

    #[test]
    fn totals_grouped_by_agent() {
        let store = seeded();
        let groups = store.totals_grouped(&UsageFilter::new(), GroupBy::Agent);
        assert_eq!(groups.len(), 2);
        let test = groups.iter().find(|g| g.key == "test_writer").unwrap();
        assert_eq!(test.totals.records, 2);
        assert_eq!(test.totals.input_tokens, 3_000);
        let doc = groups.iter().find(|g| g.key == "doc_updater").unwrap();
        assert_eq!(doc.totals.records, 1);
    }

    #[test]
    fn totals_grouped_by_model_uses_unknown_for_none() {
        let mut store = seeded();
        store.usage.record(usage(
            "run-C",
            "raw_command_agent",
            "claude_code",
            None,
            Some("feature"),
            1_700_200_000,
            10,
            10,
        ));
        let groups = store.totals_grouped(&UsageFilter::new(), GroupBy::Model);
        let keys: Vec<&str> = groups.iter().map(|g| g.key.as_str()).collect();
        assert!(keys.contains(&UNKNOWN_KEY));
    }

    #[test]
    fn totals_grouped_by_day_buckets_by_utc_day() {
        let store = seeded();
        let groups = store.totals_grouped(&UsageFilter::new(), GroupBy::Day);
        let keys: Vec<&str> = groups.iter().map(|g| g.key.as_str()).collect();
        assert_eq!(keys, vec!["2023-11-14", "2023-11-16"]);
        let day_a = groups.iter().find(|g| g.key == "2023-11-14").unwrap();
        assert_eq!(day_a.totals.records, 2);
    }

    #[test]
    fn totals_grouped_respects_filter() {
        let store = seeded();
        let filter = UsageFilter::new().agent("test_writer");
        let groups = store.totals_grouped(&filter, GroupBy::Provider);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].key, "claude_code");
        assert_eq!(groups[0].totals.records, 2);
    }

    #[test]
    fn group_by_parse() {
        assert_eq!(GroupBy::parse("agent"), Some(GroupBy::Agent));
        assert_eq!(GroupBy::parse("provider"), Some(GroupBy::Provider));
        assert_eq!(GroupBy::parse("model"), Some(GroupBy::Model));
        assert_eq!(GroupBy::parse("task_type"), Some(GroupBy::TaskType));
        assert_eq!(GroupBy::parse("day"), Some(GroupBy::Day));
        assert_eq!(GroupBy::parse("unknown"), None);
    }

    #[test]
    fn unix_day_key_known_dates() {
        assert_eq!(unix_day_key(0), "1970-01-01");
        assert_eq!(unix_day_key(1_700_000_000), "2023-11-14");
        // 2000-02-29 00:00:00 UTC — leap-year edge.
        assert_eq!(unix_day_key(951_782_400), "2000-02-29");
        // 2000-03-01 00:00:00 UTC — day after the leap day.
        assert_eq!(unix_day_key(951_868_800), "2000-03-01");
    }

    #[test]
    fn history_store_json_roundtrip() {
        let original = seeded();
        let json = serde_json::to_string(&original).unwrap();
        let back: HistoryStore = serde_json::from_str(&json).unwrap();
        assert_eq!(back.usage.len(), original.usage.len());
        assert_eq!(back.logs.len(), original.logs.len());
    }
}
