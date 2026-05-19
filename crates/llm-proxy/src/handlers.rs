use axum::{
    body::{Body, Bytes},
    extract::State,
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};

use crate::{
    ProxyState,
    log_record,
    is_authorized,
    generate_request_id,
    utc_now,
};

/// Main entry point for POST /v1/responses.
/// Detects streaming requests and routes to the appropriate handler.
pub async fn responses(
    State(state): State<ProxyState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    // Quick auth check before parsing
    if !is_authorized(&headers, &state.api_key) {
        return auth_failure_response(&state, &generate_request_id(), "generation", "POST", "/v1/responses").await;
    }

    // Parse request body to check for streaming
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
            return (
                StatusCode::BAD_REQUEST,
                "request body must be valid JSON",
            )
                .into_response()
        }
    };

    // Check if this is a streaming request
    let is_streaming = payload.get("stream").and_then(Value::as_bool) == Some(true);

    if is_streaming {
        crate::streaming::responses_streaming(
            State(state),
            headers,
            body,
        ).await
    } else {
        responses_non_streaming(
            State(state),
            headers,
            body,
        ).await
    }
}

/// Handle non-streaming POST /v1/responses requests.
async fn responses_non_streaming(
    State(state): State<ProxyState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    // Auth check — log failure without request body
    if !is_authorized(&headers, &state.api_key) {
        return auth_failure_response(&state, &generate_request_id(), "generation", "POST", "/v1/responses").await;
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

/// Write start/end records for an auth failure and return 401.
async fn auth_failure_response(
    state: &ProxyState,
    request_id: &str,
    kind: &str,
    method: &str,
    path: &str,
) -> Response {
    let started_at = utc_now();
    log_record(
        &state.log,
        &json!({
            "record_type": "request_start",
            "request_id": request_id,
            "started_at": started_at,
            "kind": kind,
            "method": method,
            "path": path,
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
            "kind": kind,
            "method": method,
            "path": path,
            "status_code": 401,
            "error": "unauthorized",
        }),
    )
    .await;

    eprintln!("proxy auth failure: {method} {path}");
    StatusCode::UNAUTHORIZED.into_response()
}
