use std::time::Instant;

use axum::{
    body::{Body, Bytes},
    extract::State,
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use futures_util::StreamExt;
use serde_json::{json, Value};

use crate::{generate_request_id, is_authorized, log_record, utc_now, ProxyState};

/// Main entry point for POST /v1/messages.
pub async fn messages(
    State(state): State<ProxyState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !is_authorized(&headers, &state.api_key) {
        return auth_failure(&state, &generate_request_id()).await;
    }

    let payload = match serde_json::from_slice::<Value>(&body) {
        Ok(Value::Object(payload)) => payload,
        Ok(_) => {
            return (
                StatusCode::BAD_REQUEST,
                "request body must be a JSON object",
            )
                .into_response()
        }
        Err(_) => {
            return (StatusCode::BAD_REQUEST, "request body must be valid JSON").into_response()
        }
    };

    if payload.get("stream").and_then(Value::as_bool) == Some(true) {
        messages_streaming(State(state), headers, body).await
    } else {
        messages_non_streaming(State(state), headers, body).await
    }
}

async fn messages_non_streaming(
    State(state): State<ProxyState>,
    _headers: HeaderMap,
    body: Bytes,
) -> Response {
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
            return (StatusCode::BAD_REQUEST, "request body must be valid JSON").into_response()
        }
    };

    let original_model = payload
        .get("model")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_default();
    payload.insert("model".to_owned(), Value::String(state.model_name.clone()));

    let request_id = generate_request_id();
    let started_at = utc_now();
    let start_instant = Instant::now();

    log_record(
        &state.log,
        &json!({
            "record_type": "request_start",
            "request_id": request_id,
            "started_at": started_at,
            "kind": "generation",
            "method": "POST",
            "path": "/v1/messages",
            "original_model": original_model,
            "upstream_model": state.model_name,
            "request_body": &payload,
        }),
    )
    .await;

    let upstream_url = format!("{}/messages", state.upstream_base_url.trim_end_matches('/'));
    let mut request = state.client.post(&upstream_url).json(&payload);
    if !state.upstream_api_key.is_empty() {
        request = request.bearer_auth(&state.upstream_api_key);
    }

    let response = match request.send().await {
        Ok(r) => r,
        Err(e) => {
            log_end(
                &state.log,
                &request_id,
                start_instant.elapsed().as_millis() as u64,
                "generation",
                "POST",
                "/v1/messages",
                &original_model,
                &state.model_name,
                0,
                None,
                None,
                Some(&format!("failed to reach upstream: {e}")),
            )
            .await;
            return (
                StatusCode::BAD_GATEWAY,
                format!("failed to reach upstream: {e}"),
            )
                .into_response();
        }
    };

    let status = response.status().as_u16();
    let response_body = match response.bytes().await {
        Ok(b) => b,
        Err(e) => {
            log_end(
                &state.log,
                &request_id,
                start_instant.elapsed().as_millis() as u64,
                "generation",
                "POST",
                "/v1/messages",
                &original_model,
                &state.model_name,
                status,
                None,
                None,
                Some(&format!("failed to read upstream: {e}")),
            )
            .await;
            return (
                StatusCode::BAD_GATEWAY,
                format!("failed to read upstream: {e}"),
            )
                .into_response();
        }
    };

    // Extract usage (best-effort, works with both Anthropic and OpenAI format)
    let usage = extract_usage(&response_body);

    let response_value = serde_json::from_slice::<Value>(&response_body)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&response_body).to_string()));

    log_end(
        &state.log,
        &request_id,
        start_instant.elapsed().as_millis() as u64,
        "generation",
        "POST",
        "/v1/messages",
        &original_model,
        &state.model_name,
        status,
        Some(&response_value),
        usage.as_ref(),
        None,
    )
    .await;

    let content_type = response_value
        .as_object()
        .map(|_| header::HeaderValue::from_static("application/json"))
        .unwrap_or_else(|| header::HeaderValue::from_static("application/octet-stream"));

    Response::builder()
        .status(StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR))
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from(response_body))
        .unwrap_or_else(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to build response: {e}"),
            )
                .into_response()
        })
}

