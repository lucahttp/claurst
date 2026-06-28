// providers/minimax.rs — MinimaxProvider: Anthropic-compatible provider for MiniMax.
// Minimax requires API key in Authorization header without Bearer prefix.

use std::pin::Pin;

use async_stream::stream;
use async_trait::async_trait;
use claurst_core::provider_id::{ModelId, ProviderId};
use claurst_core::types::{ContentBlock, UsageInfo};
use futures::Stream;
use reqwest::{Client, header};
use serde_json::Value;

use crate::provider::{LlmProvider, ModelInfo};
use crate::provider_error::ProviderError;
use crate::provider_types::{
    ProviderCapabilities, ProviderRequest, ProviderResponse, ProviderStatus, StopReason,
    StreamEvent, SystemPromptStyle,
};
use crate::types::{ApiMessage, ApiToolDefinition, CreateMessageRequest};

use super::message_normalization::normalize_anthropic_messages;

pub struct MinimaxProvider {
    http_client: Client,
    api_key: String,
    api_base: String,
    id: ProviderId,
}

impl MinimaxProvider {
    pub fn new(api_key: String) -> Self {
        // Check MINIMAX_BASE_URL first (claurst native), then ANTHROPIC_BASE_URL (Claude Code compat)
        let api_base = std::env::var("MINIMAX_BASE_URL")
            .or_else(|_| std::env::var("ANTHROPIC_BASE_URL"))
            .unwrap_or_else(|_| "https://api.minimax.io/anthropic".to_string());
        let mut headers = header::HeaderMap::new();
        headers.insert("X-Api-Key", header::HeaderValue::from_str(&api_key).expect("unable to parse api key for http header"));
        let http_client = Client::builder()
            .default_headers(headers)
            .timeout(std::time::Duration::from_secs(600))
            .build()
            .expect("MinimaxProvider: failed to build HTTP client");

        Self {
            http_client,
            api_key,
            api_base,
            id: ProviderId::new(ProviderId::MINIMAX),
        }
    }

    fn build_request(request: &ProviderRequest) -> CreateMessageRequest {
        let normalized_messages = normalize_anthropic_messages(&request.messages);
        let api_messages: Vec<ApiMessage> = normalized_messages
            .iter()
            .map(ApiMessage::from)
            .collect();

        let api_tools: Option<Vec<ApiToolDefinition>> = if request.tools.is_empty() {
            None
        } else {
            Some(request.tools.iter().map(ApiToolDefinition::from).collect())
        };

        let system = request.system_prompt.clone();

        let mut builder = CreateMessageRequest::builder(&request.model, request.max_tokens)
            .messages(api_messages);

        if let Some(sys) = system {
            builder = builder.system(sys);
        }
        if let Some(tools) = api_tools {
            builder = builder.tools(tools);
        }
        if let Some(t) = request.temperature {
            builder = builder.temperature(t as f32);
        }
        if let Some(p) = request.top_p {
            builder = builder.top_p(p as f32);
        }
        if let Some(k) = request.top_k {
            builder = builder.top_k(k);
        }
        if !request.stop_sequences.is_empty() {
            builder = builder.stop_sequences(request.stop_sequences.clone());
        }
        if let Some(tc) = request.thinking.clone() {
            builder = builder.thinking(tc);
        }

        builder.build()
    }

    fn map_stop_reason(s: &str) -> StopReason {
        match s {
            "end_turn" => StopReason::EndTurn,
            "stop_sequence" => StopReason::StopSequence,
            "max_tokens" => StopReason::MaxTokens,
            "tool_use" => StopReason::ToolUse,
            other => StopReason::Other(other.to_string()),
        }
    }

