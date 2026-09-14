# 瞬贴 (UniDrop) 阿里云公网服务器生产部署实战指南 (Alibaba Cloud Linux)

> **实战环境规格**：
> * **操作系统**：Alibaba Cloud Linux 3 / 2 (Alinux，兼容 RHEL/CentOS 8，`dnf`/`yum` 包管理器)
> * **网络拓扑**：独立公网 IP + 自有域名（如 `drop.yourdomain.com`）
> * **安全策略**：全生僻高位非标端口（防扫描）+ WSS 强加密 + 内部服务回环绑定 + Systemd 沙箱隔离
> * **架构选型**：原生单二进制（无容器）+ Systemd 守护进程 + Nginx 反向代理 + Let's Encrypt 免费 SSL 证书

---

## 架构拓扑与端口规划

```text
                                        [公网客户端 (瞬贴桌面端)]
                                                    |
                         +--------------------------+--------------------------+
                         | (TCP 58921 / WSS 加密流)                             | (UDP 58922 / STUN 探测)
                         v                                                     v
          +------------------------------+                      +------------------------------+
          |   Nginx 反向代理 (SSL 终止)   |                      |  UniDrop 内置 STUN 探测服务   |
          |   监听: 0.0.0.0:58921 ssl    |                      |      监听: :58922 (UDP)      |
          +--------------+---------------+                      +------------------------------+
                         | 本地明文 HTTP/WS (127.0.0.1:18080)
                         v
          +------------------------------+
          |     unidrop-server 守护进程   |
          |  (Systemd 托管，低权限运行)   |
          +------------------------------+
```

### 端口矩阵分配
* **TCP `58921`**：对外开放的 WSS / HTTPS 唯一加密业务端口（由 Nginx 监听并反代，替代传统 443）。
* **UDP `58922`**：对外开放的 STUN NAT 穿透辅助端口（由 UniDrop 直接监听，替代传统 3478）。
* **TCP `18080`**：UniDrop Go 服务端本地监听端口（仅绑定 `127.0.0.1`，公网完全隐身）。

---

## 第一阶段：阿里云 ECS 安全组配置

登录 **阿里云 ECS 管理控制台 -> 实例 -> 安全组 -> 添加入方向规则**：

| 规则名称 | 授权策略 | 协议类型 | 端口范围 | 授权对象 | 说明 |
| :--- | :--- | :--- | :--- | :--- | :--- |
| **WSS 加密业务** | 允许 | **自定义 TCP** | **`58921`** | `0.0.0.0/0` | 客户端连接的核心加密端口 |
| **STUN NAT 探测** | 允许 | **自定义 UDP** | **`58922`** | `0.0.0.0/0` | 客户端 NAT 打洞探测端口 |
| **证书签发临时** | 允许 | **自定义 TCP** | **`80`** | `0.0.0.0/0` | 首次运行 Certbot 申请证书时放行，完成后可关闭 |

> [!NOTE]
> 传统的 `443`、`8080`、`3478` 端口均**无需放行**，最大化规避全网端口扫描器（如 Shodan、Censys）的指纹探测。

---

## 第二阶段：编译与安装 UniDrop 服务端

### 1. 安装基础依赖
在阿里云终端中执行：
```bash
sudo dnf install -y git golang nginx openssl-devel
```
*(注：在较早版本 Alinux 上若 dnf 不可用，可直接替换为 `sudo yum install -y`)*

### 2. 获取源码并编译
国内阿里云机器直接访问 GitHub 可能遇到 `Empty reply from server`，推荐使用国内加速镜像克隆：

```bash
cd /tmp
# 使用高速镜像代理拉取
git clone https://ghproxy.net/https://github.com/striver2006/UniDrop.git
cd UniDrop/server

# 配置国内 Go 模块代理（加速依赖下载）
go env -w GOPROXY=https://goproxy.cn,direct

# 静态编译极简单二进制（剥离调试符号）
CGO_ENABLED=0 go build -ldflags="-w -s" -o unidrop-server ./cmd/unidrop-server
```

> [!TIP]
> **备选方案（本地 Mac 跨平台秒传）**：也可以直接在开发机 Mac 终端执行 `CGO_ENABLED=0 GOOS=linux GOARCH=amd64 go build -ldflags="-w -s" -o unidrop-server ./cmd/unidrop-server`，然后通过 `scp unidrop-server 用户名@ECS公网IP:/tmp/` 上传，服务器甚至无需安装 Go 环境。

