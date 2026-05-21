use super::*;
use std::{
    io::{Read, Write},
    net::TcpListener as StdTcpListener,
    sync::mpsc,
    thread,
};

struct MockChatUpstream {
    base_url: String,
    body: mpsc::Receiver<serde_json::Value>,
}

impl MockChatUpstream {
    fn start() -> Self {
        let listener = StdTcpListener::bind("127.0.0.1:0").expect("bind mock chat upstream");
        let address = listener.local_addr().expect("mock chat upstream address");
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept chat request");
            let mut request = Vec::new();
            let mut buffer = [0u8; 4096];
            loop {
                let read = stream.read(&mut buffer).expect("read request");
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
                if request.windows(4).any(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request);
                    let content_length = headers
                        .lines()
                        .find_map(|l| {
                            l.strip_prefix("content-length:")
                                .or_else(|| l.strip_prefix("Content-Length:"))
                        })
                        .and_then(|v| v.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    let header_end = request.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
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
            let body = br#"{"id":"chatcmpl-1","object":"chat.completion","model":"smoke-local","choices":[{"index":0,"message":{"role":"assistant","content":"Hello!"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}}"#;
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

struct MockChatSseUpstream {
    base_url: String,
    body: mpsc::Receiver<serde_json::Value>,
}

impl MockChatSseUpstream {
    fn start() -> Self {
        let listener = StdTcpListener::bind("127.0.0.1:0").expect("bind mock chat SSE upstream");
        let address = listener
            .local_addr()
            .expect("mock chat SSE upstream address");
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept chat SSE request");
            let mut request = Vec::new();
            let mut buffer = [0u8; 4096];
            loop {
                let read = stream.read(&mut buffer).expect("read request");
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
                if request.windows(4).any(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request);
                    let content_length = headers
                        .lines()
                        .find_map(|l| {
                            l.strip_prefix("content-length:")
                                .or_else(|| l.strip_prefix("Content-Length:"))
                        })
                        .and_then(|v| v.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    let header_end = request.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
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
            let sse_response = b"data: {\"id\":\"chatcmpl-1\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"Hello\"},\"finish_reason\":null}]}\n\ndata: {\"id\":\"chatcmpl-1\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\" world\"},\"finish_reason\":null}]}\n\ndata: {\"id\":\"chatcmpl-1\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":20,\"completion_tokens\":8,\"total_tokens\":28}}\n\ndata: [DONE]\n\n";
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: keep-alive\r\n\r\n").expect("write SSE headers");
            stream.write_all(sse_response).expect("write SSE body");
        });
        Self {
            base_url: format!("http://{address}/v1"),
            body: rx,
        }
    }
}

#[tokio::test]
async fn chat_completions_rewrites_model_and_preserves_fields() {
    let upstream = MockChatUpstream::start();
    let tmp = tempfile::NamedTempFile::new().expect("create temp log file");
    let proxy = crate::start_proxy(crate::ProxyConfig {
        model_name: "smoke-local".to_owned(),
        upstream_base_url: upstream.base_url.clone(),
        upstream_api_key: "local".to_owned(),
        proxy_log_path: tmp.path().to_owned(),
    })
    .await
    .expect("start proxy");

    let url = format!("{}/v1/chat/completions", proxy.base_url);
    let api_key = proxy.api_key.clone();
    let response = tokio::task::spawn_blocking(move || {
        ureq::post(&url)
            .set("Authorization", &format!("Bearer {api_key}"))
            .send_json(serde_json::json!({
                "model": "gpt-4o",
                "messages": [{"role": "user", "content": "hello"}]
            }))
            .expect("chat response")
            .into_json::<serde_json::Value>()
            .expect("json response")
    })
    .await
    .expect("task");

    assert_eq!(response["id"], "chatcmpl-1");
    assert_eq!(response["choices"][0]["message"]["content"], "Hello!");

    let upstream_body = upstream.body.recv().expect("upstream body");
    assert_eq!(upstream_body["model"], "smoke-local");
    assert_eq!(upstream_body["messages"][0]["role"], "user");

    proxy.shutdown().await.expect("shutdown proxy");

    let log_contents = std::fs::read_to_string(tmp.path()).expect("read log");
    let records: Vec<Value> = log_contents
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    assert_eq!(records.len(), 2);

    let start = &records[0];
    let end = &records[1];

    assert_eq!(start["path"], "/v1/chat/completions");
    assert_eq!(start["original_model"], "gpt-4o");
    assert_eq!(start["upstream_model"], "smoke-local");
    assert_eq!(start["request_body"]["model"], "smoke-local");

    assert_eq!(end["status_code"], 200);
    assert_eq!(end["usage"]["input_tokens"], 10);
    assert_eq!(end["usage"]["output_tokens"], 5);
    assert_eq!(end["usage"]["total_tokens"], 15);
    assert_eq!(start["request_id"], end["request_id"]);
}

#[tokio::test]
async fn chat_completions_streaming_forwards_events_and_logs_them() {
    let upstream = MockChatSseUpstream::start();
    let tmp = tempfile::NamedTempFile::new().expect("create temp log file");
    let proxy = crate::start_proxy(crate::ProxyConfig {
        model_name: "smoke-local".to_owned(),
        upstream_base_url: upstream.base_url.clone(),
        upstream_api_key: "local".to_owned(),
        proxy_log_path: tmp.path().to_owned(),
    })
    .await
    .expect("start proxy");

    let url = format!("{}/v1/chat/completions", proxy.base_url);
    let api_key = proxy.api_key.clone();

    let response = tokio::task::spawn_blocking(move || {
        let resp = ureq::post(&url)
            .set("Authorization", &format!("Bearer {api_key}"))
            .send_json(serde_json::json!({
                "model": "gpt-4o",
                "messages": [{"role": "user", "content": "hello"}],
                "stream": true
            }))
            .expect("SSE request");
        let mut reader = resp.into_reader();
        let mut body = String::new();
        reader.read_to_string(&mut body).expect("read SSE body");
        body
    })
    .await
    .expect("task");

    assert!(response.contains("Hello"));
    assert!(response.contains("world"));
    assert!(response.contains("finish_reason"));

    let upstream_body = upstream.body.recv().expect("upstream body");
    assert_eq!(upstream_body["model"], "smoke-local");
    assert_eq!(upstream_body["stream"], true);

    proxy.shutdown().await.expect("shutdown proxy");

    let log_contents = std::fs::read_to_string(tmp.path()).expect("read log");
    let records: Vec<Value> = log_contents
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();

    let stream_events: Vec<&Value> = records
        .iter()
        .filter(|r| r["record_type"] == "stream_event")
        .collect();
    assert!(
        stream_events.len() >= 3,
        "expected at least 3 stream events, got {}",
        stream_events.len()
    );

    let start = &records[0];
    let end = records.last().expect("end record");

    assert_eq!(start["path"], "/v1/chat/completions");
    assert_eq!(end["record_type"], "request_end");
    assert_eq!(end["usage"]["input_tokens"], 20);
    assert_eq!(end["usage"]["output_tokens"], 8);
    assert_eq!(end["usage"]["total_tokens"], 28);
    assert_eq!(start["request_id"], end["request_id"]);
}

#[tokio::test]
async fn chat_completions_auth_failure_omits_request_body() {
    let tmp = tempfile::NamedTempFile::new().expect("create temp log file");
    let proxy = crate::start_proxy(crate::ProxyConfig {
        model_name: "smoke-local".to_owned(),
        upstream_base_url: "http://127.0.0.1:1/v1".to_owned(),
        upstream_api_key: "local".to_owned(),
        proxy_log_path: tmp.path().to_owned(),
    })
    .await
    .expect("start proxy");

    let url = format!("{}/v1/chat/completions", proxy.base_url);
    let response = tokio::task::spawn_blocking(move || {
        ureq::post(&url).send_json(serde_json::json!({"model": "gpt-4o", "messages": []}))
    })
    .await
    .expect("task");
    match response.unwrap_err() {
        ureq::Error::Status(status, _) => assert_eq!(status, 401),
        error => panic!("expected 401, got {error}"),
    }

    proxy.shutdown().await.expect("shutdown proxy");

    let log_contents = std::fs::read_to_string(tmp.path()).expect("read log");
    let records: Vec<Value> = log_contents
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    assert_eq!(records.len(), 2);
    assert!(records[0].get("request_body").is_none());
    assert_eq!(records[1]["error"], "unauthorized");
}
