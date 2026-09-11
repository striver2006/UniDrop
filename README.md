# UniDrop (UniClip)

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
  * **数据面（Data Plane）**：独立 WSS 通道，支持 64 字节定长帧头、4MB 流水线滑动窗口（Window Size=4）、动态 RTO 与选择性重传（ACK / NACK）。
* **轻量常驻与低资源开销**：
  * **服务端 (Go)**：极简轻量级中继，零磁盘内存管道透传，单实例常驻仅需 ~15MB。
  * **客户端 (Tauri 2.0)**：基于 Rust 原生守护进程 + 按需呼出的 React 18 前端面板，主进程常驻内存仅 20~30MB。
* **企业级安全设计**：
  * **沙盒防护与防路径穿越（PathGuard）**：严格拦截目录逃逸（`..`）及 Windows 历史保留设备文件名（`CON`, `PRN`, `NUL` 等）。
  * **隐私敏感数据过滤**：自动识别并忽略 1Password、Keepass 等密码管理器的隐藏标签（`Clipboard Viewer Ignore` / `org.nspasteboard.ConcealedType`）。
  * **2 小时剪贴板保护锁**：防止临时文件被 TTL / LRU 扫描器提前清理而导致资源管理器悬空指针。
  * **端到端加密（E2EE 规划）**：基于设备长期 Ed25519 签名的认证密钥协商（Noise-based AKE）与 AES-256-GCM 分块加密。

---

## 🏗 架构拓扑 (Architecture)

```text
+---------------------+                            +---------------------+
|  Client A (发送端)  |                            |  Client B (接收端)  |
|  Tauri (Rust Core)  |                            |  Tauri (Rust Core)  |
+----------+----------+                            +----------+----------+
           |                                                  |
           | 1. 控制信令 (WSS /ws/control)                    | 1. 控制信令 (WSS /ws/control)
           v                                                  v
+------------------------------------------------------------------------+
|                      UniDrop 公网中继服务 (Go)                         |
|   ├── DeviceRegistry (sync.RWMutex 在线设备管理)                       |
|   ├── RelayPipeManager (sync.Pool 零磁盘流式中转)                      |
|   └── STUN Server (RFC 8489 NAT 探测辅助)                              |
+------------------------------------------------------------------------+
           |                                                  |
           | 2. 4MB 分片数据 (WSS /ws/data)                   | 2. 实时流式分片下发
           +=================================================>+
                                                              |
                                                              v
                                                [临时沙盒缓存 & PathGuard]
                                                              |
                                                    (用户点击系统 Toast)
                                                              v
                                                [注入底层系统原生剪贴板]
                                                              |
                                                    (用户按 Ctrl+V / Cmd+V)
                                                              v
                                                [资源管理器将文件复制到目标目录]
```

---

## 📁 目录结构 (Directory Structure)

```text
.
├── server/                         # Go 服务端根目录
│   ├── cmd/unidrop-server/         # 服务端入口 main.go
│   ├── configs/                    # 配置文件示例
│   └── internal/                   # 服务端内部模块
│       ├── auth/                   # HMAC-SHA256 接入鉴权与 60s 防重放
│       ├── config/                 # 配置映射器
│       ├── controller/             # 控制面与数据面 WebSocket 路由
│       ├── protocol/               # 64 字节定长帧与 JSON Envelope 编解码
│       ├── registry/               # sync.RWMutex 设备在线表与会话管理
│       └── relay/                  # sync.Pool 驱动的流式数据中继管道
├── client/                         # Tauri 客户端根目录
│   ├── src-tauri/                  # Rust 原生后台守护核心
│   │   ├── src/core/               # PathGuard 路径安全、滑动窗口、缓存管理
│   │   ├── src/platform/           # Win32 / macOS / Linux 底层剪贴板注入与监听
│   │   ├── src/protocol/           # 跨平台协议对齐定义
│   │   └── src/storage/            # SQLite 本地任务、位图与公钥存储
│   └── src/                        # 前端轻量面板 (React 18 + Vite + Tailwind)
├── docs/                           # 系统需求规格 (PRD) 与详细设计 (LLD)
├── scripts/                        # 常用辅助脚本
└── .github/workflows/              # CI/CD 自动化流水线
```

---

## 🚀 快速开始 (Quick Start)

### 1. 服务端运行 (Server)
环境要求：Go 1.22+

```bash
# 进入服务端目录
cd server

# 运行单元测试
go test -v -race ./internal/...

# 编译并运行服务端 (默认监听 8080 端口)
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
```

---

## 📖 设计文档

* [需求规格说明书 (REQUIREMENTS.md)](./REQUIREMENTS.md)
* [系统详细设计说明书 (DESIGN.md)](./DESIGN.md)

---

## 📄 开源许可证

本项目采用 [Apache-2.0 License](./LICENSE) 许可证。
