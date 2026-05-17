//! HTTP API exposing the runner's history (past runs, token usage, logs).
//!
//! Sync [`tiny_http`]-backed server. Matches the runner's existing
//! synchronous shape — no tokio runtime, no executor — and is small enough
//! to read end-to-end.
//!
//! Routes (all GET, all `application/json`):
//!
//! ```text
//! /healthz
//! /v1/runs
//! /v1/runs/{run_id}                                  (run_id may be percent-encoded)
//! /v1/usage?agent=&provider=&model=&task_type=&run_id=&since=&until=
//! /v1/usage/totals?<filters>&group_by=agent|provider|model|task_type|day
//! ```
//!
//! Filter query params mirror [`agit_core::usage::UsageFilter`] field-for-
//! field. On `/v1/usage/totals`, the optional `group_by` parameter switches
//! the response from a single totals object to an array of `{ key, totals }`
//! buckets.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::thread::JoinHandle;

use agit_core::history::{GroupBy, HistoryStore};
use agit_core::usage::UsageFilter;
use anyhow::{anyhow, Context, Result};
use serde::Serialize;
use serde_json::json;
use tiny_http::{Header, Method, Response, Server, StatusCode};

/// Load a [`HistoryStore`] from a JSON file. Returns an empty store when
/// `path` is `None` or the file is missing — the runner can spin up before
/// any history has been recorded.
pub fn load_history(path: Option<&Path>) -> Result<HistoryStore> {
    let Some(path) = path else {
        return Ok(HistoryStore::new());
    };
    if !path.exists() {
        eprintln!(
            "agit-runner: history file {} not found — starting with empty store",
            path.display()
        );
        return Ok(HistoryStore::new());
    }
    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("reading history file {}", path.display()))?;
    let store: HistoryStore = serde_json::from_str(&contents)
        .with_context(|| format!("parsing {} as HistoryStore JSON", path.display()))?;
    Ok(store)
}

/// Block on the HTTP server until the underlying socket is closed.
pub fn serve(store: HistoryStore, addr: &str) -> Result<()> {
    let server = bind(addr)?;
    serve_loop(server, Arc::new(store));
    Ok(())
}

/// Spawn the HTTP server on a background thread. Used by `watch` so the
/// poll loop and the API live in the same process.
pub fn spawn(store: HistoryStore, addr: &str) -> Result<JoinHandle<()>> {
    let server = bind(addr)?;
    let store = Arc::new(store);
    Ok(std::thread::spawn(move || serve_loop(server, store)))
}

fn bind(addr: &str) -> Result<Server> {
    let server = Server::http(addr).map_err(|e| anyhow!("could not bind {addr}: {e}"))?;
    eprintln!("agit-runner: history API listening on http://{addr}");
    Ok(server)
}

fn serve_loop(server: Server, store: Arc<HistoryStore>) {
    for request in server.incoming_requests() {
        let api = handle(&store, request.method(), request.url());
        let resp = build_response(api);
        if let Err(e) = request.respond(resp) {
            eprintln!("agit-runner: failed to send response: {e}");
        }
    }
}

/// HTTP-shaped response — pure data so tests can assert on it without going
/// through a real socket.
#[derive(Clone, Debug, PartialEq)]
pub struct ApiResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

#[cfg(test)]
impl ApiResponse {
    fn parse_body<T: serde::de::DeserializeOwned>(&self) -> serde_json::Result<T> {
        serde_json::from_slice(&self.body)
    }
}

/// Pure router. `method` is a tiny_http method; `url` is the request URL
/// including any query string.
pub fn handle(store: &HistoryStore, method: &Method, url: &str) -> ApiResponse {
    if method != &Method::Get {
        return error(405, "method not allowed");
    }
    let (path, query) = split_query(url);
    let params = parse_query(query);

    if path == "/healthz" {
        return json_ok(&json!({ "ok": true }));
    }
    if path == "/v1/runs" {
        return json_ok(&store.list_runs());
    }
    if let Some(run_id_enc) = path.strip_prefix("/v1/runs/") {
        let run_id = percent_decode(run_id_enc);
        return get_run(store, &run_id);
    }
    if path == "/v1/usage" {
        return list_usage(store, &params);
    }
    if path == "/v1/usage/totals" {
        return usage_totals(store, &params);
    }

    error(404, "not found")
}

