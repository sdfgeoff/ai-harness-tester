use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use axum::{
    body::{Body, Bytes},
    extract::State,
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use reqwest::Client;
use serde_json::{json, Value};
use tokio::{
    io::{AsyncWriteExt, BufWriter},
    net::TcpListener,
    sync::{Mutex, oneshot},
    task::JoinHandle,
};

// ── Public config and handle ────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ProxyConfig {
    pub model_name: String,
    pub upstream_base_url: String,
    pub upstream_api_key: String,
    pub proxy_log_path: std::path::PathBuf,
}

#[derive(Debug)]
pub struct ProxyHandle {
    pub base_url: String,
    pub api_key: String,
    shutdown: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
}

impl ProxyHandle {
    pub async fn shutdown(mut self) -> Result<(), String> {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        self.task
            .await
            .map_err(|error| format!("proxy task failed to join: {error}"))
    }
}

// ── Internal state ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct ProxyState {
    model_name: String,
    upstream_base_url: String,
    upstream_api_key: String,
    api_key: String,
    client: Client,
    log: Arc<Mutex<BufWriter<tokio::fs::File>>>,
}

// ── Proxy log writer ────────────────────────────────────────────────────────

/// Write a single NDJSON line to the shared proxy log.
async fn log_record(log: &Mutex<BufWriter<tokio::fs::File>>, record: &Value) {
    let mut writer = log.lock().await;
    let line = serde_json::to_string(record).expect("serialize log record");
    let _ = writer.write_all(line.as_bytes()).await;
    let _ = writer.write_all(b"\n").await;
    let _ = writer.flush().await;
}

// ── Entry point ─────────────────────────────────────────────────────────────

pub async fn start_proxy(config: ProxyConfig) -> Result<ProxyHandle, String> {
    let api_key = generate_api_key();

    let log_file = tokio::fs::File::create(&config.proxy_log_path)
        .await
        .map_err(|error| {
            format!(
                "failed to create proxy log {}: {error}",
                config.proxy_log_path.display()
            )
        })?;
    let log = Arc::new(Mutex::new(BufWriter::new(log_file)));

    let state = ProxyState {
        model_name: config.model_name,
        upstream_base_url: config.upstream_base_url,
        upstream_api_key: config.upstream_api_key,
        api_key,
        client: Client::new(),
        log,
    };

    let api_key = state.api_key.clone();

    let app = Router::new()
        .route("/v1/models", get(handlers::models))
        .route("/v1/responses", post(handlers::responses))
        .with_state(state);

    let listener = TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0))
        .await
        .map_err(|error| format!("failed to bind proxy listener: {error}"))?;
    let address = listener
        .local_addr()
        .map_err(|error| format!("failed to read proxy listener address: {error}"))?;

    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let task = tokio::spawn(async move {
        let server = axum::serve(listener, app).with_graceful_shutdown(async move {
            let _ = shutdown_rx.await;
        });
        if let Err(error) = server.await {
            eprintln!("proxy server error: {error}");
        }
    });

    Ok(ProxyHandle {
        base_url: format!("http://127.0.0.1:{}", address.port()),
        api_key,
        shutdown: Some(shutdown_tx),
        task,
    })
}

// ── Handlers ────────────────────────────────────────────────────────────────

mod handlers {
    use super::*;

