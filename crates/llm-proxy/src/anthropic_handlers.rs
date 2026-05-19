use std::time::Instant;

use axum::{
    body::{Body, Bytes},
    extract::State,
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use futures_util::StreamExt;
use serde_json::{json, Value};

use crate::{
    ProxyState,
    log_record,
    is_authorized,
    generate_request_id,
    utc_now,
};

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
        Ok(_) => return (StatusCode::BAD_REQUEST, "request body must be a JSON object").into_response(),
        Err(_) => return (StatusCode::BAD_REQUEST, "request body must be valid JSON").into_response(),
    };

    if payload.get("stream").and_then(Value::as_bool) == Some(true) {
        messages_streaming(State(state), headers, body).await
    } else {
        messages_non_streaming(State(state), headers, body).await
    }
}

async fn messages_non_streaming(
    State(state): State<ProxyState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !is_authorized(&headers, &state.api_key) {
        return auth_failure(&state, &generate_request_id()).await;
    }

    let mut payload = match serde_json::from_slice::<Value>(&body) {
        Ok(Value::Object(payload)) => payload,
        Ok(_) => return (StatusCode::BAD_REQUEST, "request body must be a JSON object").into_response(),
        Err(_) => return (StatusCode::BAD_REQUEST, "request body must be valid JSON").into_response(),
    };

    let original_model = payload.get("model").and_then(Value::as_str).map(str::to_owned).unwrap_or_default();
    payload.insert("model".to_owned(), Value::String(state.model_name.clone()));

    let request_id = generate_request_id();
    let started_at = utc_now();
    let start_instant = Instant::now();

    log_record(&state.log, &json!({
        "record_type": "request_start",
        "request_id": request_id,
        "started_at": started_at,
        "kind": "generation",
        "method": "POST",
        "path": "/v1/messages",
        "original_model": original_model,
        "upstream_model": state.model_name,
        "request_body": payload,
    })).await;

    let openai_payload = anthropic_to_openai(&payload);
    let upstream_url = format!("{}/chat/completions", state.upstream_base_url.trim_end_matches('/'));
    let mut request = state.client.post(&upstream_url).json(&openai_payload);
    if !state.upstream_api_key.is_empty() {
        request = request.bearer_auth(&state.upstream_api_key);
    }

    let (status, response_body, error) = match request.send().await {
        Ok(response) => {
            let status = response.status().as_u16();
            match response.bytes().await {
                Ok(body) => (status, body, None::<String>),
                Err(e) => {
                    log_end(&state.log, &request_id, start_instant.elapsed().as_millis() as u64,
                        "generation", "POST", "/v1/messages", &original_model, &state.model_name,
                        status, None, None, Some(&format!("failed to read upstream: {e}"))).await;
                    return (StatusCode::BAD_GATEWAY, format!("failed to read upstream: {e}")).into_response();
                }
            }
        }
        Err(e) => {
            log_end(&state.log, &request_id, start_instant.elapsed().as_millis() as u64,
                "generation", "POST", "/v1/messages", &original_model, &state.model_name,
                0, None, None, Some(&format!("failed to reach upstream: {e}"))).await;
            return (StatusCode::BAD_GATEWAY, format!("failed to reach upstream: {e}")).into_response();
        }
    };

    let usage = extract_openai_usage(&response_body);
    let anthropic_body = openai_to_anthropic(&response_body, &original_model);
    let anthropic_value = serde_json::from_slice::<Value>(&anthropic_body).unwrap_or_else(
        |_| Value::String(String::from_utf8_lossy(&anthropic_body).to_string()),
    );

    log_end(&state.log, &request_id, start_instant.elapsed().as_millis() as u64,
        "generation", "POST", "/v1/messages", &original_model, &state.model_name,
        status, Some(&anthropic_value), usage.as_ref(), error.as_ref().map(|s| s.as_str())).await;

    let content_type = anthropic_value.as_object()
        .map(|_| header::HeaderValue::from_static("application/json"))
        .unwrap_or_else(|| header::HeaderValue::from_static("application/octet-stream"));

    Response::builder()
        .status(StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR))
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from(anthropic_body))
        .unwrap_or_else(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("failed to build response: {e}")).into_response())
}

