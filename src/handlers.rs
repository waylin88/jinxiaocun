//! HTTP 路由处理器

use axum::{
    extract::{Path, Query, State},
    Json,
};

use crate::{
    auth::{issue_token, require_admin, verify_password, AuthUser},
    error::{AppError, AppResult},
    models::*,
    ops,
    state::AppState,
};

// ==================== 认证 ====================

pub async fn login(
    State(s): State<AppState>,
    Json(req): Json<LoginReq>,
) -> ApiResult<LoginResp> {
    let row: Option<(i64, String, String, String, i64)> = sqlx::query_as(
        "SELECT id, password_hash, name, role, active FROM users WHERE username = ?",
    )
    .bind(req.username.trim())
    .fetch_optional(&s.pool)
    .await?;

    let (id, hash, name, role, active) =
        row.ok_or_else(|| AppError::Unauthorized.clone_for_login())?;

    if active == 0 {
        return Err(AppError::Forbidden);
    }
    if !verify_password(&req.password, &hash) {
        return Err(AppError::Unauthorized);
    }

    let token = issue_token(&s.cfg.jwt_secret, s.cfg.jwt_ttl_hours, id, &name, &role)?;

    Ok(Json(ApiOk::new(LoginResp {
        token,
        user: UserInfo {
            id,
            username: req.username,
            name,
            role,
        },
    })))
}

pub async fn me(user: AuthUser) -> ApiResult<UserInfo> {
    Ok(Json(ApiOk::new(UserInfo {
        id: user.id,
        username: String::new(),
        name: user.name,
        role: user.role,
    })))
}

pub async fn change_password(
    State(s): State<AppState>,
    user: AuthUser,
    Json(body): Json<serde_json::Value>,
) -> ApiResult<serde_json::Value> {
    let old = body.get("old_password").and_then(|v| v.as_str()).unwrap_or("");
    let new = body.get("new_password").and_then(|v| v.as_str()).unwrap_or("");

    if new.len() < 6 {
        return Err(AppError::bad("新密码至少 6 位"));
    }

    let hash: Option<String> =
        sqlx::query_scalar("SELECT password_hash FROM users WHERE id = ?")
            .bind(user.id)
            .fetch_optional(&s.pool)
            .await?;

    match hash {
        Some(h) if verify_password(old, &h) => {}
        _ => return Err(AppError::bad("原密码不正确")),
    }

    let nh = crate::auth::hash_password(new)?;
    sqlx::query("UPDATE users SET password_hash = ? WHERE id = ?")
        .bind(nh)
        .bind(user.id)
        .execute(&s.pool)
        .await?;

    Ok(Json(ApiOk::new(serde_json::json!({"ok": true}))))
}

// ==================== 型号管理 ====================

pub async fn list_products(
    State(s): State<AppState>,
    _u: AuthUser,
    Query(q): Query<ListQuery>,
) -> ApiResult<Vec<Product>> {
    let kw = q.keyword.unwrap_or_default();
    let like = format!("%{kw}%");
    let limit = q.limit.unwrap_or(200).clamp(1, 1000);

    let rows = sqlx::query_as::<_, Product>(
        "SELECT id, sku, name, spec, unit, cost_price, sale_price, stock_qty, warn_qty, active
         FROM products
         WHERE active = 1 AND (? = '%%' OR sku LIKE ? OR name LIKE ?)
         ORDER BY sku LIMIT ?",
    )
    .bind(&like)
    .bind(&like)
    .bind(&like)
    .bind(limit)
    .fetch_all(&s.pool)
    .await?;

    Ok(Json(ApiOk::new(rows)))
}

