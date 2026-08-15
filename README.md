# DeepSeek Harness Desktop Client

一款为 [DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness) 打造的现代桌面客户端（Tauri 2 原创外壳）。

> **原则声明**：本客户端**绝不改动官方 DeepSeek Harness 的任何源码**。官方 `@deepseek-ai/dsh` 运行时以 npm 原包形式随应用分发与在线更新；剪贴板桥等适配全部通过运行时注入实现。

---

## ✨ 特色

| 特性 | 说明 |
|---|---|
| 🖱️ **复制按钮真正可用（macOS）** | 剪贴板桥为**运行时注入**（优先 Tauri 原生剪贴板 → `navigator.clipboard` → `execCommand` 兜底），修复 WKWebView 下复制假成功问题，**不修改官方前端产物** |
| 🔄 **官方版本检测 + 在线更新** | 控制台实时检测 npm 上官方 `@deepseek-ai/dsh` 最新版，一键在线更新到用户级 runtime（完整性 sha512 校验），自动重启生效；应用自身更新检查 GitHub Releases |
| 🀄 **官方界面汉化** | 控制台「界面语言」开关写入官方 `locale.preference`（官方原生内置中文，含 web UI 与右键菜单文案）；并内置官方 CLI 命令的中文参考 |
| ⌨️ **快捷键完整可用（macOS）** | 标准「编辑」菜单（Cmd+C / Cmd+V / Cmd+X / Cmd+A），不再依赖系统默认菜单 |
| 🎛️ **品牌化控制台** | 原创启动/状态页：服务状态、托管标识、实时日志流、一键 启动/重启/停止/在浏览器打开 |
| 🖥️ **托盘驻留** | 关窗不退出，托盘一键 显示窗口 / 打开浏览器 / 重启服务 / 退出 |
| 🛟 **可靠看门狗** | 服务异常退出自动拉起（带退避与崩溃上限）；停止/退出必定终止服务进程，不残留孤儿 |
| 🔌 **多开支持** | `DSH_DESKTOP_PORT` 自定义端口（能力按端口动态授权），可并行运行多个实例 |
| 🌅 **开机自启** | 控制台一键开关（macOS LaunchAgent / Windows 注册表） |
| 📐 **窗口记忆** | 记住窗口位置与大小 |
| 🛡️ **最小权限设计** | 远程 dsh 页面仅授予「剪贴板写入」一项 IPC 能力；端口抢占防护（attach 身份校验）；数据目录权限收紧 |

## 🚀 快速开始

### 直接使用（发布版）
从 [Releases](https://github.com/Cnkore007/dsh-Desktop-Client/releases) 下载对应平台安装包。

### 从源码构建

前置：Node.js ≥ 22、Rust 工具链（[rustup](https://rustup.rs)）、macOS 需 Xcode Command Line Tools。

```bash
npm install              # 安装 @tauri-apps/cli 等开发依赖
npm run setup            # 下载 Node 运行时（SHA256 校验）+ 安装官方 @deepseek-ai/dsh
npm run tauri dev        # 开发模式
npm run bundle           # 构建发布版（产物在 src-tauri/target/release/bundle/）
```

> `npm run setup` = `fetch:node`（下载官方 Node 二进制，含 SHASUMS256 校验）+ `setup:runtime`（安装官方 `@deepseek-ai/dsh`，锁文件已提交，`npm ci` 可复现构建）。
> 官方 dsh 更新请直接在控制台「更新官方 dsh」一键完成，无需重新打包。

## 🏗️ 架构

```
┌─────────────────────────────────────┐
│  DeepSeek Harness Desktop Client     │ 原创 Rust 代码：窗口/菜单/托盘/服务管理
│  ├─ 控制台 (本地页面)                 │  服务状态、官方版本检测与更新、日志、设置
│  └─ WebView → 127.0.0.1:3080        │  官方 @deepseek-ai/dsh web UI
├─────────────────────────────────────┤
│  dsh web (官方运行时)                 │  node …/@deepseek-ai/dsh/lib/bin.js web
│  ├─ bundle 内置 runtime（只读）       │  src-tauri/resources/runtime
│  └─ 用户级 runtime（在线更新目标）      │  ~/.dsh/desktop-runtime（DSH_HOME 可覆盖）
└─────────────────────────────────────┘
```

- 外壳通过**内置 Node 运行时**拉起官方 `dsh web` 服务，并管理其生命周期（启动/停止/看门狗/日志）；
- 远程页面仅被授予**剪贴板写入**一项 IPC 权限（capability 白名单，最小权限）；
- 数据目录沿用官方 `$DSH_HOME`（缺省 `~/.dsh`），与官方 CLI 完全兼容；
- 剪贴板桥通过**页面脚本运行时注入**（每 3s 幂等检查），官方前端 bundle 保持原样。

## 🛡️ 安全设计（对抗审计后）

本项目经历过一轮多角度对抗审计（见 `AUDIT.md`），正式版落地了以下加固：

- **生命周期**：修复看门狗持锁死锁、停止/退出无法终止子进程（孤儿残留）、崩溃后自动重启失效、崩溃计数不重置、start 竞态双 spawn、backoff 期间停止被取消；
- **端口抢占防护**：attach 前校验应答是否为官方 dsh 页面（非 dsh 服务占用端口将明确报错）；attach 的外部实例持续健康探测并在 UI 标注「外部进程」；
- **最小权限**：远程页删除剪贴板**读**权限（官方 UI 实测不使用），`local:false` 隔离本地页；控制台命令改为显式 ACL 权限；
- **供应链**：Node 下载校验 SHASUMS256、运行时锁文件提交（`npm ci` 可复现）、CI actions 钉死 SHA 与最小权限、发布物附 SHA256SUMS、依赖自动更新（Dependabot）；
- **导航**：应用打开后停留在控制台，进入官方界面需用户手动点击「进入界面」（导航由后端单一权威控制）；服务离开运行态时自动回到控制台，不残留连接拒绝死页。

已知限制：macOS 签名/公证与 Windows 代码签名需开发者证书，暂未配置（见「发布」节）。

## 🗺️ 路线图

- [ ] 会话/工作区快捷切换（原生命令面板）
- [ ] 托盘图标显示服务状态
- [ ] Windows/Linux 构建矩阵完善
- [ ] 自动更新（签名版）

## 📦 发布

1. 打 tag（`v*`）触发 [GitHub Actions](.github/workflows/release.yml)，产出 macOS（app/dmg）与 Windows（NSIS）安装包 + `SHA256SUMS.txt`；
2. 产物当前未签名/未公证：macOS 首次打开需右键 → 打开；Windows 会触发 SmartScreen 提示；
3. 配置 Apple Developer ID（`APPLE_CERTIFICATE` / `APPLE_ID` 等 secret）与 Windows 代码签名证书后，可启用 `tauri.conf.json` 的 `bundle.macOS.signingIdentity` / `bundle.windows.certificateThumbprint` 并接入公证。

## 🔒 数据与隐私

- 数据目录：`$DSH_HOME`（缺省 `~/.dsh`），与官方 CLI 完全兼容；
- 凭证/会话文件权限沿用官方策略（0600/0700），数据目录权限已收紧为 0700；
- 更新检查仅向 npm registry 与 GitHub Releases 发起只读请求。

## 📄 License

MIT
