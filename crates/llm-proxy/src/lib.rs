use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    time::{SystemTime, UNIX_EPOCH},
};

use axum::{
    extract::State,
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde_json::json;
use tokio::{net::TcpListener, sync::oneshot, task::JoinHandle};

#[derive(Debug, Clone)]
pub struct ProxyConfig {
    pub model_name: String,
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
    api_key: String,
}

pub async fn start_proxy(config: ProxyConfig) -> Result<ProxyHandle, String> {
    let api_key = generate_api_key();
    let state = ProxyState {
        model_name: config.model_name,
        api_key: api_key.clone(),
    };
    let app = Router::new()
        .route("/v1/models", get(models))
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

    #[tokio::test]
    async fn models_requires_auth() {
        let proxy = start_proxy(ProxyConfig {
            model_name: "smoke-local".to_owned(),
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
}