pub async fn save_product(
    State(s): State<AppState>,
    _u: AuthUser,
    Json(p): Json<ProductUpsert>,
) -> ApiResult<Product> {
    let sku = p.sku.trim();
    if sku.is_empty() {
        return Err(AppError::bad("型号不能为空"));
    }
    if p.name.trim().is_empty() {
        return Err(AppError::bad("名称不能为空"));
    }
    let ts = crate::db::now_str();

    let id = match p.id {
        Some(id) => {
            sqlx::query(
                "UPDATE products SET sku = ?, name = ?, spec = ?, unit = ?,
                        cost_price = ?, sale_price = ?, warn_qty = ?, updated_at = ?
                 WHERE id = ?",
            )
            .bind(sku)
            .bind(p.name.trim())
            .bind(p.spec.as_deref())
            .bind(&p.unit)
            .bind(p.cost_price)
            .bind(p.sale_price)
            .bind(p.warn_qty)
            .bind(&ts)
            .bind(id)
            .execute(&s.pool)
            .await?;
            id
        }
        None => {
            let r = sqlx::query(
                "INSERT INTO products (sku, name, spec, unit, cost_price, sale_price, stock_qty, warn_qty, active, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?, 0, ?, 1, ?, ?)",
            )
            .bind(sku)
            .bind(p.name.trim())
            .bind(p.spec.as_deref())
            .bind(&p.unit)
            .bind(p.cost_price)
            .bind(p.sale_price)
            .bind(p.warn_qty)
            .bind(&ts)
            .bind(&ts)
            .execute(&s.pool)
            .await
            .map_err(|e| match &e {
                sqlx::Error::Database(d) if d.is_unique_violation() => {
                    AppError::conflict(format!("型号「{sku}」已存在"))
                }
                _ => AppError::from(e),
            })?;
            r.last_insert_rowid()
        }
    };

    let row = sqlx::query_as::<_, Product>(
        "SELECT id, sku, name, spec, unit, cost_price, sale_price, stock_qty, warn_qty, active
         FROM products WHERE id = ?",
    )
    .bind(id)
    .fetch_one(&s.pool)
    .await?;

    Ok(Json(ApiOk::new(row)))
}

pub async fn delete_product(
    State(s): State<AppState>,
    user: AuthUser,
    Path(id): Path<i64>,
) -> ApiResult<serde_json::Value> {
    require_admin(&user)?;

    // 有库存变动记录的型号只能停用，不能删除 —— 保住历史流水可追溯
    let used: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM stock_movements WHERE product_id = ?")
            .bind(id)
            .fetch_one(&s.pool)
            .await?;
    if used > 0 {
        sqlx::query("UPDATE products SET active = 0 WHERE id = ?")
            .bind(id)
            .execute(&s.pool)
            .await?;
        return Ok(Json(ApiOk::new(serde_json::json!({
            "id": id, "action": "disabled", "reason": "已有库存流水，已停用而非删除"
        }))));
    }

    sqlx::query("DELETE FROM products WHERE id = ?")
        .bind(id)
        .execute(&s.pool)
        .await?;
    Ok(Json(ApiOk::new(serde_json::json!({"id": id, "action": "deleted"}))))
}

// ==================== 入库 ====================

pub async fn create_inbound(
    State(s): State<AppState>,
    user: AuthUser,
    Json(req): Json<InboundCreateReq>,
) -> ApiResult<InboundDetail> {
    let d = ops::create_inbound(&s.pool, user.id, &req, true).await?;
    Ok(Json(ApiOk::new(d)))
}

pub async fn list_inbound(
    State(s): State<AppState>,
    _u: AuthUser,
    Query(q): Query<ListQuery>,
) -> ApiResult<Vec<InboundOrder>> {
    let kw = q.keyword.unwrap_or_default();
    let like = format!("%{kw}%");
    let status = q.status.unwrap_or_default();
    let limit = q.limit.unwrap_or(50).clamp(1, 500);
    let offset = q.offset.unwrap_or(0).max(0);

    let rows = sqlx::query_as::<_, InboundOrder>(
        "SELECT id, doc_no, waybill_no, supplier_name, remark, status,
                total_boxes, total_qty, created_by, created_at, posted_at
         FROM inbound_orders
         WHERE (? = '%%' OR waybill_no LIKE ? OR doc_no LIKE ? OR IFNULL(supplier_name,'') LIKE ?)
           AND (? = '' OR status = ?)
         ORDER BY id DESC LIMIT ? OFFSET ?",
    )
    .bind(&like)
    .bind(&like)
    .bind(&like)
    .bind(&like)
    .bind(&status)
    .bind(&status)
    .bind(limit)
    .bind(offset)
    .fetch_all(&s.pool)
    .await?;

    Ok(Json(ApiOk::new(rows)))
}