### 3. 创建低特权用户与安装目录
```bash
# 创建不允许登录系统的专用运行账户 unidrop
sudo useradd -r -s /sbin/nologin unidrop 2>/dev/null || true

# 规范化安装目录
sudo mkdir -p /opt/unidrop
sudo mv unidrop-server /opt/unidrop/
sudo chown -R unidrop:unidrop /opt/unidrop
sudo chmod 750 /opt/unidrop/unidrop-server
```

---

## 第三阶段：环境变量配置与预共享密钥 (PSK)

```bash
# 1. 生成一段 32 字节高强度随机密钥
openssl rand -hex 32
# 终端将输出类似：7a8f9c2d1e0b4a5f6e8d7c9b0a1f2e3d4c5b6a7f8e9d0c1b2a3f4e5d6c7b8a9f
# 请务必复制保存这串密钥！客户端配对需要用到。

# 2. 创建环境变量配置文件
sudo tee /etc/default/unidrop > /dev/null <<EOF
UNIDROP_LISTEN_ADDR=127.0.0.1:18080
UNIDROP_STUN_ADDR=:58922
UNIDROP_PSK_SECRET=你的高强度随机密钥串
UNIDROP_HEARTBEAT_TIMEOUT=45
EOF

# 3. 严格加固权限：仅 root 可读写
sudo chmod 600 /etc/default/unidrop
```

---

## 第四阶段：配置 Systemd 守护进程与开机自启

创建 `/etc/systemd/system/unidrop.service` 系统服务单元：

```bash
sudo tee /etc/systemd/system/unidrop.service > /dev/null <<'EOF'
[Unit]
Description=UniDrop Relay Server Daemon
After=network.target network-online.target
Wants=network-online.target

[Service]
Type=simple
User=unidrop
Group=unidrop
WorkingDirectory=/opt/unidrop
ExecStart=/opt/unidrop/unidrop-server
Restart=always
RestartSec=5s
EnvironmentFile=/etc/default/unidrop

# 安全沙箱强化 (阻止提权、文件系统隔离)
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
PrivateTmp=true
ProtectKernelTunables=true
ProtectControlGroups=true
LimitNOFILE=65535

[Install]
WantedBy=multi-user.target
EOF
```

启动并设置开机自启：
```bash
sudo systemctl daemon-reload
sudo systemctl enable --now unidrop

# 检查运行状态（应显示 active (running)）
sudo systemctl status unidrop

# 测试本地健康接口
curl http://127.0.0.1:18080/healthz
# 正常返回：{"active_pipes":0,"connected_devices":0,"goroutines":12,"status":"ok",...}
```

---

## 第五阶段：准备 SSL 证书

**先确定走哪条路。** 两条路的后续配置不同，选错要返工：

| 你的情况 | 走哪条 | 客户端连接安全档位 |
| --- | --- | --- |
| 有已备案域名，能解析到这台 ECS | **路径 A**：Let's Encrypt | 「校验公共 CA 证书」（默认档） |
| 域名未备案，或干脆只想用 IP 连 | **路径 B**：长期自签证书 | 「信任指定证书」（填指纹） |

> **为什么未备案会卡住路径 A**：阿里云大陆节点对未备案域名的 80 / 443 访问会
> 施加拦截。UniDrop 自身走 58921 非标端口，通常不受影响——**用 IP 连是能正常
> 工作的**。卡住的是证书签发：Let's Encrypt 的 HTTP-01 验证要带着**域名**访问
> 80 端口，那一步会被拦下，于是签不出证书。
>
> 这种情况下不必绕弯子（申请备案、换境外节点、折腾 DNS-01），直接走路径 B——
> 客户端的「信任指定证书」档就是为这个场景设计的。

### 路径 A：域名 + Let's Encrypt

```bash
# 1. 安装 Certbot
sudo dnf install -y certbot || sudo pip3 install certbot

# 2. 临时停止可能在运行的 Nginx，释放 80 端口
sudo systemctl stop nginx

# 3. 独立模式申请正式证书（将 drop.yourdomain.com 替换为您的真实域名）
sudo certbot certonly --standalone -d drop.yourdomain.com

# 申请成功后证书文件保存在：
# 证书链：/etc/letsencrypt/live/你的真实域名/fullchain.pem
# 私钥：  /etc/letsencrypt/live/你的真实域名/privkey.pem
```

