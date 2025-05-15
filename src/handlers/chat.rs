use chrono::Utc;
use futures_util::future;
use futures_util::stream::{self, Stream, StreamExt};
use nanoid::nanoid;
use poe_api_process::{EventResponse, EventType, PoeError};
use salvo::http::header;
use salvo::prelude::*;
use serde_json::json;
use std::pin::Pin;
use std::time::Instant;
use tracing::{debug, error, info};

use crate::types::*;
use crate::poe_client::{PoeClientWrapper, create_query_request};
use crate::utils::{format_bytes_length, format_duration,get_cached_config};

// Helper function to format content with <think> tags for non-streaming and ReplaceResponse cases
fn format_reasoning_content(content: &str) -> String {
    let mut result = String::new();
    let mut lines = content.lines().peekable();
    let mut in_reasoning_block = false;
    let mut reasoning_text = String::new();
    const THINKING_MARKER_1: &str = "Thinking...";
    const THINKING_MARKER_2: &str = "*Thinking...*";

    while let Some(line_str) = lines.next() {
        let trimmed_line_for_marker_check = line_str.trim(); // Used for "Thinking..." check

        if trimmed_line_for_marker_check == THINKING_MARKER_1 || trimmed_line_for_marker_check == THINKING_MARKER_2 {
            if in_reasoning_block {
                // Already in a block, treat "Thinking..." as part of the ongoing reasoning content
                if !reasoning_text.is_empty() {
                    reasoning_text.push_str(line_str);
                    reasoning_text.push('\n');
                } else {
                    // This case (in_reasoning_block true but reasoning_text empty) implies
                    // "Thinking..." -> empty lines -> "Thinking...". Treat second as content.
                    // Or, if it's just "Thinking..." -> "Thinking...", the first one is consumed,
                    // and this one starts a new block or becomes content if no ">" follows.
                    // For simplicity, if already in_reasoning_block, it's content.
                    // This path might need refinement if nested <think> is a concern or
                    // if "Thinking..." itself should never be inside <think>.
                    // Current logic: "Thinking..." line is consumed if it starts a block.
                    // If another "Thinking..." appears while processing ">" lines, it's content.
                    result.push_str(line_str);
                    result.push('\n');
                }
            } else {
                // Start of a new reasoning block
                in_reasoning_block = true;
                reasoning_text.clear();
                // The "Thinking..." line itself is consumed and not included in <think> content.
                // Skip immediate blank lines after "Thinking..."
                while let Some(next_line) = lines.peek() {
                    if next_line.trim().is_empty() {
                        lines.next(); // Consume the blank line
                    } else {
                        break;
                    }
                }
            }
        } else if in_reasoning_block {
            if line_str.starts_with('>') {
                let content_start_index = if line_str.starts_with("> ") { 2 } else { 1 };
                reasoning_text.push_str(&line_str[content_start_index..]);
                reasoning_text.push('\n');
            } else {
                // Line does not start with '>', so reasoning block ends.
                if !reasoning_text.is_empty() {
                    result.push_str("<think>");
                    result.push_str(reasoning_text.trim_end_matches('\n'));
                    result.push_str("</think>\n");
                    reasoning_text.clear();
                }
                in_reasoning_block = false;
                result.push_str(line_str); // Current line is normal content
                result.push('\n');
            }
        } else {
            result.push_str(line_str);
            result.push('\n');
        }
    }

    // If the input ends while in a reasoning block
    if in_reasoning_block && !reasoning_text.is_empty() {
        result.push_str("<think>");
        result.push_str(reasoning_text.trim_end_matches('\n'));
        result.push_str("</think>\n");
    }

    // Preserve original ending: if original ended with \n, result should too.
    // Otherwise, trim the final \n if result has one and original didn't.
    let final_result = result.trim_end_matches('\n').to_string();
    if content.ends_with('\n') && !final_result.is_empty() {
        format!("{}\n", final_result)
    } else {
        final_result
    }
}

// Struct and methods for processing thinking blocks in streaming mode
struct PoeThinkingStreamProcessor {
    active: bool,       // True if "Thinking..." has been encountered, looking for ">" or end of block
    in_think_tag: bool, // True if <think> has been emitted and </think> has not
    buffer: String,     // Accumulates current line being processed
}

impl PoeThinkingStreamProcessor {
    fn new() -> Self {
        PoeThinkingStreamProcessor {
            active: false,
            in_think_tag: false,
            buffer: String::new(),
        }
    }

