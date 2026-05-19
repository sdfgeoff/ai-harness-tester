use std::time::Instant;

use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response, sse::Event},
};
use futures_util::StreamExt;
use reqwest::Response as UpstreamResponse;
use serde_json::{json, Value};
use tokio_stream::wrappers::ReceiverStream;

use crate::{
    ProxyState,
    log_record,
    is_authorized,
    generate_request_id,
    utc_now,
};

/// Handle a streaming POST /v1/responses request.
pub async fn responses_streaming(
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

        tracing::warn!(path = "POST /v1/responses", streaming = true, "proxy auth failure");
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
    let start_instant = Instant::now();

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

    // Forward to upstream with streaming
    let upstream_url = format!(
        "{}/responses",
        state.upstream_base_url.trim_end_matches('/')
    );
    let mut request = state.client.post(&upstream_url).json(&payload);
    if !state.upstream_api_key.is_empty() {
        request = request.bearer_auth(&state.upstream_api_key);
    }

    let response: UpstreamResponse = match request.send().await {
        Ok(response) => response,
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

    let status = response.status().as_u16();

    // If not 200, read the error body and return it
    if status != 200 {
        let error_body = response.bytes().await.unwrap_or_default();
        let error_text = String::from_utf8_lossy(&error_body).to_string();
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
                "error": error_text,
            }),
        )
        .await;
        return Response::builder()
            .status(StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR))
            .body(axum::body::Body::from(error_body))
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
    }

    // Set up channel for SSE events
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, std::convert::Infallible>>(32);
    let log = state.log.clone();
    let request_id_clone = request_id.clone();
    let original_model_clone = original_model.clone();
    let upstream_model = state.model_name.clone();
    let start_instant_clone = start_instant.clone();

    // Spawn task to read upstream stream, forward events, and log them
    tokio::spawn(async move {
        let mut usage: Option<Value> = None;
        let mut current_event_name: Option<String> = None;

        // Read upstream response as byte stream
        let mut stream = response.bytes_stream();
        let mut buffer = Vec::new();

        while let Some(chunk_result) = stream.next().await {
            match chunk_result {
                Ok(chunk) => {
                    buffer.extend_from_slice(&chunk);

                    // Process complete SSE frames from buffer
                    let mut remaining = buffer.clone();
                    loop {
                        // Find the end of an SSE frame (double newline)
                        match remaining.windows(2).position(|w| w == b"\n\n") {
                            Some(pos) => {
                                let frame_end = pos + 2;
                                // Extract frame as owned Vec to avoid borrow conflict
                                let frame = remaining[..frame_end].to_vec();
                                remaining = remaining[frame_end..].to_vec();

                                // Parse SSE frame
                                let frame_text = String::from_utf8_lossy(&frame);
                                for line in frame_text.lines() {
                                    if line.starts_with("event: ") {
                                        current_event_name = Some(line[7..].to_string());
                                    } else if line.starts_with("data: ") {
                                        let data_raw = line[6..].to_string();
                                        let event_name = current_event_name.clone().unwrap_or_default();

                                        // Log stream_event record
                                        log_record(
                                            &log,
                                            &json!({
                                                "record_type": "stream_event",
                                                "request_id": request_id_clone,
                                                "received_at": utc_now(),
                                                "event": event_name,
                                                "data_raw": data_raw,
                                            }),
                                        )
                                        .await;

                                        // Try to extract usage from this data
                                        if let Ok(parsed) = serde_json::from_str::<Value>(&data_raw) {
                                            if let Some(usage_obj) = parsed.get("usage") {
                                                if usage_obj.is_object() {
                                                    usage = Some(json!({
                                                        "input_tokens": usage_obj.get("input_tokens"),
                                                        "output_tokens": usage_obj.get("output_tokens"),
                                                        "total_tokens": usage_obj.get("total_tokens"),
                                                        "cache_read_tokens": usage_obj.get("cache_read_tokens"),
                                                        "cache_write_tokens": usage_obj.get("cache_write_tokens"),
                                                    }));
                                                }
                                            }
                                        }
                                    }
                                }

                                // Forward the complete frame to the client as SSE data
                                let _ = tx.send(Ok(Event::default().data(frame_text.trim_end_matches("\n\n")))).await;

                                current_event_name = None;
                            }
                            None => break,
                        }
                    }
                    buffer = remaining;
                }
                Err(read_error) => {
                    tracing::error!(error = %read_error, "proxy stream read error");
                    break;
                }
            }
        }

        // Drain any remaining buffer as final event
        if !buffer.is_empty() {
            let frame_text = String::from_utf8_lossy(&buffer).to_string();
            let trimmed = frame_text.trim_end_matches("\n\n").to_string();
            if !trimmed.is_empty() {
                // Parse remaining data for usage
                for line in trimmed.lines() {
                    if line.starts_with("data: ") {
                        let data_raw = line[6..].to_string();
                        if let Ok(parsed) = serde_json::from_str::<Value>(&data_raw) {
                            if let Some(usage_obj) = parsed.get("usage") {
                                if usage_obj.is_object() {
                                    usage = Some(json!({
                                        "input_tokens": usage_obj.get("input_tokens"),
                                        "output_tokens": usage_obj.get("output_tokens"),
                                        "total_tokens": usage_obj.get("total_tokens"),
                                        "cache_read_tokens": usage_obj.get("cache_read_tokens"),
                                        "cache_write_tokens": usage_obj.get("cache_write_tokens"),
                                    }));
                                }
                            }
                        }
                    }
                }
                let _ = tx.send(Ok(Event::default().data(trimmed))).await;
            }
        }

        // Write request_end with extracted usage
        let duration_ms = start_instant_clone.elapsed().as_millis() as u64;
        log_record(
            &log,
            &json!({
                "record_type": "request_end",
                "request_id": request_id_clone,
                "finished_at": utc_now(),
                "duration_ms": duration_ms,
                "kind": "generation",
                "method": "POST",
                "path": "/v1/responses",
                "original_model": original_model_clone,
                "upstream_model": upstream_model,
                "status_code": status,
                "response_body": null,
                "usage": usage.unwrap_or(Value::Null),
                "error": null,
            }),
        )
        .await;

        // Channel closes, stream ends
    });

    // Build SSE response
    let stream = ReceiverStream::new(rx);
    axum::response::Sse::new(stream).into_response()
}
