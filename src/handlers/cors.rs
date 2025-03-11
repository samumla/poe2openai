use salvo::http::{HeaderValue, Method, StatusCode, header};
use salvo::prelude::*;
use tracing::{debug, info};

#[handler]
pub async fn cors_middleware(
    req: &mut Request,
    depot: &mut Depot,
    res: &mut Response,
    ctrl: &mut FlowCtrl,
) {
    // 從請求中獲取Origin頭
    let origin = req
        .headers()
        .get(header::ORIGIN)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("null");

    // 記錄請求的Origin用於調試
    debug!("📡 接收到來自Origin: {} 的請求", origin);

    // 設置CORS頭部
    match HeaderValue::from_str(origin) {
        Ok(origin_value) => {
            res.headers_mut()
                .insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin_value);
        }
        Err(e) => {
            debug!("⚠️ 無效的Origin頭: {}, 錯誤: {}", origin, e);
            res.headers_mut().insert(
                header::ACCESS_CONTROL_ALLOW_ORIGIN,
                HeaderValue::from_static("null"),
            );
        }
    }

    res.headers_mut().insert(
        header::ACCESS_CONTROL_ALLOW_CREDENTIALS,
        HeaderValue::from_static("true"),
    );

    // 為所有回應添加Vary頭，表明回應基於Origin頭變化
    res.headers_mut()
        .insert(header::VARY, HeaderValue::from_static("Origin"));

    // 如果是OPTIONS請求，直接處理並停止後續流程
    if req.method() == Method::OPTIONS {
        handle_preflight_request(req, res);
        ctrl.skip_rest();
    } else {
        // 非OPTIONS請求，繼續正常流程
        ctrl.call_next(req, depot, res).await;
    }
}

/// 專門處理CORS預檢請求
fn handle_preflight_request(req: &Request, res: &mut Response) {
    info!("🔍 處理OPTIONS預檢請求: {}", req.uri());

    // 設置CORS預檢回應的標準頭部
    res.headers_mut().insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("GET, POST, OPTIONS, PUT, DELETE, PATCH, HEAD"),
    );

    res.headers_mut().insert(
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static(
            "\
            Authorization, Content-Type, User-Agent, Accept, Origin, \
            X-Requested-With, Access-Control-Request-Method, \
            Access-Control-Request-Headers, Accept-Encoding, Accept-Language, \
            Cache-Control, Connection, Referer, Sec-Fetch-Dest, Sec-Fetch-Mode, \
            Sec-Fetch-Site, Pragma, X-Api-Key\
        ",
        ),
    );

    res.headers_mut().insert(
        header::ACCESS_CONTROL_MAX_AGE,
        HeaderValue::from_static("3600"),
    );

    // 添加Vary頭，表明回應會根據這些請求頭變化
    res.headers_mut().insert(
        header::VARY,
        HeaderValue::from_static("Access-Control-Request-Method, Access-Control-Request-Headers"),
    );

    // 設置正確的狀態碼: 204 No Content
    res.status_code(StatusCode::NO_CONTENT);
}