    // Processes a single, complete line (must end with \n)
    fn process_line_internal(&mut self, line_with_newline: &str) -> String {
        let mut processed_line_output = String::new();
        // Operate on the line without its trailing newline for logic, then add it back if needed.
        let line_trimmed_content = line_with_newline.trim_end_matches('\n');
        let trimmed_line_for_marker_check = line_trimmed_content.trim(); // For marker check

        if !self.active {
            if trimmed_line_for_marker_check == "Thinking..." || trimmed_line_for_marker_check == "*Thinking...*" {
                self.active = true;
                // "Thinking..." line is consumed.
            } else {
                processed_line_output.push_str(line_with_newline);
            }
        } else { // self.active is true
            if !self.in_think_tag { // Looking for ">" or empty lines after "Thinking..."
                if line_trimmed_content.is_empty() {
                    // Still active, skipping empty line after "Thinking...". Consumed.
                } else if line_trimmed_content.starts_with('>') {
                    processed_line_output.push_str("<think>");
                    self.in_think_tag = true;
                    let content_start_index = if line_trimmed_content.starts_with("> ") { 2 } else { 1 };
                    processed_line_output.push_str(&line_trimmed_content[content_start_index..]);
                    processed_line_output.push('\n'); // Line content gets its newline
                } else {
                    // "Thinking..." was active, but next non-empty line isn't ">".
                    // Thinking block is empty/false alarm. Deactivate.
                    self.active = false;
                    processed_line_output.push_str(line_with_newline); // Output current line as normal
                }
            } else { // self.in_think_tag is true
                if line_trimmed_content.starts_with('>') {
                    let content_start_index = if line_trimmed_content.starts_with("> ") { 2 } else { 1 };
                    processed_line_output.push_str(&line_trimmed_content[content_start_index..]);
                    processed_line_output.push('\n'); // Line content gets its newline
                } else {
                    // Line doesn't start with ">", so thinking block ends.
                    processed_line_output.push_str("</think>\n"); // </think> gets its own newline
                    self.in_think_tag = false;
                    self.active = false; // Deactivate
                    processed_line_output.push_str(line_with_newline); // Output current line as normal
                }
            }
        }
        processed_line_output
    }

    // Process an incoming chunk of text, may contain multiple lines or partial lines
    pub fn process_chunk(&mut self, chunk: &str) -> String {
        self.buffer.push_str(chunk);
        let mut output_for_this_chunk = String::new();

        while let Some(newline_pos) = self.buffer.find('\n') {
            let line_to_process = self.buffer.drain(..=newline_pos).collect::<String>();
            output_for_this_chunk.push_str(&self.process_line_internal(&line_to_process));
        }
        // Remaining in buffer is a partial line, to be processed by next call or finalize()
        output_for_this_chunk
    }

    // Called at the end of the stream to process any remaining buffer and close tags
    pub fn finalize(&mut self) -> String {
        let mut final_output = String::new();
        if !self.buffer.is_empty() {
            let last_line_from_buffer = self.buffer.drain(..).collect::<String>();
            // Ensure it's processed as if it had a newline for consistent logic,
            // then trim the added newline if the original buffer didn't end with one.
            let needs_trailing_newline_in_original_buffer = last_line_from_buffer.ends_with('\n');
            let line_to_process_for_logic = if needs_trailing_newline_in_original_buffer {
                last_line_from_buffer.clone()
            } else {
                format!("{}\n", last_line_from_buffer) // Temporarily add \n
            };
            
            let processed_buffered_content = self.process_line_internal(&line_to_process_for_logic);

            if needs_trailing_newline_in_original_buffer {
                final_output.push_str(&processed_buffered_content);
            } else {
                final_output.push_str(processed_buffered_content.trim_end_matches('\n'));
            }
        }

        if self.in_think_tag {
            // If an open <think> tag exists, close it.
            // Content inside <think> should have its newlines handled by process_line_internal.
            // Add newline after </think> if final_output is not empty and doesn't end with one,
            // or if final_output is empty (meaning </think> is on its own line).
            if !final_output.is_empty() && !final_output.ends_with('\n') {
                 final_output.push('\n'); // Ensure content before </think> ends with newline
            }
            final_output.push_str("</think>");
            // If final_output was empty, </think> is by itself. If it's the very end of all text,
            // a final \n might be desired by consumers, but usually [DONE] follows.
            // Let's ensure </think> itself doesn't add an extra \n if not needed.
            // The `process_line_internal` adds \n after content. `</think>\n` is common.
            // If `final_output` is empty, `</think>` is the only thing.
            // If `final_output` has content, it ends with `\n`. So `</think>` follows.
            // Let's make sure </think> is followed by a newline if it's not the absolute end of all text.
            // The `create_stream_chunk` will send this. If it's the last text before [DONE],
            // it should probably end with a newline.
            // The `</think>\n` in `process_line_internal` handles cases where content follows.
            // Here, it's the absolute end of the <think> block.
            if !final_output.ends_with('\n') { // If, after appending </think>, it doesn't end with \n
                final_output.push('\n'); // Add one.
            }

            self.in_think_tag = false; // Mark as closed
        }
        self.active = false; // Reset active state
        final_output
    }
}


