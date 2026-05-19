use std::time::Instant;

use axum::{
    body::{Body, Bytes},
    extract::State,
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde_json::{json, Value};

use crate::{
    ProxyState,
    log_record,
    is_authorized,
    generate_request_id,
    utc_now,
};

/// Main entry point for POST /v1/chat/completions.
pub async fn chat_completions(
    State(state): State<ProxyState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !is_authorized(&headers, &state.api_key) {
        return auth_failure_response(&state, &generate_request_id(), "generation", "POST", "/v1/chat/completions").await;
    }

    let payload = match serde_json::from_slice::<Value>(&body) {
        Ok(Value::Object(payload)) => payload,
        Ok(_) => return (StatusCode::BAD_REQUEST, "request body must be a JSON object").into_response(),
        Err(_) => return (StatusCode::BAD_REQUEST, "request body must be valid JSON").into_response(),
    };

    let is_streaming = payload.get("stream").and_then(Value::as_bool) == Some(true);

    if is_streaming {
        chat_completions_streaming(State(state), headers, body).await
    } else {
        chat_completions_non_streaming(State(state), headers, body).await
    }
}

/// Handle non-streaming POST /v1/chat/completions.
async fn chat_completions_non_streaming(
    State(state): State<ProxyState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !is_authorized(&headers, &state.api_key) {
        return auth_failure_response(&state, &generate_request_id(), "generation", "POST", "/v1/chat/completions").await;
    }

    let mut payload = match serde_json::from_slice::<Value>(&body) {
        Ok(Value::Object(payload)) => payload,
        Ok(_) => return (StatusCode::BAD_REQUEST, "request body must be a JSON object").into_response(),
        Err(_) => return (StatusCode::BAD_REQUEST, "request body must be valid JSON").into_response(),
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
            "path": "/v1/chat/completions",
            "original_model": original_model,
            "upstream_model": state.model_name,
            "request_body": payload,
        }),
    )
    .await;

    let upstream_url = format!(
        "{}/chat/completions",
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
                    log_request_end(&state.log, &request_id, start_instant.elapsed().as_millis() as u64,
                        "generation", "POST", "/v1/chat/completions", &original_model, &state.model_name,
                        status, None, None, Some(&format!("failed to read upstream response body: {read_error}"))).await;
                    return (StatusCode::BAD_GATEWAY, format!("failed to read upstream response body: {read_error}")).into_response();
                }
            };
            (status, response_body, None::<String>)
        }
        Err(send_error) => {
            log_request_end(&state.log, &request_id, start_instant.elapsed().as_millis() as u64,
                "generation", "POST", "/v1/chat/completions", &original_model, &state.model_name,
                0, None, None, Some(&format!("failed to reach upstream model endpoint: {send_error}"))).await;
            return (StatusCode::BAD_GATEWAY, format!("failed to reach upstream model endpoint: {send_error}")).into_response();
        }
    };

    // Extract usage from chat/completions response
    let usage = if let Ok(parsed) = serde_json::from_slice::<Value>(&response_body) {
        if let Some(usage_obj) = parsed.get("usage").and_then(Value::as_object) {
            json!({
                "input_tokens": usage_obj.get("prompt_tokens"),
                "output_tokens": usage_obj.get("completion_tokens"),
                "total_tokens": usage_obj.get("total_tokens"),
                "cache_read_tokens": usage_obj.get("prompt_tokens_details").and_then(|v| v.get("cached_tokens")),
                "cache_write_tokens": usage_obj.get("completion_tokens_details").and_then(|v| v.get("cached_tokens")),
            })
        } else {
            json!(null)
        }
    } else {
        json!(null)
    };

    let response_body_value = serde_json::from_slice::<Value>(&response_body).unwrap_or_else(
        |_| Value::String(String::from_utf8_lossy(&response_body).to_string()),
    );

    log_request_end(&state.log, &request_id, start_instant.elapsed().as_millis() as u64,
        "generation", "POST", "/v1/chat/completions", &original_model, &state.model_name,
        status, Some(&response_body_value), Some(&usage), error.as_ref().map(|x| x.as_str())).await;

    let content_type = response_body_value
        .as_object()
        .map(|_| header::HeaderValue::from_static("application/json"))
        .unwrap_or_else(|| header::HeaderValue::from_static("application/octet-stream"));

    Response::builder()
        .status(StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR))
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from(response_body))
        .unwrap_or_else(|error| (StatusCode::INTERNAL_SERVER_ERROR, format!("failed to build proxy response: {error}")).into_response())
}

/// Handle streaming POST /v1/chat/completions.
pub async fn chat_completions_streaming(
    State(state): State<ProxyState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !is_authorized(&headers, &state.api_key) {
        let request_id = generate_request_id();
        log_request_end(&state.log, &request_id, 0, "generation", "POST", "/v1/chat/completions",
            "", "", 401, None, None, Some("unauthorized")).await;
        return StatusCode::UNAUTHORIZED.into_response();
    }

    let mut payload = match serde_json::from_slice::<Value>(&body) {
        Ok(Value::Object(payload)) => payload,
        Ok(_) => return (StatusCode::BAD_REQUEST, "request body must be a JSON object").into_response(),
        Err(_) => return (StatusCode::BAD_REQUEST, "request body must be valid JSON").into_response(),
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
            "path": "/v1/chat/completions",
            "original_model": original_model,
            "upstream_model": state.model_name,
            "request_body": payload,
        }),
    )
    .await;

    let upstream_url = format!(
        "{}/chat/completions",
        state.upstream_base_url.trim_end_matches('/')
    );
    let mut request = state.client.post(&upstream_url).json(&payload);
    if !state.upstream_api_key.is_empty() {
        request = request.bearer_auth(&state.upstream_api_key);
    }

    let response = match request.send().await {
        Ok(response) => response,
        Err(send_error) => {
            log_request_end(&state.log, &request_id, start_instant.elapsed().as_millis() as u64,
                "generation", "POST", "/v1/chat/completions", &original_model, &state.model_name,
                0, None, None, Some(&format!("failed to reach upstream model endpoint: {send_error}"))).await;
            return (StatusCode::BAD_GATEWAY, format!("failed to reach upstream model endpoint: {send_error}")).into_response();
        }
    };

    crate::streaming::forward_sse(
        response,
        &state,
        request_id,
        "/v1/chat/completions",
        original_model,
        crate::streaming::ApiKind::ChatCompletions,
        start_instant,
    )
    .await
}

async fn auth_failure_response(
    state: &ProxyState,
    request_id: &str,
    kind: &str,
    method: &str,
    path: &str,
) -> Response {
    log_record(&state.log, &json!({
        "record_type": "request_start",
        "request_id": request_id,
        "started_at": utc_now(),
        "kind": kind,
        "method": method,
        "path": path,
    })).await;
    log_request_end(&state.log, request_id, 0, kind, method, path, "", "", 401, None, None, Some("unauthorized")).await;
    StatusCode::UNAUTHORIZED.into_response()
}

/// Helper to write a request_end record.
async fn log_request_end(
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
    if let Some(body) = response_body { record["response_body"] = body.clone(); }
    if let Some(u) = usage { record["usage"] = u.clone(); }
    if let Some(e) = error { record["error"] = Value::String(e.to_owned()); }
    else { record["error"] = Value::Null; }
    log_record(log, &record).await;
}

#[cfg(test)]
mod tests;
