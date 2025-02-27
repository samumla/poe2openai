use salvo::{cors::{AllowOrigin, Cors}, http::Method};
use salvo::prelude::*;
use tracing::{info, debug};
use std::env;
mod types;
mod handlers;
mod poe_client;
mod utils;

fn get_env_or_default(key: &str, default: &str) -> String {
    let value = env::var(key).unwrap_or_else(|_| default.to_string());
    if key == "ADMIN_PASSWORD" {
        debug!("🔧 環境變數 {} = {}", key, "*".repeat(value.len()));
    } else {
        debug!("🔧 環境變數 {} = {}", key, value);
    }
    value
}

fn setup_logging(log_level: &str) {
    tracing_subscriber::fmt()
        .with_target(false)
        .with_thread_ids(true)
        .with_level(true)
        .with_file(false)
        .with_line_number(false)
        .with_env_filter(log_level)
        .init();

    info!("🚀 日誌系統初始化完成，日誌級別: {}", log_level);
}

#[handler]
async fn handle_options(req: &mut Request, res: &mut Response) {
    info!("處理OPTIONS請求: {}", req.uri());
    res.status_code(StatusCode::OK);
}

#[tokio::main]
async fn main() {
    let log_level = get_env_or_default("LOG_LEVEL", "debug");
    setup_logging(&log_level);
    
    let host = get_env_or_default("HOST", "0.0.0.0");
    let port = get_env_or_default("PORT", "8080");
    get_env_or_default("ADMIN_USERNAME", "admin");
    get_env_or_default("ADMIN_PASSWORD", "123456");
    let salvo_max_size = get_env_or_default("MAX_REQUEST_SIZE", "1073741824")
        .parse()
        .unwrap_or(1024 * 1024 * 1024); // 預設 1GB

    let bind_address = format!("{}:{}", host, port);
    
    let cors_handler = Cors::new()
        .allow_origin(AllowOrigin::any())
        .allow_methods(vec![Method::GET, Method::POST, Method::OPTIONS, Method::HEAD])
        .allow_headers(vec![
            "Authorization", 
            "Content-Type", 
            "User-Agent", 
            "Accept",
            "Origin", 
            "X-Requested-With",
            "Access-Control-Request-Method",
            "Access-Control-Request-Headers",
            "Accept-Encoding",
            "Accept-Language",
            "Cache-Control",
            "Connection",
            "Referer"
        ])
        .max_age(3600)
        .into_handler();

    info!("🌟 正在啟動 Poe API To OpenAI API 服務...");
    debug!("📍 服務綁定地址: {}", bind_address);

    let api_router = Router::new()
        .hoop(cors_handler)
        .options(handle_options)
        .push(Router::with_path("models").get(handlers::get_models).options(handle_options))
        .push(Router::with_path("chat/completions").post(handlers::chat_completions).options(handle_options))
        .push(Router::with_path("api/models").get(handlers::get_models).options(handle_options))
        .push(Router::with_path("v1/models").get(handlers::get_models).options(handle_options))
        .push(Router::with_path("v1/chat/completions").post(handlers::chat_completions).options(handle_options));

    let router: Router = Router::new()
        .hoop(max_size(salvo_max_size.try_into().unwrap()))
        .push(Router::with_path("static/<**path>").get(StaticDir::new(["static"])))
        .push(handlers::admin_routes())
        .push(api_router);

    info!("🛣️  API 路由配置完成");
    
    let acceptor = TcpListener::new(&bind_address).bind().await;
    info!("🎯 服務已啟動並監聽於 {}", bind_address);
    
    Server::new(acceptor).serve(router).await;
}