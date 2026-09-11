<h1 align="center">瞬贴 (UniDrop)</h1>

<p align="center">
  <strong>无缝、高效、安全的跨平台剪贴板与文件流转系统</strong><br>
  <em>Seamless, Secure, and Native Cross-Platform Clipboard & File Distribution</em>
</p>

<p align="center">
  <a href="./LICENSE"><img src="https://img.shields.io/badge/license-Apache--2.0-blue.svg" alt="License"></a>
  <a href="https://go.dev/"><img src="https://img.shields.io/badge/go-1.22+-00ADD8.svg" alt="Go Version"></a>
  <a href="https://tauri.app/"><img src="https://img.shields.io/badge/tauri-2.0-FFC131.svg" alt="Tauri"></a>
  <a href="https://www.rust-lang.org/"><img src="https://img.shields.io/badge/rust-1.78+-orange.svg" alt="Rust Version"></a>
</p>

---

## 🌟 核心特性 (Features)

* **原生剪贴板无感装载**：突破传统文件传输工具必须“打开特定目录再复制”的割裂体验，远端文件传输就绪后直接注入宿主机原生系统剪贴板（Windows `CF_HDROP` / macOS `fileURL` / Linux `text/uri-list`），在任意目录直接 `Ctrl+V` / `Cmd+V` 落地！
* **双通道可靠架构**：
  * **控制面（Control Plane）**：基于 WSS 长连接实现心跳保活、设备在线表同步与传输邀约协商。
  * **数据面（Data Plane）**：基于单次传输授权令牌（Data Plane Auth Token）的中继安全管道，支持 64 字节定长帧头、4MB 流水线滑动窗口（Window Size=4）、动态 RTO 与选择性重传（ACK / NACK）。
* **轻量常驻与低资源开销**：
  * **服务端 (Go)**：极简轻量级中继，零磁盘内存管道透传，专用小缓冲池隔离控制流与反向 ACK 流，单实例常驻仅需 ~15MB。
  * **客户端 (Tauri 2.0)**：基于 Rust 原生守护进程 + 按需呼出的 React 18 前端面板，主进程常驻内存仅 20~30MB。
* **企业级安全设计**：
  * **数据面预授权与单次令牌**：数据通道在 WS 升级握手阶段即完成 Token 校验，拦截未授权访问并防遍历拒绝服务。
  * **沙盒防护与防路径穿越（PathGuard）**：严格拦截目录逃逸（`..`）及 Windows 历史保留设备文件名（`CON`, `PRN`, `NUL` 等）。
  * **HMAC-SHA256 盐化挑战应答**：客户端连接校验强制绑定 NonceSalt 与时间戳，防止重放与降级攻击。
  * **持久化设备身份与配置**：基于 SQLite `local_config` 长期稳固标识设备，自动记忆服务端地址与 PSK。

---

## 🏗 架构拓扑 (Architecture)

```text
+---------------------+                                      +---------------------+
|  Client A (发送端)  |                                      |  Client B (接收端)  |
|  Tauri (Rust Core)  |                                      |  Tauri (Rust Core)  |
+----------+----------+                                      +----------+----------+
           |                                                            |
           | 1. 控制信令 (WSS /ws/control)                              | 1. 控制信令 (WSS /ws/control)
           |    HMAC-SHA256 盐化接入握手                                |    HMAC-SHA256 盐化接入握手
           v                                                            v
+----------------------------------------------------------------------------------+
|                            UniDrop 公网中继服务 (Go)                             |
|   ├── DeviceRegistry (CAS 安全注销、并发安全原子心跳、在线表广播)                 |
|   ├── RelayManager (会话状态机、数据面 Token 签发与 403 预拦截)                   |
|   ├── RelayPipe (4MB 双缓冲池 + 64B ACK 专用缓冲池，零磁盘流式中转)               |
|   └── STUN Server (RFC 8489 NAT 探测辅助，优雅平滑停机)                          |
+----------------------------------------------------------------------------------+
           |                                                            |
           | 2. 携带 DataToken 连接 (/ws/data)                          | 2. 携带 DataToken 连接 (/ws/data)
           |    4MB 流水线分块帧 (CRC32 校验)                           |    滑动窗口确认 (ACK/NACK 反向流)
           +===========================================================>+
                                                                        |
                                                                        v
                                                            [临时沙盒缓存 & PathGuard]
                                                                        |
                                                                        v (整文件 SHA-256 校验通过)
                                                            [注入系统原生剪贴板]
                                                                        |
                                                                        v (用户按下 Ctrl+V / Cmd+V)
                                                            [资源管理器无感粘贴落地]
```

