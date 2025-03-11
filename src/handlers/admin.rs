use crate::types::Config;
use crate::utils::get_config_path;
use askama::Template;
use salvo::basic_auth::{BasicAuth, BasicAuthValidator};
use salvo::prelude::*;
use serde_json::json;
use std::fs;

#[derive(Template)]
#[template(path = "admin.html")]
struct AdminTemplate;

#[handler]
async fn admin_page(res: &mut Response) {
    let template = AdminTemplate;
    res.render(Text::Html(template.render().unwrap()));
}

#[handler]
async fn get_config(res: &mut Response) {
    let config = load_config().unwrap_or_default();
    res.render(Json(config));
}

#[handler]
async fn save_config(req: &mut Request, res: &mut Response) {
    match req.parse_json::<Config>().await {
        Ok(config) => {
            if let Err(e) = save_config_to_file(&config) {
                res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
                res.render(Json(json!({ "error": e.to_string() })));
            } else {
                res.render(Json(json!({ "status": "success" })));
            }
        }
        Err(e) => {
            res.status_code(StatusCode::BAD_REQUEST);
            res.render(Json(json!({ "error": e.to_string() })));
        }
    }
}

fn load_config() -> Result<Config, Box<dyn std::error::Error>> {
    let config_path = get_config_path("models.yaml");
    if config_path.exists() {
        let contents = fs::read_to_string(config_path)?;
        Ok(serde_yaml::from_str(&contents)?)
    } else {
        Ok(Config {
            enable: Some(false),
            models: std::collections::HashMap::new(),
        })
    }
}

fn save_config_to_file(config: &Config) -> Result<(), Box<dyn std::error::Error>> {
    let yaml = serde_yaml::to_string(config)?;
    let config_path = get_config_path("models.yaml");
    fs::write(config_path, yaml)?;
    Ok(())
}

// 定義驗證器結構
pub struct AdminAuthValidator;

impl BasicAuthValidator for AdminAuthValidator {
    async fn validate(&self, username: &str, password: &str, _depot: &mut Depot) -> bool {
        let valid_username =
            std::env::var("ADMIN_USERNAME").unwrap_or_else(|_| "admin".to_string());
        let valid_password =
            std::env::var("ADMIN_PASSWORD").unwrap_or_else(|_| "123456".to_string());
        username == valid_username && password == valid_password
    }
}

// 修改路由函數
pub fn admin_routes() -> Router {
    let auth_handler = BasicAuth::new(AdminAuthValidator);
    Router::new()
        .hoop(auth_handler) // 加入認證中間件
        .push(Router::with_path("admin").get(admin_page))
        .push(
            Router::with_path("api/admin/config")
                .get(get_config)
                .post(save_config),
        )
}