`--standalone` 需要 80 端口**从公网可达**（Let's Encrypt 要来访问它），且续期时
同样需要。若安全组或防火墙关闭了 80，续期会在到期前 30 天静默失败。

### 路径 B：IP 直连 + 长期自签证书

客户端用 IP 连接时，公共 CA 证书本来就派不上用场（证书签给域名，IP 连接过不了
名称校验），只能靠指纹。既然如此，直接签一张十年期的自签证书，指纹十年不变，
省掉 certbot 与续期维护：

```bash
sudo mkdir -p /etc/nginx/ssl
sudo openssl req -x509 -newkey rsa:2048 -nodes -days 3650 \
  -keyout /etc/nginx/ssl/unidrop.key \
  -out /etc/nginx/ssl/unidrop.crt \
  -subj "/CN=UniDrop" \
  -addext "subjectAltName=IP:你的公网IP"
sudo chmod 600 /etc/nginx/ssl/unidrop.key
sudo chmod 644 /etc/nginx/ssl/unidrop.crt

# 必须确认是 X.509 v3——客户端在解析阶段就会拒绝 v1 证书，
# 那一步早于任何校验逻辑，选哪个档位都绕不过
openssl x509 -in /etc/nginx/ssl/unidrop.crt -noout -text | grep Version
# 必须输出 Version: 3 (0x2)

# 取指纹，填进客户端「设置 → 连接安全 → 信任指定证书」
sudo openssl x509 -fingerprint -sha256 -noout -in /etc/nginx/ssl/unidrop.crt
```

走路径 B 时，第一阶段安全组里的 **80 端口可以一并关掉**——它只服务于
Let's Encrypt 的验证，而路径 B 不需要。