#[handler]
pub async fn chat_completions(req: &mut Request, res: &mut Response) {
    let start_time = Instant::now();
    info!("📝 收到新的聊天完成請求");

    let max_size: usize = std::env::var("MAX_REQUEST_SIZE")
        .unwrap_or_else(|_| "1073741824".to_string())
        .parse()
        .unwrap_or(1024 * 1024 * 1024);

    // 從緩存獲取 models.yaml 配置
    let config = get_cached_config().await;
    debug!("🔧 從緩存獲取配置 | 啟用狀態: {:?}", config.enable);

    let access_key = match req.headers().get("Authorization") {
        Some(auth) => {
            let auth_str = auth.to_str().unwrap_or("");
            if let Some(stripped) = auth_str.strip_prefix("Bearer ") {
                debug!("🔑 驗證令牌長度: {}", stripped.len());
                stripped.to_string()
            } else {
                error!("❌ 無效的授權格式");
                res.status_code(StatusCode::UNAUTHORIZED);
                res.render(Json(json!({ "error": "無效的 Authorization" })));
                return;
            }
        }
        None => {
            error!("❌ 缺少授權標頭");
            res.status_code(StatusCode::UNAUTHORIZED);
            res.render(Json(json!({ "error": "缺少 Authorization" })));
            return;
        }
    };

    let chat_request = match req.payload_with_max_size(max_size).await {
        Ok(bytes) => match serde_json::from_slice::<ChatCompletionRequest>(bytes) {
            Ok(req) => {
                debug!(
                    "📊 請求解析成功 | 模型: {} | 訊息數量: {} | 是否串流: {:?}",
                    req.model,
                    req.messages.len(),
                    req.stream
                );
                req
            }
            Err(e) => {
                error!("❌ JSON 解析失敗: {}", e);
                res.status_code(StatusCode::BAD_REQUEST);
                res.render(Json(OpenAIErrorResponse {
                    error: OpenAIError {
                        message: format!("JSON 解析失敗: {}", e),
                        r#type: "invalid_request_error".to_string(),
                        code: "parse_error".to_string(),
                        param: None,
                    },
                }));
                return;
            }
        },
        Err(e) => {
            error!("❌ 請求大小超過限制或讀取失敗: {}", e);
            res.status_code(StatusCode::PAYLOAD_TOO_LARGE);
            res.render(Json(OpenAIErrorResponse {
                error: OpenAIError {
                    message: format!("請求大小超過限制 ({} bytes) 或讀取失敗: {}", max_size, e),
                    r#type: "invalid_request_error".to_string(),
                    code: "payload_too_large".to_string(),
                    param: None,
                },
            }));
            return;
        }
    };

    // 尋找映射的原始模型名稱
    let (display_model, original_model) = if config.enable.unwrap_or(false) {
        let requested_model = chat_request.model.clone();

        // 檢查當前請求的模型是否是某個映射的目標
        let mapping_entry = config.models.iter().find(|(_, cfg)| {
            if let Some(mapping) = &cfg.mapping {
                mapping.to_lowercase() == requested_model.to_lowercase()
            } else {
                false
            }
        });

        if let Some((original_name, _)) = mapping_entry {
            // 如果找到映射，使用原始模型名稱
            debug!("🔄 反向模型映射: {} -> {}", requested_model, original_name);
            (requested_model, original_name.clone())
        } else {
            // 如果沒找到映射，檢查是否有直接映射配置
            if let Some(model_config) = config.models.get(&requested_model) {
                if let Some(mapped_name) = &model_config.mapping {
                    debug!("🔄 直接模型映射: {} -> {}", requested_model, mapped_name);
                    (requested_model.clone(), requested_model)
                } else {
                    // 沒有映射配置，使用原始名稱
                    (requested_model.clone(), requested_model)
                }
            } else {
                // 完全沒有相關配置，使用原始名稱
                (requested_model.clone(), requested_model)
            }
        }
    } else {
        // 配置未啟用，直接使用原始名稱
        (chat_request.model.clone(), chat_request.model.clone())
    };

    info!("🤖 使用模型: {} (原始: {})", display_model, original_model);

    let client = PoeClientWrapper::new(&original_model, &access_key);

    let query_request: poe_api_process::QueryRequest = create_query_request(
        &original_model,
        chat_request.messages,
        chat_request.temperature,
        chat_request.tools,
    ).await;

    let stream = chat_request.stream.unwrap_or(false);
    debug!("🔄 請求模式: {}", if stream { "串流" } else { "非串流" });

    match client.stream_request(query_request).await {
        Ok(event_stream) => {
            if stream {
                handle_stream_response(res, event_stream, &display_model).await;
            } else {
                handle_non_stream_response(res, event_stream, &display_model).await;
            }
        }
        Err(e) => {
            error!("❌ 建立串流請求失敗: {}", e);
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(json!({ "error": e.to_string() })));
        }
    }

    let duration = start_time.elapsed();
    info!("✅ 請求處理完成 | 耗時: {}", format_duration(duration));
}

