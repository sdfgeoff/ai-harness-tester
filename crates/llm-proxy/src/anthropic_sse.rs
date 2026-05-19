use serde_json::json;

/// Transform an OpenAI SSE frame to Anthropic SSE format.
pub fn transform_openai_sse_to_anthropic(frame: &str, model: &str, content_index: &mut usize) -> String {
    let mut output = String::new();

    for line in frame.lines() {
        if line.starts_with("data: ") {
            let data_raw = &line[6..];
            if data_raw == "[DONE]" {
                output.push_str("event: message_stop\ndata: {}\n\n");
                continue;
            }

            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(data_raw) {
                // Check for usage (final chunk)
                if parsed.get("usage").is_some() {
                    let stop_reason = parsed.get("choices")
                        .and_then(|c| c.as_array())
                        .and_then(|c| c.first())
                        .and_then(|c| c.get("finish_reason"))
                        .and_then(|r| r.as_str())
                        .map(map_stop_reason)
                        .unwrap_or("end_turn");

                    output.push_str(&format!(
                        "event: message_delta\ndata: {}\n\n",
                        serde_json::to_string(&json!({
                            "type": "message_delta",
                            "delta": {"stop_reason": stop_reason, "stop_sequence": null},
                            "usage": {"output_tokens": parsed["usage"]["completion_tokens"].clone()}
                        })).unwrap_or_default()
                    ));
                    continue;
                }

                if let Some(choices) = parsed.get("choices").and_then(|v| v.as_array()) {
                    for choice in choices {
                        let delta = choice.get("delta");
                        let finish_reason = choice.get("finish_reason").and_then(|v| v.as_str());

                        if let Some(delta) = delta {
                            if let Some("assistant") = delta.get("role").and_then(|v| v.as_str()) {
                                output.push_str(&format!(
                                    "event: message_start\ndata: {}\n\n",
                                    serde_json::to_string(&json!({
                                        "type": "message_start",
                                        "message": {
                                            "id": parsed.get("id").cloned().unwrap_or_default(),
                                            "type": "message",
                                            "role": "assistant",
                                            "content": [],
                                            "model": model,
                                        }
                                    })).unwrap_or_default()
                                ));
                                output.push_str(&format!(
                                    "event: content_block_start\ndata: {}\n\n",
                                    serde_json::to_string(&json!({
                                        "type": "content_block_start",
                                        "index": *content_index,
                                        "content_block": {"type": "text", "text": ""}
                                    })).unwrap_or_default()
                                ));
                            }

                            if let Some(text) = delta.get("content").and_then(|v| v.as_str()) {
                                if !text.is_empty() {
                                    output.push_str(&format!(
                                        "event: content_block_delta\ndata: {}\n\n",
                                        serde_json::to_string(&json!({
                                            "type": "content_block_delta",
                                            "index": *content_index,
                                            "delta": {"type": "text_delta", "text": text}
                                        })).unwrap_or_default()
                                    ));
                                }
                            }

                            if let Some(tool_calls) = delta.get("tool_calls").and_then(|v| v.as_array()) {
                                for tc in tool_calls {
                                    let id = tc.get("id").cloned().unwrap_or_default();
                                    let name = tc.get("function").and_then(|f| f.get("name")).cloned().unwrap_or_default();
                                    output.push_str(&format!(
                                        "event: content_block_start\ndata: {}\n\n",
                                        serde_json::to_string(&json!({
                                            "type": "content_block_start",
                                            "index": *content_index,
                                            "content_block": {"type": "tool_use", "id": id, "name": name, "input": {}}
                                        })).unwrap_or_default()
                                    ));
                                    *content_index += 1;
                                    if let Some(args) = tc.get("function").and_then(|f| f.get("arguments")).and_then(|v| v.as_str()) {
                                        output.push_str(&format!(
                                            "event: content_block_delta\ndata: {}\n\n",
                                            serde_json::to_string(&json!({
                                                "type": "content_block_delta",
                                                "index": *content_index,
                                                "partial_json": args
                                            })).unwrap_or_default()
                                        ));
                                    }
                                }
                            }
                        }

                        if let Some(reason) = finish_reason {
                            output.push_str(&format!(
                                "event: message_delta\ndata: {}\n\n",
                                serde_json::to_string(&json!({
                                    "type": "message_delta",
                                    "delta": {"stop_reason": map_stop_reason(reason), "stop_sequence": null},
                                    "usage": {"output_tokens": 0}
                                })).unwrap_or_default()
                            ));
                            output.push_str("event: message_stop\ndata: {}\n\n");
                        }
                    }
                } else {
                    output.push_str(line);
                    output.push('\n');
                }
            } else {
                output.push_str(line);
                output.push('\n');
            }
        } else {
            output.push_str(line);
            output.push('\n');
        }
    }

    output
}

fn map_stop_reason(reason: &str) -> &str {
    match reason {
        "stop" => "end_turn",
        "tool_calls" => "tool_use",
        "length" => "max_tokens",
        _ => reason,
    }
}
