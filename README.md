# 进销存系统

电商批发用的轻量进销存，Rust + Axum + SQLite，服务端部署在 Ubuntu，手机浏览器直接当 App 用。

## 这个系统解决什么

核心场景是**仓库点数**：一个运单号到货，里面分若干箱，每箱装着不同型号、不同数量。系统要能逐箱录入，最后回答三个问题：

- 这一单总共多少件、几箱
- 每个型号总共多少件
- 具体分布在哪几箱里

## 功能

**入库点数**

- 填运单号 → 逐箱录入型号和数量 → 自动汇总 → 提交
- 录一箱时支持搜索已有型号，也支持直接输入新型号自动建档
- 提交后按运单号可随时回查，每箱明细完整保留
- 新型号录入时自动创建档案，之后可以在「型号管理」补全名称和规格

**出库开单**

- 选客户 → 加型号数量 → 提交即扣库存
- 库存不足会拦截并提示具体缺多少

**库存**

- 实时结存、低库存预警
- 每个型号可查看完整出入库流水（谁、什么时候、入/出多少、结存多少）

**离线可用**

- 断网时照常录单，操作存本地队列
- 恢复网络自动重传，靠幂等 ID 保证不会重复入库
- 顶栏显示待同步条数，可手动查看队列和失败原因

**权限**

- 管理员 / 操作员两种角色，操作人全程留痕
- 型号删除保护：有历史流水的型号只能停用，保证账目可追溯

## 技术栈

| 层   | 选型                            |
| --- | ----------------------------- |
| 服务端 | Rust + Axum + Tokio           |
| 数据库 | SQLite（WAL 模式，单文件）            |
| 前端  | 原生 JS PWA，无构建步骤               |
| 离线  | Service Worker + IndexedDB 队列 |
| 鉴权  | JWT + Argon2                  |

前端不用打包器，整个 `static/` 目录通过 `rust-embed` 编译进二进制，部署时只有一个可执行文件 + 一个数据库文件。

## 目录结构

```
src/
  main.rs        入口、路由挂载、静态资源服务
  config.rs      环境变量配置
  db.rs          连接、迁移、初始管理员
  auth.rs        JWT 签发校验、登录态提取器
  models.rs      请求/响应数据结构
  ops.rs         业务核心：入库、出库、库存流水（事务 + 幂等）
  handlers.rs    HTTP 处理器
  error.rs       统一错误类型
migrations/      SQLite 表结构
static/          前端 PWA（编译进二进制）
deploy.sh        Ubuntu 一键部署
```

## 数据模型

```
运单（inbound_orders）
  └── 箱（inbound_boxes）        箱号、总件数
        └── 型号明细（inbound_box_items）  型号 + 数量  ← 点数核心
```

库存结存不直接依赖明细汇总，而是由 `stock_movements` 流水表驱动，每次变动都记录变动量和变动后结存，保证账目可追溯、可对账。

## 快速开始

### 本地开发

```bash
cargo run
```

默认监听 `0.0.0.0:8080`，浏览器打开 `http://127.0.0.1:8080`。

首次启动会自动创建管理员账号 `admin / admin123`，**登录后请立即修改密码**。

### Ubuntu 部署

```bash
git clone <仓库地址> jinxiaocun
cd jinxiaocun
sudo bash deploy.sh
```

脚本会自动：装依赖 → 建服务用户 → 复制文件 → 生成随机 JWT 密钥 → 编译 release → 注册 systemd 服务 → 启动。

常用命令：

```bash
systemctl status jinxiaocun    # 查看状态
systemctl restart jinxiaocun   # 重启
journalctl -u jinxiaocun -f    # 看日志
```

### 配置项

在 `/opt/jinxiaocun/.env` 中：

| 变量               | 说明            | 默认值                    |
| ---------------- | ------------- | ---------------------- |
| `BIND_ADDR`      | 监听地址          | `0.0.0.0:8080`         |
| `DATABASE_URL`   | 数据库路径         | `sqlite://data/jxc.db` |
| `JWT_SECRET`     | 签名密钥，**生产必改** | 随机生成                   |
| `JWT_TTL_HOURS`  | 登录有效期（小时）     | `720`                  |
| `ADMIN_USERNAME` | 初始管理员         | `admin`                |
| `ADMIN_PASSWORD` | 初始密码          | `admin123`             |
| `RUST_LOG`       | 日志级别          | `jinxiaocun=info`      |

### 数据备份

SQLite 单文件，备份就是拷一个文件（WAL 模式下需连 `-wal` `-shm` 一起拷，或先停服务）：

```bash
systemctl stop jinxiaocun
cp /opt/jinxiaocun/data/jxc.db /backup/jxc-$(date +%F).db
systemctl start jinxiaocun
```

## 手机使用

浏览器打开后选「添加到主屏幕」，即可像 App 一样全屏运行。

建议手机和服务器在同一 WiFi 下，或配置 Nginx + HTTPS 后通过域名访问。

## API 概览

| 方法       | 路径                              | 说明            |
| -------- | ------------------------------- | ------------- |
| POST     | `/api/auth/login`               | 登录            |
| GET      | `/api/me`                       | 当前用户          |
| PUT      | `/api/me/password`              | 改密码           |
| GET/POST | `/api/products`                 | 型号列表 / 新建修改   |
| DELETE   | `/api/products/{id}`            | 删除（有流水则停用）    |
| POST/GET | `/api/inbound`                  | 建入库单 / 列表     |
| GET      | `/api/inbound/{id}`             | 入库单详情（箱 + 明细） |
| GET      | `/api/inbound/by-waybill/{运单号}` | 按运单号查         |
| POST/GET | `/api/outbound`                 | 建出库单 / 列表     |
| GET      | `/api/outbound/{id}`            | 出库单详情         |
| GET      | `/api/stock`                    | 库存列表          |
| GET      | `/api/stock/low`                | 低库存预警         |
| GET      | `/api/stock/{id}/movements`     | 出入库流水         |
| GET      | `/api/dashboard`                | 工作台统计         |
| POST     | `/api/sync/push`                | 离线队列批量重放      |

统一响应格式：成功 `{ "ok": true, "data": ... }`，失败 `{ "ok": false, "code": "...", "message": "..." }`。