    pub async fn responses(
        State(state): State<ProxyState>,
        headers: HeaderMap,
        body: Bytes,
    ) -> Response {
        // Auth check — log failure without request body
        if !is_authorized(&headers, &state.api_key) {
            let request_id = generate_request_id();
            let started_at = utc_now();
            log_record(
                &state.log,
                &json!({
                    "record_type": "request_start",
                    "request_id": request_id,
                    "started_at": started_at,
                    "kind": "generation",
                    "method": "POST",
                    "path": "/v1/responses",
                }),
            )
            .await;

            log_record(
                &state.log,
                &json!({
                    "record_type": "request_end",
                    "request_id": request_id,
                    "finished_at": utc_now(),
                    "duration_ms": 0,
                    "kind": "generation",
                    "method": "POST",
                    "path": "/v1/responses",
                    "status_code": 401,
                    "error": "unauthorized",
                }),
            )
            .await;

            eprintln!("proxy auth failure: POST /v1/responses");
            return StatusCode::UNAUTHORIZED.into_response();
        }

        // Parse request body
        let mut payload = match serde_json::from_slice::<Value>(&body) {
            Ok(Value::Object(payload)) => payload,
            Ok(_) => {
                return (
                    StatusCode::BAD_REQUEST,
                    "request body must be a JSON object",
                )
                    .into_response()
            }
            Err(_) => {
                return (
                    StatusCode::BAD_REQUEST,
                    "request body must be valid JSON",
                )
                    .into_response()
            }
        };

        // Capture original model before rewrite
        let original_model = payload
            .get("model")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_default();

        // Rewrite model
        payload.insert("model".to_owned(), Value::String(state.model_name.clone()));

        // Write request_start
        let request_id = generate_request_id();
        let started_at = utc_now();
        let start_instant = std::time::Instant::now();

        log_record(
            &state.log,
            &json!({
                "record_type": "request_start",
                "request_id": request_id,
                "started_at": started_at,
                "kind": "generation",
                "method": "POST",
                "path": "/v1/responses",
                "original_model": original_model,
                "upstream_model": state.model_name,
                "request_body": payload,
            }),
        )
        .await;

        // Forward to upstream
        let upstream_url = format!(
            "{}/responses",
            state.upstream_base_url.trim_end_matches('/')
        );
        let mut request = state.client.post(&upstream_url).json(&payload);
        if !state.upstream_api_key.is_empty() {
            request = request.bearer_auth(&state.upstream_api_key);
        }

        let (status, response_body, error) = match request.send().await {
            Ok(response) => {
                let status = response.status().as_u16();
                let response_body = match response.bytes().await {
                    Ok(body) => body,
                    Err(read_error) => {
                        let duration_ms = start_instant.elapsed().as_millis() as u64;
                        log_record(
                            &state.log,
                            &json!({
                                "record_type": "request_end",
                                "request_id": request_id,
                                "finished_at": utc_now(),
                                "duration_ms": duration_ms,
                                "kind": "generation",
                                "method": "POST",
                                "path": "/v1/responses",
                                "original_model": original_model,
                                "upstream_model": state.model_name,
                                "status_code": status,
                                "error": format!("failed to read upstream response body: {read_error}"),
                            }),
                        )
                        .await;
                        return (
                            StatusCode::BAD_GATEWAY,
                            format!("failed to read upstream response body: {read_error}"),
                        )
                            .into_response();
                    }
                };
                (status, response_body, None::<String>)
            }
            Err(send_error) => {
                let duration_ms = start_instant.elapsed().as_millis() as u64;
                log_record(
                    &state.log,
                    &json!({
                        "record_type": "request_end",
                        "request_id": request_id,
                        "finished_at": utc_now(),
                        "duration_ms": duration_ms,
                        "kind": "generation",
                        "method": "POST",
                        "path": "/v1/responses",
                        "original_model": original_model,
                        "upstream_model": state.model_name,
                        "error": format!("failed to reach upstream model endpoint: {send_error}"),
                    }),
                )
                .await;
                return (
                    StatusCode::BAD_GATEWAY,
                    format!("failed to reach upstream model endpoint: {send_error}"),
                )
                    .into_response();
            }
        };

        // Extract usage from response body (best-effort)
        let usage = if let Ok(parsed) = serde_json::from_slice::<Value>(&response_body) {
            let usage_obj = &parsed["usage"];
            if usage_obj.is_object() {
                json!({
                    "input_tokens": usage_obj.get("input_tokens"),
                    "output_tokens": usage_obj.get("output_tokens"),
                    "total_tokens": usage_obj.get("total_tokens"),
                    "cache_read_tokens": usage_obj.get("cache_read_tokens"),
                    "cache_write_tokens": usage_obj.get("cache_write_tokens"),
                })
            } else {
                json!(null)
            }
        } else {
            json!(null)
        };

        // Parse response body for the log record (store as JSON Value if possible)
        let response_body_value = serde_json::from_slice::<Value>(&response_body).unwrap_or_else(
            |_| {
                let text = String::from_utf8_lossy(&response_body).to_string();
                Value::String(text)
            },
        );

        // Write request_end
        let duration_ms = start_instant.elapsed().as_millis() as u64;
        log_record(
            &state.log,
            &json!({
                "record_type": "request_end",
                "request_id": request_id,
                "finished_at": utc_now(),
                "duration_ms": duration_ms,
                "kind": "generation",
                "method": "POST",
                "path": "/v1/responses",
                "original_model": original_model,
                "upstream_model": state.model_name,
                "status_code": status,
                "response_body": response_body_value,
                "usage": usage,
                "error": error,
            }),
        )
        .await;

        // Forward response to harness
        let content_type = response_body_value
            .as_object()
            .map(|_| header::HeaderValue::from_static("application/json"))
            .unwrap_or_else(|| header::HeaderValue::from_static("application/octet-stream"));

        Response::builder()
            .status(StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR))
            .header(header::CONTENT_TYPE, content_type)
            .body(Body::from(response_body))
            .unwrap_or_else(|error| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("failed to build proxy response: {error}"),
                )
                    .into_response()
            })
    }

    pub async fn models(State(state): State<ProxyState>, headers: HeaderMap) -> Response {
        // Auth check — log failure without request body
        if !is_authorized(&headers, &state.api_key) {
            let request_id = generate_request_id();
            let started_at = utc_now();
            log_record(
                &state.log,
                &json!({
                    "record_type": "request_start",
                    "request_id": request_id,
                    "started_at": started_at,
                    "kind": "discovery",
                    "method": "GET",
                    "path": "/v1/models",
                }),
            )
            .await;

            log_record(
                &state.log,
                &json!({
                    "record_type": "request_end",
                    "request_id": request_id,
                    "finished_at": utc_now(),
                    "duration_ms": 0,
                    "kind": "discovery",
                    "method": "GET",
                    "path": "/v1/models",
                    "status_code": 401,
                    "error": "unauthorized",
                }),
            )
            .await;

            eprintln!("proxy auth failure: GET /v1/models");
            return StatusCode::UNAUTHORIZED.into_response();
        }

        // Write request_start (no body for GET)
        let request_id = generate_request_id();
        let started_at = utc_now();
        let start_instant = std::time::Instant::now();

        log_record(
            &state.log,
            &json!({
                "record_type": "request_start",
                "request_id": request_id,
                "started_at": started_at,
                "kind": "discovery",
                "method": "GET",
                "path": "/v1/models",
            }),
        )
        .await;

        let response_body = json!({
            "object": "list",
            "data": [
                {
                    "id": state.model_name,
                    "object": "model"
                }
            ]
        });

        // Write request_end
        let duration_ms = start_instant.elapsed().as_millis() as u64;
        log_record(
            &state.log,
            &json!({
                "record_type": "request_end",
                "request_id": request_id,
                "finished_at": utc_now(),
                "duration_ms": duration_ms,
                "kind": "discovery",
                "method": "GET",
                "path": "/v1/models",
                "status_code": 200,
                "response_body": response_body,
                "usage": null,
                "error": null,
            }),
        )
        .await;

        Json(response_body).into_response()
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn is_authorized(headers: &HeaderMap, api_key: &str) -> bool {
    let Some(value) = headers.get(header::AUTHORIZATION) else {
        return false;
    };
    let Ok(value) = value.to_str() else {
        return false;
    };
    value == format!("Bearer {api_key}")
}

fn generate_api_key() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!("harness-test-{nanos:x}")
}

