use serde::Serialize;
use serde_json::Value;
use std::{fs::File, path::Path};

// ── Run result models ───────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct RunResult {
    pub run_id: String,
    pub batch_id: String,
    pub status: RunStatus,
    pub harness_exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RunError>,
    pub started_at: String,
    pub finished_at: String,
    pub duration_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inputs: Option<RunInputs>,
    pub selection: RunSelection,
    pub resolved: RunResolved,
    pub metrics: RunMetrics,
    pub artifacts: RunArtifacts,
}

/// Structured error for non-completed runs.
#[derive(Debug, Serialize)]
pub struct RunError {
    pub kind: String,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct RunInputs {
    pub initial_state_sha256: String,
    pub prompt_sha256: String,
}

#[derive(Debug, Serialize)]
pub struct RunSelection {
    pub test: String,
    pub harness: String,
    pub model: String,
}

#[derive(Debug, Serialize)]
pub struct RunResolved {
    pub harness: ResolvedHarness,
    pub model: ResolvedModel,
}

#[derive(Debug, Serialize)]
pub struct ResolvedHarness {
    pub image: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ResolvedModel {
    pub model_name: String,
    pub base_url: String,
}

#[derive(Debug, Serialize)]
pub struct RunArtifacts {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working_dir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    pub harness_log: String,
    pub proxy_log: String,
}

#[derive(Debug)]
pub struct RunExecution {
    pub run_id: String,
    pub status: RunStatus,
}

// ── Run metrics (ticket 022) ────────────────────────────────────────────────

/// Aggregated LLM usage metrics derived from proxy.ndjson.
#[derive(Debug, Serialize)]
pub struct RunMetrics {
    /// Total number of generation requests (/v1/responses), excluding discovery.
    pub request_count: u64,
    /// Sum of input_tokens across all requests, or null if any request missing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    /// Sum of output_tokens across all requests, or null if any request missing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    /// Sum of total_tokens across all requests, or null if any request missing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
    /// Sum of cache_read_tokens, or null if any request missing the field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u64>,
    /// Sum of cache_write_tokens, or null if any request missing the field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_write_tokens: Option<u64>,
}

/// Aggregate raw proxy.ndjson records into RunMetrics.
///
/// - Counts all `request_start` records with `kind: "generation"`.
/// - Sums usage from `request_end` records only when **all** generation requests
///   provide the given usage field. If any request is missing a field, the
///   aggregate for that field is `null`.
impl RunMetrics {
    pub fn from_proxy_log(proxy_log_path: &Path) -> Result<Self, String> {
        aggregate_proxy_metrics(proxy_log_path)
    }
}

fn aggregate_proxy_metrics(proxy_log_path: &Path) -> Result<RunMetrics, String> {
    let contents = std::fs::read_to_string(proxy_log_path).map_err(|error| {
        format!(
            "failed to read proxy log {}: {error}",
            proxy_log_path.display()
        )
    })?;

    let records: Vec<Value> = contents
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();

    // Count generation request_start records
    let request_count = records
        .iter()
        .filter(|r| r["record_type"] == "request_start" && r["kind"] == "generation")
        .count() as u64;

    // Collect usage from generation request_end records
    let end_records: Vec<&Value> = records
        .iter()
        .filter(|r| r["record_type"] == "request_end" && r["kind"] == "generation")
        .collect();

    let mut input_tokens: Option<u64> = Some(0);
    let mut output_tokens: Option<u64> = Some(0);
    let mut total_tokens: Option<u64> = Some(0);
    let mut cache_read_tokens: Option<u64> = Some(0);
    let mut cache_write_tokens: Option<u64> = Some(0);

    for record in &end_records {
        let usage = &record["usage"];

        // input_tokens
        if let Some(val) = usage.get("input_tokens").and_then(|v| v.as_u64()) {
            input_tokens = input_tokens.map(|sum| sum + val);
        } else {
            input_tokens = None;
        }

        // output_tokens
        if let Some(val) = usage.get("output_tokens").and_then(|v| v.as_u64()) {
            output_tokens = output_tokens.map(|sum| sum + val);
        } else {
            output_tokens = None;
        }

        // total_tokens
        if let Some(val) = usage.get("total_tokens").and_then(|v| v.as_u64()) {
            total_tokens = total_tokens.map(|sum| sum + val);
        } else {
            total_tokens = None;
        }

        // cache_read_tokens — null or absent means upstream didn't report it
        match usage.get("cache_read_tokens") {
            Some(val) => {
                if let Some(v) = val.as_u64() {
                    cache_read_tokens = cache_read_tokens.map(|sum| sum + v);
                } else {
                    // Present but null or non-numeric
                    cache_read_tokens = None;
                }
            }
            None => {
                // Field absent
                cache_read_tokens = None;
            }
        }

        // cache_write_tokens
        match usage.get("cache_write_tokens") {
            Some(val) => {
                if let Some(v) = val.as_u64() {
                    cache_write_tokens = cache_write_tokens.map(|sum| sum + v);
                } else {
                    cache_write_tokens = None;
                }
            }
            None => {
                cache_write_tokens = None;
            }
        }
    }

    Ok(RunMetrics {
        request_count,
        input_tokens,
        output_tokens,
        total_tokens,
        cache_read_tokens,
        cache_write_tokens,
    })
}

// ── Batch models ────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct BatchSummary {
    pub batch_id: String,
    pub started_at: String,
    pub finished_at: String,
    pub duration_ms: u64,
    pub config_path: String,
    pub runs: Vec<BatchRunReference>,
}

#[derive(Debug, Serialize)]
pub struct BatchRunReference {
    pub run_id: String,
    pub results_path: String,
}

pub fn write_batch_summary(batch_dir: &Path, summary: &BatchSummary) -> Result<(), String> {
    let path = batch_dir.join("summary.json");
    let file = File::create(&path)
        .map_err(|error| format!("failed to create summary file {}: {error}", path.display()))?;
    serde_json::to_writer_pretty(file, summary)
        .map_err(|error| format!("failed to write summary file {}: {error}", path.display()))
}

// ── Run status ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Completed,
    Failed,
    TimedOut,
    SetupFailed,
}

