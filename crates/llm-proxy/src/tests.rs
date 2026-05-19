    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener as StdTcpListener,
        sync::mpsc,
        thread,
    };

    #[tokio::test]
    async fn models_requires_auth() {
        let tmp = tempfile::NamedTempFile::new().expect("create temp log file");
        let proxy = start_proxy(ProxyConfig {
            model_name: "smoke-local".to_owned(),
            upstream_base_url: "http://127.0.0.1:1/v1".to_owned(),
            upstream_api_key: "local".to_owned(),
            proxy_log_path: tmp.path().to_owned(),
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

        // Verify auth failure was logged without request body
        let log_contents = std::fs::read_to_string(tmp.path()).expect("read log");
        let records: Vec<Value> = log_contents
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();
        assert_eq!(records.len(), 2, "expected start + end records");
        assert_eq!(records[0]["record_type"], "request_start");
        assert_eq!(records[0]["kind"], "discovery");
        assert_eq!(records[1]["record_type"], "request_end");
        assert_eq!(records[1]["status_code"], 401);
        assert_eq!(records[1]["error"], "unauthorized");
        assert!(records[0].get("request_body").is_none());
    }

    #[tokio::test]
    async fn models_returns_selected_model() {
        let tmp = tempfile::NamedTempFile::new().expect("create temp log file");
        let proxy = start_proxy(ProxyConfig {
            model_name: "smoke-local".to_owned(),
            upstream_base_url: "http://127.0.0.1:1/v1".to_owned(),
            upstream_api_key: "local".to_owned(),
            proxy_log_path: tmp.path().to_owned(),
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

        // Verify NDJSON log has linked start/end records
        let log_contents = std::fs::read_to_string(tmp.path()).expect("read log");
        let records: Vec<Value> = log_contents
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();
        assert_eq!(records.len(), 2, "expected start + end records");

        let start = &records[0];
        let end = &records[1];

        assert_eq!(start["record_type"], "request_start");
        assert_eq!(end["record_type"], "request_end");
        assert_eq!(start["kind"], "discovery");
        assert_eq!(end["kind"], "discovery");
        assert_eq!(start["request_id"], end["request_id"]);
        assert_ne!(start["request_id"], Value::String("".to_owned()));
        assert_eq!(end["status_code"], 200);
    }

    #[tokio::test]
    async fn responses_rewrites_model_and_preserves_fields() {
        let upstream = MockUpstream::start();
        let tmp = tempfile::NamedTempFile::new().expect("create temp log file");
        let proxy = start_proxy(ProxyConfig {
            model_name: "smoke-local".to_owned(),
            upstream_base_url: upstream.base_url.clone(),
            upstream_api_key: "local".to_owned(),
            proxy_log_path: tmp.path().to_owned(),
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

        // Verify NDJSON log
        let log_contents = std::fs::read_to_string(tmp.path()).expect("read log");
        let records: Vec<Value> = log_contents
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();
        assert_eq!(records.len(), 2, "expected start + end records");

        let start = &records[0];
        let end = &records[1];

        assert_eq!(start["record_type"], "request_start");
        assert_eq!(start["kind"], "generation");
        assert_eq!(start["method"], "POST");
        assert_eq!(start["path"], "/v1/responses");
        assert_eq!(start["original_model"], "original-model");
        assert_eq!(start["upstream_model"], "smoke-local");
        assert!(start.get("request_body").is_some());
        assert_eq!(start["request_body"]["model"], "smoke-local");
        assert_eq!(start["request_body"]["input"], "hello");

        assert_eq!(end["record_type"], "request_end");
        assert_eq!(end["kind"], "generation");
        assert_eq!(end["status_code"], 200);
        assert!(end.get("response_body").is_some());
        assert_eq!(end["response_body"]["id"], "response-id");
        assert_eq!(end["usage"]["input_tokens"], 1);
        assert_eq!(end["usage"]["output_tokens"], 2);
        assert_eq!(end["usage"]["total_tokens"], 3);
        assert_eq!(end["error"], Value::Null);
        assert_eq!(start["request_id"], end["request_id"]);
        assert!(end["duration_ms"].as_u64().is_some());
    }

    #[tokio::test]
    async fn responses_auth_failure_omits_request_body() {
        let tmp = tempfile::NamedTempFile::new().expect("create temp log file");
        let proxy = start_proxy(ProxyConfig {
            model_name: "smoke-local".to_owned(),
            upstream_base_url: "http://127.0.0.1:1/v1".to_owned(),
            upstream_api_key: "local".to_owned(),
            proxy_log_path: tmp.path().to_owned(),
        })
        .await
        .expect("start proxy");

        let url = format!("{}/v1/responses", proxy.base_url);
        let response = tokio::task::spawn_blocking(move || {
            ureq::post(&url)
                .send_json(serde_json::json!({
                    "model": "original-model",
                    "input": "secret data"
                }))
        })
        .await
        .expect("blocking request task");
        match response.unwrap_err() {
            ureq::Error::Status(status, _) => assert_eq!(status, 401),
            error => panic!("expected 401 status error, got {error}"),
        }

        proxy.shutdown().await.expect("shutdown proxy");

        let log_contents = std::fs::read_to_string(tmp.path()).expect("read log");
        let records: Vec<Value> = log_contents
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();
        assert_eq!(records.len(), 2);
        assert!(records[0].get("request_body").is_none());
        assert!(records[1].get("request_body").is_none());
        assert_eq!(records[1]["error"], "unauthorized");
    }

    // ── Mock upstream ─────────────────────────────────────────────────────

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

    // ── Mock SSE upstream ─────────────────────────────────────────────────

    struct MockSseUpstream {
        base_url: String,
        body: mpsc::Receiver<serde_json::Value>,
    }

    impl MockSseUpstream {
        fn start() -> Self {
            let listener = StdTcpListener::bind("127.0.0.1:0").expect("bind mock SSE upstream");
            let address = listener.local_addr().expect("mock SSE upstream address");
            let (tx, rx) = mpsc::channel();
            thread::spawn(move || {
                let (mut stream, _) = listener.accept().expect("accept SSE request");
                let mut request = Vec::new();
                let mut buffer = [0u8; 1024];
                loop {
                    let read = stream.read(&mut buffer).expect("read SSE request");
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
                            let read = stream.read(&mut buffer).expect("read SSE body");
                            if read == 0 {
                                break;
                            }
                            request.extend_from_slice(&buffer[..read]);
                        }
                        let body = &request[header_end..header_end + content_length];
                        tx.send(serde_json::from_slice(body).expect("SSE request json"))
                            .expect("send SSE body");
                        break;
                    }
                }
                // Send SSE response with usage in the final event
                let sse_response = b"data: {\"type\":\"response.created\",\"id\":\"resp-1\"}\n\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"Hello\"}\n\ndata: {\"type\":\"response.completed\",\"usage\":{\"input_tokens\":10,\"output_tokens\":5,\"total_tokens\":15}}\n\n";
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: keep-alive\r\n\r\n"
                )
                .expect("write SSE headers");
                stream.write_all(sse_response).expect("write SSE body");
            });

            Self {
                base_url: format!("http://{address}/v1"),
                body: rx,
            }
        }
    }

    #[tokio::test]
    async fn streaming_forwards_events_and_logs_them() {
        let upstream = MockSseUpstream::start();
        let tmp = tempfile::NamedTempFile::new().expect("create temp log file");
        let proxy = start_proxy(ProxyConfig {
            model_name: "smoke-local".to_owned(),
            upstream_base_url: upstream.base_url.clone(),
            upstream_api_key: "local".to_owned(),
            proxy_log_path: tmp.path().to_owned(),
        })
        .await
        .expect("start proxy");

        let url = format!("{}/v1/responses", proxy.base_url);
        let api_key = proxy.api_key.clone();

        // Make streaming request
        let response = tokio::task::spawn_blocking(move || {
            let resp = ureq::post(&url)
                .set("Authorization", &format!("Bearer {api_key}"))
                .send_json(serde_json::json!({
                    "model": "original-model",
                    "input": "hello",
                    "stream": true
                }))
                .expect("SSE request");
            // Read the raw response body
            let mut reader = resp.into_reader();
            let mut body = String::new();
            reader.read_to_string(&mut body).expect("read SSE body");
            body
        })
        .await
        .expect("blocking request task");

        // Verify SSE events were forwarded
        assert!(response.contains("response.created"));
        assert!(response.contains("response.output_text.delta"));
        assert!(response.contains("response.completed"));
        assert!(response.contains("Hello"));

        // Verify upstream received rewritten model
        let upstream_body = upstream.body.recv().expect("upstream body");
        assert_eq!(upstream_body["model"], "smoke-local");
        assert_eq!(upstream_body["stream"], true);

        proxy.shutdown().await.expect("shutdown proxy");

        // Verify NDJSON log
        let log_contents = std::fs::read_to_string(tmp.path()).expect("read log");
        let records: Vec<Value> = log_contents
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();

        // Should have: request_start, stream_event x3, request_end
        assert!(records.len() >= 5, "expected at least 5 records, got {}", records.len());

        let start = &records[0];
        let end = records.last().expect("end record");

        assert_eq!(start["record_type"], "request_start");
        assert_eq!(start["kind"], "generation");

        // Check stream events
        let stream_events: Vec<&Value> = records
            .iter()
            .filter(|r| r["record_type"] == "stream_event")
            .collect();
        assert_eq!(stream_events.len(), 3, "expected 3 stream events");
        assert_eq!(stream_events[0]["event"], "");
        assert_eq!(stream_events[0]["data_raw"], r#"{"type":"response.created","id":"resp-1"}"#);
        assert_eq!(stream_events[1]["event"], "");
        assert_eq!(stream_events[1]["data_raw"], r#"{"type":"response.output_text.delta","delta":"Hello"}"#);
        assert_eq!(stream_events[2]["event"], "");
        assert_eq!(stream_events[2]["data_raw"], r#"{"type":"response.completed","usage":{"input_tokens":10,"output_tokens":5,"total_tokens":15}}"#);

        // Check request_end has usage extracted from stream
        assert_eq!(end["record_type"], "request_end");
        assert_eq!(end["status_code"], 200);
        assert_eq!(end["response_body"], Value::Null); // streaming = null
        assert_eq!(end["usage"]["input_tokens"], 10);
        assert_eq!(end["usage"]["output_tokens"], 5);
        assert_eq!(end["usage"]["total_tokens"], 15);

        // Linked by request_id
        assert_eq!(start["request_id"], end["request_id"]);
        assert_eq!(start["request_id"], stream_events[0]["request_id"]);
    }
