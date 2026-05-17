//! Combined view of one agent task: token consumption + log entries.
//!
//! The runner records two parallel streams per run: how much each model call
//! cost ([`crate::usage`]) and what the agent said while it worked
//! ([`crate::logs`]). Consumers — a CLI subcommand, a dashboard widget — almost
//! always want both at once for a single task. This module is the small
//! join layer that produces that combined view.

use crate::logs::{LogFilter, LogRecord, LogStore};
use crate::usage::{TokenTotals, TokenUsageRecord, TokenUsageStore, UsageFilter};

/// Borrowed view of one agent task's recorded activity.
///
/// `usage` is the list of token-accounting records emitted by the run;
/// `logs` is the list of textual log entries. Both come back in insertion
/// order, scoped to the same `run_id`.
#[derive(Debug)]
pub struct RunTrace<'a> {
    pub run_id: String,
    pub usage: Vec<&'a TokenUsageRecord>,
    pub logs: Vec<&'a LogRecord>,
}

impl<'a> RunTrace<'a> {
    /// Assemble a trace for a single run by querying both stores in lockstep.
    pub fn collect(usage: &'a TokenUsageStore, logs: &'a LogStore, run_id: &str) -> Self {
        let usage_records = usage.query(&UsageFilter::new().run_id(run_id));
        let log_records = logs.query(&LogFilter::new().run_id(run_id));
        Self {
            run_id: run_id.into(),
            usage: usage_records,
            logs: log_records,
        }
    }

    /// Token totals for this run. Convenience over re-querying the store.
    pub fn token_totals(&self) -> TokenTotals {
        let mut totals = TokenTotals::default();
        for record in &self.usage {
            totals.add(record);
        }
        totals
    }

    /// True iff neither the usage stream nor the log stream has any entries
    /// for this run.
    pub fn is_empty(&self) -> bool {
        self.usage.is_empty() && self.logs.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logs::LogLevel;

    fn usage_rec(run_id: &str, recorded_at: i64, input: u64, output: u64) -> TokenUsageRecord {
        TokenUsageRecord {
            run_id: run_id.into(),
            agent: "test_writer".into(),
            provider: "claude_code".into(),
            model: Some("claude-opus-4-7".into()),
            task_type: Some("test".into()),
            mission_id: None,
            recorded_at,
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            cost_usd: None,
        }
    }

    fn log_rec(run_id: &str, recorded_at: i64, level: LogLevel, message: &str) -> LogRecord {
        LogRecord {
            run_id: run_id.into(),
            agent: "test_writer".into(),
            provider: "claude_code".into(),
            task_type: Some("test".into()),
            mission_id: None,
            recorded_at,
            level,
            message: message.into(),
        }
    }

    fn seeded() -> (TokenUsageStore, LogStore) {
        let mut usage = TokenUsageStore::new();
        usage.record(usage_rec("run-A", 100, 1_000, 500));
        usage.record(usage_rec("run-A", 200, 2_000, 800));
        usage.record(usage_rec("run-B", 300, 300, 100));

        let mut logs = LogStore::new();
        logs.record(log_rec("run-A", 100, LogLevel::Info, "starting"));
        logs.record(log_rec("run-A", 150, LogLevel::Warn, "test flaky"));
        logs.record(log_rec("run-A", 250, LogLevel::Info, "done"));
        logs.record(log_rec("run-B", 300, LogLevel::Info, "starting B"));

        (usage, logs)
    }

    #[test]
    fn collect_returns_only_records_for_the_requested_run() {
        let (usage, logs) = seeded();
        let trace = RunTrace::collect(&usage, &logs, "run-A");

        assert_eq!(trace.run_id, "run-A");
        assert_eq!(trace.usage.len(), 2);
        assert!(trace.usage.iter().all(|r| r.run_id == "run-A"));
        assert_eq!(trace.logs.len(), 3);
        assert!(trace.logs.iter().all(|r| r.run_id == "run-A"));
    }

    #[test]
    fn token_totals_aggregates_only_records_in_trace() {
        let (usage, logs) = seeded();
        let trace = RunTrace::collect(&usage, &logs, "run-A");
        let totals = trace.token_totals();
        assert_eq!(totals.records, 2);
        assert_eq!(totals.input_tokens, 3_000);
        assert_eq!(totals.output_tokens, 1_300);
    }

    #[test]
    fn collect_for_unknown_run_is_empty() {
        let (usage, logs) = seeded();
        let trace = RunTrace::collect(&usage, &logs, "run-none");
        assert!(trace.is_empty());
        assert_eq!(trace.token_totals().records, 0);
    }

    #[test]
    fn collect_can_return_logs_with_no_usage() {
        let (_, logs) = seeded();
        let usage = TokenUsageStore::new();
        let trace = RunTrace::collect(&usage, &logs, "run-A");
        assert!(trace.usage.is_empty());
        assert_eq!(trace.logs.len(), 3);
        assert!(!trace.is_empty());
    }
}
