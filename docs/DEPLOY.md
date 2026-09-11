# 瞬贴 (UniDrop) 服务端部署与运维手册 (Deployment & Operations Guide)

本文档面向系统管理员与运维工程师，详细说明“瞬贴 (UniDrop)”公网中继服务（`unidrop-server`）的架构配置、容器化与原生系统服务部署、反向代理（SSL/WSS 终止）、安全加固以及监控运维方案。

---

## 1. 服务端架构与网络端口规划

UniDrop 服务端采用极简高性能的 Go 语言编写，核心职责为**设备在线状态维护、信令调度转发、数据管道零磁盘流式中转（Zero-Disk Relay Pipe）以及轻量 STUN 探测辅助**。

### 1.1 服务与端口矩阵

| 协议 / 端口 | 路径 / 服务 | 默认端口 | 职责说明 | 防火墙策略 |
| :--- | :--- | :--- | :--- | :--- |
| **TCP** | `/ws/control` | `8080` (或 443 经代理) | 控制面信令（HMAC 盐化握手、心跳保活、在线表广播、传输协商） | 公网放行 |
| **TCP** | `/ws/data` | `8080` (或 443 经代理) | 数据面高速管道（64B 定长头 + 4MB 滑动窗口流式中转） | 公网放行 |
| **TCP** | `/healthz` | `8080` (或内部端口) | 健康检查探针（返回 JSON 状态） | 负载均衡/内部放行 |
| **TCP** | `/metrics` | `8080` (或内部端口) | Prometheus 性能监控指标接口 | 监控集群内网放行 |
| **UDP** | STUN 协议 | `3478` | RFC 8489 兼容的 NAT 映射与网络打洞探测服务 | 公网 UDP 放行 |

---

## 2. 环境变量与配置参数全解

服务端支持通过系统环境变量或配置文件进行参数注入。**生产环境中，推荐使用环境变量进行安全注入。**

| 环境变量名 | 默认值 | 说明 |
| :--- | :--- | :--- |
| `UNIDROP_LISTEN_ADDR` | `:8080` | HTTP / WebSocket 服务监听地址与端口 |
| `UNIDROP_PSK_SECRET` | *(内置弱默认值)* | **【必须修改】** 接入鉴权预共享密钥（Pre-Shared Key），用于 HMAC-SHA256 签名校验 |
| `UNIDROP_STUN_ADDR` | `:3478` | STUN 探测监听地址（若设为空字符串 `""` 则停用内置 STUN 探测服务） |
| `UNIDROP_HEARTBEAT_TIMEOUT` | `45` | 客户端心跳超时时间（秒），超过此时间未发心跳将被注销并下线广播 |

> [!WARNING]
> **安全警示**：若启动时使用默认密钥，服务端日志将发出高危警告。生产部署前，必须生成高强度随机密钥！推荐使用命令：
> ```bash
> openssl rand -hex 32
> ```

---

## 3. 部署方案一：Docker 与 Docker Compose 容器化部署（推荐）

UniDrop 官方提供了针对 Linux 环境优化、基于 Alpine 的多阶段极简安全镜像，镜像体积小于 20MB。

### 3.1 使用 Docker Compose 一键启动

在项目根目录下，直接使用预置的 `docker-compose.yml`：

```yaml
version: '3.8'

services:
  unidrop-server:
    build:
      context: ./server
      dockerfile: Dockerfile
    image: unidrop-server:latest
    container_name: unidrop-server
    restart: unless-stopped
    ports:
      - "8080:8080"      # HTTP & WSS
      - "3478:3478/udp"  # STUN NAT 探测
    environment:
      - UNIDROP_LISTEN_ADDR=:8080
      - UNIDROP_PSK_SECRET=你的高强度安全密钥_openssl_rand_hex_32
      - UNIDROP_STUN_ADDR=:3478
    healthcheck:
      test: ["CMD", "wget", "--no-verbose", "--tries=1", "--spider", "http://localhost:8080/healthz"]
      interval: 30s
      timeout: 5s
      retries: 3
      start_period: 5s
```

#### 操作指令
```bash
# 1. 启动容器（后台运行）
docker compose up -d --build

# 2. 查看容器运行状态
docker compose ps

# 3. 实时查看结构化运行日志
docker compose logs -f unidrop-server
```

---

## 4. 部署方案二：原生二进制编译与 Systemd 守护进程

对于无需容器环境的实体机或轻量云服务器，推荐直接编译单二进制部署。

### 4.1 编译可执行文件

```bash
cd server
CGO_ENABLED=0 go build -ldflags="-w -s" -o /tmp/unidrop-server ./cmd/unidrop-server
```

### 4.2 安装与权限配置

```bash
# 1. 创建专用低特权系统用户
sudo useradd -r -s /bin/false unidrop

# 2. 安装二进制至规范目录
sudo mkdir -p /opt/unidrop
sudo mv /tmp/unidrop-server /opt/unidrop/
sudo chown -R unidrop:unidrop /opt/unidrop
sudo chmod 750 /opt/unidrop/unidrop-server

# 3. 配置安全环境变量文件
sudo mkdir -p /etc/default
sudo tee /etc/default/unidrop > /dev/null <<EOF
UNIDROP_LISTEN_ADDR=:8080
UNIDROP_STUN_ADDR=:3478
UNIDROP_PSK_SECRET=$(openssl rand -hex 32)
EOF
sudo chmod 600 /etc/default/unidrop
```

### 4.3 配置并启动 Systemd 服务

复制预置的系统单元配置文件：

