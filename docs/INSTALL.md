# 瞬贴 (UniDrop) 客户端安装手册 (Installation Guide)

本文档旨在指导终端用户与研发测试人员在各类桌面操作系统（macOS、Windows、Linux）上正确安装、配置系统权限并成功运行“瞬贴 (UniDrop)”客户端。

---

## 1. 系统配置要求

| 项目 | 推荐要求 | 最低要求 |
| :--- | :--- | :--- |
| **操作系统** | macOS 12+ (Apple Silicon / Intel)<br>Windows 10 / 11 64-bit (21H2+)<br>Ubuntu 22.04+ / Debian 12+ / Fedora 38+ | macOS 11.0 (Big Sur)<br>Windows 10 64-bit<br>Linux (主流发行版，glibc 2.31+) |
| **处理器** | 64 位双核及以上 (x86_64 或 ARM64) | 64 位单核处理器 |
| **内存开销** | 常驻运行约 20MB ~ 35MB | 512MB 可用系统物理内存 |
| **磁盘空间** | 预留至少 200MB 运行空间（传输大文件时需视文件体积预留沙盒缓存空间） | 100MB |
| **显示服务** | macOS Quartz / Windows DWM / Linux (X11 或 Wayland) | 支持现代 WebView 渲染引擎的环境 |

---

## 2. 方式一：直接安装官方预编译客户端

### 2.1 macOS 安装流程

1. **下载安装包**：获取对应的 `UniDrop_{version}_aarch64.dmg`（适用于 M1/M2/M3/M4 芯片）或 `UniDrop_{version}_x64.dmg`（适用于 Intel 处理器）。
2. **挂载并拖拽**：双击打开 `.dmg` 镜像，将 `UniDrop.app` 拖拽到 `Applications`（应用程序）文件夹中。
3. **安全与隐私绕过（首次启动）**：
   * 若首次启动弹出 **“无法打开，因为 Apple 无法检查其是否包含恶意软件”**：
     * 打开 **系统设置** -> **隐私与安全性** -> 向下滚动至“安全性”一栏，点击 **“仍要打开”**。
     * 或者在终端执行以下解除隔离属性的命令：
       ```bash
       sudo xattr -r -d com.apple.quarantine /Applications/UniDrop.app
       ```
4. **授予剪贴板与通知权限**：
   * UniDrop 需要在后台探测与无感注入剪贴板内容。如果系统提示请求通知或辅助功能权限，请勾选允许。

---

### 2.2 Windows 安装流程

1. **下载安装包**：获取 `UniDrop_{version}_x64_en-US.msi`（安装向导版）或免安装绿色版可执行程序。
2. **执行安装**：双击运行 `.msi` 安装向导，按提示完成安装。
3. **Microsoft Defender SmartScreen 提示处理**：
   * 若安装时出现 Windows Defender 蓝色保护提示（“Windows 已保护你的电脑”），点击 **“更多信息”** -> **“仍要运行”**。
4. **WebView2 运行环境检查**：
   * Windows 10/11 通常已内置 **Microsoft Edge WebView2 Runtime**。若系统提示缺失，请前往微软官网下载并安装 Evergreen Standalone 运行时。
5. **防火墙提示**：
   * 若弹出 Windows Defender 防火墙提示，请允许 UniDrop 访问专用和公用网络（用于中继信令通信）。

---

### 2.3 Linux 安装流程

UniDrop 客户端支持 `.deb` 包或通用 `.AppImage` 镜像。

#### 依赖库前置安装
在 Linux 环境下，UniDrop 依赖 WebKitGTK 以及剪贴板底层通信工具：

* **Ubuntu / Debian 系列**：
  ```bash
  sudo apt update
  sudo apt install -y libwebkit2gtk-4.1-0 libappindicator3-1 xclip wl-clipboard
  ```
  > **说明**：X11 桌面环境使用 `xclip`，Wayland 桌面环境使用 `wl-clipboard`。同时安装可确保在任一显示服务器下均可无缝支持剪贴板文件装载。