    /// Map an Anthropic-style SSE event to one or more StreamEvents.
    /// Returns a Vec because a complete non-streaming "message" event needs to be
    /// converted into multiple streaming events to simulate the stream.
    fn map_anthropic_event(value: Value) -> Vec<StreamEvent> {
        let event_type = match value.get("type").and_then(|v| v.as_str()) {
            Some(t) => t,
            None => return Vec::new(),
        };

        match event_type {
            "message_start" => {
                let id = match value.get("message").and_then(|m| m.get("id")).and_then(|v| v.as_str()) {
                    Some(s) => s.to_string(),
                    None => return Vec::new(),
                };
                let model = match value.get("message").and_then(|m| m.get("model")).and_then(|v| v.as_str()) {
                    Some(s) => s.to_string(),
                    None => return Vec::new(),
                };
                let usage = UsageInfo {
                    input_tokens: value.get("message").and_then(|m| m.get("usage")).and_then(|u| u.get("input_tokens")).and_then(|v| v.as_u64()).unwrap_or(0),
                    output_tokens: 0,
                    cache_creation_input_tokens: 0,
                    cache_read_input_tokens: 0,
                };
                vec![StreamEvent::MessageStart { id, model, usage }]
            }
            "content_block_start" => {
                let index = match value.get("index").and_then(|v| v.as_u64()) {
                    Some(i) => i as usize,
                    None => return Vec::new(),
                };
                let content_type = match value.get("content_block").and_then(|c| c.get("type")).and_then(|v| v.as_str()) {
                    Some(t) => t,
                    None => return Vec::new(),
                };

                let content_block = match content_type {
                    "text" => ContentBlock::Text {
                        text: String::new(),
                    },
                    "tool_use" => {
                        let id = match value.get("content_block").and_then(|c| c.get("id")).and_then(|v| v.as_str()) {
                            Some(s) => s.to_string(),
                            None => return Vec::new(),
                        };
                        let name = match value.get("content_block").and_then(|c| c.get("name")).and_then(|v| v.as_str()) {
                            Some(s) => s.to_string(),
                            None => return Vec::new(),
                        };
                        ContentBlock::ToolUse {
                            id,
                            name,
                            input: serde_json::Value::Object(Default::default()),
                        }
                    }
                    _ => return Vec::new(),
                };

                vec![StreamEvent::ContentBlockStart { index, content_block }]
            }
            "content_block_delta" => {
                let index = match value.get("index").and_then(|v| v.as_u64()) {
                    Some(i) => i as usize,
                    None => return Vec::new(),
                };
                let delta_type = match value.get("delta").and_then(|d| d.get("type")).and_then(|v| v.as_str()) {
                    Some(t) => t,
                    None => return Vec::new(),
                };

                match delta_type {
                    "text_delta" => {
                        let text = match value.get("delta").and_then(|d| d.get("text")).and_then(|v| v.as_str()) {
                            Some(s) => s.to_string(),
                            None => return Vec::new(),
                        };
                        vec![StreamEvent::TextDelta { index, text }]
                    }
                    "thinking_delta" => {
                        let thinking = match value.get("delta").and_then(|d| d.get("thinking")).and_then(|v| v.as_str()) {
                            Some(s) => s.to_string(),
                            None => return Vec::new(),
                        };
                        vec![StreamEvent::ThinkingDelta { index, thinking }]
                    }
                    "signature_delta" => {
                        let signature = match value.get("delta").and_then(|d| d.get("signature")).and_then(|v| v.as_str()) {
                            Some(s) => s.to_string(),
                            None => return Vec::new(),
                        };
                        vec![StreamEvent::SignatureDelta { index, signature }]
                    }
                    "input_json_delta" => {
                        let partial_json = match value.get("delta").and_then(|d| d.get("partial_json")).and_then(|v| v.as_str()) {
                            Some(s) => s.to_string(),
                            None => return Vec::new(),
                        };
                        vec![StreamEvent::InputJsonDelta { index, partial_json }]
                    }
                    _ => Vec::new(),
                }
            }
            "content_block_stop" => {
                let index = match value.get("index").and_then(|v| v.as_u64()) {
                    Some(i) => i as usize,
                    None => return Vec::new(),
                };
                vec![StreamEvent::ContentBlockStop { index }]
            }
            "message_delta" => {
                let stop_reason = value.get("delta")
                    .and_then(|d| d.get("stop_reason"))
                    .and_then(|v| v.as_str())
                    .map(Self::map_stop_reason);

                let usage = value.get("delta").and_then(|u| u.get("usage"))
                    .map(|u| UsageInfo {
                        input_tokens: u.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
                        output_tokens: u.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
                        cache_creation_input_tokens: u.get("cache_creation_input_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
                        cache_read_input_tokens: u.get("cache_read_input_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
                    });

                vec![StreamEvent::MessageDelta {
                    stop_reason,
                    usage,
                }]
            }
            "message_stop" => vec![StreamEvent::MessageStop],
            "error" => {
                let error_type = match value.get("error").and_then(|e| e.get("type")).and_then(|v| v.as_str()) {
                    Some(s) => s.to_string(),
                    None => return Vec::new(),
                };
                let message = match value.get("error").and_then(|e| e.get("message")).and_then(|v| v.as_str()) {
                    Some(s) => s.to_string(),
                    None => return Vec::new(),
                };
                vec![StreamEvent::Error { error_type, message }]
            }
            "ping" => Vec::new(),
            // Handle non-streaming "message" event type by expanding into multiple stream events
            "message" => {
                let mut events = Vec::new();

                // MessageStart
                let id = match value.get("id").and_then(|v| v.as_str()) {
                    Some(s) => s.to_string(),
                    None => return Vec::new(),
                };
                let model = match value.get("model").and_then(|v| v.as_str()) {
                    Some(s) => s.to_string(),
                    None => return Vec::new(),
                };
                let usage = UsageInfo {
                    input_tokens: value.get("usage").and_then(|u| u.get("input_tokens")).and_then(|v| v.as_u64()).unwrap_or(0),
                    output_tokens: value.get("usage").and_then(|u| u.get("output_tokens")).and_then(|v| v.as_u64()).unwrap_or(0),
                    cache_creation_input_tokens: 0,
                    cache_read_input_tokens: value.get("usage").and_then(|u| u.get("cache_read_input_tokens")).and_then(|v| v.as_u64()).unwrap_or(0),
                };
                events.push(StreamEvent::MessageStart { id, model, usage: usage.clone() });

                // Content blocks
                if let Some(content) = value.get("content").and_then(|c| c.as_array()) {
                    for (index, block) in content.iter().enumerate() {
                        let block_type = block.get("type").and_then(|v| v.as_str()).unwrap_or("text");

                        // ContentBlockStart
                        let content_block = match block_type {
                            "text" => ContentBlock::Text { text: String::new() },
                            "tool_use" => {
                                let id = block.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                                let name = block.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                                ContentBlock::ToolUse {
                                    id,
                                    name,
                                    input: block.get("input").cloned().unwrap_or(serde_json::Value::Object(Default::default())),
                                }
                            }
                            _ => ContentBlock::Text { text: String::new() },
                        };
                        events.push(StreamEvent::ContentBlockStart { index, content_block });

                        // ContentBlockDelta for text
                        if block_type == "text" {
                            if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                                events.push(StreamEvent::TextDelta { index, text: text.to_string() });
                            }
                        }

                        // ContentBlockStop
                        events.push(StreamEvent::ContentBlockStop { index });
                    }
                }

                // MessageDelta with stop_reason
                let stop_reason = value.get("stop_reason")
                    .and_then(|v| v.as_str())
                    .map(Self::map_stop_reason);

                events.push(StreamEvent::MessageDelta {
                    stop_reason,
                    usage: Some(usage),
                });

                // MessageStop
                events.push(StreamEvent::MessageStop);

                events
            }
            _ => Vec::new(),
        }
    }
}

#[async_trait]
impl LlmProvider for MinimaxProvider {
    fn id(&self) -> &ProviderId {
        &self.id
    }

