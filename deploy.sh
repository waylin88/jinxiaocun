#!/usr/bin/env bash
# Ubuntu 一键部署脚本
# 用法：sudo bash deploy.sh
set -euo pipefail

APP_NAME=jinxiaocun
APP_DIR=/opt/jinxiaocun
SVC_USER=jinxiaocun

echo "==> 安装系统依赖"
apt-get update -qq
apt-get install -y -qq curl ca-certificates

if ! command -v rustc &> /dev/null; then
  echo "==> 安装 Rust 工具链"
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
  source "$HOME/.cargo/env"
fi

echo "==> 创建服务用户"
id -u "$SVC_USER" &>/dev/null || useradd -r -m -d "$APP_DIR" -s /bin/bash "$SVC_USER"

echo "==> 同步程序文件"
mkdir -p "$APP_DIR"
rsync -a --delete \
  --exclude 'target' --exclude '.git' --exclude 'data' \
  ./ "$APP_DIR/src" 2>/dev/null || true

# 简单方式：整目录拷贝
mkdir -p "$APP_DIR/data"
cp -r ./src "$APP_DIR/"
cp -r ./static "$APP_DIR/"
cp -r ./migrations "$APP_DIR/"
cp ./Cargo.toml "$APP_DIR/"

# .env：不存在就用示例创建
if [ ! -f "$APP_DIR/.env" ]; then
  cp ./.env.example "$APP_DIR/.env"
  SECRET=$(openssl rand -hex 32)
  sed -i "s|^JWT_SECRET=.*|JWT_SECRET=${SECRET}|" "$APP_DIR/.env"
  echo "==> 已生成 .env，JWT_SECRET 已随机设置"
  echo "    初始账号见 .env 中 ADMIN_USERNAME / ADMIN_PASSWORD，登录后请立即修改"
fi

chown -R "$SVC_USER:$SVC_USER" "$APP_DIR"

echo "==> 编译 release 版本"
cd "$APP_DIR"
su -s /bin/bash "$SVC_USER" -c "cd $APP_DIR && source \$HOME/.cargo/env 2>/dev/null; cargo build --release"
# cargo 在 /root/.cargo 时兜底
if [ ! -x "$APP_DIR/target/release/jinxiaocun" ]; then
  cargo build --release
fi

echo "==> 注册 systemd 服务"
cat > /etc/systemd/system/${APP_NAME}.service <<EOF
[Unit]
Description=进销存管理服务
After=network.target

[Service]
Type=simple
User=${SVC_USER}
WorkingDirectory=${APP_DIR}
EnvironmentFile=${APP_DIR}/.env
ExecStart=${APP_DIR}/target/release/jinxiaocun
Restart=always
RestartSec=3
# 允许数据库文件落在工作目录
ReadWritePaths=${APP_DIR}/data

[Install]
WantedBy=multi-user.target
EOF

systemctl daemon-reload
systemctl enable ${APP_NAME}
systemctl restart ${APP_NAME}

echo "==> 部署完成"
sleep 2
systemctl --no-pager status ${APP_NAME} || true
echo
echo "本机访问：http://127.0.0.1:8080"
echo "手机访问：http://<服务器IP>:8080"
echo "如需外网访问，请自行配置 Nginx 反向代理并申请 HTTPS 证书"