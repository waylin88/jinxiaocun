use std::env;

#[derive(Clone, Debug)]
pub struct Config {
    pub database_url: String,
    pub bind_addr: String,
    pub jwt_secret: String,
    pub jwt_ttl_hours: i64,
    pub admin_username: String,
    pub admin_password: String,
    pub allow_reset: bool,
}

fn var_or(key: &str, default: &str) -> String {
    match env::var(key) {
        Ok(v) if !v.trim().is_empty() => v,
        _ => default.to_string(),
    }
}

impl Config {
    pub fn from_env() -> Self {
        let jwt_secret = var_or("JWT_SECRET", "");
        let insecure = jwt_secret.is_empty();
        let jwt_secret = if insecure {
            tracing::warn!(
                "JWT_SECRET 未设置，已生成临时密钥。仅供本地调试，生产环境必须在 .env 中配置固定密钥！"
            );
            use rand::RngCore;
            let mut buf = [0u8; 32];
            rand::thread_rng().fill_bytes(&mut buf);
            buf.iter().map(|b| format!("{b:02x}")).collect()
        } else {
            jwt_secret
        };

        Self {
            database_url: var_or("DATABASE_URL", "sqlite://data/jxc.db"),
            bind_addr: var_or("BIND_ADDR", "0.0.0.0:8080"),
            jwt_secret,
            jwt_ttl_hours: var_or("JWT_TTL_HOURS", "720").parse().unwrap_or(720),
            admin_username: var_or("ADMIN_USERNAME", "admin"),
            admin_password: var_or("ADMIN_PASSWORD", "admin123"),
            allow_reset: var_or("ALLOW_RESET", "1") == "1",
        }
    }
}