fn convert_poe_error_to_openai(
    error: &poe_api_process::types::ErrorResponse,
) -> (StatusCode, OpenAIErrorResponse) {
    debug!("🔄 轉換錯誤響應 | 錯誤文本: {}", error.text);

    let (status, error_type, code) = if error.text.contains("Internal server error") {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            "internal_error",
        )
    } else if error.text.contains("rate limit") {
        (
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limit_exceeded",
            "rate_limit_exceeded",
        )
    } else if error.text.contains("Invalid token") || error.text.contains("Unauthorized") {
        (StatusCode::UNAUTHORIZED, "invalid_auth", "invalid_api_key")
    } else if error.text.contains("Bot does not exist") {
        (StatusCode::NOT_FOUND, "model_not_found", "model_not_found")
    } else {
        (StatusCode::BAD_REQUEST, "invalid_request", "bad_request")
    };

    debug!(
        "📋 錯誤轉換結果 | 狀態碼: {} | 錯誤類型: {}",
        status.as_u16(),
        error_type
    );

    (
        status,
        OpenAIErrorResponse {
            error: OpenAIError {
                message: error.text.clone(),
                r#type: error_type.to_string(),
                code: code.to_string(),
                param: None,
            },
        },
    )
}

async fn handle_stream_response(
    res: &mut Response,
    mut event_stream: Pin<Box<dyn Stream<Item = Result<EventResponse, PoeError>> + Send>>,
    model: &str,
) {
    let start_time = Instant::now();
    let id = nanoid!(10);
    let created = Utc::now().timestamp();
    let model = model.to_string(); // 轉換為擁有的 String

    info!("🌊 開始處理串流響應 | ID: {} | 模型: {}", id, model);

    res.headers_mut()
        .insert(header::CONTENT_TYPE, "text/event-stream".parse().unwrap());
    res.headers_mut()
        .insert(header::CACHE_CONTROL, "no-cache".parse().unwrap());
    res.headers_mut()
        .insert(header::CONNECTION, "keep-alive".parse().unwrap());

    let mut replace_response = false;
    let mut full_content = String::new();
    let mut first_two_events = Vec::new();

    debug!("🔍 檢查初始事件");
    for _ in 0..2 {
        if let Some(Ok(event)) = event_stream.next().await {
            debug!("📥 收到初始事件: {:?}", event.event);
            first_two_events.push(event);
        }
    }

    for event in first_two_events {
        match event.event {
            EventType::ReplaceResponse => {
                debug!("🔄 檢測到 ReplaceResponse 模式");
                replace_response = true;
                if let Some(data) = event.data {
                    full_content = data.text;
                }
            }
            EventType::Text => {
                if let Some(data) = event.data {
                    if !replace_response {
                        full_content.push_str(&data.text);
                    }
                }
            }
            EventType::Json => {
                debug!("📝 收到 JSON 事件");
                // 檢查是否包含工具調用
                if let Some(tool_calls) = event.tool_calls {
                    debug!("🔧 收到工具調用，數量: {}", tool_calls.len());
                    // 在流式模式下，我們會在後續處理中處理工具調用
                }
            }
            EventType::Error => {
                if !replace_response {
                    if let Some(error) = event.error {
                        error!("❌ 串流處理錯誤: {}", error.text);
                        let (status, error_response) = convert_poe_error_to_openai(&error);
                        res.status_code(status);
                        res.render(Json(error_response));
                        return;
                    }
                }
            }
            EventType::Done => {
                debug!("✅ 初始事件處理完成");
                break;
            }
        }
    }

    let id_for_log = id.clone(); // 為最後的日誌克隆一個副本

    if replace_response {
        debug!("🔄 使用 ReplaceResponse 處理模式");
        let processed_stream = {
            let id = id.clone(); // 為閉包克隆一個副本
            let model = model.clone(); // 為閉包克隆一個副本
            let initial_content_for_handler = full_content.clone(); // 複製初始內容以傳遞

            stream::once(async move {
                // 將初始內容傳遞給 handle_replace_response
                let original_content =
                    handle_replace_response(event_stream, initial_content_for_handler).await;
                debug!(
                    "📤 ReplaceResponse 原始內容長度: {}",
                    format_bytes_length(original_content.len())
                );
                let content = format_reasoning_content(&original_content);
                debug!(
                    "📤 ReplaceResponse 處理後內容長度: {}",
                    format_bytes_length(content.len())
                );

                let content_chunk = create_stream_chunk(&id, created, &model, &content, None);
                let content_json = serde_json::to_string(&content_chunk).unwrap();
                let content_message = format!("data: {}\n\n", content_json);

                let final_chunk =
                    create_stream_chunk(&id, created, &model, "", Some("stop".to_string()));
                let final_json = serde_json::to_string(&final_chunk).unwrap();
                let final_message = format!(
                    "{}data: {}\n\ndata: [DONE]\n\n",
                    content_message, final_json
                );

                Ok::<_, std::convert::Infallible>(final_message)
            })
        };

        res.stream(processed_stream);
    } else {
        debug!("🔄 使用標準串流處理模式");

        // Initialize the thinking processor and process the initial full_content
        let mut stream_processor = PoeThinkingStreamProcessor::new();
        let processed_initial_text = stream_processor.process_chunk(&full_content);
        // Note: any remaining buffer in stream_processor is for the *next* chunk from event_stream.
        // finalize() is not called here as the stream continues.

        let initial_chunk = create_stream_chunk(&id, created, &model, &processed_initial_text, None);
        let initial_chunk_json = serde_json::to_string(&initial_chunk).unwrap();
        let initial_message = format!("data: {}\n\n", initial_chunk_json);

        let processed_stream = {
            let id_clone = id.clone();
            let model_clone = model.clone();

            stream::once(future::ready(Ok::<_, std::convert::Infallible>(
                initial_message,
            )))
            .chain(stream::unfold(
                // Pass the processor with its current state into unfold
                (event_stream, false, stream_processor),
                move |(mut event_stream, mut is_done, mut processor)| { // Use 'processor'
                    let id = id_clone.clone();
                    let model = model_clone.clone();

                    async move {
                        if is_done {
                            debug!("✅ 串流處理完成 (unfold loop end)");
                            return None;
                        }
                        match event_stream.next().await {
                            Some(Ok(event)) => match event.event {
                                EventType::Text => {
                                    if let Some(data) = event.data {
                                        let processed_text_chunk = processor.process_chunk(&data.text);
                                        if !processed_text_chunk.is_empty() {
                                            let chunk = create_stream_chunk(
                                                &id, created, &model, &processed_text_chunk, None,
                                            );
                                            let chunk_json = serde_json::to_string(&chunk).unwrap();
                                            Some((
                                                Ok(format!("data: {}\n\n", chunk_json)),
                                                (event_stream, is_done, processor),
                                            ))
                                        } else {
                                            // No output from processor for this chunk, continue
                                            Some((Ok(String::new()), (event_stream, is_done, processor)))
                                        }
                                    } else {
                                        Some((Ok(String::new()), (event_stream, is_done, processor)))
                                    }
                                }
                                EventType::Json => {
                                    // 處理工具調用事件
                                    if let Some(tool_calls) = event.tool_calls {
                                        debug!("🔧 處理工具調用，數量: {}", tool_calls.len());

                                        // 創建包含工具調用的 delta
                                        let tool_delta = Delta {
                                            role: Some("assistant".to_string()),
                                            content: None,
                                            refusal: None,
                                            tool_calls: Some(tool_calls),
                                        };

                                        // 創建包含工具調用的 chunk
                                        let tool_chunk = ChatCompletionChunk {
                                            id: format!("chatcmpl-{}", id),
                                            object: "chat.completion.chunk".to_string(),
                                            created,
                                            model: model.to_string(),
                                            choices: vec![Choice {
                                                index: 0,
                                                delta: tool_delta,
                                                finish_reason: Some("tool_calls".to_string()),
                                            }],
                                        };

                                        let tool_chunk_json =
                                            serde_json::to_string(&tool_chunk).unwrap();
                                        debug!("📤 發送工具調用 chunk");

                                        Some((
                                            Ok(format!("data: {}\n\n", tool_chunk_json)),
                                            (event_stream, is_done, processor),
                                        ))
                                    } else {
                                        debug!("⏭️ 收到 JSON 事件但沒有工具調用");
                                        Some((Ok(String::new()), (event_stream, is_done, processor)))
                                    }
                                }
                                EventType::Error => {
                                    if let Some(error) = event.error {
                                        error!("❌ 串流處理錯誤: {}", error.text);
                                        // Finalize processor state before erroring out, though its output might be lost
                                        let _ = processor.finalize();
                                        let error_chunk_val = json!({ // Renamed to avoid conflict
                                            "error": {
                                                "message": error.text,
                                                "type": "stream_error",
                                                "code": "stream_error"
                                            }
                                        });
                                        let error_message = format!(
                                            "data: {}\n\ndata: [DONE]\n\n",
                                            serde_json::to_string(&error_chunk_val).unwrap()
                                        );
                                        Some((Ok(error_message), (event_stream, true, processor)))
                                    } else {
                                        Some((Ok(String::new()), (event_stream, true, processor)))
                                    }
                                }
                                EventType::Done => {
                                    debug!("✅ 串流完成 (EventType::Done received)");
                                    is_done = true; // Mark as done for the unfold loop

                                    let final_processed_text = processor.finalize();
                                    let mut messages_to_send = Vec::new();

                                    if !final_processed_text.is_empty() {
                                        let content_chunk = create_stream_chunk(&id, created, &model, &final_processed_text, None);
                                        messages_to_send.push(format!("data: {}\n\n", serde_json::to_string(&content_chunk).unwrap()));
                                    }

                                    let final_stop_chunk = create_stream_chunk(
                                        &id,
                                        created,
                                        &model,
                                        "", // No text content for the stop chunk itself
                                        Some("stop".to_string()),
                                    );
                                    messages_to_send.push(format!("data: {}\n\n", serde_json::to_string(&final_stop_chunk).unwrap()));
                                    messages_to_send.push("data: [DONE]\n\n".to_string());
                                    
                                    Some((
                                        Ok(messages_to_send.join("")),
                                        (event_stream, is_done, processor),
                                    ))
                                }
                                _ => {
                                    debug!("⏭️ 忽略其他事件類型");
                                    Some((Ok(String::new()), (event_stream, is_done, processor)))
                                }
                            },
                            _ => None,
                        }
                    }
                },
            ))
        };

        res.stream(processed_stream);
    }

    let duration = start_time.elapsed();
    info!(
        "✅ 串流響應處理完成 | ID: {} | 耗時: {}",
        id_for_log,
        format_duration(duration)
    );
}

