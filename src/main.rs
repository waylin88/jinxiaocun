mod auth;
mod config;
mod db;
mod error;
mod handlers;
mod models;
mod ops;
mod state;

use axum::{
    extract::State,
    http::{header, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use rust_embed::RustEmbed;
use serde_json::json;
use tower_http::trace::TraceLayer;

use crate::{
    auth::AuthUser,
    config::Config,
    state::AppState,
};

#[derive(RustEmbed)]
#[folder = "static/"]
struct Assets;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "jinxiaocun=info,tower_http=warn".into()),
        )
        .init();

    let cfg = Config::from_env();
    let pool = db::init(&cfg).await?;
    let state = AppState { pool, cfg };

    let app = Router::new()
        // ---- 公开 ----
        .route("/api/auth/login", post(handlers::login))
        .route("/api/health", get(health))
        // ---- 需登录 ----
        .route("/api/me", get(handlers::me))
        .route("/api/me/password", put(handlers::change_password))
        // 型号
        .route("/api/products", get(handlers::list_products).post(handlers::save_product))
        .route("/api/products/{id}", axum::routing::delete(handlers::delete_product))
        // 入库
        .route("/api/inbound", post(handlers::create_inbound).get(handlers::list_inbound))
        .route("/api/inbound/{id}", get(handlers::get_inbound))
        .route("/api/inbound/by-waybill/{waybill}", get(handlers::find_by_waybill))
        // 出库
        .route("/api/outbound", post(handlers::create_outbound).get(handlers::list_outbound))
        .route("/api/outbound/{id}", get(handlers::get_outbound))
        // 库存
        .route("/api/stock", get(handlers::list_stock))
        .route("/api/stock/low", get(handlers::low_stock))
        .route("/api/stock/{id}/movements", get(handlers::movements))
        .route("/api/dashboard", get(handlers::dashboard))
        // 离线同步
        .route("/api/sync/push", post(handlers::sync_push))
        // 前端静态资源（打包进二进制，单文件部署）
        .fallback(static_handler)
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let bind = app
        .into_make_service_with_connect_info::<std::net::SocketAddr>();

    let listener = tokio::net::TcpListener::bind(&cfg_addr()).await?;
    tracing::info!("进销存服务已启动: http://{}", cfg_addr());

    axum::serve(listener, bind)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

fn cfg_addr() -> String {
    std::env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".to_string())
}

async fn health(State(s): State<AppState>) -> Response {
    let r: Result<i64, _> = sqlx::query_scalar("SELECT 1").fetch_one(&s.pool).await;
    let ok = r.is_ok();
    let body = json!({ "ok": ok, "time": chrono::Utc::now().to_rfc3339() });
    let code = if ok { StatusCode::OK } else { StatusCode::SERVICE_UNAVAILABLE };
    (code, Json(body)).into_response()
}

/// 静态资源 + SPA 回退
async fn static_handler(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };

    match Assets::get(path) {
        Some(file) => {
            let mime = mime_of(path);
            (
                [
                    (header::CONTENT_TYPE, mime.to_string()),
                    // Service Worker 必须能拿到最新版本，否则更新不生效
                    (
                        header::CACHE_CONTROL,
                        if path == "sw.js" {
                            "no-cache"
                        } else {
                            "public, max-age=3600"
                        }
                        .to_string(),
                    ),
                ],
                file.data.into_owned(),
            )
                .into_response()
        }
        None => match Assets::get("index.html") {
            Some(f) => (
                [(header::CONTENT_TYPE, "text/html; charset=utf-8".to_string())],
                f.data.into_owned(),
            )
                .into_response(),
            None => (StatusCode::NOT_FOUND, "index.html missing").into_response(),
        },
    }
}

fn mime_of(p: &str) -> &'static str {
    match p.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "application/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("webmanifest") => "application/manifest+json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("ico") => "image/x-icon",
        _ => "application/octet-stream",
    }
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    tracing::info!("收到停止信号，正在关闭...");
}