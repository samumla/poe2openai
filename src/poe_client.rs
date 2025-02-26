use futures_util::Stream;
use poe_api_process::{EventResponse, PoeClient, PoeError, ProtocolMessage, QueryRequest};
use std::path::Path;
use std::pin::Pin;
use tracing::{debug, error, info};
use std::time::Instant;

use crate::types::*;

pub struct PoeClientWrapper {
    client: PoeClient,
    _model: String,
}

impl PoeClientWrapper {
    pub fn new(model: &str, access_key: &str) -> Self {
        info!("🔑 初始化 POE 客戶端 | 模型: {}", model);
        Self {
            client: PoeClient::new(model, access_key),
            _model: model.to_string(),
        }
    }

    pub async fn stream_request(&self, query_request: QueryRequest) -> Result<Pin<Box<dyn Stream<Item = Result<EventResponse, PoeError>> + Send>>, PoeError> {
        let start_time = Instant::now();
        debug!("📤 發送串流請求 | 訊息數量: {} | 溫度設置: {:?}", 
            query_request.query.len(),
            query_request.temperature
        );

        let result = self.client.stream_request(query_request).await;
        
        match &result {
            Ok(_) => {
                let duration = start_time.elapsed();
                info!("✅ 串流請求建立成功 | 耗時: {}", 
                    crate::utils::format_duration(duration)
                );
            },
            Err(e) => {
                let duration = start_time.elapsed();
                error!("❌ 串流請求失敗 | 錯誤: {} | 耗時: {}", 
                    e,
                    crate::utils::format_duration(duration)
                );
            }
        }
        
        result
    }
}

pub fn create_query_request(model: &str, messages: Vec<Message>, temperature: Option<f32>) -> QueryRequest {
    debug!("📝 創建查詢請求 | 模型: {} | 訊息數量: {} | 溫度設置: {:?}", 
        model, messages.len(), temperature);
    
    // 讀取 models.yaml 配置
    let config = match Path::new("models.yaml").exists() {
        true => {
            match std::fs::read_to_string("models.yaml") {
                Ok(contents) => {
                    match serde_yaml::from_str::<Config>(&contents) {
                        Ok(config) => config,
                        Err(e) => {
                            error!("❌ 解析 models.yaml 失敗: {}", e);
                            Config {
                                enable: Some(false),
                                models: std::collections::HashMap::new(),
                            }
                        }
                    }
                },
                Err(e) => {
                    error!("❌ 讀取 models.yaml 失敗: {}", e);
                    Config {
                        enable: Some(false),
                        models: std::collections::HashMap::new(),
                    }
                }
            }
        },
        false => {
            debug!("⚠️ models.yaml 不存在，使用標準處理");
            Config {
                enable: Some(false),
                models: std::collections::HashMap::new(),
            }
        }
    };

    // 檢查模型是否需要 replace_response 處理
    let should_replace_response = if let Some(model_config) = config.models.get(model) {
        model_config.replace_response.unwrap_or(false)
    } else {
        false
    };

    debug!("🔍 模型 {} 的 replace_response 設置: {}", model, should_replace_response);

    let query = messages.into_iter().map(|msg| {
        let original_role = &msg.role;
        let role = match original_role.as_str() {
            // 總是將 assistant 轉換為 bot
            "assistant" => "bot",
            // 總是將 developer 轉換為 user
            "developer" => "user",
            // 只有在 replace_response 為 true 時才轉換 system 為 user
            "system" if should_replace_response => "user",
            // 其他情況保持原樣
            other => other
        }.to_string();

        debug!("🔄 處理訊息 | 原始角色: {} | 轉換後角色: {} | 內容長度: {}", 
            original_role,
            role,
            crate::utils::format_bytes_length(msg.content.len())
        );

        ProtocolMessage {
            role,
            content: msg.content,
            content_type: "text/markdown".to_string(),
        }
    }).collect();

    QueryRequest {
        version: "1".to_string(),
        r#type: "query".to_string(),
        query,
        temperature,
        user_id: "".to_string(),
        conversation_id: "".to_string(),
        message_id: "".to_string(),
    }
}