async fn messages_streaming(
    State(state): State<ProxyState>,
    _headers: HeaderMap,
    body: Bytes,
) -> Response {
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
            return (StatusCode::BAD_REQUEST, "request body must be valid JSON").into_response()
        }
    };

    let original_model = payload
        .get("model")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_default();
    payload.insert("model".to_owned(), Value::String(state.model_name.clone()));

    let request_id = generate_request_id();
    let started_at = utc_now();
    let start_instant = Instant::now();

    log_record(
        &state.log,
        &json!({
            "record_type": "request_start",
            "request_id": request_id,
            "started_at": started_at,
            "kind": "generation",
            "method": "POST",
            "path": "/v1/messages",
            "original_model": original_model,
            "upstream_model": state.model_name,
            "request_body": &payload,
        }),
    )
    .await;

    let upstream_url = format!("{}/messages", state.upstream_base_url.trim_end_matches('/'));
    let mut request = state.client.post(&upstream_url).json(&payload);
    if !state.upstream_api_key.is_empty() {
        request = request.bearer_auth(&state.upstream_api_key);
    }

    let response = match request.send().await {
        Ok(r) => r,
        Err(e) => {
            log_end(
                &state.log,
                &request_id,
                start_instant.elapsed().as_millis() as u64,
                "generation",
                "POST",
                "/v1/messages",
                &original_model,
                &state.model_name,
                0,
                None,
                None,
                Some(&format!("failed to reach upstream: {e}")),
            )
            .await;
            return (
                StatusCode::BAD_GATEWAY,
                format!("failed to reach upstream: {e}"),
            )
                .into_response();
        }
    };

    forward_sse(response, &state, request_id, original_model, start_instant).await
}

async fn forward_sse(
    response: reqwest::Response,
    state: &ProxyState,
    request_id: String,
    original_model: String,
    start_instant: Instant,
) -> Response {
    let status = response.status().as_u16();
    if status != 200 {
        let error_body = response.bytes().await.unwrap_or_default();
        let error_text = String::from_utf8_lossy(&error_body).to_string();
        log_end(
            &state.log,
            &request_id,
            start_instant.elapsed().as_millis() as u64,
            "generation",
            "POST",
            "/v1/messages",
            &original_model,
            &state.model_name,
            status,
            None,
            None,
            Some(&error_text),
        )
        .await;
        return Response::builder()
            .status(StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR))
            .header("content-type", "application/json")
            .body(Body::from(error_body))
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
    }

    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::convert::Infallible>>(32);
    let log = state.log.clone();
    let request_id_clone = request_id.clone();
    let original_model_clone = original_model.clone();
    let upstream_model = state.model_name.clone();
    let start_instant_clone = start_instant.clone();

    tokio::spawn(async move {
        let mut usage: Option<Value> = None;

        let mut stream = response.bytes_stream();
        while let Some(result) = stream.next().await {
            match result {
                Ok(chunk) => {
                    // Log raw SSE data lines for telemetry
                    for line in String::from_utf8_lossy(&chunk).lines() {
                        if line.starts_with("data: ") {
                            let data_raw = &line[6..];
                            if data_raw.is_empty() || data_raw == "[DONE]" {
                                continue;
                            }
                            log_record(
                                &log,
                                &json!({
                                    "record_type": "stream_event",
                                    "request_id": request_id_clone,
                                    "received_at": utc_now(),
                                    "data_raw": data_raw,
                                }),
                            )
                            .await;
                            // Try to extract usage from any data chunk
                            if let Ok(parsed) = serde_json::from_str::<Value>(data_raw) {
                                if let Some(u) = parsed.get("usage") {
                                    usage = Some(u.clone());
                                }
                            }
                        }
                    }
                    let _ = tx.send(Ok(chunk)).await;
                }
                Err(e) => {
                    tracing::error!(error = %e, "stream read error");
                    break;
                }
            }
        }
        log_end(
            &log,
            &request_id_clone,
            start_instant_clone.elapsed().as_millis() as u64,
            "generation",
            "POST",
            "/v1/messages",
            &original_model_clone,
            &upstream_model,
            status,
            None,
            usage.as_ref(),
            None,
        )
        .await;
    });

    Response::builder()
        .header("content-type", "text/event-stream; charset=utf-8")
        .header("cache-control", "no-cache")
        .header("connection", "keep-alive")
        .body(Body::from_stream(
            crate::streaming::ReceiverStreamCompat::new(rx),
        ))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// Extract usage from response body, handling both Anthropic and OpenAI formats.
