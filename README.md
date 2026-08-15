# DeepSeek Harness Desktop Client

<p align="center">
  <img src="src-tauri/icons/128x128@2x.png" width="100" height="100" alt="DeepSeek Harness Desktop Client Logo" />
</p>

<p align="center">
  <strong>一款为 <a href="https://github.com/deepseek-ai/deepseek-harness">DeepSeek Harness (dsh)</a> 打造的高性能现代化桌面客户端</strong><br>
  基于 Tauri 2 原生外壳 · 零侵入官方源码 · 运行时动态汉化 · 社区生态友好
</p>

<p align="center">
  <a href="https://github.com/Cnkore007/dsh-Desktop-Client/releases"><img src="https://img.shields.io/github/v/release/Cnkore007/dsh-Desktop-Client?color=blue&label=Release" alt="Release Version"></a>
  <a href="https://github.com/topics/dsh-plugin"><img src="https://img.shields.io/badge/Topic-dsh--plugin-0969da?logo=github" alt="dsh-plugin Topic"></a>
  <a href="https://github.com/deepseek-ai/deepseek-harness"><img src="https://img.shields.io/badge/Official%20dsh-0.1.0--rc.6-brightgreen" alt="Official dsh"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-MIT-green" alt="MIT License"></a>
</p>

---

## 📸 界面预览 (Screenshots)

<p align="center">
  <img src="assets/preview-commands.png" width="48%" alt="命令与权限中文注释预览" />
  <img src="assets/preview-plugins.png" width="48%" alt="159+ 插件列表全量中文功能注释预览" />
</p>

---

## ✨ 核心特色功能 (Key Features)

### 1. 🇨🇳 运行时全量中文注释与汉化引擎
- **零侵入官方源码**：通过客户端 WebView 运行时注入，绝不改动官方 `@deepseek-ai/dsh` 任何发布包与前端代码；
- **全量命令与权限注释**：输入 `/` 或点击「命令」按钮，所有命令（`compact`、`export`、`goal`、`plan`、`feedback` 等）与权限模式（`Full access`、`Sandbox`、`Ask before changes` 等）均拥有准确优雅的中文功能注释；
- **159+ 插件与生态看板**：在「设置」→「插件」→「插件列表」中，为官方全量 159 个 Cordis / dsh 模块提供精致的双行卡片排版与详细中文职责说明，且自适应未来任意新增的第三方插件！

### 2. 🔗 对话超链接与代码块 URL 原生浏览器直达
- 对话气泡、Markdown 引用及代码块（如 `http://127.0.0.1:3080`、`http://localhost:*`、`https://...`）自动增强交互高亮与手型指针；
- 点击任意链接均通过原生 IPC 直接调起操作系统默认浏览器（Chrome / Safari / Edge 等），杜绝内部容器跳转阻塞。

### 3. 📂 自由工作区目录选择（CWD 进程绑定）
- 支持通过系统原生文件夹对话框自由指定代码工程作为 Agent 的工作区（CWD）；
- 启动服务时直接注入，使 AI 智能体的终端命令与文件读写（`tool-fs`、`tool-bash`）精准作用于目标项目；支持一键在系统访达/资源管理器中打开与重置。

### 4. 💾 会话历史统计与一键安全备份/清理
- 实时统计 `~/.dsh/sessions` 下的会话数量与磁盘占用（如 `5 个会话 · 1.2 MB`）；
- 支持一键打包压缩为 `.tar.gz` 备份并定位到系统文件夹；清理历史缓存前自动创建快照，安全防丢失。

### 5. 🔄 官方版本双通道检测与在线更新
- 实时检测 npm 官方源与 [GitHub 官方仓库](https://github.com/deepseek-ai/deepseek-harness) 的最新发布版本；
- 有新版时支持一键在线安全下载更新到用户级 runtime（sha512 完整性校验），自动重启生效。

### 6. 🛟 严格生命周期管理与干净退出
- **完全释放端口**：关闭窗口或退出应用时，采用 Unix 进程组（`SIGTERM/SIGKILL`）与 Windows 进程树彻底终止后台 `node` / `dsh` 服务，绝不残留孤儿进程或占用 `3080` 端口；
- **原生剪贴板修复**：通过运行时桥接彻底解决 macOS WKWebView 容器中代码块复制按钮假成功问题。

---

## 🤝 加入社区与生态合作 (Community & Ecosystem)

本项目旨在为 **DeepSeek Harness (dsh)** 开发者与用户提供最佳的桌面端体验，拥抱开源社区协同：

1. **官方架构与生态协同**：
   - 官方核心项目：[deepseek-ai/deepseek-harness](https://github.com/deepseek-ai/deepseek-harness)
   - 严格遵循零侵入官方源码原则，与官方 CLI 及数据配置（`$DSH_HOME`）保持 100% 互通兼容。
2. **GitHub [`dsh-plugin`](https://github.com/topics/dsh-plugin) 生态探索**：
   - 本项目已加入 GitHub [`dsh-plugin`](https://github.com/topics/dsh-plugin) 社区生态主题；
   - 欢迎社区开发者接入外部 MCP (Model Context Protocol) 工具与自定义 Cordis 插件扩展。
3. **扩展与贡献**：
   - 欢迎提交 Issue 反馈问题、优化建议或提交 Pull Request。

---

## 🚀 快速开始 (Quick Start)

### 1. 直接下载安装（推荐）

前往 [Releases 发布页面](https://github.com/Cnkore007/dsh-Desktop-Client/releases) 下载最新安装包：
- **macOS**：`.dmg` / `.app`（支持 Apple Silicon 与 Intel 架构）
- **Windows**：`.exe` (NSIS 安装程序)
- **Linux**：`.deb` / `.AppImage`

### 2. 源码构建与开发

#### 前置环境
- **Node.js** ≥ 22
- **Rust 工具链**（[rustup](https://rustup.rs) 最新 stable）

#### 常用命令
```bash
# 1. 克隆仓库并安装依赖
git clone https://github.com/Cnkore007/dsh-Desktop-Client.git
cd dsh-Desktop-Client
npm install

# 2. 初始化运行环境（下载 Node.js 二进制 + 官方 @deepseek-ai/dsh 原包并校验）
npm run setup

# 3. 启动本地开发模式
npm run dev

# 4. 执行自动化测试与代码检查
cd src-tauri && cargo test && cargo check

# 5. 构建生产分发包
npm run build
```

---

## 📄 开源许可证 (License)

本项目基于 [MIT License](LICENSE) 开源发布。
DeepSeek Harness 官方版权与商标归 DeepSeek AI 所有。