```bash
sudo cp docs/systemd/unidrop.service /etc/systemd/system/unidrop.service

# 重新加载 systemd 并设置自启
sudo systemctl daemon-reload
sudo systemctl enable --now unidrop

# 检查服务运行状态
sudo systemctl status unidrop
```

---

## 5. 生产环境安全网关：Nginx / Caddy 反向代理与 SSL/TLS 终止

为了保证数据在公网传输中的端到端安全性，强烈建议在外层部署带有 SSL 证书的 Web 代理（终止为标准的 `wss://` 连接）。

### 5.1 Nginx 反向代理配置样例

编辑 `/etc/nginx/sites-available/unidrop.conf`：

```nginx
# WebSocket 连接升级映射
map $http_upgrade $connection_upgrade {
    default upgrade;
    ''      close;
}

server {
    listen 80;
    server_name unidrop.yourdomain.com;
    return 301 https://$host$request_uri;
}

server {
    listen 443 ssl http2;
    server_name unidrop.yourdomain.com;

    # SSL 证书配置 (推荐使用 Let's Encrypt / Certbot)
    ssl_certificate /etc/letsencrypt/live/unidrop.yourdomain.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/unidrop.yourdomain.com/privkey.pem;
    ssl_protocols TLSv1.2 TLSv1.3;
    ssl_ciphers HIGH:!aNULL:!MD5;

    # 禁用客户端上传大小限制（UniDrop 数据走流式 WebSocket，不走传统 HTTP POST）
    client_max_body_size 0;

    # 根路由反向代理至 unidrop-server
    location / {
        proxy_pass http://127.0.0.1:8080;
        proxy_http_version 1.1;

        # 核心：透传 WebSocket 协议头
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection $connection_upgrade;

        # 真实客户端 IP 与主机名传递
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;

        # 核心：长连接保持超时设置（防止代理因空闲主动断开长连接）
        proxy_read_timeout 3600s;
        proxy_send_timeout 3600s;

        # 禁用代理缓冲以保障实时流传输低延迟
        proxy_buffering off;
    }

    # 运维监控端点访问控制（仅限内部巡检 IP 访问）
    location /metrics {
        allow 127.0.0.1;
        allow 192.168.0.0/16;
        allow 10.0.0.0/8;
        deny all;

        proxy_pass http://127.0.0.1:8080/metrics;
    }
}
```

### 5.2 Caddy 反向代理配置样例 (Caddyfile)

如果使用 Caddy，其内置全自动证书申领，配置仅需如下几行：

```caddy
unidrop.yourdomain.com {
    reverse_proxy 127.0.0.1:8080 {
        header_up Host {host}
        header_up X-Real-IP {remote_host}
    }
}
```

---

## 6. 系统与防火墙放行策略

### 6.1 UFW (Ubuntu / Debian)
```bash
sudo ufw allow 80/tcp comment 'HTTP ACME'
sudo ufw allow 443/tcp comment 'HTTPS & WSS Relay'
sudo ufw allow 3478/udp comment 'UniDrop STUN Server'
sudo ufw reload
```

### 6.2 Firewalld (CentOS / RHEL / Fedora)
```bash
sudo firewall-cmd --permanent --add-port=80/tcp
sudo firewall-cmd --permanent --add-port=443/tcp
sudo firewall-cmd --permanent --add-port=3478/udp
sudo firewall-cmd --reload
```

---

## 7. 运维监控与指标巡检

UniDrop 服务端内置轻量级结构化日志输出以及标准的 Prometheus 兼容监控接口。

### 7.1 健康检查接口 (`/healthz`)
执行命令：
```bash
curl -i http://127.0.0.1:8080/healthz
```
正常响应（HTTP 200 OK）：
```json
{
  "status": "ok",
  "uptime_seconds": 128,
  "connected_devices": 3,
  "active_pipes": 1,
  "goroutines": 14
}
```

### 7.2 Prometheus 监控指标 (`/metrics`)
UniDrop 暴露以下标准的 Prometheus 格式运行时指标：

* `unidrop_online_devices`（Gauge）：当前注册并维持在线长连接的设备总数。
* `unidrop_active_relay_pipes`（Gauge）：当前正在进行数据流转的中继管道数量。
* `unidrop_relayed_bytes_total`（Counter）：累计流经服务端中继转发的总字节数。

#### Prometheus 抓取作业样例 (`prometheus.yml`)：
```yaml
scrape_configs:
  - job_name: 'unidrop'
    scrape_interval: 15s
    static_configs:
      - targets: ['127.0.0.1:8080']
```

---

## 8. 运维常见故障排查 (Troubleshooting)

### Q1: 客户端连接出现 "鉴权失败: signature mismatch / timestamp expired"
* **原因**：客户端配置的 `psk_secret` 与服务端 `UNIDROP_PSK_SECRET` 不一致，或者客户端与服务器之间的系统时间偏差超过了防重放容差窗口（±60秒）。
* **排查方法**：
  1. 核对两端 PSK 字符串是否完全一致（避免前后多出空格）；
  2. 运行 `chrony` 或 `ntpdate` 同步服务器和客户端的时钟。

### Q2: 客户端显示离线或频繁重连
* **原因**：反向代理（如 Nginx）的 WebSocket 超时过短，在空闲无传输时强行切断了长连接。
* **解决**：在 Nginx 对应 location 中设置 `proxy_read_timeout 3600s;`，确保心跳周期（45s）能够正常续约。

### Q3: STUN 探测无响应
* **原因**：云服务商安全组或主机防火墙未放行 **UDP 3478** 端口（仅放行了 TCP 是无效的）。
* **解决**：检查安全组规则，明确添加 `UDP 3478` 允许通行。