* **Arch Linux / Manjaro**：
  ```bash
  sudo pacman -S webkit2gtk-4.1 libappindicator-gtk3 xclip wl-clipboard
  ```

#### 安装与启动
* **通过 DEB 包安装**：
  ```bash
  sudo dpkg -i UniDrop_{version}_amd64.deb
  sudo apt-get install -f # 自动补齐缺失依赖
  ```
* **通过 AppImage 运行**：
  ```bash
  chmod +x UniDrop_{version}_amd64.AppImage
  ./UniDrop_{version}_amd64.AppImage
  ```

---

## 3. 方式二：从源码编译构建 (Source Build)

适合开发者或需要针对特定发行版打包的人员。

### 3.1 编译环境准备

1. **Node.js 与 包管理器**：
   * Node.js 18.0+
   * pnpm 8.0+ (`npm install -g pnpm`)
2. **Rust 工具链**：
   * Rust 1.78.0+ (`curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`)
3. **平台构建工具集**：
   * **macOS**：Xcode Command Line Tools (`xcode-select --install`)
   * **Windows**：Visual Studio 2022 C++ 生成工具 (Desktop development with C++)
   * **Linux**：
     ```bash
     sudo apt install -y build-essential curl wget libssl-dev libgtk-3-dev libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev
     ```

---

### 3.2 源码克隆与编译步骤

```bash
# 1. 克隆代码仓库
git clone https://github.com/unidrop/unidrop.git
cd unidrop

# 2. 进入客户端目录
cd client

# 3. 安装前端依赖
pnpm install

# 4. 本地启动开发环境调试 (含热重载)
pnpm tauri dev

# 5. 打包正式发行版本 (生产环境优化产物)
pnpm tauri build
```

---

### 3.3 构建产物路径对照

打包完成后，二进制与安装镜像将输出在 `client/src-tauri/target/release/bundle/` 目录：

* **macOS**：
  * `bundle/dmg/UniDrop_0.1.0_aarch64.dmg`（安装镜像）
  * `bundle/macos/UniDrop.app`（独立应用程序包）
* **Windows**：
  * `bundle/msi/UniDrop_0.1.0_x64_en-US.msi`（Windows 安装程序）
  * `bundle/nsis/UniDrop_0.1.0_x64-setup.exe`
* **Linux**：
  * `bundle/deb/unidrop_0.1.0_amd64.deb`
  * `bundle/appimage/unidrop_0.1.0_amd64.AppImage`

---

## 4. 首次运行与验证

1. 启动 UniDrop 后，观察桌面任务栏或顶部菜单栏中是否出现 **UniDrop 剪贴板图标**。

   > **macOS 26 (Tahoe) 例外**：如果菜单栏里没有图标，但 Dock 里有图标、应用也确实在运行，**这不是安装失败**。macOS 26 新增了菜单栏准入控制，到「系统设置 → 菜单栏 → 应用程序」把 UniDrop 的开关打开即可；若它已经是开着的，多半是**你用来启动 UniDrop 的那个应用**（终端、代码编辑器等）的开关关着，把那一条也打开就会立刻恢复。应用会在启动几秒后自动打开主窗口并在顶部说明如何处理，完整步骤见 [用户使用手册 2.2 节](./USER_GUIDE.md#22-macos-26-tahoe菜单栏图标不显示的恢复办法)。

2. 左键点击托盘图标，即可呼出或隐藏主控面板；
3. 初次启动客户端会自动在本地生成固定的硬件身份指纹（UUID）并持久化存入 SQLite，面板下方将显示：
   * 本机设备名及系统架构（如：`本机: MacBook-Pro (MACOS)`）；
   * 连接状态指示器。
4. 接下来请参阅 [用户使用手册 (USER_GUIDE.md)](./USER_GUIDE.md) 完成网络中继服务器和 PSK 密钥的配置。
