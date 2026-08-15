# DSh Desktop

一款为 [DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness) 打造的现代桌面客户端（Tauri 外壳，原创实现）。

> 不是 `xiincs/deepseek-harness-desktop` 的分支或派生：本项目外壳代码完全原创，
> 只依赖 DeepSeek Harness 官方发布的 `@deepseek-ai/dsh` 运行时，并内置了官方版缺失的
> macOS 剪贴板/快捷键修复。

---

## ✨ 特色

| 特性 | 说明 |
|---|---|
| 🖱️ **复制按钮真正可用（macOS）** | 内置原生剪贴板桥接（`tauri-plugin-clipboard-manager`）+ web 端 `execCommand` 兜底，修复 WKWebView 下 `navigator.clipboard.writeText` 假成功问题 |
| ⌨️ **快捷键完整可用（macOS）** | 标准「编辑」菜单（Cmd+C / Cmd+V / Cmd+X / Cmd+A），不再依赖系统默认菜单 |
| 🎛️ **品牌化控制台** | 原创启动/状态页：服务状态卡片、实时日志流、一键 启动/重启/停止/在浏览器打开 |
| 🖥️ **托盘驻留** | 关窗不退出，托盘一键 显示窗口 / 打开浏览器 / 重启服务 / 退出 |
| 🛟 **看门狗** | 服务进程异常退出自动拉起（带防抖与日志），无需手动干预 |
| 🔌 **多开支持** | `DSH_DESKTOP_PORT` 或 `--port` 自定义端口，可并行运行多个实例 |
| 🚀 **自动构建发布** | GitHub Actions 在 tag 时自动产出 macOS（app/dmg）与 Windows（NSIS）安装包 |
| 🔄 **内置更新检查** | 控制台内检查 GitHub Releases 新版本，一键跳转下载 |
| 🌅 **开机自启** | 控制台一键开关（macOS LaunchAgent / Windows 注册表） |
| 📐 **窗口记忆** | 记住窗口位置与大小 |

## 🚀 快速开始

### 直接使用（发布版）
从 [Releases](https://github.com/Cnkore007/dsh-desktop-client/releases) 下载对应平台安装包。

### 从源码构建

前置：Node.js ≥ 22、Rust 工具链（[rustup](https://rustup.rs)）、macOS 需 Xcode Command Line Tools。

```bash
npm install                 # 安装 @tauri-apps/cli 等开发依赖
npm run setup             # 下载 Node 运行时 + 安装 @deepseek-ai/dsh + 应用剪贴板修复补丁
npm run tauri dev           # 开发模式
npm run bundle              # 构建发布版（产物在 src-tauri/target/release/bundle/）
```

> `npm run setup` 中的 `patch:web` 步骤会把 `writeClipboard` 加固补丁应用到
> `dsh-web-frontend` 产物上（优先原生剪贴板 → `writeText` → `execCommand` 兜底），
> 这是 macOS 复制按钮可用的关键之一；若上游 dsh 已内置该修复，补丁会自动跳过。

## 🏗️ 架构

```
┌─────────────────────────────┐
│  DSh Desktop (Tauri 外壳)    │  原创 Rust 代码：窗口/菜单/托盘/服务管理
│  ├─ 控制台 (本地页面)         │  服务状态、日志、启动控制、设置
│  └─ WebView → 127.0.0.1:3080│  官方 @deepseek-ai/dsh web UI
├─────────────────────────────┤
│  dsh web (官方运行时)         │  node …/@deepseek-ai/dsh/bin.js web
└─────────────────────────────┘
```

- 外壳通过**内置 Node 运行时**拉起官方 `dsh web` 服务，并管理其生命周期；
- 远程页面仅被授予**剪贴板读写**一项 IPC 权限（capability 白名单，最小权限）；
- 数据目录沿用 `~/.dsh`，与官方 CLI 完全兼容。

## 🗺️ 路线图

- [ ] 会话/工作区快捷切换（原生命令面板）
- [ ] 托盘图标显示服务状态
- [ ] Windows/Linux 构建矩阵完善
- [ ] 自动更新（签名版）

## 📄 License

MIT