async fn handle_non_stream_response(
    res: &mut Response,
    mut event_stream: Pin<Box<dyn Stream<Item = Result<EventResponse, PoeError>> + Send>>,
    model: &str,
) {
    let start_time = Instant::now();
    let id = nanoid!(10);

    info!("📦 開始處理非串流響應 | ID: {} | 模型: {}", id, model);

    let mut replace_response = false;
    let mut full_content = String::new();
    let mut first_two_events = Vec::new();
    let mut accumulated_tool_calls: Vec<poe_api_process::types::ToolCall> = Vec::new();

    debug!("🔍 檢查初始事件");
    for _ in 0..2 {
        if let Some(Ok(event)) = event_stream.next().await {
            debug!("📥 收到初始事件: {:?}", event.event);
            first_two_events.push(event);
        }
    }

    for event in first_two_events {
        match event.event {
            EventType::ReplaceResponse => {
                debug!("🔄 檢測到 ReplaceResponse 模式");
                replace_response = true;
                if let Some(data) = event.data {
                    full_content = data.text;
                }
            }
            EventType::Text => {
                if let Some(data) = event.data {
                    if !replace_response {
                        full_content.push_str(&data.text);
                    }
                }
            }
            EventType::Json => {
                debug!("📝 收到 JSON 事件");
                // 檢查是否包含工具調用
                if let Some(tool_calls) = event.tool_calls {
                    debug!("🔧 收到工具調用，數量: {}", tool_calls.len());
                    accumulated_tool_calls.extend(tool_calls);
                }
            }
            EventType::Error => {
                if let Some(error) = event.error {
                    error!("❌ 處理錯誤: {}", error.text);
                    let (status, error_response) = convert_poe_error_to_openai(&error);
                    res.status_code(status);
                    res.render(Json(error_response));
                    return;
                }
            }
            EventType::Done => {
                debug!("✅ 初始事件處理完成");
                break;
            }
        }
    }

    if replace_response {
        debug!("🔄 使用 ReplaceResponse 處理模式 (非串流)");
        // 將初始內容傳遞給 handle_replace_response
        let initial_content_for_handler = full_content.clone();
        let original_content = handle_replace_response(event_stream, initial_content_for_handler).await;
        debug!(
            "📤 ReplaceResponse 原始內容長度 (非串流): {}",
            format_bytes_length(original_content.len())
        );
        let content = format_reasoning_content(&original_content);
        debug!(
            "📤 ReplaceResponse 處理後內容長度 (非串流): {}",
            format_bytes_length(content.len())
        );


        // 在 ReplaceResponse 模式下，我們不處理工具調用
        let response = ChatCompletionResponse {
            id: format!("chatcmpl-{}", nanoid!(10)),
            object: "chat.completion".to_string(),
            created: Utc::now().timestamp(),
            model: model.to_string(),
            choices: vec![CompletionChoice {
                index: 0,
                message: CompletionMessage {
                    role: "assistant".to_string(),
                    content,
                    refusal: None,
                    tool_calls: None,
                },
                logprobs: None,
                finish_reason: Some("stop".to_string()),
            }],
            usage: None,
        };

        res.render(Json(response));
    } else {
        debug!("🔄 使用標準非串流處理模式");
        let mut response_content = full_content;

        while let Some(Ok(event)) = event_stream.next().await {
            match event.event {
                EventType::Text => {
                    if let Some(data) = event.data {
                        response_content.push_str(&data.text);
                    }
                }
                EventType::Json => {
                    // 檢查是否包含工具調用
                    if let Some(tool_calls) = event.tool_calls {
                        debug!("🔧 處理工具調用，數量: {}", tool_calls.len());
                        accumulated_tool_calls.extend(tool_calls);
                    }
                }
                EventType::Error => {
                    if let Some(error) = event.error {
                        error!("❌ 處理錯誤: {}", error.text);
                        let (status, error_response) = convert_poe_error_to_openai(&error);
                        res.status_code(status);
                        res.render(Json(error_response));
                        return;
                    }
                }
                EventType::Done => {
                    debug!("✅ 回應收集完成");
                    break;
                }
                _ => {
                    debug!("⏭️ 忽略其他事件類型");
                }
            }
        }

        // 確定 finish_reason
        let finish_reason = if !accumulated_tool_calls.is_empty() {
            "tool_calls".to_string()
        } else {
            "stop".to_string()
        };
        
        let processed_response_content = format_reasoning_content(&response_content);

        debug!(
            "📤 準備發送回應 | 原始內容長度: {} | 處理後內容長度: {} | 工具調用數量: {} | 完成原因: {}",
            format_bytes_length(response_content.len()),
            format_bytes_length(processed_response_content.len()),
            accumulated_tool_calls.len(),
            finish_reason
        );

        let response = ChatCompletionResponse {
            id: format!("chatcmpl-{}", id),
            object: "chat.completion".to_string(),
            created: Utc::now().timestamp(),
            model: model.to_string(),
            choices: vec![CompletionChoice {
                index: 0,
                message: CompletionMessage {
                    role: "assistant".to_string(),
                    content: processed_response_content, // Use processed content
                    refusal: None,
                    tool_calls: if accumulated_tool_calls.is_empty() {
                        None
                    } else {
                        Some(accumulated_tool_calls)
                    },
                },
                logprobs: None,
                finish_reason: Some(finish_reason),
            }],
            usage: None,
        };

        res.render(Json(response));
    }

    let duration = start_time.elapsed();
    info!(
        "✅ 非串流響應處理完成 | ID: {} | 耗時: {}",
        id,
        format_duration(duration)
    );
}

