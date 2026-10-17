use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// ---------- 通用 ----------
#[derive(Debug, Serialize)]
pub struct ApiOk<T: Serialize> {
    pub ok: bool,
    pub data: T,
}

impl<T: Serialize> ApiOk<T> {
    pub fn new(data: T) -> Self {
        Self { ok: true, data }
    }
}

pub type ApiResult<T> = Result<axum::Json<ApiOk<T>>, crate::error::AppError>;

/// ---------- 登录 ----------
#[derive(Debug, Deserialize)]
pub struct LoginReq {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Serialize)]
pub struct LoginResp {
    pub token: String,
    pub user: UserInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct UserInfo {
    pub id: i64,
    pub username: String,
    pub name: String,
    pub role: String,
}

/// ---------- 商品 / 型号 ----------
#[derive(Debug, Clone, Deserialize)]
pub struct ProductUpsert {
    pub sku: String,
    pub name: String,
    #[serde(default)]
    pub spec: Option<String>,
    #[serde(default = "default_unit")]
    pub unit: String,
    #[serde(default)]
    pub cost_price: f64,
    #[serde(default)]
    pub sale_price: f64,
    #[serde(default)]
    pub warn_qty: i64,
    /// 编辑时传 id，新建时留空
    #[serde(default)]
    pub id: Option<i64>,
}

fn default_unit() -> String {
    "件".to_string()
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Product {
    pub id: i64,
    pub sku: String,
    pub name: String,
    pub spec: Option<String>,
    pub unit: String,
    pub cost_price: f64,
    pub sale_price: f64,
    pub stock_qty: i64,
    pub warn_qty: i64,
    pub active: i64,
}

/// ---------- 入库 ----------
#[derive(Debug, Clone, Deserialize)]
pub struct InboundCreateReq {
    pub waybill_no: String,
    #[serde(default)]
    pub supplier_name: Option<String>,
    #[serde(default)]
    pub remark: Option<String>,
    pub boxes: Vec<BoxInput>,
    /// 客户端生成的幂等 ID，离线重放时保证不重复入库
    #[serde(default)]
    pub op_id: Option<String>,
    /// true = 保存即过账（直接入库存）；false = 存草稿
    #[serde(default = "default_true")]
    pub post: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
pub struct BoxInput {
    /// 箱号，1..N 或自定义
    pub box_no: String,
    #[serde(default)]
    pub remark: Option<String>,
    pub items: Vec<BoxItemInput>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BoxItemInput {
    /// 型号；也允许只传 product_id
    #[serde(default)]
    pub sku: Option<String>,
    #[serde(default)]
    pub product_id: Option<i64>,
    pub qty: i64,
    #[serde(default)]
    pub remark: Option<String>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct InboundOrder {
    pub id: i64,
    pub doc_no: String,
    pub waybill_no: String,
    pub supplier_name: Option<String>,
    pub remark: Option<String>,
    pub status: String,
    pub total_boxes: i64,
    pub total_qty: i64,
    pub created_by: i64,
    pub created_at: String,
    pub posted_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct InboundBox {
    pub id: i64,
    pub order_id: i64,
    pub box_no: String,
    pub remark: Option<String>,
    pub total_qty: i64,
    pub sort_no: i64,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct BoxItem {
    pub id: i64,
    pub box_id: i64,
    pub product_id: i64,
    pub qty: i64,
    pub remark: Option<String>,
    pub sku: String,
    pub name: String,
    pub unit: String,
}

/// 箱 + 箱内明细
#[derive(Debug, Clone, Serialize)]
pub struct BoxWithItems {
    #[serde(flatten)]
    pub box_info: InboundBox,
    pub items: Vec<BoxItem>,
}

#[derive(Debug, Clone, Serialize)]
pub struct InboundDetail {
    #[serde(flatten)]
    pub order: InboundOrder,
    pub creator_name: String,
    pub boxes: Vec<BoxWithItems>,
    /// 按型号汇总的合计，用于"这单一共多少件、每型号多少件"
    pub summary: Vec<ModelSummary>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ModelSummary {
    pub product_id: i64,
    pub sku: String,
    pub name: String,
    pub unit: String,
    pub qty: i64,
    pub boxes: i64,
}

/// ---------- 出库 ----------
#[derive(Debug, Clone, Deserialize)]
pub struct OutboundCreateReq {
    pub customer: String,
    #[serde(default)]
    pub remark: Option<String>,
    /// 明细：型号 + 数量
    pub items: Vec<OutItemInput>,
    #[serde(default)]
    pub op_id: Option<String>,
    #[serde(default = "default_true")]
    pub post: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OutItemInput {
    #[serde(default)]
    pub sku: Option<String>,
    #[serde(default)]
    pub product_id: Option<i64>,
    pub qty: i64,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct OutboundOrder {
    pub id: i64,
    pub doc_no: String,
    pub customer: String,
    pub remark: Option<String>,
    pub status: String,
    pub total_qty: i64,
    pub created_by: i64,
    pub created_at: String,
    pub posted_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct OutItem {
    pub id: i64,
    pub order_id: i64,
    pub product_id: i64,
    pub qty: i64,
    pub sku: String,
    pub name: String,
    pub unit: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct OutboundDetail {
    #[serde(flatten)]
    pub order: OutboundOrder,
    pub creator_name: String,
    pub items: Vec<OutItem>,
}

/// ---------- 库存 ----------
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct StockRow {
    pub id: i64,
    pub sku: String,
    pub name: String,
    pub spec: Option<String>,
    pub unit: String,
    pub stock_qty: i64,
    pub warn_qty: i64,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Movement {
    pub id: i64,
    pub product_id: i64,
    pub change_qty: i64,
    pub balance_qty: i64,
    pub biz_type: String,
    pub biz_no: Option<String>,
    pub created_at: String,
    pub sku: String,
    pub name: String,
    pub operator_name: String,
}

/// ---------- 工作台 ----------
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Dashboard {
    pub sku_total: i64,
    pub stock_total: i64,
    pub today_in_qty: i64,
    pub today_out_qty: i64,
    pub today_in_orders: i64,
    pub today_out_orders: i64,
    pub pending_draft: i64,
    pub low_stock_count: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub keyword: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub limit: Option<i64>,
    #[serde(default)]
    pub offset: Option<i64>,
}

/// ---------- 离线同步 ----------
#[derive(Debug, Clone, Deserialize)]
pub struct SyncPushReq {
    /// 客户端离线期间排队的操作，按发生顺序排列
    pub ops: Vec<SyncOp>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SyncOp {
    pub op_id: String,
    /// inbound_create | outbound_create
    pub op_type: String,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct SyncOpResult {
    pub op_id: String,
    pub status: String,
    pub message: Option<String>,
    pub data: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SyncPushResp {
    pub results: Vec<SyncOpResult>,
}

#[derive(Debug, Serialize)]
pub struct SyncMeta {
    pub server_time: DateTime<Utc>,
    pub status_code: u16,
}