    fn name(&self) -> &str {
        "MiniMax"
    }

    async fn create_message(
        &self,
        request: ProviderRequest,
    ) -> Result<ProviderResponse, ProviderError> {
        let mut stream = self.create_message_stream(request).await?;

        let mut id = String::from("unknown");
        let mut model = String::new();
        let mut text_parts: Vec<(usize, String)> = Vec::new();
        let mut content_blocks: Vec<ContentBlock> = Vec::new();
        let mut stop_reason = StopReason::EndTurn;
        let mut usage = UsageInfo::default();

        let mut tool_buffers: std::collections::HashMap<usize, (String, String, String)> =
            std::collections::HashMap::new();

        use futures::StreamExt;
        while let Some(result) = stream.next().await {
            match result {
                Err(e) => return Err(e),
                Ok(evt) => match evt {
                    StreamEvent::MessageStart {
                        id: msg_id,
                        model: msg_model,
                        usage: msg_usage,
                    } => {
                        id = msg_id;
                        model = msg_model;
                        usage = msg_usage;
                    }
                    StreamEvent::ContentBlockStart {
                        index,
                        content_block,
                    } => match content_block {
                        ContentBlock::Text { text } => {
                            text_parts.push((index, text));
                        }
                        ContentBlock::ToolUse {
                            id: tool_id,
                            name,
                            input: _,
                        } => {
                            tool_buffers.insert(index, (tool_id, name, String::new()));
                        }
                        other => {
                            content_blocks.push(other);
                        }
                    },
                    StreamEvent::TextDelta { index, text } => {
                        if let Some(entry) = text_parts.iter_mut().find(|(i, _)| *i == index) {
                            entry.1.push_str(&text);
                        }
                    }
                    StreamEvent::InputJsonDelta {
                        index,
                        partial_json,
                    } => {
                        if let Some((_, _, buf)) = tool_buffers.get_mut(&index) {
                            buf.push_str(&partial_json);
                        }
                    }
                    StreamEvent::ContentBlockStop { index } => {
                        if let Some((tool_id, name, json_buf)) = tool_buffers.remove(&index) {
                            let input = serde_json::from_str(&json_buf)
                                .unwrap_or(serde_json::Value::Object(Default::default()));
                            content_blocks.push(ContentBlock::ToolUse {
                                id: tool_id,
                                name,
                                input,
                            });
                        }
                    }
                    StreamEvent::MessageDelta {
                        stop_reason: sr,
                        usage: delta_usage,
                    } => {
                        if let Some(r) = sr {
                            stop_reason = r;
                        }
                        if let Some(u) = delta_usage {
                            usage.output_tokens += u.output_tokens;
                        }
                    }
                    StreamEvent::MessageStop => break,
                    StreamEvent::Error { error_type, message } => {
                        return Err(ProviderError::StreamError {
                            provider: self.id.clone(),
                            message: format!("[{}] {}", error_type, message),
                            partial_response: None,
                        });
                    }
                    _ => {}
                },
            }
        }

        text_parts.sort_by_key(|(i, _)| *i);
        let mut all_blocks: Vec<(usize, ContentBlock)> = text_parts
            .into_iter()
            .map(|(i, text)| (i, ContentBlock::Text { text }))
            .collect();
        for block in content_blocks {
            all_blocks.push((usize::MAX, block));
        }
        let final_content: Vec<ContentBlock> = all_blocks.into_iter().map(|(_, b)| b).collect();

        Ok(ProviderResponse {
            id,
            content: final_content,
            stop_reason,
            usage,
            model,
        })
    }

