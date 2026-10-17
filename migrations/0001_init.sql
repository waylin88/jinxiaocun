-- ============================================================
-- 进销存系统 初始表结构
-- SQLite / WAL
-- ============================================================

-- 用户 ------------------------------------------------------------
CREATE TABLE IF NOT EXISTS users (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    username      TEXT    NOT NULL UNIQUE,
    password_hash TEXT    NOT NULL,
    name          TEXT    NOT NULL,
    role          TEXT    NOT NULL DEFAULT 'operator',  -- admin | operator
    active        INTEGER NOT NULL DEFAULT 1,
    created_at    TEXT    NOT NULL
);

-- 型号字典（商品主数据）------------------------------------------
CREATE TABLE IF NOT EXISTS products (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    sku        TEXT    NOT NULL UNIQUE,          -- 型号
    name       TEXT    NOT NULL,                 -- 商品名称
    spec       TEXT,                             -- 规格
    unit       TEXT    NOT NULL DEFAULT '件',    -- 单位
    barcode    TEXT,                             -- 预留条码（暂未使用）
    cost_price REAL    NOT NULL DEFAULT 0,
    sale_price REAL    NOT NULL DEFAULT 0,
    stock_qty  INTEGER NOT NULL DEFAULT 0,       -- 库存结存
    warn_qty   INTEGER NOT NULL DEFAULT 0,       -- 低库存预警线
    active     INTEGER NOT NULL DEFAULT 1,
    created_at TEXT    NOT NULL,
    updated_at TEXT    NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_products_sku    ON products(sku);
CREATE INDEX IF NOT EXISTS idx_products_name   ON products(name);
CREATE INDEX IF NOT EXISTS idx_products_active ON products(active);

-- 供应商 ------------------------------------------------------------
CREATE TABLE IF NOT EXISTS suppliers (
    id      INTEGER PRIMARY KEY AUTOINCREMENT,
    name    TEXT    NOT NULL UNIQUE,
    contact TEXT,
    phone   TEXT,
    active  INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL
);

-- 入库单（一张单 = 一个运单号）--------------------------------------
CREATE TABLE IF NOT EXISTS inbound_orders (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    doc_no      TEXT    NOT NULL UNIQUE,
    waybill_no  TEXT    NOT NULL,             -- 运单号
    supplier_id INTEGER REFERENCES suppliers(id),
    supplier_name TEXT,                       -- 冗余：供应商可直接填名
    remark      TEXT,
    status      TEXT    NOT NULL DEFAULT 'draft',  -- draft | posted | void
    total_boxes INTEGER NOT NULL DEFAULT 0,
    total_qty   INTEGER NOT NULL DEFAULT 0,
    created_by  INTEGER NOT NULL REFERENCES users(id),
    created_at  TEXT    NOT NULL,
    posted_at   TEXT,
    posted_by   INTEGER REFERENCES users(id)
);
CREATE INDEX IF NOT EXISTS idx_inbound_waybill ON inbound_orders(waybill_no);
CREATE INDEX IF NOT EXISTS idx_inbound_status  ON inbound_orders(status);
CREATE INDEX IF NOT EXISTS idx_inbound_created ON inbound_orders(created_at);

-- 箱（入库单下的一箱）-----------------------------------------------
CREATE TABLE IF NOT EXISTS inbound_boxes (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    order_id   INTEGER NOT NULL REFERENCES inbound_orders(id) ON DELETE CASCADE,
    box_no     TEXT    NOT NULL,               -- 箱号（1..N 或自定义箱标）
    remark     TEXT,
    total_qty  INTEGER NOT NULL DEFAULT 0,
    sort_no    INTEGER NOT NULL DEFAULT 0,
    created_at TEXT    NOT NULL,
    updated_at TEXT    NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_boxes_order ON inbound_boxes(order_id);

-- 箱内明细：型号 + 数量  ★核心表★ ---------------------------------
CREATE TABLE IF NOT EXISTS inbound_box_items (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    box_id     INTEGER NOT NULL REFERENCES inbound_boxes(id) ON DELETE CASCADE,
    order_id   INTEGER NOT NULL REFERENCES inbound_orders(id) ON DELETE CASCADE,
    product_id INTEGER NOT NULL REFERENCES products(id),
    qty        INTEGER NOT NULL,
    remark     TEXT
);
CREATE INDEX IF NOT EXISTS idx_boxitems_box     ON inbound_box_items(box_id);
CREATE INDEX IF NOT EXISTS idx_boxitems_product ON inbound_box_items(product_id);
CREATE UNIQUE INDEX IF NOT EXISTS idx_boxitems_uniq ON inbound_box_items(box_id, product_id);

-- 出库单 --------------------------------------------------------------
CREATE TABLE IF NOT EXISTS outbound_orders (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    doc_no     TEXT    NOT NULL UNIQUE,
    customer   TEXT    NOT NULL,               -- 客户名
    remark     TEXT,
    status     TEXT    NOT NULL DEFAULT 'draft',  -- draft | posted | void
    total_qty  INTEGER NOT NULL DEFAULT 0,
    created_by INTEGER NOT NULL REFERENCES users(id),
    created_at TEXT    NOT NULL,
    posted_at  TEXT,
    posted_by  INTEGER REFERENCES users(id)
);
CREATE INDEX IF NOT EXISTS idx_outbound_status  ON outbound_orders(status);
CREATE INDEX IF NOT EXISTS idx_outbound_created ON outbound_orders(created_at);
CREATE INDEX IF NOT EXISTS idx_outbound_customer ON outbound_orders(customer);

CREATE TABLE IF NOT EXISTS outbound_items (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    order_id   INTEGER NOT NULL REFERENCES outbound_orders(id) ON DELETE CASCADE,
    product_id INTEGER NOT NULL REFERENCES products(id),
    qty        INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_outitems_order   ON outbound_items(order_id);
CREATE INDEX IF NOT EXISTS idx_outitems_product ON outbound_items(product_id);
CREATE UNIQUE INDEX IF NOT EXISTS idx_outitems_uniq ON outbound_items(order_id, product_id);

-- 库存流水（一切库存变动的唯一入口）----------------------------------
CREATE TABLE IF NOT EXISTS stock_movements (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    product_id   INTEGER NOT NULL REFERENCES products(id),
    change_qty   INTEGER NOT NULL,        -- 正 = 入库，负 = 出库
    balance_qty  INTEGER NOT NULL,        -- 变动后结存
    biz_type     TEXT    NOT NULL,        -- inbound | outbound
    biz_id       INTEGER NOT NULL,
    biz_no       TEXT,
    operator_id  INTEGER NOT NULL REFERENCES users(id),
    created_at   TEXT    NOT NULL,
    remark       TEXT
);
CREATE INDEX IF NOT EXISTS idx_mv_product ON stock_movements(product_id, id DESC);
CREATE INDEX IF NOT EXISTS idx_mv_biz     ON stock_movements(biz_type, biz_id);

-- 离线同步幂等表 ----------------------------------------------------
CREATE TABLE IF NOT EXISTS sync_ops (
    op_id       TEXT    PRIMARY KEY,      -- 客户端生成的 UUID，重放不重复执行
    user_id     INTEGER NOT NULL,
    op_type     TEXT    NOT NULL,
    status      TEXT    NOT NULL,         -- ok | error
    result      TEXT,
    error       TEXT,
    created_at  TEXT    NOT NULL
);