fn get_run(store: &HistoryStore, run_id: &str) -> ApiResponse {
    let Some(summary) = store.run_summary(run_id) else {
        return error(404, &format!("run '{run_id}' not found"));
    };
    let trace = store
        .run_trace(run_id)
        .expect("run_summary matched but trace was empty");
    json_ok(&json!({
        "summary": summary,
        "usage": trace.usage,
        "logs": trace.logs,
    }))
}

fn list_usage(store: &HistoryStore, params: &BTreeMap<String, String>) -> ApiResponse {
    let filter = match build_filter(params) {
        Ok(f) => f,
        Err(e) => return error(400, &e),
    };
    let records = store.usage_records(&filter);
    let totals = store.totals(&filter);
    json_ok(&json!({
        "records": records,
        "totals": totals,
    }))
}

fn usage_totals(store: &HistoryStore, params: &BTreeMap<String, String>) -> ApiResponse {
    let filter = match build_filter(params) {
        Ok(f) => f,
        Err(e) => return error(400, &e),
    };

    let Some(group_by_raw) = params.get("group_by") else {
        return json_ok(&store.totals(&filter));
    };
    let Some(group_by) = GroupBy::parse(group_by_raw) else {
        return error(
            400,
            &format!(
                "unknown group_by '{group_by_raw}' — expected one of: agent, provider, model, task_type, day"
            ),
        );
    };
    json_ok(&store.totals_grouped(&filter, group_by))
}

fn build_filter(params: &BTreeMap<String, String>) -> Result<UsageFilter, String> {
    let mut filter = UsageFilter::new();
    if let Some(v) = params.get("agent") {
        filter = filter.agent(v.clone());
    }
    if let Some(v) = params.get("provider") {
        filter = filter.provider(v.clone());
    }
    if let Some(v) = params.get("model") {
        filter = filter.model(v.clone());
    }
    if let Some(v) = params.get("task_type") {
        filter = filter.task_type(v.clone());
    }
    if let Some(v) = params.get("run_id") {
        filter = filter.run_id(v.clone());
    }
    if let Some(v) = params.get("since") {
        let n: i64 = v.parse().map_err(|_| format!("invalid `since`: {v}"))?;
        filter = filter.since(n);
    }
    if let Some(v) = params.get("until") {
        let n: i64 = v.parse().map_err(|_| format!("invalid `until`: {v}"))?;
        filter = filter.until(n);
    }
    Ok(filter)
}

fn json_ok<T: Serialize>(body: &T) -> ApiResponse {
    let bytes = serde_json::to_vec(body).unwrap_or_else(|_| b"{}".to_vec());
    ApiResponse {
        status: 200,
        body: bytes,
    }
}

fn error(status: u16, message: &str) -> ApiResponse {
    let body = json!({ "error": message });
    let bytes = serde_json::to_vec(&body).unwrap_or_else(|_| b"{\"error\":\"\"}".to_vec());
    ApiResponse {
        status,
        body: bytes,
    }
}

fn build_response(api: ApiResponse) -> Response<std::io::Cursor<Vec<u8>>> {
    let header = Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
        .expect("static header bytes");
    Response::from_data(api.body)
        .with_status_code(StatusCode(api.status))
        .with_header(header)
}

fn split_query(url: &str) -> (&str, &str) {
    match url.find('?') {
        Some(i) => (&url[..i], &url[i + 1..]),
        None => (url, ""),
    }
}

fn parse_query(query: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    if query.is_empty() {
        return out;
    }
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let key = percent_decode(key);
        if key.is_empty() {
            continue;
        }
        let value = percent_decode(value);
        out.insert(key, value);
    }
    out
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'+' {
            out.push(b' ');
            i += 1;
            continue;
        }
        if b == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex_value(bytes[i + 1]), hex_value(bytes[i + 2])) {
                out.push((hi << 4) | lo);
                i += 3;
                continue;
            }
        }
        out.push(b);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_default()
}