fn extract_usage(response_body: &[u8]) -> Option<Value> {
    let parsed = serde_json::from_slice::<Value>(response_body).ok()?;
    let usage = parsed.get("usage")?;
    let obj = usage.as_object()?;

    // Anthropic format
    if obj.contains_key("input_tokens") {
        return Some(json!({
            "input_tokens": obj.get("input_tokens"),
            "output_tokens": obj.get("output_tokens"),
            "total_tokens": obj.get("total_tokens"),
            "cache_read_tokens": obj.get("cache_read_tokens"),
            "cache_write_tokens": obj.get("cache_write_tokens"),
        }));
    }

    // OpenAI format
    if obj.contains_key("prompt_tokens") {
        return Some(json!({
            "input_tokens": obj.get("prompt_tokens"),
            "output_tokens": obj.get("completion_tokens"),
            "total_tokens": obj.get("total_tokens"),
            "cache_read_tokens": obj.get("prompt_tokens_details").and_then(|v| v.get("cached_tokens")),
            "cache_write_tokens": obj.get("completion_tokens_details").and_then(|v| v.get("cached_tokens")),
        }));
    }

    Some(usage.clone())
}

async fn auth_failure(state: &ProxyState, request_id: &str) -> Response {
    log_record(
        &state.log,
        &json!({
            "record_type": "request_start",
            "request_id": request_id,
            "started_at": utc_now(),
            "kind": "generation",
            "method": "POST",
            "path": "/v1/messages",
        }),
    )
    .await;
    log_end(
        &state.log,
        request_id,
        0,
        "generation",
        "POST",
        "/v1/messages",
        "",
        "",
        401,
        None,
        None,
        Some("unauthorized"),
    )
    .await;
    StatusCode::UNAUTHORIZED.into_response()
}

async fn log_end(
    log: &tokio::sync::Mutex<tokio::io::BufWriter<tokio::fs::File>>,
    request_id: &str,
    duration_ms: u64,
    kind: &str,
    method: &str,
    path: &str,
    original_model: &str,
    upstream_model: &str,
    status_code: u16,
    response_body: Option<&Value>,
    usage: Option<&Value>,
    error: Option<&str>,
) {
    let mut record = json!({
        "record_type": "request_end",
        "request_id": request_id,
        "finished_at": utc_now(),
        "duration_ms": duration_ms,
        "kind": kind,
        "method": method,
        "path": path,
        "original_model": original_model,
        "upstream_model": upstream_model,
        "status_code": status_code,
    });
    if let Some(body) = response_body {
        if let Some(obj) = record.as_object_mut() {
            obj.insert("response_body".to_owned(), body.clone());
        }
    }
    if let Some(u) = usage {
        if let Some(obj) = record.as_object_mut() {
            obj.insert("usage".to_owned(), u.clone());
        }
    } else {
        if let Some(obj) = record.as_object_mut() {
            obj.insert("usage".to_owned(), Value::Null);
        }
    }
    if let Some(e) = error {
        if let Some(obj) = record.as_object_mut() {
            obj.insert("error".to_owned(), Value::String(e.to_owned()));
        }
    } else {
        if let Some(obj) = record.as_object_mut() {
            obj.insert("error".to_owned(), Value::Null);
        }
    }
    log_record(log, &record).await;
}
