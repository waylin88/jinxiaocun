use std::str::FromStr;

use sqlx::{
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
    SqlitePool,
};

use crate::{config::Config, error::AppResult};

pub type Db = SqlitePool;

/// 建库 + 跑迁移 + 种子数据
pub async fn init(cfg: &Config) -> AppResult<Db> {
    let url = &cfg.database_url;

    // SQLite 单文件库：确保父目录存在
    if let Some(path) = url.strip_prefix("sqlite://") {
        if let Some(parent) = std::path::Path::new(path).parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
    }

    let opts = SqliteConnectOptions::from_str(url)?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .foreign_keys(true)
        .busy_timeout(std::time::Duration::from_secs(10));

    let pool = SqlitePoolOptions::new()
        .max_connections(4)
        .acquire_timeout(std::time::Duration::from_secs(15))
        .connect_with(opts)
        .await?;

    sqlx::migrate!("./migrations").run(&pool).await?;

    seed_admin(&pool, cfg).await?;

    tracing::info!("数据库就绪: {url}");
    Ok(pool)
}

/// 首次启动创建管理员账号
async fn seed_admin(pool: &Db, cfg: &Config) -> AppResult<()> {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(pool)
        .await?;

    if count > 0 {
        return Ok(());
    }

    let hash = crate::auth::hash_password(&cfg.admin_password)?;
    sqlx::query("INSERT INTO users (username, password_hash, name, role, active, created_at) VALUES (?, ?, ?, 'admin', 1, ?)")
        .bind(&cfg.admin_username)
        .bind(&hash)
        .bind("系统管理员")
        .bind(chrono::Utc::now().to_rfc3339())
        .execute(pool)
        .await?;

    tracing::warn!(
        "已创建初始管理员账号: {} / {} —— 请登录后立即修改密码",
        cfg.admin_username,
        cfg.admin_password
    );
    Ok(())
}

pub fn now_str() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// 生成单号：RK20261018-0001 / CK20261018-0001
pub async fn next_doc_no(pool: &Db, prefix: &str) -> AppResult<String> {
    let date = chrono::Utc::now().format("%Y%m%d").to_string();
    let like = format!("{prefix}{date}-%");
    let start = (prefix.len() + date.len() + 1) as i64; // SQLite substr 从 1 开始计数

    let sql = match prefix {
        "RK" => "SELECT COALESCE(MAX(CAST(substr(doc_no, ?) AS INTEGER)), 0) + 1
                 FROM inbound_orders WHERE doc_no LIKE ?",
        _ => "SELECT COALESCE(MAX(CAST(substr(doc_no, ?) AS INTEGER)), 0) + 1
               FROM outbound_orders WHERE doc_no LIKE ?",
    };

    let next: i64 = sqlx::query_scalar(sql)
        .bind(start)
        .bind(&like)
        .fetch_one(pool)
        .await?;

    Ok(format!("{prefix}{date}-{next:04}"))
}