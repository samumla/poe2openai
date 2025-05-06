use poe_api_process::{get_model_list, ModelInfo};
use salvo::prelude::*;
use serde_json::json;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;
use tracing::{debug, error, info};

use crate::{types::*, utils::get_cached_config};

// 注意：此緩存不適用於 /api/models 路徑
static API_MODELS_CACHE: RwLock<Option<Arc<Vec<ModelInfo>>>> = RwLock::const_new(None);

#[handler]
pub async fn get_models(req: &mut Request, res: &mut Response) {
    let path = req.uri().path();
    info!("📋 收到獲取模型列表請求 | 路徑: {}", path);
    let start_time = Instant::now();

    // 處理 /api/models 特殊路徑 (不使用緩存) ---
    if path == "/api/models" {
        info!("⚡️ api/models 路徑：直接從 Poe 取得（無緩存）");
        match get_model_list(Some("zh-Hant")).await {
            Ok(model_list) => {
                let lowercase_models = model_list
                    .data
                    .into_iter()
                    .map(|mut model| {
                        model.id = model.id.to_lowercase();
                        model
                    })
                    .collect::<Vec<_>>();

                let models_arc = Arc::new(lowercase_models);

                {
                    let mut cache_guard = API_MODELS_CACHE.write().await;
                    *cache_guard = Some(models_arc.clone());
                    info!("🔄 Updated API_MODELS_CACHE after /api/models request.");
                }

                let response = json!({
                    "object": "list",
                    "data": &*models_arc
                });

                let duration = start_time.elapsed();
                info!(
                    "✅ [/api/models] 成功獲取未過濾模型列表並更新緩存 | 模型數量: {} | 處理時間: {}",
                    models_arc.len(), // 使用 Arc 的長度
                    crate::utils::format_duration(duration)
                );
                res.render(Json(response));
            }
            Err(e) => {
                let duration = start_time.elapsed();
                error!(
                    "❌ [/api/models] 獲取模型列表失敗 | 錯誤: {} | 耗時: {}",
                    e,
                    crate::utils::format_duration(duration)
                );
                res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
                res.render(Json(json!({ "error": e.to_string() })));
            }
        }
        return;
    }

    let config = get_cached_config().await;

    let is_enabled = config.enable.unwrap_or(false);
    debug!("🔍 設定檔啟用狀態 (來自緩存): {}", is_enabled);

    let yaml_config_map: std::collections::HashMap<String, ModelConfig> = config
        .models
        .clone() // Clone HashMap from Arc<Config>
        .into_iter()
        .map(|(k, v)| (k.to_lowercase(), v))
        .collect();

    if is_enabled {
        info!("⚙️ 合併緩存的 Poe API 列表與 models.yaml (啟用)");

        let api_models_data_arc: Arc<Vec<ModelInfo>>;

        let read_guard = API_MODELS_CACHE.read().await;
        if let Some(cached_data) = &*read_guard {
            // 緩存命中
            debug!("✅ 模型緩存命中。");
            api_models_data_arc = cached_data.clone();
            drop(read_guard);
        } else {
            // 緩存未命中
            debug!("❌ 模型緩存未命中。正在嘗試填充...");
            drop(read_guard);

            let mut write_guard = API_MODELS_CACHE.write().await;
            // 再次檢查，防止在獲取寫入鎖期間其他線程已填充緩存
            if let Some(cached_data) = &*write_guard {
                 debug!("✅ API 模型緩存在等待寫入鎖時由另一個執行緒填充。");
                 api_models_data_arc = cached_data.clone();
            } else {
                // 緩存確實是空的，從 API 獲取數據
                info!("⏳ 從 API 取得模型以填充快取中……");
                match get_model_list(Some("zh-Hant")).await {
                    Ok(list) => {
                        let lowercase_models = list
                            .data
                            .into_iter()
                            .map(|mut model| {
                                model.id = model.id.to_lowercase();
                                model
                            })
                            .collect::<Vec<_>>();
                        let new_data = Arc::new(lowercase_models);
                        *write_guard = Some(new_data.clone());
                        api_models_data_arc = new_data;
                        info!("✅ API models cache populated successfully.");
                    }
                    Err(e) => {
                        // 如果填充緩存失敗，返回錯誤
                        let duration = start_time.elapsed(); // 計算耗時
                        error!(
                            "❌ 無法填充 API 模型快取：{} | 耗時：{}。",
                             e, crate::utils::format_duration(duration) // 在日誌中使用 duration
                        );
                        res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
                        res.render(Json(json!({ "error": format!("未能檢索模型列表以填充快取：{}", e) })));
                        drop(write_guard);
                        return;
                    }
                }
            }
             drop(write_guard);
        }

        let mut api_model_ids: HashSet<String> = HashSet::new();
        for model_ref in api_models_data_arc.iter() {
            api_model_ids.insert(model_ref.id.to_lowercase());
        }

        let mut processed_models_enabled: Vec<ModelInfo> = Vec::new();

        for api_model_ref in api_models_data_arc.iter() {
            let api_model_id_lower = api_model_ref.id.to_lowercase();
            match yaml_config_map.get(&api_model_id_lower) {
                Some(yaml_config) => {
                    // 在 YAML 中找到：檢查是否啟用，若啟用則應用 mapping
                    if yaml_config.enable.unwrap_or(true) {
                        let final_id = if let Some(mapping) = &yaml_config.mapping {
                            let new_id = mapping.to_lowercase();
                            debug!("🔄 API 模型改名 (YAML 啟用): {} -> {}", api_model_id_lower, new_id);
                            new_id
                        } else {
                            debug!("✅ 保留 API 模型 (YAML 啟用，無 mapping): {}", api_model_id_lower);
                            api_model_id_lower.clone()
                        };
                        processed_models_enabled.push(ModelInfo {
                            id: final_id,
                            object: api_model_ref.object.clone(),
                            created: api_model_ref.created,
                            owned_by: api_model_ref.owned_by.clone(),
                        });
                    } else {
                        debug!("❌ 排除 API 模型 (YAML 停用): {}", api_model_id_lower);
                    }
                }
                None => {
                    debug!("✅ 保留 API 模型 (不在 YAML 中): {}", api_model_id_lower);
                    processed_models_enabled.push(ModelInfo {
                        id: api_model_id_lower.clone(),
                        object: api_model_ref.object.clone(),
                        created: api_model_ref.created,
                        owned_by: api_model_ref.owned_by.clone(),
                    });
                }
            }
        }

        let response = json!({
            "object": "list",
            "data": processed_models_enabled
        });

        let duration = start_time.elapsed();
        info!(
            "✅ 成功獲取處理後模型列表 | 來源: {} | 模型數量: {} | 處理時間: {}",
            "YAML + Cached API",
            processed_models_enabled.len(),
            crate::utils::format_duration(duration)
        );

        res.render(Json(response));

    } else {
        info!("🔌 YAML 停用，直接從 Poe API 獲取模型列表 (無緩存，無 YAML 規則)...");

        match get_model_list(Some("zh-Hant")).await {
            Ok(model_list) => {
                let lowercase_models = model_list
                    .data
                    .into_iter()
                    .map(|mut model| {
                        model.id = model.id.to_lowercase();
                        model
                    })
                    .collect::<Vec<_>>();

                let response = json!({
                    "object": "list",
                    "data": lowercase_models
                });
                let duration = start_time.elapsed();
                info!(
                    "✅ [直連 Poe] 成功直接獲取模型列表 | 模型數量: {} | 處理時間: {}",
                    lowercase_models.len(),
                    crate::utils::format_duration(duration)
                );
                res.render(Json(response));
            }
            Err(e) => {
                let duration = start_time.elapsed();
                error!(
                    "❌ [直連 Poe] 直接獲取模型列表失敗 | 錯誤: {} | 耗時: {}",
                    e,
                    crate::utils::format_duration(duration)
                );
                res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
                res.render(Json(json!({ "error": format!("無法直接從API獲取模型：{}", e) })));
            }
        }
    }
}
