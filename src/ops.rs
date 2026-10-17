//! 业务核心：库存增减、入库单、出库单
//!
//! 所有写操作都在**单个事务**内完成，并通过 `sync_ops` 幂等表去重，
//! 保证离线队列重放时不会产生重复单据或重复扣减库存。

use sqlx::{Sqlite, SqlitePool, Transaction};

use crate::{
    db::{next_doc_no, now_str},
    error::{AppError, AppResult},
    models::*,
};

type Tx<'e> = Transaction<'e, Sqlite>;

// ---------- 幂等 ----------

/// 若该 op_id 已成功执行过，返回 true —— 调用方应跳过执行
pub async fn op_exists(tx: &mut Tx<'_>, op_id: &str) -> AppResult<bool> {
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sync_ops WHERE op_id = ?")
        .bind(op_id)
        .fetch_one(&mut **tx)
        .await?;
    Ok(n > 0)
}

pub async fn record_op(
    tx: &mut Tx<'_>,
    op_id: &str,
    user_id: i64,
    op_type: &str,
    status: &str,
    result: Option<&serde_json::Value>,
    err: Option<&str>,
) -> AppResult<()> {
    sqlx::query(
        "INSERT OR REPLACE INTO sync_ops (op_id, user_id, op_type, status, result, error, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(op_id)
    .bind(user_id)
    .bind(op_type)
    .bind(status)
    .bind(result.map(|v| v.to_string()))
    .bind(err)
    .bind(now_str())
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// 幂等命中时，取回当时记录的单据 id
async fn op_order_id(pool: &SqlitePool, op_id: &str) -> AppResult<i64> {
    let payload: Option<String> = sqlx::query_scalar("SELECT result FROM sync_ops WHERE op_id = ?")
        .bind(op_id)
        .fetch_optional(pool)
        .await?;
    let p = payload.ok_or_else(|| AppError::internal("幂等记录缺少回放数据"))?;
    let v: serde_json::Value = serde_json::from_str(&p).unwrap_or_default();
    v.get("order_id")
        .and_then(|x| x.as_i64())
        .ok_or_else(|| AppError::internal("幂等记录缺少 order_id"))
}

// ---------- 型号解析 ----------

/// 按 id 或 sku 定位商品。sku 不存在且 auto_create 为真时自动建档。
async fn resolve_product(
    tx: &mut Tx<'_>,
    id: Option<i64>,
    sku: Option<&str>,
    auto_create: bool,
) -> AppResult<i64> {
    if let Some(pid) = id {
        let ok: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM products WHERE id = ?")
            .bind(pid)
            .fetch_one(&mut **tx)
            .await?;
        if ok == 0 {
            return Err(AppError::not_found(format!("商品(id={pid})")));
        }
        return Ok(pid);
    }

    let sku = sku
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::bad("型号不能为空"))?;

    if let Some(pid) = sqlx::query_scalar::<_, i64>("SELECT id FROM products WHERE sku = ?")
        .bind(sku)
        .fetch_optional(&mut **tx)
        .await?
    {
        return Ok(pid);
    }

    if !auto_create {
        return Err(AppError::bad(format!("型号「{sku}」不存在，请先在型号管理中建立")));
    }

    let ts = now_str();
    // 自动建档：先用型号占位名称，型号管理页可随时补全
    let r = sqlx::query(
        "INSERT INTO products (sku, name, unit, stock_qty, active, created_at, updated_at)
         VALUES (?, ?, '件', 0, 1, ?, ?)",
    )
    .bind(sku)
    .bind(sku)
    .bind(&ts)
    .bind(&ts)
    .execute(&mut **tx)
    .await?;

    Ok(r.last_insert_rowid())
}

// ---------- 库存流水 ----------

/// 库存增减的唯一入口。正数=入库，负数=出库。返回变动后结存。
async fn apply_stock(
    tx: &mut Tx<'_>,
    product_id: i64,
    delta: i64,
    biz_type: &str,
    biz_id: i64,
    biz_no: &str,
    operator_id: i64,
    remark: Option<&str>,
    allow_negative: bool,
) -> AppResult<i64> {
    let cur: i64 = sqlx::query_scalar("SELECT stock_qty FROM products WHERE id = ?")
        .bind(product_id)
        .fetch_one(&mut **tx)
        .await?;
    let balance = cur + delta;

    if balance < 0 && !allow_negative {
        let info: (String, String, String) =
            sqlx::query_as("SELECT sku, name, unit FROM products WHERE id = ?")
                .bind(product_id)
                .fetch_one(&mut **tx)
                .await?;
        return Err(AppError::conflict(format!(
            "型号「{} {}」库存不足：当前 {} {}，本次需出 {} {}",
            info.0,
            info.1,
            cur,
            info.2,
            -delta,
            info.2
        )));
    }

    sqlx::query("UPDATE products SET stock_qty = ?, updated_at = ? WHERE id = ?")
        .bind(balance)
        .bind(now_str())
        .bind(product_id)
        .execute(&mut **tx)
        .await?;

    sqlx::query(
        "INSERT INTO stock_movements
         (product_id, change_qty, balance_qty, biz_type, biz_id, biz_no, operator_id, created_at, remark)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(product_id)
    .bind(delta)
    .bind(balance)
    .bind(biz_type)
    .bind(biz_id)
    .bind(biz_no)
    .bind(operator_id)
    .bind(now_str())
    .bind(remark)
    .execute(&mut **tx)
    .await?;

    Ok(balance)
}

// ---------- 供应商 ----------

async fn upsert_supplier(tx: &mut Tx<'_>, name: &str) -> AppResult<i64> {
    if let Some(id) = sqlx::query_scalar::<_, i64>("SELECT id FROM suppliers WHERE name = ?")
        .bind(name)
        .fetch_optional(&mut **tx)
        .await?
    {
        return Ok(id);
    }
    let r = sqlx::query("INSERT INTO suppliers (name, active, created_at) VALUES (?, 1, ?)")
        .bind(name)
        .bind(now_str())
        .execute(&mut **tx)
        .await?;
    Ok(r.last_insert_rowid())
}

// ---------- 入库单 ----------

/// 写入一张入库单（运单 → 箱 → 型号数量），并按需过账
pub async fn create_inbound(
    pool: &SqlitePool,
    user_id: i64,
    req: &InboundCreateReq,
    allow_negative: bool,
) -> AppResult<InboundDetail> {
    let waybill = req.waybill_no.trim().to_string();
    if waybill.is_empty() {
        return Err(AppError::bad("运单号不能为空"));
    }
    if req.boxes.is_empty() {
        return Err(AppError::bad("至少需要录入一箱"));
    }
    // 提前校验明细，避免开事务后才发现脏数据
    for (i, b) in req.boxes.iter().enumerate() {
        if b.items.is_empty() {
            return Err(AppError::bad(format!("第 {} 箱没有明细", i + 1)));
        }
        if b.box_no.trim().is_empty() {
            return Err(AppError::bad(format!("第 {} 箱缺箱号", i + 1)));
        }
        for it in &b.items {
            if it.qty <= 0 {
                return Err(AppError::bad(format!("第 {} 箱存在非正数数量", b.box_no)));
            }
        }
    }

    // 幂等短路：离线重放直接返回原单
    if let Some(op_id) = req.op_id.as_deref() {
        if !op_id.is_empty() {
            let exists: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sync_ops WHERE op_id = ?")
                .bind(op_id)
                .fetch_one(pool)
                .await?;
            if exists > 0 {
                return inbound_detail(pool, op_order_id(pool, op_id).await?).await;
            }
        }
    }

    let ts = now_str();
    let doc_no = next_doc_no(pool, "RK").await?;
    let supplier_name = req
        .supplier_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    let mut tx = pool.begin().await?;

    let supplier_id = match supplier_name.as_deref() {
        Some(n) => Some(upsert_supplier(&mut tx, n).await?),
        None => None,
    };

    let status = if req.post { "posted" } else { "draft" };

    let r = sqlx::query(
        "INSERT INTO inbound_orders
         (doc_no, waybill_no, supplier_id, supplier_name, remark, status, total_boxes, total_qty, created_by, created_at, posted_at)
         VALUES (?, ?, ?, ?, ?, ?, 0, 0, ?, ?, ?)",
    )
    .bind(&doc_no)
    .bind(&waybill)
    .bind(supplier_id)
    .bind(&supplier_name)
    .bind(req.remark.as_deref())
    .bind(status)
    .bind(user_id)
    .bind(&ts)
    .bind(if req.post { Some(&ts) } else { None })
    .execute(&mut tx)
    .await?;

    let order_id = r.last_insert_rowid();
    let mut order_total: i64 = 0;

    for (idx, b) in req.boxes.iter().enumerate() {
        let box_no = b.box_no.trim().to_string();
        let br = sqlx::query(
            "INSERT INTO inbound_boxes (order_id, box_no, remark, total_qty, sort_no, created_at, updated_at)
             VALUES (?, ?, ?, 0, ?, ?, ?)",
        )
        .bind(order_id)
        .bind(&box_no)
        .bind(b.remark.as_deref())
        .bind(idx as i64)
        .bind(&ts)
        .bind(&ts)
        .execute(&mut tx)
        .await?;
        let box_id = br.last_insert_rowid();

        let mut box_total: i64 = 0;
        for it in &b.items {
            let pid = resolve_product(&mut tx, it.product_id, it.sku.as_deref(), true).await?;

            // 同一箱内同型号合并
            sqlx::query(
                "INSERT INTO inbound_box_items (box_id, order_id, product_id, qty, remark)
                 VALUES (?, ?, ?, ?, ?)
                 ON CONFLICT(box_id, product_id)
                 DO UPDATE SET qty = qty + excluded.qty",
            )
            .bind(box_id)
            .bind(order_id)
            .bind(pid)
            .bind(it.qty)
            .bind(it.remark.as_deref())
            .execute(&mut tx)
            .await?;

            box_total += it.qty;

            if req.post {
                apply_stock(
                    &mut tx,
                    pid,
                    it.qty,
                    "inbound",
                    order_id,
                    &doc_no,
                    user_id,
                    Some(&box_no),
                    allow_negative,
                )
                .await?;
            }
        }

        sqlx::query("UPDATE inbound_boxes SET total_qty = ? WHERE id = ?")
            .bind(box_total)
            .bind(box_id)
            .execute(&mut tx)
            .await?;
        order_total += box_total;
    }

    sqlx::query("UPDATE inbound_orders SET total_boxes = ?, total_qty = ? WHERE id = ?")
        .bind(req.boxes.len() as i64)
        .bind(order_total)
        .bind(order_id)
        .execute(&mut tx)
        .await?;

    if let Some(op_id) = req.op_id.as_deref().filter(|s| !s.is_empty()) {
        record_op(
            &mut tx,
            op_id,
            user_id,
            "inbound_create",
            "ok",
            Some(&serde_json::json!({"order_id": order_id, "doc_no": doc_no})),
            None,
        )
        .await?;
    }

    tx.commit().await?;
    inbound_detail(pool, order_id).await
}

pub async fn inbound_detail(pool: &SqlitePool, id: i64) -> AppResult<InboundDetail> {
    let order = sqlx::query_as::<_, InboundOrder>(
        "SELECT id, doc_no, waybill_no, supplier_name, remark, status,
                total_boxes, total_qty, created_by, created_at, posted_at
         FROM inbound_orders WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| AppError::not_found("入库单"))?;

    let creator_name: String = sqlx::query_scalar("SELECT name FROM users WHERE id = ?")
        .bind(order.created_by)
        .fetch_one(pool)
        .await?;

    let box_rows = sqlx::query_as::<_, InboundBox>(
        "SELECT id, order_id, box_no, remark, total_qty, sort_no
         FROM inbound_boxes WHERE order_id = ? ORDER BY sort_no, id",
    )
    .bind(id)
    .fetch_all(pool)
    .await?;

    let item_rows = sqlx::query_as::<_, BoxItem>(
        "SELECT bi.id, bi.box_id, bi.product_id, bi.qty, bi.remark, p.sku, p.name, p.unit
         FROM inbound_box_items bi
         JOIN products p ON p.id = bi.product_id
         WHERE bi.order_id = ?
         ORDER BY bi.box_id, bi.id",
    )
    .bind(id)
    .fetch_all(pool)
    .await?;

    let boxes = box_rows
        .into_iter()
        .map(|b| BoxWithItems {
            items: item_rows.iter().filter(|i| i.box_id == b.id).cloned().collect(),
            box_info: b,
        })
        .collect();

    let summary = sqlx::query_as::<_, ModelSummary>(
        "SELECT bi.product_id, p.sku, p.name, p.unit,
                SUM(bi.qty) AS qty, COUNT(DISTINCT bi.box_id) AS boxes
         FROM inbound_box_items bi
         JOIN products p ON p.id = bi.product_id
         WHERE bi.order_id = ?
         GROUP BY bi.product_id, p.sku, p.name, p.unit
         ORDER BY p.sku",
    )
    .bind(id)
    .fetch_all(pool)
    .await?;

    Ok(InboundDetail {
        order,
        creator_name,
        boxes,
        summary,
    })
}

// ---------- 出库单 ----------

pub async fn create_outbound(
    pool: &SqlitePool,
    user_id: i64,
    req: &OutboundCreateReq,
    allow_negative: bool,
) -> AppResult<OutboundDetail> {
    let customer = req.customer.trim().to_string();
    if customer.is_empty() {
        return Err(AppError::bad("客户名不能为空"));
    }
    if req.items.is_empty() {
        return Err(AppError::bad("至少需要一条出库明细"));
    }
    for it in &req.items {
        if it.qty <= 0 {
            return Err(AppError::bad("出库数量必须大于 0"));
        }
    }

    if let Some(op_id) = req.op_id.as_deref() {
        if !op_id.is_empty() {
            let exists: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sync_ops WHERE op_id = ?")
                .bind(op_id)
                .fetch_one(pool)
                .await?;
            if exists > 0 {
                return outbound_detail(pool, op_order_id(pool, op_id).await?).await;
            }
        }
    }

    let ts = now_str();
    let doc_no = next_doc_no(pool, "CK").await?;

    let mut tx = pool.begin().await?;
    let status = if req.post { "posted" } else { "draft" };

    let r = sqlx::query(
        "INSERT INTO outbound_orders
         (doc_no, customer, remark, status, total_qty, created_by, created_at, posted_at)
         VALUES (?, ?, ?, ?, 0, ?, ?, ?)",
    )
    .bind(&doc_no)
    .bind(&customer)
    .bind(req.remark.as_deref())
    .bind(status)
    .bind(user_id)
    .bind(&ts)
    .bind(if req.post { Some(&ts) } else { None })
    .execute(&mut tx)
    .await?;

    let order_id = r.last_insert_rowid();
    let mut total: i64 = 0;

    for it in &req.items {
        // 出库不自动建档：型号必须已存在
        let pid = resolve_product(&mut tx, it.product_id, it.sku.as_deref(), false).await?;

        sqlx::query(
            "INSERT INTO outbound_items (order_id, product_id, qty) VALUES (?, ?, ?)
             ON CONFLICT(order_id, product_id) DO UPDATE SET qty = qty + excluded.qty",
        )
        .bind(order_id)
        .bind(pid)
        .bind(it.qty)
        .execute(&mut tx)
        .await?;

        total += it.qty;

        if req.post {
            apply_stock(
                &mut tx,
                pid,
                -it.qty,
                "outbound",
                order_id,
                &doc_no,
                user_id,
                Some(&customer),
                allow_negative,
            )
            .await?;
        }
    }

    sqlx::query("UPDATE outbound_orders SET total_qty = ? WHERE id = ?")
        .bind(total)
        .bind(order_id)
        .execute(&mut tx)
        .await?;

    if let Some(op_id) = req.op_id.as_deref().filter(|s| !s.is_empty()) {
        record_op(
            &mut tx,
            op_id,
            user_id,
            "outbound_create",
            "ok",
            Some(&serde_json::json!({"order_id": order_id, "doc_no": doc_no})),
            None,
        )
        .await?;
    }

    tx.commit().await?;
    outbound_detail(pool, order_id).await
}

pub async fn outbound_detail(pool: &SqlitePool, id: i64) -> AppResult<OutboundDetail> {
    let order = sqlx::query_as::<_, OutboundOrder>(
        "SELECT id, doc_no, customer, remark, status, total_qty, created_by, created_at, posted_at
         FROM outbound_orders WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| AppError::not_found("出库单"))?;

    let creator_name: String = sqlx::query_scalar("SELECT name FROM users WHERE id = ?")
        .bind(order.created_by)
        .fetch_one(pool)
        .await?;

    let items = sqlx::query_as::<_, OutItem>(
        "SELECT oi.id, oi.order_id, oi.product_id, oi.qty, p.sku, p.name, p.unit
         FROM outbound_items oi
         JOIN products p ON p.id = oi.product_id
         WHERE oi.order_id = ? ORDER BY p.sku",
    )
    .bind(id)
    .fetch_all(pool)
    .await?;

    Ok(OutboundDetail {
        order,
        creator_name,
        items,
    })
}