async fn messages_streaming(
    State(state): State<ProxyState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !is_authorized(&headers, &state.api_key) {
        return auth_failure(&state, &generate_request_id()).await;
    }

    let mut payload = match serde_json::from_slice::<Value>(&body) {
        Ok(Value::Object(payload)) => payload,
        Ok(_) => return (StatusCode::BAD_REQUEST, "request body must be a JSON object").into_response(),
        Err(_) => return (StatusCode::BAD_REQUEST, "request body must be valid JSON").into_response(),
    };

    let original_model = payload.get("model").and_then(Value::as_str).map(str::to_owned).unwrap_or_default();
    payload.insert("model".to_owned(), Value::String(state.model_name.clone()));

    let request_id = generate_request_id();
    let started_at = utc_now();
    let start_instant = Instant::now();

    log_record(&state.log, &json!({
        "record_type": "request_start",
        "request_id": request_id,
        "started_at": started_at,
        "kind": "generation",
        "method": "POST",
        "path": "/v1/messages",
        "original_model": original_model,
        "upstream_model": state.model_name,
        "request_body": payload,
    })).await;

    let mut openai_payload = anthropic_to_openai(&payload);
    openai_payload["stream"] = Value::Bool(true);
    openai_payload["stream_options"] = json!({"include_usage": true});

    let upstream_url = format!("{}/chat/completions", state.upstream_base_url.trim_end_matches('/'));
    let mut request = state.client.post(&upstream_url).json(&openai_payload);
    if !state.upstream_api_key.is_empty() {
        request = request.bearer_auth(&state.upstream_api_key);
    }

    let response = match request.send().await {
        Ok(r) => r,
        Err(e) => {
            log_end(&state.log, &request_id, start_instant.elapsed().as_millis() as u64,
                "generation", "POST", "/v1/messages", &original_model, &state.model_name,
                0, None, None, Some(&format!("failed to reach upstream: {e}"))).await;
            return (StatusCode::BAD_GATEWAY, format!("failed to reach upstream: {e}")).into_response();
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
        log_end(&state.log, &request_id, start_instant.elapsed().as_millis() as u64,
            "generation", "POST", "/v1/messages", &original_model, &state.model_name,
            status, None, None, Some(&error_text)).await;
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
        let mut buffer = String::new();
        let mut content_index: usize = 0;

        let mut stream = response.bytes_stream();
        while let Some(Ok(chunk)) = stream.next().await {
            buffer.push_str(&String::from_utf8_lossy(&chunk).into_owned());
            loop {
                let frame_end = match buffer.find("\n\n") { Some(p) => p + 2, None => break };
                let frame = buffer[..frame_end].to_string();
                buffer = buffer[frame_end..].to_string();

                let transformed = crate::anthropic_sse::transform_openai_sse_to_anthropic(&frame, &original_model, &mut content_index);

                for line in transformed.lines() {
                    if line.starts_with("data: ") {
                        let data_raw = &line[6..];
                        if data_raw == "[DONE]" { continue; }
                        log_record(&log, &json!({
                            "record_type": "stream_event",
                            "request_id": request_id_clone,
                            "received_at": utc_now(),
                            "event": "",
                            "data_raw": data_raw,
                        })).await;
                        if let Ok(parsed) = serde_json::from_str::<Value>(data_raw) {
                            if let Some(u) = parsed.get("usage").and_then(|v| v.as_object()) {
                                usage = Some(extract_anthropic_usage(u));
                            }
                        }
                    }
                }
                let _ = tx.send(Ok(Bytes::from(transformed.into_bytes()))).await;
            }
        }
        if !buffer.is_empty() {
            let _ = tx.send(Ok(Bytes::from(buffer.into_bytes()))).await;
        }
        log_end(&log, &request_id_clone, start_instant_clone.elapsed().as_millis() as u64,
            "generation", "POST", "/v1/messages", &original_model_clone, &upstream_model,
            status, None, usage.as_ref(), None).await;
    });

    let byte_stream = crate::streaming::ReceiverStreamCompat::new(rx);
    Response::builder()
        .header("content-type", "text/event-stream; charset=utf-8")
        .header("cache-control", "no-cache")
        .header("connection", "keep-alive")
        .body(Body::from_stream(byte_stream))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

fn anthropic_to_openai(payload: &serde_json::Map<String, Value>) -> serde_json::Map<String, Value> {
    let mut openai = serde_json::Map::new();
    if let Some(model) = payload.get("model") {
        openai.insert("model".to_owned(), model.clone());
    }

    let mut messages = Vec::new();
    if let Some(system) = payload.get("system") {
        let system_text = if let Some(s) = system.as_str() { s.to_owned() }
        else if let Some(arr) = system.as_array() {
            arr.iter().filter_map(|b| b.get("text").and_then(Value::as_str)).map(str::to_owned).collect::<Vec<_>>().join("\n")
        } else { String::new() };
        if !system_text.is_empty() {
            messages.push(json!({"role": "system", "content": system_text}));
        }
    }
    if let Some(msgs) = payload.get("messages").and_then(Value::as_array) {
        for msg in msgs {
            if let Some(role) = msg.get("role").and_then(Value::as_str) {
                messages.push(json!({"role": role, "content": anthropic_content_to_openai(msg.get("content"))}));
            }
        }
    }
    if !messages.is_empty() {
        openai.insert("messages".to_owned(), Value::Array(messages));
    }

    for field in &["max_tokens", "temperature", "top_p"] {
        if let Some(v) = payload.get(*field) { openai.insert(field.to_string(), v.clone()); }
    }
    if let Some(stop) = payload.get("stop_sequences") { openai.insert("stop".to_owned(), stop.clone()); }
    if let Some(tools) = payload.get("tools") { openai.insert("tools".to_owned(), tools.clone()); }
    if let Some(stream) = payload.get("stream") { openai.insert("stream".to_owned(), stream.clone()); }

    openai
}

fn anthropic_content_to_openai(content: Option<&Value>) -> Value {
    let Some(content) = content else { return Value::Null };
    if let Some(s) = content.as_str() { return Value::String(s.to_owned()); }
    if let Some(arr) = content.as_array() {
        let texts: Vec<String> = arr.iter()
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .map(str::to_owned)
            .collect();
        if !texts.is_empty() { return Value::String(texts.join("\n")); }
    }
    Value::Null
}

fn openai_to_anthropic(response_body: &[u8], model: &str) -> Vec<u8> {
    let parsed = match serde_json::from_slice::<Value>(response_body) {
        Ok(v) => v,
        Err(_) => return response_body.to_vec(),
    };

    let choice = parsed.get("choices").and_then(Value::as_array).and_then(|c| c.first());
    let usage = parsed.get("usage");
    let mut content = Vec::new();

    if let Some(choice) = choice {
        if let Some(msg) = choice.get("message").or_else(|| choice.get("delta")) {
            if let Some(text) = msg.get("content").and_then(Value::as_str) {
                content.push(json!({"type": "text", "text": text}));
            }
            if let Some(tool_calls) = msg.get("tool_calls").and_then(Value::as_array) {
                for tc in tool_calls {
                    content.push(json!({
                        "type": "tool_use",
                        "id": tc.get("id").cloned().unwrap_or_default(),
                        "name": tc.get("function").and_then(|f| f.get("name")).cloned().unwrap_or_default(),
                        "input": tc.get("function").and_then(|f| f.get("arguments")).cloned().unwrap_or_default(),
                    }));
                }
            }
        }
    }

    let stop_reason = choice.and_then(|c| c.get("finish_reason").and_then(Value::as_str))
        .map(|r| match r { "stop" => "end_turn", "tool_calls" => "tool_use", "length" => "max_tokens", _ => r });

    let mut anthropic = json!({
        "id": parsed.get("id").cloned().unwrap_or_default(),
        "type": "message",
        "role": "assistant",
        "content": if content.is_empty() { json!([{"type": "text", "text": ""}]) } else { Value::Array(content) },
        "model": model,
        "stop_reason": stop_reason,
        "stop_sequence": null,
    });

    if let Some(u) = usage {
        anthropic["usage"] = json!({
            "input_tokens": u.get("prompt_tokens").cloned().unwrap_or(Value::from(0)),
            "output_tokens": u.get("completion_tokens").cloned().unwrap_or(Value::from(0)),
        });
    }

    serde_json::to_vec(&anthropic).unwrap_or_else(|_| response_body.to_vec())
}

fn extract_openai_usage(response_body: &[u8]) -> Option<Value> {
    serde_json::from_slice::<Value>(response_body).ok()?
        .get("usage").and_then(Value::as_object).map(|u| json!({
            "input_tokens": u.get("prompt_tokens"),
            "output_tokens": u.get("completion_tokens"),
            "total_tokens": u.get("total_tokens"),
            "cache_read_tokens": u.get("prompt_tokens_details").and_then(|v| v.get("cached_tokens")),
            "cache_write_tokens": u.get("completion_tokens_details").and_then(|v| v.get("cached_tokens")),
        }))
}

fn extract_anthropic_usage(u: &serde_json::Map<String, Value>) -> Value {
    json!({
        "input_tokens": u.get("input_tokens"),
        "output_tokens": u.get("output_tokens"),
        "total_tokens": u.get("total_tokens"),
        "cache_read_tokens": u.get("cache_read_tokens"),
        "cache_write_tokens": u.get("cache_write_tokens"),
    })
}

async fn auth_failure(state: &ProxyState, request_id: &str) -> Response {
    log_record(&state.log, &json!({
        "record_type": "request_start",
        "request_id": request_id,
        "started_at": utc_now(),
        "kind": "generation",
        "method": "POST",
        "path": "/v1/messages",
    })).await;
    log_end(&state.log, request_id, 0, "generation", "POST", "/v1/messages", "", "", 401, None, None, Some("unauthorized")).await;
    StatusCode::UNAUTHORIZED.into_response()
}

async fn log_end(
    log: &tokio::sync::Mutex<tokio::io::BufWriter<tokio::fs::File>>,
    request_id: &str, duration_ms: u64, kind: &str, method: &str, path: &str,
    original_model: &str, upstream_model: &str, status_code: u16,
    response_body: Option<&Value>, usage: Option<&Value>, error: Option<&str>,
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