// ── Results writing ─────────────────────────────────────────────────────────

pub fn write_results(run_dir: &Path, result: &RunResult) -> Result<(), String> {
    let path = run_dir.join("results.json");
    let file = File::create(&path)
        .map_err(|error| format!("failed to create results file {}: {error}", path.display()))?;
    serde_json::to_writer_pretty(file, result)
        .map_err(|error| format!("failed to write results file {}: {error}", path.display()))
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn aggregates_complete_usage() {
        let mut log = NamedTempFile::new().expect("create temp log");
        writeln!(log, r#"{{"record_type":"request_start","request_id":"a1","kind":"generation","method":"POST","path":"/v1/responses"}}"#).unwrap();
        writeln!(log, r#"{{"record_type":"request_end","request_id":"a1","kind":"generation","status_code":200,"usage":{{"input_tokens":100,"output_tokens":50,"total_tokens":150,"cache_read_tokens":null,"cache_write_tokens":null}}}}"#).unwrap();
        writeln!(log, r#"{{"record_type":"request_start","request_id":"a2","kind":"generation","method":"POST","path":"/v1/responses"}}"#).unwrap();
        writeln!(log, r#"{{"record_type":"request_end","request_id":"a2","kind":"generation","status_code":200,"usage":{{"input_tokens":200,"output_tokens":100,"total_tokens":300,"cache_read_tokens":null,"cache_write_tokens":null}}}}"#).unwrap();

        let metrics = aggregate_proxy_metrics(log.path()).expect("aggregate");
        assert_eq!(metrics.request_count, 2);
        assert_eq!(metrics.input_tokens, Some(300));
        assert_eq!(metrics.output_tokens, Some(150));
        assert_eq!(metrics.total_tokens, Some(450));
        assert_eq!(metrics.cache_read_tokens, None);
        assert_eq!(metrics.cache_write_tokens, None);
    }

    #[test]
    fn aggregates_partial_usage_produces_null() {
        let mut log = NamedTempFile::new().expect("create temp log");
        writeln!(log, r#"{{"record_type":"request_start","request_id":"b1","kind":"generation","method":"POST","path":"/v1/responses"}}"#).unwrap();
        writeln!(log, r#"{{"record_type":"request_end","request_id":"b1","kind":"generation","status_code":200,"usage":{{"input_tokens":100,"output_tokens":50,"total_tokens":150}}}}"#).unwrap();
        writeln!(log, r#"{{"record_type":"request_start","request_id":"b2","kind":"generation","method":"POST","path":"/v1/responses"}}"#).unwrap();
        // Second request has no usage at all (e.g., error response)
        writeln!(log, r#"{{"record_type":"request_end","request_id":"b2","kind":"generation","status_code":500,"error":"upstream error"}}"#).unwrap();

        let metrics = aggregate_proxy_metrics(log.path()).expect("aggregate");
        assert_eq!(metrics.request_count, 2);
        // All token fields are null because one request is missing usage
        assert_eq!(metrics.input_tokens, None);
        assert_eq!(metrics.output_tokens, None);
        assert_eq!(metrics.total_tokens, None);
    }

    #[test]
    fn excludes_discovery_traffic() {
        let mut log = NamedTempFile::new().expect("create temp log");
        writeln!(log, r#"{{"record_type":"request_start","request_id":"c1","kind":"discovery","method":"GET","path":"/v1/models"}}"#).unwrap();
        writeln!(log, r#"{{"record_type":"request_end","request_id":"c1","kind":"discovery","status_code":200,"usage":null}}"#).unwrap();
        writeln!(log, r#"{{"record_type":"request_start","request_id":"c2","kind":"generation","method":"POST","path":"/v1/responses"}}"#).unwrap();
        writeln!(log, r#"{{"record_type":"request_end","request_id":"c2","kind":"generation","status_code":200,"usage":{{"input_tokens":10,"output_tokens":5,"total_tokens":15}}}}"#).unwrap();

        let metrics = aggregate_proxy_metrics(log.path()).expect("aggregate");
        assert_eq!(metrics.request_count, 1); // Only generation, not discovery
        assert_eq!(metrics.input_tokens, Some(10));
    }

    #[test]
    fn empty_log_produces_zero_counts() {
        let log = NamedTempFile::new().expect("create temp log");
        let metrics = aggregate_proxy_metrics(log.path()).expect("aggregate");
        assert_eq!(metrics.request_count, 0);
        assert_eq!(metrics.input_tokens, Some(0));
        assert_eq!(metrics.output_tokens, Some(0));
        assert_eq!(metrics.total_tokens, Some(0));
    }
}