pub async fn get_inbound(
    State(s): State<AppState>,
    _u: AuthUser,
    Path(id): Path<i64>,
) -> ApiResult<InboundDetail> {
    Ok(Json(ApiOk::new(ops::inbound_detail(&s.pool, id).await?)))
}

/// 按运单号查最近一单 —— 仓库里最常用的"这单货点数了没"
pub async fn find_by_waybill(
    State(s): State<AppState>,
    _u: AuthUser,
    Path(waybill): Path<String>,
) -> ApiResult<InboundDetail> {
    let id: i64 = sqlx::query_scalar(
        "SELECT id FROM inbound_orders WHERE waybill_no = ? ORDER BY id DESC LIMIT 1",
    )
    .bind(waybill.trim())
    .fetch_optional(&s.pool)
    .await?
    .ok_or_else(|| AppError::not_found("该运单号"))?;

    Ok(Json(ApiOk::new(ops::inbound_detail(&s.pool, id).await?)))
}

// ==================== 出库 ====================

pub async fn create_outbound(
    State(s): State<AppState>,
    user: AuthUser,
    Json(req): Json<OutboundCreateReq>,
) -> ApiResult<OutboundDetail> {
    let d = ops::create_outbound(&s.pool, user.id, &req, false).await?;
    Ok(Json(ApiOk::new(d)))
}

pub async fn list_outbound(
    State(s): State<AppState>,
    _u: AuthUser,
    Query(q): Query<ListQuery>,
) -> ApiResult<Vec<OutboundOrder>> {
    let kw = q.keyword.unwrap_or_default();
    let like = format!("%{kw}%");
    let status = q.status.unwrap_or_default();
    let limit = q.limit.unwrap_or(50).clamp(1, 500);
    let offset = q.offset.unwrap_or(0).max(0);

    let rows = sqlx::query_as::<_, OutboundOrder>(
        "SELECT id, doc_no, customer, remark, status, total_qty, created_by, created_at, posted_at
         FROM outbound_orders
         WHERE (? = '%%' OR customer LIKE ? OR doc_no LIKE ?)
           AND (? = '' OR status = ?)
         ORDER BY id DESC LIMIT ? OFFSET ?",
    )
    .bind(&like)
    .bind(&like)
    .bind(&like)
    .bind(&status)
    .bind(&status)
    .bind(limit)
    .bind(offset)
    .fetch_all(&s.pool)
    .await?;

    Ok(Json(ApiOk::new(rows)))
}

pub async fn get_outbound(
    State(s): State<AppState>,
    _u: AuthUser,
    Path(id): Path<i64>,
) -> ApiResult<OutboundDetail> {
    Ok(Json(ApiOk::new(ops::outbound_detail(&s.pool, id).await?)))
}

// ==================== 库存 ====================

pub async fn list_stock(
    State(s): State<AppState>,
    _u: AuthUser,
    Query(q): Query<ListQuery>,
) -> ApiResult<Vec<StockRow>> {
    let kw = q.keyword.unwrap_or_default();
    let like = format!("%{kw}%");
    let limit = q.limit.unwrap_or(200).clamp(1, 1000);

    let rows = sqlx::query_as::<_, StockRow>(
        "SELECT id, sku, name, spec, unit, stock_qty, warn_qty
         FROM products
         WHERE active = 1 AND (? = '%%' OR sku LIKE ? OR name LIKE ?)
         ORDER BY sku LIMIT ?",
    )
    .bind(&like)
    .bind(&like)
    .bind(&like)
    .bind(limit)
    .fetch_all(&s.pool)
    .await?;

    Ok(Json(ApiOk::new(rows)))
}

pub async fn low_stock(
    State(s): State<AppState>,
    _u: AuthUser,
) -> ApiResult<Vec<StockRow>> {
    let rows = sqlx::query_as::<_, StockRow>(
        "SELECT id, sku, name, spec, unit, stock_qty, warn_qty
         FROM products
         WHERE active = 1 AND warn_qty > 0 AND stock_qty <= warn_qty
         ORDER BY stock_qty ASC LIMIT 100",
    )
    .fetch_all(&s.pool)
    .await?;
    Ok(Json(ApiOk::new(rows)))
}

