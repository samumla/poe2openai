use poe_api_process::get_model_list;
use salvo::prelude::*;
use serde_json::json;
use std::path::Path;
use std::time::Instant;
use tracing::{debug, error, info};

use crate::types::*;

#[handler]
pub async fn get_models(req: &mut Request, res: &mut Response) {
    let path = req.uri().path();
    info!("📋 收到獲取模型列表請求 | 路徑: {}", path);
    let start_time = Instant::now();

    match get_model_list(Some("zh-Hant")).await {
        Ok(model_list) => {
            debug!("📊 原始模型數量: {}", model_list.data.len());

            // 首先進行全部小寫轉換
            let lowercase_models = model_list
                .data
                .into_iter()
                .map(|mut model| {
                    debug!("🏷️ 轉換小寫: {} -> {}", model.id, model.id.to_lowercase());
                    model.id = model.id.to_lowercase();
                    model
                })
                .collect::<Vec<_>>();

            // 如果是 api/models 路徑，直接返回小寫轉換後的結果
            if path == "/api/models" {
                let response = json!({
                    "object": "list",
                    "data": lowercase_models
                });

                let duration = start_time.elapsed();
                info!(
                    "✅ 成功獲取未過濾模型列表 | 模型數量: {} | 處理時間: {}",
                    lowercase_models.len(),
                    crate::utils::format_duration(duration)
                );

                res.render(Json(response));
                return;
            }

            // 讀取並解析 models.yaml 配置
            let config = match Path::new("models.yaml").exists() {
                true => match std::fs::read_to_string("models.yaml") {
                    Ok(contents) => match serde_yaml::from_str::<Config>(&contents) {
                        Ok(config) => config,
                        Err(e) => {
                            error!("❌ 解析 models.yaml 失敗: {}", e);
                            Config {
                                enable: Some(false),
                                models: std::collections::HashMap::new(),
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
                },
                false => {
                    debug!("⚠️ models.yaml 不存在，預設為不啟用");
                    Config {
                        enable: Some(false),
                        models: std::collections::HashMap::new(),
                    }
                }
            };

            let is_enabled = config.enable.unwrap_or(false);
            debug!("🔍 設定檔啟用狀態: {}", is_enabled);

            // 將 config.models 的鍵轉換為小寫以匹配轉換後的模型 ID
            let lowercase_config_models: std::collections::HashMap<String, ModelConfig> = config
                .models
                .into_iter()
                .map(|(k, v)| (k.to_lowercase(), v))
                .collect();

            let processed_models = lowercase_models
                .into_iter()
                .filter_map(|mut model| {
                    let model_id = model.id.clone();
                    let config = lowercase_config_models.get(&model_id);

                    if !is_enabled {
                        // 全域停用時，只處理 mapping
                        if let Some(model_config) = config {
                            if let Some(mapping) = &model_config.mapping {
                                debug!("🔄 模型改名: {} -> {}", model_id, mapping);
                                model.id = mapping.to_lowercase();
                            }
                            Some(model)
                        } else {
                            Some(model)
                        }
                    } else {
                        // 全域啟用時，檢查個別模型設定
                        match config {
                            Some(model_config) => {
                                // enable 預設為 true
                                if model_config.enable.unwrap_or(true) {
                                    if let Some(mapping) = &model_config.mapping {
                                        debug!("🔄 模型改名並保留: {} -> {}", model_id, mapping);
                                        model.id = mapping.to_lowercase();
                                    }
                                    Some(model)
                                } else {
                                    debug!("❌ 排除停用模型: {}", model_id);
                                    None
                                }
                            }
                            None => {
                                debug!("✅ 無配置，保留模型: {}", model_id);
                                Some(model)
                            }
                        }
                    }
                })
                .collect::<Vec<_>>();

            let response = json!({
                "object": "list",
                "data": processed_models
            });

            let duration = start_time.elapsed();
            info!(
                "✅ 成功獲取處理後模型列表 | 模型數量: {} | 處理時間: {}",
                processed_models.len(),
                crate::utils::format_duration(duration)
            );

            res.render(Json(response));
        }
        Err(e) => {
            let duration = start_time.elapsed();
            error!(
                "❌ 獲取模型列表失敗 | 錯誤: {} | 耗時: {}",
                e,
                crate::utils::format_duration(duration)
            );
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(json!({ "error": e.to_string() })));
        }
    }
}