---

## 📁 目录结构 (Directory Structure)

```text
.
├── server/                         # Go 服务端根目录
│   ├── cmd/unidrop-server/         # 服务端入口 main.go
│   ├── configs/                    # 配置文件示例
│   └── internal/                   # 服务端内部模块
│       ├── auth/                   # HMAC-SHA256 接入鉴权与 60s 防重放 (含盐化防降级)
│       ├── config/                 # 配置映射器
│       ├── controller/             # 控制面与数据面 WebSocket 路由与鉴权拦截
│       ├── protocol/               # 64 字节定长帧与 JSON Envelope 编解码
│       ├── registry/               # CAS 原子设备注销、设备在线表与原子心跳
│       ├── relay/                  # sync.Pool 驱动的流式数据中继管道与会话状态机
│       └── stun/                   # 兼容 RFC 8489 的 STUN 探测与平滑停机
├── client/                         # Tauri 客户端根目录
│   ├── src-tauri/                  # Rust 原生后台守护核心
│   │   ├── src/core/               # PathGuard 路径安全、滑动窗口、传输引擎、缓存管理
│   │   ├── src/platform/           # Win32 / macOS / Linux 底层剪贴板注入与监听
│   │   ├── src/protocol/           # 跨平台协议对齐定义
│   │   └── src/storage/            # SQLite 本地任务、位图、设置与长期设备 ID 持久化
│   └── src/                        # 前端轻量面板 (React 18 + Vite + Tailwind)
│       ├── components/             # 设备列表、传输进度条、设置弹窗、文件发送弹窗
│       └── App.tsx                 # 拖拽移动无边框窗口、信令事件监听与响应
├── .github/workflows/              # CI/CD 自动化流水线 (gofmt, go vet, cargo test, build)
├── REQUIREMENTS.md                 # 系统需求规格 (PRD)
└── DESIGN.md                       # 系统详细设计说明书 (LLD)
```

---

## 🚀 快速开始 (Quick Start)

### 1. 服务端运行 (Server)
环境要求：Go 1.22+

```bash
# 进入服务端目录
cd server

# 运行全量单元测试 (启用并发竞争检测器)
go test -v -race ./...

# 编译并运行服务端 (默认监听 8080 端口，STUN 监听 3478)
# 可通过环境变量指定 PSK: export UNIDROP_PSK_SECRET="your-strong-secret-key"
go run ./cmd/unidrop-server
```

### 2. 客户端运行 (Client)
环境要求：Node 18+ (推荐 pnpm), Rust 1.78+

```bash
# 进入客户端目录
cd client

# 安装前端依赖
pnpm install

# 启动开发模式 (需本地已安装 cargo)
pnpm tauri dev

# 编译发布版客户端
pnpm tauri build
```

---

## 📖 官方文档与技术手册

* [客户端安装手册 (INSTALL.md)](./docs/INSTALL.md) - macOS / Windows / Linux 安装与权限配置指南
* [服务端部署运维手册 (DEPLOY.md)](./docs/DEPLOY.md) - Docker、Systemd、Nginx 反代与监控方案
* [客户端用户使用手册 (USER_GUIDE.md)](./docs/USER_GUIDE.md) - 首次配对、设备发现、文件发送与剪贴板无感粘贴
* [需求规格说明书 (REQUIREMENTS.md)](./REQUIREMENTS.md) - 产品 PRD 规格
* [系统详细设计说明书 (DESIGN.md)](./DESIGN.md) - 架构与协议详细设计

---

## 📄 开源许可证

本项目采用 [Apache-2.0 License](./LICENSE) 许可证。