> 自签证书在这个用法下并不比公共 CA 弱。公共 CA 证明的是「对方拥有那个域名」，
> 而你要确认的是「对方是我那台服务器」——指纹直接钉住了后者。前提是**在服务器上**
> 取指纹，而不是从客户端看到什么就信什么。
>
> 指纹的日常维护、轮换与核对方式，见
> [用户手册第 6 节「证书指纹的维护」](USER_GUIDE.md#6-证书指纹的维护只有信任指定证书档需要)。

---

## 第六阶段：配置 Nginx 监听非标端口 (`58921`)

在 Alibaba Cloud Linux 下，Nginx 配置文件存放在 `/etc/nginx/conf.d/*.conf` 下即自动载入生效。

创建配置文件 `/etc/nginx/conf.d/unidrop.conf`。**证书那两行按你在第五阶段选的
路径填**：路径 A 用 `/etc/letsencrypt/live/你的域名/` 下的，路径 B 用
`/etc/nginx/ssl/` 下的（模板里已标出）。

```bash
sudo tee /etc/nginx/conf.d/unidrop.conf > /dev/null <<'EOF'
# WebSocket 协议升级映射
map $http_upgrade $connection_upgrade {
    default upgrade;
    ''      close;
}

# ── 访问日志脱敏 ──────────────────────────────────────────────
# 数据面 URL 形如 /ws/data?session_id=..&device_id=..&token=..，
# 而 nginx 默认的 combined 格式记录完整 $request——一次性会话令牌
# 会跟着落盘，而 access.log 权限通常较宽，还会被日志收集与备份带走。
#
# 只抹 token：session_id 与 device_id 是排查传输问题的主要线索，不能一起丢。
map $args $args_safe {
    # 必须用命名捕获组：map 里 $1/$2 不是有效变量，
    # 写成 $1 会让 nginx 报 unknown "1" variable 而拒绝启动。
    ~^(?<pre>.*)token=[^&]*(?<post>.*)$  "${pre}token=<redacted>${post}";
    default                              $args;
}

# 没有查询串时不要留下一个光秃秃的问号
map $args $query_suffix {
    ""      "";
    default "?$args_safe";
}

log_format unidrop '$remote_addr - $remote_user [$time_local] '
                   '"$request_method $uri$query_suffix $server_protocol" '
                   '$status $body_bytes_sent "$http_referer" "$http_user_agent"';
# ─────────────────────────────────────────────────────────────

server {
    # 直接监听高位非标加密端口，启用 HTTP/2 与 SSL
    listen 58921 ssl http2;

    # 路径 A 填你的域名；路径 B（IP 直连）保留下划线即可，表示匹配任意主机名
    server_name drop.yourdomain.com;

    # ↓↓ 路径 A：Let's Encrypt 证书
    ssl_certificate /etc/letsencrypt/live/drop.yourdomain.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/drop.yourdomain.com/privkey.pem;
    # ↓↓ 路径 B：改用自签证书（把上面两行换成下面两行）
    # ssl_certificate     /etc/nginx/ssl/unidrop.crt;
    # ssl_certificate_key /etc/nginx/ssl/unidrop.key;
    ssl_protocols TLSv1.2 TLSv1.3;
    ssl_ciphers HIGH:!aNULL:!MD5;

    # 启用上面定义的脱敏日志格式
    access_log /var/log/nginx/access.log unidrop;

    # 禁用上传限制（剪贴板传输走分块 WebSocket 流）
    client_max_body_size 0;

    location / {
        proxy_pass http://127.0.0.1:18080;
        proxy_http_version 1.1;

        # 核心：透传 WebSocket 协议升级头
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection $connection_upgrade;

        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;

        # 核心：防断连超时调优（防止长连接被 Nginx 静默断开）
        proxy_read_timeout 3600s;
        proxy_send_timeout 3600s;

        # 关闭缓冲，保证剪贴板内容零延迟到达
        proxy_buffering off;
    }
}
EOF
```

### 处理 Alibaba Cloud Linux 特有策略（SELinux 网络策略）
如果 Alinux 系统开启了 SELinux，Nginx 默认无法反向代理网络连接，需放行权限：
```bash
# 允许 Nginx 作为反向代理访问本地网络端口
sudo setsebool -P httpd_can_network_connect 1

# 若开启了强制策略，将 58921 端口加入 http 端口策略
sudo semanage port -a -t http_port_t -p tcp 58921 2>/dev/null || true
```

测试配置并启动 Nginx：
```bash
sudo nginx -t
# 提示 syntax is ok, test is successful 后启动
sudo systemctl restart nginx
```

---

## 第七阶段：公网全链路联通性验证

在您的**本地笔记本电脑（Mac/Windows）**终端中，执行以下测试命令：

```bash
curl -i https://drop.yourdomain.com:58921/healthz
```

**预期成功响应 (HTTP 200 OK)**：
```http
HTTP/2 200 
content-type: application/json

{"active_pipes":0,"connected_devices":0,"goroutines":12,"status":"ok","uptime_seconds":120}
```

> **安全验证**：尝试在浏览器直接访问不带端口的 `https://drop.yourdomain.com`，会直接提示“连接拒绝”或超时，说明常规端口完全隐蔽，仅您指定的非标端口正常提供加密服务！

---

## 第八阶段：客户端（瞬贴）配对与使用

在需要协同流转剪贴板的多台电脑（Mac / Windows / Linux）上打开 **瞬贴 (UniDrop)** 客户端：

1. 点击主界面右上角 **齿轮（偏好设置）图标**；
2. 填写连接与安全参数：
   * **公网中继服务器地址**：`wss://drop.yourdomain.com:58921`（协议为 **`wss://`**，包含非标端口号）
   * **账号标识 (Account ID)**：`team_work`（多台设备填写完全相同的 ID 归属同一空间）
   * **预共享密钥 (PSK)**：第三阶段生成的随机密钥串
   * **静默自动装载剪贴板**：勾选 `[X]`
3. 点击 **保存配置**（配置将即时热更新并重连生效，无需重启客户端）。

客户端主界面底部将亮起绿色盾牌 **“PSK 接入安全就绪”**，且设备列表能够实时感知对端电脑。直接拖拽文件发送，接收端即可无感按下 **`Ctrl+V` / `Cmd+V`** 直接粘贴落地！

---

## 常用运维指令速查

| 操作需求 | 执行命令 |
| :--- | :--- |
| **查看 UniDrop 实时运行日志** | `sudo journalctl -u unidrop -f -o cat` |
| **重启 UniDrop 中继服务** | `sudo systemctl restart unidrop` |
| **查看中继服务当前状态** | `sudo systemctl status unidrop` |
| **检查 Nginx 访问与错误日志** | `sudo tail -f /var/log/nginx/error.log` |
| **更新服务（Git 拉取并重编译）** | `cd /tmp/UniDrop && git pull && CGO_ENABLED=0 go build -ldflags="-w -s" -o /opt/unidrop/unidrop-server ./server/cmd/unidrop-server && sudo systemctl restart unidrop` |
