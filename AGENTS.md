# DeepSeek Harness Desktop Client — 开发与协作规范 (AGENTS.md)

本文件是本项目（DeepSeek Harness Desktop Client）的核心架构规范、设计原则与代码规范指南，供开发人员及 AI 编程助手（Antigravity / Gemini CLI / Cursor / Claude 等）在阅读、修改和扩展本项目时严格遵守。

---

## 1. 项目定位与核心原则

1. **零侵入官方源码原则（Zero-Modification Invariant）**：
   - 本项目是为 [DeepSeek Harness (dsh)](https://github.com/deepseek-ai/deepseek-harness) 打造的原生桌面客户端外壳（Tauri 2）。
   - **严禁修改任何官方 `@deepseek-ai/dsh` 源码或前端产物**。
   - 所有桌面端特化能力（如 WKWebView 剪贴板修复）一律通过运行时脚本注入（`CLIPBOARD_BRIDGE_SCRIPT`）实现；
   - 官方 dsh 运行时以 npm 原包分发并随应用打包在 `src-tauri/resources/runtime`，在线更新安装至用户目录 `~/.dsh/desktop-runtime`。

2. **最小权限原则（Principle of Least Privilege）**：
   - 远程 dsh Web 界面（`http://127.0.0.1:3080/*`）仅授予 `clipboard-manager:allow-write-text`（剪贴板写入），绝不授予读取权限（`read-text`），隔离 XSS 攻击面；
   - 本地控制台页面（`ui/`）的所有自定义 IPC 命令必须在 `src-tauri/permissions/app.toml` 中显式声明，并在 `src-tauri/capabilities/default.json` 中明确授权；
   - 严禁对远程页面开放任何未受 ACL 管控的内部命令。

3. **单一权威与无悬挂状态（Single Source of Truth & No Dead States）**：
   - 窗口导航与服务状态完全由 Rust 后端掌控；
   - 应用启动默认停留在本地控制台页面，用户手动点击「进入界面」后由后端权威导航；服务离开运行态时自动切回控制台，杜绝网络连接拒绝死页。

---

## 2. 架构概览与技术栈

```
┌─────────────────────────────────────────────────────────────┐
│  DeepSeek Harness Desktop Client (Tauri 2 / Rust 后端)      │
│  ├─ 控制台 UI (Vanilla HTML/CSS/JS): 服务生命周期/日志/设置  │
│  └─ 主窗口 WebView: 官方 dsh Web UI (127.0.0.1:3080)        │
├─────────────────────────────────────────────────────────────┤
│  官方 dsh 运行时 (Node.js 进程组)                           │
│  ├─ 内置只读运行时: src-tauri/resources/runtime              │
│  └─ 在线更新运行时: ~/.dsh/desktop-runtime (优先加载)       │
└─────────────────────────────────────────────────────────────┘
```

- **后端**：Rust (Tauri 2.11+, Wry, Tao, serde, serde_yaml, serde_json)；
- **控制台前端**：原生 Vanilla HTML5 + CSS + Modern ES6 JavaScript（无多余框架依赖，极致轻量与快速启动）；
- **官方运行时**：Node.js LTS (随包内置独立二进制) + `@deepseek-ai/dsh` 官方发布包。

---

## 3. 服务生命周期与进程安全规范

### 3.1 进程所有权与强杀策略
- `DshServer` 内部互斥锁 `inner.child` 为子进程的单一所有者；
- **停止服务 / 退出应用时必须执行整棵进程树的彻底终止**：
  - Unix：向子进程组发送 `SIGTERM` 与 `SIGKILL`（`kill -15 -<pid>` / `kill -9 -<pid>`）；
  - Windows：使用 `taskkill /F /T /PID <pid>`；
  - 强制调用 `kill_port_occupant(port)` 释放 TCP 端口，确保关闭应用后绝无后台孤儿进程残留或端口占用。

### 3.2 锁与日志防死锁规范
- `std::sync::Mutex` 不可重入：**严禁在持有 `inner` 状态锁的作用域内调用 `self.log()`**；
- 必须遵循「先准备日志内容/释放锁，后写日志」的模式。

### 3.3 看门狗（Watchdog）与状态转移
- 服务异常崩溃后进入退避等待（`CRASH_BACKOFF`），连续崩溃上限为 `MAX_CONSECUTIVE_CRASHES` (6 次)；
- 稳定运行超过 `STABLE_WINDOW` (30 秒) 后重置崩溃计数器；
- 在自动重启前必须将状态置为 `ServerState::Stopped`，确保不会被 `start()` 的状态保护拦截。

---

## 4. 端口与网络探测规范

1. **标准端口**：默认锁定为 `3080`；
2. **HTTP 探测协议（`probe_port`）**：
   - 必须发送标准的 HTTP/1.1 请求（含 `Host: 127.0.0.1:<port>`、`User-Agent` 与 `Connection: close`）；
   - 必须**循环完整读取响应体**（不得依赖单次 socket read），检测包含 `<title>DeepSeek Harness</title>` 或 `DeepSeek Harness` 特征；
   - 严禁在遇到端口占用时擅自递增端口（如 3081+）或擅自改写用户配置文件，避免引起多实例数据并发冲突和会话失效；
3. **冲突处理**：
   - 若端口被占用，准确获取占用 PID（`find_port_pid`）；
   - 在控制台提供明确的「终止占用进程并重新拉起」与「接管现有服务」操作。

---

## 5. 文件与数据持久化规范

1. **用户数据目录**：
   - 统一使用官方 `$DSH_HOME`（默认 `~/.dsh`），保持与官方 CLI 的 100% 互通兼容；
   - 数据目录权限在 Unix 下必须收紧为 `0700`，配置文件权限为 `0600`。
2. **原子写入（Atomic Write）**：
   - 所有写入 `settings.yaml` 或 `desktop-settings.json` 的操作必须遵循「写入 `.tmp` 临时文件 + `std::fs::rename` 覆盖」模式，防止写入中断导致文件损坏。

---

## 6. 构建与依赖管理规范

1. **环境准备与锁文件**：
   - 构建前必须执行 `npm run setup`（包含 `fetch:node` 与 `setup:runtime`）；
   - `src-tauri/resources/runtime/package-lock.json` 必须提交到 Git 仓库，构建脚本检测到锁文件时必须使用 `npm ci` 进行确定性构建与 sha512 完整性校验；
   - `scripts/fetch-node.mjs` 下载 Node 二进制时必须校验官方 `SHASUMS256.txt` 校验和。
2. **编译与测试检查**：
   - 提交代码前必须在 `src-tauri` 运行并通过：
     ```bash
     cargo check
     cargo test
     ```
   - 保证 0 编译错误、0 警告。