    async fn create_message_stream(
        &self,
        request: ProviderRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent, ProviderError>> + Send>>, ProviderError>
    {
        let api_request = Self::build_request(&request);

        let body = serde_json::to_value(&api_request)
            .map_err(|e| ProviderError::Other {
                provider: self.id.clone(),
                message: format!("Failed to serialize request: {}", e),
                status: None,
                body: None,
            })?;

        let url = format!("{}/v1/messages", self.api_base);
        let api_key = self.api_key.clone();
        let http_client = self.http_client.clone();
        let provider_id = self.id.clone();

        let resp = http_client
            .post(&url)
            .header("Authorization", format!("Bearer {}", api_key))
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .header("accept", "text/event-stream")
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::Other {
                provider: provider_id.clone(),
                message: format!("HTTP request failed: {}", e),
                status: None,
                body: None,
            })?;

        let status = resp.status().as_u16();
        if !resp.status().is_success() {
            let error_body = resp.text().await.unwrap_or_default();
            return Err(ProviderError::Other {
                provider: provider_id.clone(),
                message: format!("API error: {}", error_body),
                status: Some(status),
                body: Some(error_body),
            });
        }

        let provider_id_inner = provider_id.clone();
        let s = stream! {
            let byte_stream = resp.bytes_stream();
            let mut leftover = String::new();
            let mut current_event_type: Option<String> = None;

            use futures::StreamExt;
            let mut stream = std::pin::pin!(byte_stream);

            while let Some(chunk_result) = stream.next().await {
                match chunk_result {
                    Ok(chunk) => {
                        let text = String::from_utf8_lossy(&chunk);
                        let combined = if leftover.is_empty() {
                            text.to_string()
                        } else {
                            let mut s = std::mem::take(&mut leftover);
                            s.push_str(&text);
                            s
                        };

                        let mut lines: Vec<&str> = combined.split('\n').collect();
                        if !combined.ends_with('\n') {
                            leftover = lines.pop().unwrap_or("").to_string();
                        }

                        for line in lines {
                            let line = line.trim_end_matches('\r').trim();
                            if line.is_empty() {
                                continue;
                            }

                            // Handle SSE "event:" prefix (MiniMax format)
                            if let Some(event_type) = line.strip_prefix("event:") {
                                current_event_type = Some(event_type.trim().to_string());
                                continue;
                            }

                            // Handle SSE "data:" prefix
                            let data = if let Some(rest) = line.strip_prefix("data:") {
                                rest.trim()
                            } else {
                                // Try to parse raw JSON (non-streaming fallback)
                                line
                            };

                            match serde_json::from_str::<Value>(data) {
                                Ok(value) => {
                                    // If we have a stored event type from "event:" line,
                                    // inject it into the JSON if not already present
                                    let value_with_type = if let Some(evt_type) = current_event_type.take() {
                                        if value.get("type").is_none() {
                                            let mut obj = value.as_object().unwrap().clone();
                                            obj.insert("type".to_string(), serde_json::Value::String(evt_type));
                                            serde_json::Value::Object(obj)
                                        } else {
                                            value
                                        }
                                    } else {
                                        value
                                    };

                                    for stream_evt in Self::map_anthropic_event(value_with_type) {
                                        yield Ok(stream_evt);
                                    }
                                }
                                Err(e) => {
                                    yield Err(ProviderError::Other {
                                        provider: provider_id_inner.clone(),
                                        message: format!("Failed to parse event: {}", e),
                                        status: None,
                                        body: None,
                                    });
                                }
                            }
                        }
                    }
                    Err(e) => {
                        yield Err(ProviderError::Other {
                            provider: provider_id_inner.clone(),
                            message: format!("Stream error: {}", e),
                            status: None,
                            body: None,
                        });
                    }
                }
            }
        };

        Ok(Box::pin(s))
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        let minimax_id = ProviderId::new(ProviderId::MINIMAX);
        Ok(vec![
            ModelInfo {
                id: ModelId::new("MiniMax-M2.7"),
                provider_id: minimax_id.clone(),
                name: "MiniMax-M2.7".to_string(),
                context_window: 128_000,
                max_output_tokens: 8192,
            },
        ])
    }

    async fn health_check(&self) -> Result<ProviderStatus, ProviderError> {
        Ok(ProviderStatus::Healthy)
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            streaming: true,
            tool_calling: true,
            thinking: false,
            image_input: false,
            pdf_input: false,
            audio_input: false,
            video_input: false,
            caching: false,
            structured_output: true,
            system_prompt_style: SystemPromptStyle::TopLevel,
        }
    }
}