fn hex_value(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agit_core::history::{GroupedTotals, RunSummary};
    use agit_core::logs::{LogLevel, LogRecord};
    use agit_core::usage::{TokenTotals, TokenUsageRecord};

    fn seeded() -> HistoryStore {
        let mut store = HistoryStore::new();
        store.usage.record(TokenUsageRecord {
            run_id: "agit/test/issue-1".into(),
            agent: "test_writer".into(),
            provider: "claude_code".into(),
            model: Some("claude-opus-4-7".into()),
            task_type: Some("test".into()),
            mission_id: None,
            recorded_at: 1_700_000_000,
            input_tokens: 1_000,
            output_tokens: 500,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            cost_usd: None,
        });
        store.usage.record(TokenUsageRecord {
            run_id: "agit/doc/issue-2".into(),
            agent: "doc_updater".into(),
            provider: "anthropic_api".into(),
            model: Some("claude-sonnet-4-6".into()),
            task_type: Some("doc".into()),
            mission_id: None,
            recorded_at: 1_700_100_000,
            input_tokens: 200,
            output_tokens: 100,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            cost_usd: None,
        });
        store.logs.record(LogRecord {
            run_id: "agit/test/issue-1".into(),
            agent: "test_writer".into(),
            provider: "claude_code".into(),
            task_type: Some("test".into()),
            mission_id: None,
            recorded_at: 1_700_000_050,
            level: LogLevel::Info,
            message: "hello".into(),
        });
        store
    }

    fn call(url: &str) -> ApiResponse {
        handle(&seeded(), &Method::Get, url)
    }

    #[test]
    fn healthz_returns_ok() {
        let resp = call("/healthz");
        assert_eq!(resp.status, 200);
        let body: serde_json::Value = resp.parse_body().unwrap();
        assert_eq!(body["ok"], true);
    }

    #[test]
    fn unknown_path_returns_404() {
        let resp = call("/nope");
        assert_eq!(resp.status, 404);
    }

    #[test]
    fn non_get_method_returns_405() {
        let resp = handle(&seeded(), &Method::Post, "/v1/runs");
        assert_eq!(resp.status, 405);
    }

    #[test]
    fn list_runs_returns_two_summaries() {
        let resp = call("/v1/runs");
        assert_eq!(resp.status, 200);
        let runs: Vec<RunSummary> = resp.parse_body().unwrap();
        assert_eq!(runs.len(), 2);
    }

    #[test]
    fn get_run_returns_404_for_unknown() {
        let resp = call("/v1/runs/unknown");
        assert_eq!(resp.status, 404);
    }

    #[test]
    fn get_run_handles_percent_encoded_run_id() {
        let resp = call("/v1/runs/agit%2Ftest%2Fissue-1");
        assert_eq!(resp.status, 200);
        let body: serde_json::Value = resp.parse_body().unwrap();
        assert_eq!(body["summary"]["run_id"], "agit/test/issue-1");
        assert_eq!(body["usage"].as_array().unwrap().len(), 1);
        assert_eq!(body["logs"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn get_run_accepts_raw_slash_in_path() {
        // We strip the `/v1/runs/` prefix and treat the rest as a single
        // run_id, so clients that don't bother URL-encoding still work.
        let resp = call("/v1/runs/agit/test/issue-1");
        assert_eq!(resp.status, 200);
        let body: serde_json::Value = resp.parse_body().unwrap();
        assert_eq!(body["summary"]["run_id"], "agit/test/issue-1");
    }

    #[test]
    fn list_usage_filters_by_agent() {
        let resp = call("/v1/usage?agent=doc_updater");
        assert_eq!(resp.status, 200);
        let body: serde_json::Value = resp.parse_body().unwrap();
        let records = body["records"].as_array().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0]["agent"], "doc_updater");
        assert_eq!(body["totals"]["records"], 1);
    }

    #[test]
    fn list_usage_filters_by_timeline() {
        // Only the second record (recorded_at = 1_700_100_000) should pass.
        let resp = call("/v1/usage?since=1700050000");
        assert_eq!(resp.status, 200);
        let body: serde_json::Value = resp.parse_body().unwrap();
        assert_eq!(body["records"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn list_usage_rejects_invalid_since() {
        let resp = call("/v1/usage?since=not-a-number");
        assert_eq!(resp.status, 400);
    }

    #[test]
    fn totals_without_group_by_returns_aggregate() {
        let resp = call("/v1/usage/totals");
        assert_eq!(resp.status, 200);
        let totals: TokenTotals = resp.parse_body().unwrap();
        assert_eq!(totals.records, 2);
        assert_eq!(totals.input_tokens, 1_200);
    }

    #[test]
    fn totals_with_group_by_agent() {
        let resp = call("/v1/usage/totals?group_by=agent");
        assert_eq!(resp.status, 200);
        let groups: Vec<GroupedTotals> = resp.parse_body().unwrap();
        assert_eq!(groups.len(), 2);
    }

    #[test]
    fn totals_with_group_by_day() {
        let resp = call("/v1/usage/totals?group_by=day");
        let groups: Vec<GroupedTotals> = resp.parse_body().unwrap();
        let keys: Vec<&str> = groups.iter().map(|g| g.key.as_str()).collect();
        assert_eq!(keys, vec!["2023-11-14", "2023-11-16"]);
    }

    #[test]
    fn totals_with_group_by_filtered() {
        // Filter to only the test_writer agent, then bucket by provider.
        let resp = call("/v1/usage/totals?agent=test_writer&group_by=provider");
        let groups: Vec<GroupedTotals> = resp.parse_body().unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].key, "claude_code");
    }

    #[test]
    fn totals_with_unknown_group_by_returns_400() {
        let resp = call("/v1/usage/totals?group_by=color");
        assert_eq!(resp.status, 400);
    }

    #[test]
    fn percent_decode_unicode_passthrough() {
        // Greek small letter lambda (U+03BB), UTF-8: CE BB.
        assert_eq!(percent_decode("%CE%BB"), "λ");
    }

    #[test]
    fn parse_query_handles_multiple_params() {
        let q = parse_query("a=1&b=2&c=hello%20world");
        assert_eq!(q.get("a").unwrap(), "1");
        assert_eq!(q.get("b").unwrap(), "2");
        assert_eq!(q.get("c").unwrap(), "hello world");
    }

    #[test]
    fn parse_query_handles_empty_string() {
        assert!(parse_query("").is_empty());
    }

    #[test]
    fn split_query_with_and_without_question_mark() {
        assert_eq!(split_query("/x?y=1"), ("/x", "y=1"));
        assert_eq!(split_query("/x"), ("/x", ""));
    }

    #[test]
    fn load_history_returns_empty_when_path_is_none() {
        let store = load_history(None).unwrap();
        assert!(store.usage.is_empty());
        assert!(store.logs.is_empty());
    }

    #[test]
    fn load_history_returns_empty_when_file_missing() {
        let path = std::env::temp_dir().join("agit-runner-history-does-not-exist.json");
        if path.exists() {
            std::fs::remove_file(&path).unwrap();
        }
        let store = load_history(Some(&path)).unwrap();
        assert!(store.usage.is_empty());
    }

    #[test]
    fn load_history_round_trips_through_json() {
        let mut path = std::env::temp_dir();
        path.push(format!("agit-runner-history-{}.json", std::process::id()));
        let original = seeded();
        std::fs::write(&path, serde_json::to_vec(&original).unwrap()).unwrap();

        let loaded = load_history(Some(&path)).unwrap();
        assert_eq!(loaded.usage.len(), original.usage.len());
        assert_eq!(loaded.logs.len(), original.logs.len());

        std::fs::remove_file(&path).ok();
    }
}