fn generate_request_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let random = fastrand::u64(0..u64::MAX);
    format!("{:016x}{:016x}", nanos, random)
}

fn utc_now() -> String {
    use time::format_description::well_known::Rfc3339;
    time::OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "unknown".to_owned())
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener as StdTcpListener,
        sync::mpsc,
        thread,
    };

    #[tokio::test]
    async fn models_requires_auth() {
        let tmp = tempfile::NamedTempFile::new().expect("create temp log file");
        let proxy = start_proxy(ProxyConfig {
            model_name: "smoke-local".to_owned(),
            upstream_base_url: "http://127.0.0.1:1/v1".to_owned(),
            upstream_api_key: "local".to_owned(),
            proxy_log_path: tmp.path().to_owned(),
        })
        .await
        .expect("start proxy");

        let url = format!("{}/v1/models", proxy.base_url);
        let response = tokio::task::spawn_blocking(move || ureq::get(&url).call())
            .await
            .expect("blocking request task");
        match response.unwrap_err() {
            ureq::Error::Status(status, _) => assert_eq!(status, 401),
            error => panic!("expected 401 status error, got {error}"),
        }

        proxy.shutdown().await.expect("shutdown proxy");

        // Verify auth failure was logged without request body
        let log_contents = std::fs::read_to_string(tmp.path()).expect("read log");
        let records: Vec<Value> = log_contents
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();
        assert_eq!(records.len(), 2, "expected start + end records");
        assert_eq!(records[0]["record_type"], "request_start");
        assert_eq!(records[0]["kind"], "discovery");
        assert_eq!(records[1]["record_type"], "request_end");
        assert_eq!(records[1]["status_code"], 401);
        assert_eq!(records[1]["error"], "unauthorized");
        // No request_body on auth failure
        assert!(records[0].get("request_body").is_none());
    }

    #[tokio::test]
    async fn models_returns_selected_model() {
        let tmp = tempfile::NamedTempFile::new().expect("create temp log file");
        let proxy = start_proxy(ProxyConfig {
            model_name: "smoke-local".to_owned(),
            upstream_base_url: "http://127.0.0.1:1/v1".to_owned(),
            upstream_api_key: "local".to_owned(),
            proxy_log_path: tmp.path().to_owned(),
        })
        .await
        .expect("start proxy");

        let url = format!("{}/v1/models", proxy.base_url);
        let api_key = proxy.api_key.clone();
        let response = tokio::task::spawn_blocking(move || {
            ureq::get(&url)
                .set("Authorization", &format!("Bearer {api_key}"))
                .call()
                .expect("models response")
                .into_json::<serde_json::Value>()
                .expect("json response")
        })
        .await
        .expect("blocking request task");

        assert_eq!(response["data"][0]["id"], "smoke-local");

        proxy.shutdown().await.expect("shutdown proxy");

        // Verify NDJSON log has linked start/end records
        let log_contents = std::fs::read_to_string(tmp.path()).expect("read log");
        let records: Vec<Value> = log_contents
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();
        assert_eq!(records.len(), 2, "expected start + end records");

        let start = &records[0];
        let end = &records[1];

        assert_eq!(start["record_type"], "request_start");
        assert_eq!(end["record_type"], "request_end");
        assert_eq!(start["kind"], "discovery");
        assert_eq!(end["kind"], "discovery");
        // Linked by request_id
        assert_eq!(start["request_id"], end["request_id"]);
        assert_ne!(start["request_id"], Value::String("".to_owned()));
        assert_eq!(end["status_code"], 200);
    }

    #[tokio::test]
    async fn responses_rewrites_model_and_preserves_fields() {
        let upstream = MockUpstream::start();
        let tmp = tempfile::NamedTempFile::new().expect("create temp log file");
        let proxy = start_proxy(ProxyConfig {
            model_name: "smoke-local".to_owned(),
            upstream_base_url: upstream.base_url.clone(),
            upstream_api_key: "local".to_owned(),
            proxy_log_path: tmp.path().to_owned(),
        })
        .await
        .expect("start proxy");

        let url = format!("{}/v1/responses", proxy.base_url);
        let api_key = proxy.api_key.clone();
        let response = tokio::task::spawn_blocking(move || {
            ureq::post(&url)
                .set("Authorization", &format!("Bearer {api_key}"))
                .send_json(serde_json::json!({
                    "model": "original-model",
                    "input": "hello"
                }))
                .expect("responses response")
                .into_json::<serde_json::Value>()
                .expect("json response")
        })
        .await
        .expect("blocking request task");

        assert_eq!(response["id"], "response-id");
        let upstream_body = upstream.body.recv().expect("upstream body");
        assert_eq!(upstream_body["model"], "smoke-local");
        assert_eq!(upstream_body["input"], "hello");

        proxy.shutdown().await.expect("shutdown proxy");

        // Verify NDJSON log
        let log_contents = std::fs::read_to_string(tmp.path()).expect("read log");
        let records: Vec<Value> = log_contents
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();
        assert_eq!(records.len(), 2, "expected start + end records");

        let start = &records[0];
        let end = &records[1];

        // request_start
        assert_eq!(start["record_type"], "request_start");
        assert_eq!(start["kind"], "generation");
        assert_eq!(start["method"], "POST");
        assert_eq!(start["path"], "/v1/responses");
        assert_eq!(start["original_model"], "original-model");
        assert_eq!(start["upstream_model"], "smoke-local");
        // Request body on start
        assert!(start.get("request_body").is_some());
        assert_eq!(start["request_body"]["model"], "smoke-local"); // rewritten
        assert_eq!(start["request_body"]["input"], "hello");

        // request_end
        assert_eq!(end["record_type"], "request_end");
        assert_eq!(end["kind"], "generation");
        assert_eq!(end["status_code"], 200);
        assert!(end.get("response_body").is_some());
        assert_eq!(end["response_body"]["id"], "response-id");
        // Usage extracted from upstream response
        assert_eq!(end["usage"]["input_tokens"], 1);
        assert_eq!(end["usage"]["output_tokens"], 2);
        assert_eq!(end["usage"]["total_tokens"], 3);
        assert_eq!(end["error"], Value::Null);
        // Linked by request_id
        assert_eq!(start["request_id"], end["request_id"]);
        // Timing
        assert!(end["duration_ms"].as_u64().is_some());
    }

    #[tokio::test]
    async fn responses_auth_failure_omits_request_body() {
        let tmp = tempfile::NamedTempFile::new().expect("create temp log file");
        let proxy = start_proxy(ProxyConfig {
            model_name: "smoke-local".to_owned(),
            upstream_base_url: "http://127.0.0.1:1/v1".to_owned(),
            upstream_api_key: "local".to_owned(),
            proxy_log_path: tmp.path().to_owned(),
        })
        .await
        .expect("start proxy");

        let url = format!("{}/v1/responses", proxy.base_url);
        let response = tokio::task::spawn_blocking(move || {
            ureq::post(&url)
                .send_json(serde_json::json!({
                    "model": "original-model",
                    "input": "secret data"
                }))
        })
        .await
        .expect("blocking request task");
        match response.unwrap_err() {
            ureq::Error::Status(status, _) => assert_eq!(status, 401),
            error => panic!("expected 401 status error, got {error}"),
        }

        proxy.shutdown().await.expect("shutdown proxy");

        let log_contents = std::fs::read_to_string(tmp.path()).expect("read log");
        let records: Vec<Value> = log_contents
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();
        assert_eq!(records.len(), 2);
        // No request_body on either record
        assert!(records[0].get("request_body").is_none());
        assert!(records[1].get("request_body").is_none());
        assert_eq!(records[1]["error"], "unauthorized");
    }

    // ── Mock upstream ─────────────────────────────────────────────────────

    struct MockUpstream {
        base_url: String,
        body: mpsc::Receiver<serde_json::Value>,
    }

    impl MockUpstream {
        fn start() -> Self {
            let listener = StdTcpListener::bind("127.0.0.1:0").expect("bind mock upstream");
            let address = listener.local_addr().expect("mock upstream address");
            let (tx, rx) = mpsc::channel();
            thread::spawn(move || {
                let (mut stream, _) = listener.accept().expect("accept upstream request");
                let mut request = Vec::new();
                let mut buffer = [0u8; 1024];
                loop {
                    let read = stream.read(&mut buffer).expect("read request");
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..read]);
                    if request.windows(4).any(|window| window == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&request);
                        let content_length = headers
                            .lines()
                            .find_map(|line| {
                                line.strip_prefix("content-length:")
                                    .or_else(|| line.strip_prefix("Content-Length:"))
                            })
                            .and_then(|value| value.trim().parse::<usize>().ok())
                            .unwrap_or(0);
                        let header_end = request
                            .windows(4)
                            .position(|window| window == b"\r\n\r\n")
                            .expect("header end")
                            + 4;
                        while request.len() < header_end + content_length {
                            let read = stream.read(&mut buffer).expect("read body");
                            if read == 0 {
                                break;
                            }
                            request.extend_from_slice(&buffer[..read]);
                        }
                        let body = &request[header_end..header_end + content_length];
                        tx.send(serde_json::from_slice(body).expect("request json"))
                            .expect("send body");
                        break;
                    }
                }
                let body = br#"{"id":"response-id","usage":{"input_tokens":1,"output_tokens":2,"total_tokens":3}}"#;
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
                    body.len()
                )
                .expect("write headers");
                stream.write_all(body).expect("write body");
            });

            Self {
                base_url: format!("http://{address}/v1"),
                body: rx,
            }
        }
    }
}
