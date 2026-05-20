mod anthropic_handlers;
mod chat_handlers;
mod handlers;
mod streaming;

use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use axum::{
    http::{header, HeaderMap},
    routing::{get, post},
    Router,
};
use reqwest::Client;
use serde_json::Value;
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
pub(crate) struct ProxyState {
    pub model_name: String,
    pub upstream_base_url: String,
    pub upstream_api_key: String,
    pub api_key: String,
    pub client: Client,
    pub log: Arc<Mutex<BufWriter<tokio::fs::File>>>,
}

// ── Proxy log writer ────────────────────────────────────────────────────────

/// Write a single NDJSON line to the shared proxy log.
pub(crate) async fn log_record(log: &Mutex<BufWriter<tokio::fs::File>>, record: &Value) {
    let mut writer = log.lock().await;
    let line = serde_json::to_string(record).expect("serialize log record");
    let _ = writer.write_all(line.as_bytes()).await;
    let _ = writer.write_all(b"\n").await;
    let _ = writer.flush().await;
}

// ── Helpers ─────────────────────────────────────────────────────────────────

pub(crate) fn is_authorized(headers: &HeaderMap, api_key: &str) -> bool {
    // Check Bearer token (OpenAI SDK)
    if let Some(value) = headers.get(header::AUTHORIZATION) {
        if let Ok(value) = value.to_str() {
            if value == format!("Bearer {api_key}") {
                return true;
            }
        }
    }
    // Check X-Api-Key (Anthropic SDK)
    if let Some(value) = headers.get("x-api-key") {
        if let Ok(value) = value.to_str() {
            if value == api_key {
                return true;
            }
        }
    }
    false
}

fn generate_api_key() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!("harness-test-{nanos:x}")
}

pub(crate) fn generate_request_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let random = fastrand::u64(0..u64::MAX);
    format!("{:016x}{:016x}", nanos, random)
}

pub(crate) fn utc_now() -> String {
    use time::format_description::well_known::Rfc3339;
    time::OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "unknown".to_owned())
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
        api_key: api_key.clone(),
        client: Client::new(),
        log,
    };

    let app = Router::new()
        .route("/v1/models", get(handlers::models))
        .route("/v1/responses", post(handlers::responses))
        .route("/v1/chat/completions", post(chat_handlers::chat_completions))
        .route("/v1/messages", post(anthropic_handlers::messages))
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
            tracing::error!(error = %error, "proxy server error");
        }
    });

    Ok(ProxyHandle {
        base_url: format!("http://127.0.0.1:{}", address.port()),
        api_key,
        shutdown: Some(shutdown_tx),
        task,
    })
}

#[cfg(test)]
mod tests;