async fn handle_replace_response(
    mut event_stream: Pin<Box<dyn Stream<Item = Result<EventResponse, PoeError>> + Send>>,
    initial_content: String,
) -> String {
    let start_time = Instant::now();
    debug!(
        "🔄 開始處理 ReplaceResponse | 初始內容長度: {}",
        format_bytes_length(initial_content.len())
    ); // 更新日誌

    use std::sync::{Arc, Mutex};
    use tokio::sync::mpsc;

    let (tx, mut rx) = mpsc::channel(1);
    // 使用傳入的 initial_content 初始化 last_content
    let last_content = Arc::new(Mutex::new(initial_content));
    let accumulated_text = Arc::new(Mutex::new(String::new()));

    let last_content_clone = Arc::clone(&last_content);
    let accumulated_text_clone = Arc::clone(&accumulated_text);

    tokio::spawn(async move {
        debug!("🏃 啟動背景事件收集任務");
        let mut done_received = false;

        while !done_received {
            match event_stream.next().await {
                Some(Ok(event)) => match event.event {
                    EventType::ReplaceResponse => {
                        if let Some(data) = event.data {
                            debug!(
                                "📝 更新替換內容 | 長度: {}",
                                format_bytes_length(data.text.len())
                            );
                            *last_content_clone.lock().unwrap() = data.text;
                        }
                    }
                    EventType::Text => {
                        if let Some(data) = event.data {
                            debug!(
                                "📝 累加文本內容 | 長度: {}",
                                format_bytes_length(data.text.len())
                            );
                            accumulated_text_clone.lock().unwrap().push_str(&data.text);
                        }
                    }
                    EventType::Done => {
                        debug!("✅ 收到完成信號");
                        done_received = true;
                        let _ = tx.send(()).await;
                    }
                    _ => {
                        debug!("⏭️ 忽略其他事件類型");
                    }
                },
                Some(Err(e)) => {
                    error!("❌ 事件處理錯誤: {:?}", e);
                }
                None => {
                    debug!("⚠️ 事件流結束但未收到完成信號");
                    break;
                }
            }
        }
        debug!("👋 背景任務結束");
    });

    let _ = rx.recv().await;

    let final_content = {
        let replace_content = last_content.lock().unwrap();
        let text_content = accumulated_text.lock().unwrap();

        if text_content.len() > replace_content.len() {
            debug!(
                "📊 選擇累加文本內容 (較長) | 累加長度: {} | 替換長度: {}",
                format_bytes_length(text_content.len()),
                format_bytes_length(replace_content.len())
            );
            text_content.clone()
        } else {
            debug!(
                "📊 選擇替換內容 (較長或相等) | 替換長度: {} | 累加長度: {}",
                format_bytes_length(replace_content.len()),
                format_bytes_length(text_content.len())
            );
            replace_content.clone()
        }
    };

    let duration = start_time.elapsed();
    debug!(
        "✅ ReplaceResponse 處理完成 | 最終內容長度: {} | 耗時: {}",
        format_bytes_length(final_content.len()),
        format_duration(duration)
    );

    final_content
}

fn create_stream_chunk(
    id: &str,
    created: i64,
    model: &str,
    content: &str,
    finish_reason: Option<String>,
) -> ChatCompletionChunk {
    let mut delta = Delta {
        role: None,
        content: None,
        refusal: None,
        tool_calls: None,
    };

    if content.is_empty() && finish_reason.is_none() {
        delta.role = Some("assistant".to_string());
    } else {
        delta.content = Some(content.to_string());
    }

    debug!(
        "🔧 創建串流片段 | ID: {} | 內容長度: {}",
        id,
        if let Some(content) = &delta.content {
            format_bytes_length(content.len())
        } else {
            "0 B".to_string()
        }
    );

    ChatCompletionChunk {
        id: format!("chatcmpl-{}", id),
        object: "chat.completion.chunk".to_string(),
        created,
        model: model.to_string(),
        choices: vec![Choice {
            index: 0,
            delta,
            finish_reason,
        }],
    }
}
