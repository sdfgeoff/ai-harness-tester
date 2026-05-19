use std::time::Instant;

use axum::{
    body::{Body, Bytes},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use futures_util::{Stream, StreamExt};
use reqwest::Response as UpstreamResponse;
use serde_json::{json, Value};
use tokio::sync::mpsc;

use crate::{
    ProxyState,
    log_record,
    utc_now,
};

/// Which API's usage field names to expect from upstream.
#[derive(Debug, Clone, Copy)]
pub enum ApiKind {
    /// /v1/responses: input_tokens, output_tokens, ...
    Responses,
    /// /v1/chat/completions: prompt_tokens, completion_tokens, ...
    ChatCompletions,
}

/// Forward an upstream SSE response to the client as raw bytes, logging stream events.
pub async fn forward_sse(
    response: UpstreamResponse,
    state: &ProxyState,
    request_id: String,
    path: &str,
    original_model: String,
    api_kind: ApiKind,
    start_instant: Instant,
) -> Response {
    let status = response.status().as_u16();

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
                "path": path,
                "original_model": original_model,
                "upstream_model": state.model_name,
                "status_code": status,
                "error": error_text,
            }),
        )
        .await;
        return Response::builder()
            .status(StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR))
            .header("content-type", "application/json")
            .body(Body::from(error_body))
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
    }

    let (tx, rx) = mpsc::channel::<Result<Bytes, std::convert::Infallible>>(32);
    let log = state.log.clone();
    let request_id_clone = request_id.clone();
    let original_model_clone = original_model.clone();
    let upstream_model = state.model_name.clone();
    let start_instant_clone = start_instant.clone();
    let path = path.to_owned();

    tokio::spawn(async move {
        let mut usage: Option<Value> = None;

        let mut stream = response.bytes_stream();
        while let Some(chunk_result) = stream.next().await {
            match chunk_result {
                Ok(chunk) => {
                    // Log stream events found in this chunk
                    let chunk_text = String::from_utf8_lossy(&chunk);
                    for line in chunk_text.lines() {
                        if line.starts_with("data: ") {
                            let data_raw = line[6..].to_string();

                            log_record(
                                &log,
                                &json!({
                                    "record_type": "stream_event",
                                    "request_id": request_id_clone,
                                    "received_at": utc_now(),
                                    "event": "",
                                    "data_raw": data_raw,
                                }),
                            )
                            .await;

                            // Try to extract usage
                            if let Ok(parsed) = serde_json::from_str::<Value>(&data_raw) {
                                if let Some(usage_obj) = parsed.get("usage") {
                                    if usage_obj.is_object() {
                                        usage = Some(extract_usage(usage_obj, api_kind));
                                    }
                                }
                            }
                        }
                    }

                    let _ = tx.send(Ok(chunk)).await;
                }
                Err(read_error) => {
                    tracing::error!(error = %read_error, "proxy stream read error");
                    break;
                }
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
                "path": path,
                "original_model": original_model_clone,
                "upstream_model": upstream_model,
                "status_code": status,
                "response_body": null,
                "usage": usage.unwrap_or(Value::Null),
                "error": null,
            }),
        )
        .await;
    });

    // Forward raw SSE bytes with correct content-type
    let byte_stream = ReceiverStreamCompat::new(rx);
    Response::builder()
        .header("content-type", "text/event-stream; charset=utf-8")
        .header("cache-control", "no-cache")
        .header("connection", "keep-alive")
        .body(Body::from_stream(byte_stream))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// Simple adapter to make mpsc::Receiver work as a futures::Stream.
pub struct ReceiverStreamCompat {
    rx: tokio::sync::mpsc::Receiver<Result<axum::body::Bytes, std::convert::Infallible>>,
}

impl ReceiverStreamCompat {
    pub fn new(rx: tokio::sync::mpsc::Receiver<Result<axum::body::Bytes, std::convert::Infallible>>) -> Self {
        Self { rx }
    }
}

impl Stream for ReceiverStreamCompat {
    type Item = Result<Bytes, std::convert::Infallible>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        self.rx.poll_recv(cx).map(|opt| opt.map(|r| r))
    }
}

/// Extract normalized usage from a usage object, mapping field names per API kind.
fn extract_usage(usage_obj: &Value, kind: ApiKind) -> Value {
    match kind {
        ApiKind::Responses => json!({
            "input_tokens": usage_obj.get("input_tokens"),
            "output_tokens": usage_obj.get("output_tokens"),
            "total_tokens": usage_obj.get("total_tokens"),
            "cache_read_tokens": usage_obj.get("cache_read_tokens"),
            "cache_write_tokens": usage_obj.get("cache_write_tokens"),
        }),
        ApiKind::ChatCompletions => json!({
            "input_tokens": usage_obj.get("prompt_tokens"),
            "output_tokens": usage_obj.get("completion_tokens"),
            "total_tokens": usage_obj.get("total_tokens"),
            "cache_read_tokens": usage_obj.get("prompt_tokens_details").and_then(|v| v.get("cached_tokens")),
            "cache_write_tokens": usage_obj.get("completion_tokens_details").and_then(|v| v.get("cached_tokens")),
        }),
    }
}
