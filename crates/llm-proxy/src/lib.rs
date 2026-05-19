use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
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
use tokio::{net::TcpListener, sync::oneshot, task::JoinHandle};

#[derive(Debug, Clone)]
pub struct ProxyConfig {
    pub model_name: String,
    pub upstream_base_url: String,
    pub upstream_api_key: String,
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

#[derive(Debug, Clone)]
struct ProxyState {
    model_name: String,
    upstream_base_url: String,
    upstream_api_key: String,
    api_key: String,
    client: Client,
}

pub async fn start_proxy(config: ProxyConfig) -> Result<ProxyHandle, String> {
    let api_key = generate_api_key();
    let state = ProxyState {
        model_name: config.model_name,
        upstream_base_url: config.upstream_base_url,
        upstream_api_key: config.upstream_api_key,
        api_key: api_key.clone(),
        client: Client::new(),
    };
    let app = Router::new()
        .route("/v1/models", get(models))
        .route("/v1/responses", post(responses))
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

async fn responses(State(state): State<ProxyState>, headers: HeaderMap, body: Bytes) -> Response {
    if !is_authorized(&headers, &state.api_key) {
        eprintln!("proxy auth failure: POST /v1/responses");
        return StatusCode::UNAUTHORIZED.into_response();
    }

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
    payload.insert("model".to_owned(), Value::String(state.model_name.clone()));

    let upstream_url = format!(
        "{}/responses",
        state.upstream_base_url.trim_end_matches('/')
    );
    let mut request = state.client.post(upstream_url).json(&payload);
    if !state.upstream_api_key.is_empty() {
        request = request.bearer_auth(&state.upstream_api_key);
    }

    let upstream_response = match request.send().await {
        Ok(response) => response,
        Err(error) => {
            return (
                StatusCode::BAD_GATEWAY,
                format!("failed to reach upstream model endpoint: {error}"),
            )
                .into_response();
        }
    };
    let status = upstream_response.status();
    let content_type = upstream_response
        .headers()
        .get(header::CONTENT_TYPE)
        .cloned()
        .unwrap_or_else(|| header::HeaderValue::from_static("application/json"));
    let response_body = match upstream_response.bytes().await {
        Ok(body) => body,
        Err(error) => {
            return (
                StatusCode::BAD_GATEWAY,
                format!("failed to read upstream response body: {error}"),
            )
                .into_response();
        }
    };

    Response::builder()
        .status(status)
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

async fn models(State(state): State<ProxyState>, headers: HeaderMap) -> Response {
    if !is_authorized(&headers, &state.api_key) {
        eprintln!("proxy auth failure: GET /v1/models");
        return StatusCode::UNAUTHORIZED.into_response();
    }

    Json(json!({
        "object": "list",
        "data": [
            {
                "id": state.model_name,
                "object": "model"
            }
        ]
    }))
    .into_response()
}

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
        let proxy = start_proxy(ProxyConfig {
            model_name: "smoke-local".to_owned(),
            upstream_base_url: "http://127.0.0.1:1/v1".to_owned(),
            upstream_api_key: "local".to_owned(),
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
    }

    #[tokio::test]
    async fn models_returns_selected_model() {
        let proxy = start_proxy(ProxyConfig {
            model_name: "smoke-local".to_owned(),
            upstream_base_url: "http://127.0.0.1:1/v1".to_owned(),
            upstream_api_key: "local".to_owned(),
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
    }

    #[tokio::test]
    async fn responses_rewrites_model_and_preserves_fields() {
        let upstream = MockUpstream::start();
        let proxy = start_proxy(ProxyConfig {
            model_name: "smoke-local".to_owned(),
            upstream_base_url: upstream.base_url.clone(),
            upstream_api_key: "local".to_owned(),
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
    }

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