pub async fn movements(
    State(s): State<AppState>,
    _u: AuthUser,
    Path(product_id): Path<i64>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Vec<Movement>> {
    let limit = q.limit.unwrap_or(50).clamp(1, 500);

    let rows = sqlx::query_as::<_, Movement>(
        "SELECT m.id, m.product_id, m.change_qty, m.balance_qty, m.biz_type,
                m.biz_no, m.created_at, p.sku, p.name, u.name AS operator_name
         FROM stock_movements m
         JOIN products p ON p.id = m.product_id
         JOIN users u ON u.id = m.operator_id
         WHERE m.product_id = ?
         ORDER BY m.id DESC LIMIT ?",
    )
    .bind(product_id)
    .bind(limit)
    .fetch_all(&s.pool)
    .await?;

    Ok(Json(ApiOk::new(rows)))
}

pub async fn dashboard(State(s): State<AppState>, _u: AuthUser) -> ApiResult<Dashboard> {
    let d = sqlx::query_as::<_, Dashboard>(
        "SELECT
            (SELECT COUNT(*) FROM products WHERE active = 1) AS sku_total,
            (SELECT IFNULL(SUM(stock_qty), 0) FROM products WHERE active = 1) AS stock_total,
            (SELECT IFNULL(SUM(change_qty), 0) FROM stock_movements
              WHERE biz_type = 'inbound' AND substr(created_at, 1, 10) = date('now')) AS today_in_qty,
            (SELECT IFNULL(SUM(-change_qty), 0) FROM stock_movements
              WHERE biz_type = 'outbound' AND substr(created_at, 1, 10) = date('now')) AS today_out_qty,
            (SELECT COUNT(*) FROM inbound_orders
              WHERE status = 'posted' AND substr(created_at, 1, 10) = date('now')) AS today_in_orders,
            (SELECT COUNT(*) FROM outbound_orders
              WHERE status = 'posted' AND substr(created_at, 1, 10) = date('now')) AS today_out_orders,
            (SELECT COUNT(*) FROM inbound_orders WHERE status = 'draft') AS pending_draft,
            (SELECT COUNT(*) FROM products
              WHERE active = 1 AND warn_qty > 0 AND stock_qty <= warn_qty) AS low_stock_count",
    )
    .fetch_one(&s.pool)
    .await?;

    Ok(Json(ApiOk::new(d)))
}

// ==================== 离线同步 ====================

/// 批量重放离线队列。每个 op 独立成败，一个失败不影响后面的。
pub async fn sync_push(
    State(s): State<AppState>,
    user: AuthUser,
    Json(req): Json<SyncPushReq>,
) -> ApiResult<SyncPushResp> {
    let mut results = Vec::with_capacity(req.ops.len());

    for op in req.ops {
        let result = match op.op_type.as_str() {
            "inbound_create" => {
                let payload: InboundCreateReq = match serde_json::from_value(op.payload.clone()) {
                    Ok(p) => p,
                    Err(e) => {
                        results.push(err_result(&op.op_id, &format!("数据格式错误: {e}")));
                        continue;
                    }
                };
                ops::create_inbound(&s.pool, user.id, &payload, true)
                    .await
                    .map(|d| serde_json::to_value(d).unwrap_or_default())
            }
            "outbound_create" => {
                let payload: OutboundCreateReq = match serde_json::from_value(op.payload.clone()) {
                    Ok(p) => p,
                    Err(e) => {
                        results.push(err_result(&op.op_id, &format!("数据格式错误: {e}")));
                        continue;
                    }
                };
                ops::create_outbound(&s.pool, user.id, &payload, false)
                    .await
                    .map(|d| serde_json::to_value(d).unwrap_or_default())
            }
            other => Err(AppError::bad(format!("未知操作类型: {other}"))),
        };

        results.push(match result {
            Ok(v) => SyncOpResult {
                op_id: op.op_id.clone(),
                status: "ok".into(),
                message: None,
                data: Some(v),
            },
            Err(e) => err_result(&op.op_id, &e.to_string()),
        });
    }

    Ok(Json(ApiOk::new(SyncPushResp { results })))
}

fn err_result(op_id: &str, msg: &str) -> SyncOpResult {
    SyncOpResult {
        op_id: op_id.to_string(),
        status: "error".into(),
        message: Some(msg.to_string()),
        data: None,
    }
}