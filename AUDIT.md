# DSh Desktop 对抗审计报告

> **更新（正式版 v1.0.0）**：本报告对应 `v0.1.0` 代码基线。正式版开发（DeepSeek Harness Desktop Client v1.0.0）已按「四、修复优先级路线图」落地 P0+P1 全部修复，见文末「六、正式版修复记录」。本报告保留原始发现供追溯。

- 审计日期：2026-08-15
- 审计对象：`dsh-desktop-client`（Tauri 2 桌面壳，原创代码约 1,500 行）
- 审计方法：6 个独立对抗审计面（Rust 后端 / Tauri 配置 / 前端 / 供应链与 CI / 威胁建模 / 文档一致性）+ 2 个补位审计（威胁建模剩余场景、文档-实现一致性）+ 交叉核验。**每条关键结论均对照源码、生成产物或实际执行复现**（标注 ✅=已实测/已对照生成产物核验）。
- 版本基线：tauri 2.11.5 / wry 0.55.1 / clipboard-manager 2.3.2 / `@deepseek-ai/dsh@0.1.0-rc.6`（锁定 ✅）

> 严重性定义：**Critical** = 可利用导致 RCE / 敏感数据泄露 / 权限提升；**High** = 有条件但现实可利用；**Medium** = 条件苛刻或影响有限；**Low** = 加固建议；**Info** = 信息。
> 威胁模型：本地恶意进程（可绑定 127.0.0.1 任意端口、可读写文件，**跨用户**可达 loopback）、远程 dsh UI 中的 XSS（该 UI 渲染 LLM 输出，属真实攻击面）、恶意网页、供应链投毒、竞态崩溃。
> 编号说明：原始发现 61 条 + 补位 20 条，经同链合并/去重后编号 F/H/M/L；括号内为原始编号。

---

## 一、结论摘要

本项目**安全边界设计有亮点**：远程页能力被裁剪到剪贴板、Tauri ACL 正确阻断远程页调用自定义命令（✅ 已核验生成产物 acl-manifests.json 无 app 权限键）、前端无 XSS 注入路径（✅ 全部 textContent 渲染）。但存在六组问题：

1. **官方 dsh 的 loopback API 完全无认证**（"this fence is not an auth layer"——官方源码自述），桌面壳把它随登录暴露：任意本地进程（含**其他用户**的进程）可全权驱动 harness——读取全部会话、提交消耗额度的任务、改配置、实时 WebSocket 窃听——**TM-01/TM-03**；
2. **服务生命周期致命缺陷（发布阻断级）**：看门狗持锁二次加锁导致**每次"停止服务"都死锁整个应用**（H1）；子进程句柄被看门狗独占导致 stop/退出**永远杀不掉 dsh 进程**（孤儿残留，H2/DC-01/TM-02）；崩溃后看门狗自设 Starting 状态挡住自己的重启（**自动拉起完全失效**，H3）；
3. **端口抢占即可劫持主窗口**（端到端链）：probe 只认"HTTP 200 应答"即 attach，导航 watcher 把 webview 导向伪服务页面，该页面继承剪贴板读写能力——威胁模型内确定性可利用——**F1**；
4. **剪贴板读权限是死权限却已授出**：前端 bundle 实测 0 处 readText 调用（✅），而 `allow-read-text` 同时授予远程页与本地控制台页（✅ 生成 capabilities.json 显示 dsh-page `local:true`）——**H5/TAURI-05**，删除零成本；
5. **发布流水线确定性失败**：CI 调用不存在的 `npm run prepare`（✅ 实测退出码 1，H7）；macOS 产物路径静默失效（M9）；
6. **供应链加固缺失**：Node 二进制下载无 SHA256 校验（H10）、运行时依赖树锁文件被 gitignore（588 包浮动解析，✅ M7）、产物无签名/公证/校验和（H9）、CI actions 未钉死 SHA 且 build job 持 `contents: write`（H8）。

---

## 二、发现清单

### Critical（2）

#### F1（rust-04 / TAURI-02 / DC-03 / TM-04）· 端口抢占/伪造服务 → 主窗口劫持 → 剪贴板读写（端到端链）✅
- **位置**：`src-tauri/src/server.rs:150-160, 213-222`；`src-tauri/src/lib.rs:196-211`；`src-tauri/capabilities/dsh-page.json:6-12`
- **描述**：任何本地进程（无特权，3080 端口无需管理员）可在 app 启动前、或 dsh 崩溃/重启的空窗内绑定 127.0.0.1:3080 并对 `GET /` 返回 `HTTP/1.1 200`。`probe()` 只读 64 字节判断 200 前缀（✅ server.rs:159），`start()` 即走 attach 分支（state=Running、pid=None、不 spawn 真 dsh）；导航 watcher（800ms 轮询）与前端 `scheduleNav`（1.5s）随后把 webview 导航到伪服务页面；该页面 origin 命中 `http://127.0.0.1:*/*` 通配（✅ 生成 capabilities.json 确认），获得 `clipboard-manager` 读写能力；`read_text`/`write_text` 命令无任何手势/焦点校验（✅ 已读插件 2.3.2 源码 commands.rs）。
- **影响**：跨用户剪贴板窃取（密码/API key 外传）、**粘贴劫持**（writeText 注入恶意命令，用户粘贴到终端即执行）、应用窗口内全页钓鱼（伪造 dsh UI 骗取 DeepSeek key）。开机自启场景下攻击者先占位即可每次登录自动复现。附带：spawn 路径中 probe 在 spawn 前执行，恶意进程可在 probe 后、spawn 前抢占端口 → 新 node EADDRINUSE 退出 → 崩溃路径 → 叠加 H3 永久卡 Starting（可用性 DoS 变体）。
- **现有缓解（部分有效）**：Tauri ACL 对远程 origin 强制权限检查，伪页面无法调用 start/stop/open_data_dir 等自定义命令（✅）；官方 dsh 拒绝 0.0.0.0 绑定（✅ startup.js:39）。
- **修复**：attach 前做身份握手（dsh 实例生成一次性随机 token 写入 `~/.dsh/.server-token`（0600），probe 要求携带 token）；或 app 先独占绑定端口、验证端口归属；对 attach 到的非自 spawn 服务显示警告并禁止自动导航；capability 收窄到 `http://127.0.0.1:3080/*`。

#### F2（TM-01 / TM-03）· loopback dsh API 完全无认证——任意本地进程（含跨用户）可全权驱动 harness ✅（上游设计缺陷，桌面壳暴露面）
- **位置**：`src-tauri/src/server.rs:233-235`（spawn 仅 `web --port N`，无 token）；官方包 `@deepseek-ai/dsh-client-connection/lib/index.js:184-198, 504-520, 530-562`
- **描述**：dsh web 的 `/api/*` RPC 与 `/api/events.mux`、`/api/events.host` WebSocket 下行流的唯一护栏是 `isTrustedApiRequest`——只检查 Host 是 loopback/LAN + Origin 同源 + sec-fetch-site；官方源码注释自述 **"this fence is not an auth layer"**（✅ 已 grep 原文）。桌面端 spawn 不传任何密钥，命令行也无鉴权选项（✅ startup.js:22 仅 --host/--port/--trusted-host）。loopback 按网络命名空间共享、**不按用户隔离**：同机其他用户进程、被入侵的守护进程均可直连。
- **影响**：任意本地进程可：读取全部会话/对话历史（session.list/history/search）、提交 agent 任务消耗用户 API 额度（session.prompt）、在 workspace 内执行 bash、篡改配置（改 sandbox 策略、把 telemetry exporter 指向攻击者）、探测/写入凭证、`host.openPath` 触发文件管理器；WebSocket 下行流可**实时旁路窃听**全部 agent 活动与审批流。与 H2（退出后服务残留）组合成"退出后仍在线的持久无认证后台服务"。
- **修复**：桌面端与 dsh 之间引入 per-instance 共享密钥（spawn 环境变量注入，dsh 侧对全部 /api 与 upgrade 校验）；或改用仅当前用户可访问的 UNIX domain socket 承载 API；至少由客户端对 /api 做带 token 头的拦截；修复 H2 杜绝残留实例。

### High（12）

#### H1（rust-01）· watchdog 持锁二次加锁——每次"停止服务"都死锁整个后端 ✅
- **位置**：`src-tauri/src/server.rs:309-316, 325-330`（`server.log()` 在 `let mut inner = ...lock()` 守卫存活期内被调用；`log()` 内部再次 `self.inner.lock()`）
- **验证**：std Mutex 不可重入。stop() 置 `stop_requested` → watchdog 400ms 内轮询到（300 行 break None）→ 309 行分支加锁后 314 行调 `server.log()` → 同线程二次加锁**永久死锁**。第 6 次崩溃路径（328 行）同理。这是"停止运行中服务"的**常规路径**。
- **影响**：点击"停止"→ watchdog 死锁并永久持有 Mutex → status/logs/start/stop 与两个日志捕获线程全部阻塞，控制台冻结；菜单退出同样阻塞，只能强杀。
- **修复**：log() 移出锁守卫作用域（先记本地变量、解锁后写日志）或日志路径 try_lock；watchdog 状态机禁止持锁期间再入调用。

#### H2（rust-02 / DC-01 / TM-02）· stop()/退出/重启永远杀不掉子进程——dsh 服务成为孤儿持续运行 ✅
- **位置**：`src-tauri/src/server.rs:289-293, 345-365`；`src-tauri/src/lib.rs:60-68, 154-161, 175-178`
- **验证**：watchdog 启动即 `inner.child.take()`（290 行）独占 Child 句柄；此后 `stop()` 的 take（352 行）稳态下恒为 None，kill/wait 不可达；watchdog 的 stop_requested 分支只改状态不 kill 就 drop Child。quit 路径（lib.rs:175-178）同样无法终止；"重启服务"实际变成"attach 到旧进程"（pid=None）并**丢掉看门狗**；每次重启还泄漏两个日志读取线程（旧管道永不关闭）。
- **影响**："停止服务"名存实亡且状态撒谎；退出应用后 dsh 持续运行（无认证 API + 凭证句柄在线，见 F2）并占用 3080；下次启动经 probe attach 到旧孤儿；反复启动/退出累积孤儿进程。
- **修复**：stop() 与 watchdog 的 stop 分支都须 kill+wait（进程组 SIGTERM→kill）；保留独立于 watchdog 的 Child 句柄；MENU_QUIT 先确认子进程退出再 exit；restart 等待端口释放后再 start。

#### H3（rust-03）· watchdog 自动重启是空操作——崩溃后状态永久卡 Starting ✅
- **位置**：`src-tauri/src/server.rs:319-341, 196-201`
- **验证**：崩溃分支先把 state 置 Starting（323 行），backoff 后调 `start()`（340 行），而 `start()` 对 Starting/Running 直接 return Ok（198 行）——看门狗自己挡住了自己的重启。此后无任何路径离开 Starting，服务第一次意外崩溃后**再也不重启**，UI 永远显示"starting"；手动点"启动"也被挡；6 次崩溃上限自然不可达。
- **影响**：README 承诺的"看门狗自动拉起"完全失效；一次崩溃 = 服务永久宕机且状态撒谎；配合端口抢占形成持久性可用性 DoS。
- **修复**：崩溃重启路径不经过 start() 的 Starting 守卫（抽出独立 spawn_inner），或重启前先重置状态为 Stopped。

#### H4（rust-12 / DC-02）· attach 模式（pid=None）无健康监测——外部/残留服务死亡后状态永久卡 running
- **位置**：`src-tauri/src/server.rs:214-222`；`src-tauri/src/lib.rs:185-215`
- **描述**：attach 分支不创建 watchdog 也不做周期探测。由于 H2 使 attach 成为**常态路径**（每次重启/启动都 attach 到旧孤儿），外部进程死亡/挂死后：state 永远 Running、webview 停在死端口页面、start() 守卫吞掉重复启动、控制台 refresh() 还会把页面导航到死 URL——用户只能手动 stop+start 恢复，且"看门狗自动拉起"对最常见场景完全失效。
- **修复**：attach 模式启动周期端口健康检查（probe 失败即回落 Stopped/Error 并导航回控制台）；或 attach 时识别对端 pid 纳入监控。

#### H5（TAURI-01）· 远程页被授予 clipboard read-text 死权限——XSS 一旦成立即成剪贴板窃取链 ✅
- **位置**：`src-tauri/capabilities/dsh-page.json:11`
- **验证**：打包的 dsh-web-frontend bundle（index-Dqw48FrP.js）实测 `writeText`×8、`readText`×0（✅ grep）；`read_text` 插件命令无手势/焦点校验（✅ 插件源码）；远程页出站 fetch 不受 CSP 约束（csp:null）。
- **影响**：dsh UI 渲染 LLM 输出，任何脚本注入（XSS/投毒 bundle）可静默读取剪贴板（密码/API key/2FA 码）并外传——macOS 剪贴板读取无 TCC 弹窗。最小权限设计被"读写"放宽。
- **修复**：删除 `allow-read-text`（实测零功能损失）；未来粘贴桥接改走原生菜单命令；同步修正 README"剪贴板读写"表述。

#### H6（TAURI-02）· remote.urls 通配 127.0.0.1 所有端口（与 F1 同链）✅
- **位置**：`src-tauri/capabilities/dsh-page.json:7`
- **验证**：生成 capabilities.json 确认 urls 为 `http://127.0.0.1:*/*` 端口通配（✅）。webview 内任何 loopback 页面（用户点击指向本地开发服务器/文档页的链接）都继承剪贴板能力。
- **修复**：收窄到确切端口并用 dynamic-acl 适配自定义端口；或移除 DSH_DESKTOP_PORT 覆盖。

#### H7（SC-01）· CI 调用不存在的 npm script——发布流水线确定性失败 ✅
- **位置**：`.github/workflows/release.yml:43`；`package.json:6-16`
- **验证**：**实测 `npm run prepare` 退出码 1**（Missing script）。package.json 只有 setup/setup:runtime/fetch:node/patch:web/bundle。任何 v* tag 推送都会在 build job 失败，Release 永远无法产出。
- **修复**：改为 `npm run setup`（或新增 prepare 别名）；CI 加脚本存在性断言。

#### H8（SC-02）· CI actions 未钉死 SHA + build job 持 contents:write
- **位置**：`.github/workflows/release.yml:8-9, 28, 31, 35, 61, 70, 76`
- **影响**：任一 action 仓库被攻陷/tag 被重写 → 在持有 Release 写权限的 runner 上执行任意代码 → 向 Release 推送投毒安装包 → 用户运行未签名产物即 RCE。
- **修复**：actions 全部改 40 位 commit SHA（配合 Dependabot）；permissions 改 per-job 最小化（build 仅 contents:read）。

#### H9（SC-03）· 发布产物无签名/公证/校验和
- **位置**：`tauri.conf.json:26-41`；`release.yml:60-64, 75-78`（全仓库 grep 无 signingIdentity/notarization/APPLE_*/SHA256SUMS）
- **影响**：macOS Gatekeeper 拦截/警告、Windows SmartScreen；下载通道被攻陷时用户无完整性锚点；应用内更新检查同样无法校验。
- **修复**：Developer ID 签名 + notarization；Windows 代码签名；发布 SHA256SUMS（+GPG 签名）。

#### H10（SC-04）· Node 二进制下载无 SHASUMS256 校验、无协议钉死
- **位置**：`scripts/fetch-node.mjs:43, 47-55`
- **影响**：curl -fsSL 跟随重定向无 `--proto '=https'`；未比对 nodejs.org 的 SHASUMS256.txt；下载的 node 是随 app 分发的**可信根**，nodejs.org/CDN/构建机 DNS 任一环节被攻陷即向全体用户分发攻击者代码（可读写 ~/.dsh 凭证）。
- **修复**：下载 SHASUMS256.txt（+GPG 验签）比对 sha256；curl 加 `--proto '=https' --proto-redir '=https' --tlsv1.2`。

#### H11（DC-07）· console_url 在 setup 阶段捕获不可靠——"控制台"菜单与崩溃回跳可能静默失效
- **位置**：`src-tauri/src/lib.rs:249-253, 162-168, 203-208`
- **描述**：setup 时 `win.url()` 在 WebView 首次导航提交前可能返回 about:blank 或 Err（被 `if let Ok` 静默吞掉）：前者使菜单"控制台"/崩溃回跳导航到空白页，后者使两者成为无反馈空操作（叠加 H4 后用户停在死页面）；两处 `url.parse().expect("console url")` 在 URL 非法时 panic。
- **修复**：监听 webview 首次导航完成后再捕获 URL，或对 None/about:blank 提供回退；expect 改为记录日志并跳过。

#### H12（TM-03）· WebSocket 下行流无认证——任意本地进程可实时旁路窃听全部 agent 活动
- **位置**：官方包 `@deepseek-ai/dsh-client-connection/lib/index.js:566-584, 456-465`
- **描述**：`/api/events.mux`（全部 agent 流式输出/工具调用帧）与 `/api/events.host`（宿主事件含审批流）的 upgrade 仅过 `isTrustedApiRequest`（Host/Origin 校验，非认证）；本地进程带 `Host: 127.0.0.1:3080` 即可订阅，downlink-only 不阻止监听。
- **影响**：与 F2 同源——用户毫不知情下的持续会话监控信道（即使不驱动任何操作）。
- **修复**：与 F2 一并引入 token/共享密钥校验（含 upgrade 请求）。

### Medium（14）

#### M1（rust-05）· crash_count 永不重置——"连续 6 次崩溃"实为累计 6 次
- `server.rs:20, 324-337`。成功启动/attach/重启均不清零；Error 后手动 start 成功，下一次崩溃立即再次 Error。**修复**：成功运行 N 分钟后或 attach 成功时清零，或改用滑动时间窗计数。

#### M2（rust-06）· start() 防重检查与状态写入分两次加锁（TOCTOU）——并发可双重 spawn
- `server.rs:196-210`。双击"启动"、菜单重启、setup 自动启动（400ms 后）并发时两个线程都通过检查 → 双 spawn；后者 EADDRINUSE 退出被误计崩溃。**修复**：单次加锁完成检查+置状态；EADDRINUSE 不计崩溃。

#### M3（rust-07）· state=Running 过早——冷启动导航到未就绪端口，几乎必现"连接被拒绝"死页
- `server.rs:246-253`；`lib.rs:196-211`。spawn 成功即置 Running，watcher 最快 800ms 内导航，dsh 冷启动通常 >1s；watcher 只在状态变化时导航，就绪后不重试 → 错误页停留，只能手动 Cmd+R。**修复**：导航前轮询 probe 直到成功（带超时）。

#### M4（rust-08）· backoff 期间 stop() 被静默取消；backoff 睡眠持全局 Mutex 1.5s
- `server.rs:331-340`。崩溃 → 用户点停止（state=Stopped）→ watchdog 醒来调 start()（Stopped 不挡）→ **服务在用户要求停止后复活**；333 行持锁睡眠期间所有命令阻塞 1.5s。**修复**：重启前检查 stop_requested；睡眠移出锁作用域。

#### M5（TAURI-03）· withGlobalTauri:true + csp:null——无第二层注入遏制
- `tauri.conf.json:10, 23`。能力模型是唯一边界且是"调用级"而非"注入遏制级"；远程页注入后外传通道（connect-src）完全不受限。**修复**：为本地页配置 CSP；评估 withGlobalTauri:false。

#### M6（TAURI-04）· core:default 对控制台页过度授权；自定义命令可用性依赖"ACL 未注册 app 权限"的脆弱隐含前提 ✅
- `capabilities/default.json:7`。core:default 含 event emit、menu、tray、devtools 等全套；✅ 生成 acl-manifests.json 确认无 `__app__` 键、desktop-schema.json 无任何 allow-* 应用命令条目——自定义命令目前靠"没有 app ACL 就跳过检查"放行，tauri-build 升级后可能静默拒绝全部控制台命令。**修复**：显式定义应用命令权限并裁剪 core:default。

#### M7（SC-05）· 运行时依赖树无已提交锁文件 ✅
- `scripts/prepare-runtime.mjs:15, 22, 27-30`；`.gitignore:4`。✅ 实测本地 runtime/package-lock.json（588 包，367KB）被 gitignore；每次构建重新解析全部传递依赖；rc 预发布版本可被替换重发；npm install 默认执行 588 个包的生命周期脚本；--no-audit 关闭漏洞检测。**修复**：提交锁文件 + `npm ci` + `--ignore-scripts` + CI 恢复 audit。

#### M8（SC-06）· patch-web-bundle.mjs 正则手术第三方 bundle，失败全部静默跳过
- `scripts/patch-web-bundle.mjs:28-34, 38-41, 46-49, 57-60, 64-66`。所有失败路径 exit 0；只补 entries[0]；无 bundle 哈希锚定（上游投毒但 helper 文本匹配时照打补丁放行）。**修复**：失败非零退出；对 pin 版本做 sha256 校验；补丁以 .patch 文件入库审查。

#### M9（SC-07）· macOS 发布产物缺失
- `tauri.conf.json:28-30`；`release.yml:52-58, 66-79`。targets 只有 ["app"] 不产 dmg；dmg 拷贝 `|| true` 是死代码；.app 目录被 upload/download 展平后按碎片上传。**修复**：加 `--bundles dmg` 或显式 zip .app；删除 `|| true` 并加产物断言。

#### M10（DC-04）· README 与实现不一致清单（--port 未实现 / 多开矛盾 / 一键跳转下载缺失 / DSH_HOME）
- **位置**：`README.md:17,19,34,61`；`server.rs:111-116`；`lib.rs:221`；`ui/app.js:143-158`
- 逐条核查（声称 → 实现 → 结论）：
  1. "`DSH_DESKTOP_PORT` 或 `--port` 自定义端口" → 代码只读 env（✅ grep 全仓无 env::args/clap）→ **`--port` 参数未实现**；
  2. "可并行运行多个实例" → 注册了 single-instance 插件，第二实例直接退出 → **与多开矛盾**（DSH_DESKTOP_PORT 在第二实例上完全无效）；
  3. "一键跳转下载" → 按钮只写 title 工具条，点击恒为重新检查，**从不打开 html_url**（app.js:143-158）；
  4. "数据目录 ~/.dsh 与官方 CLI 完全兼容" → 官方 DSH_HOME 优先（dsh-home-paths index.js:73-76），壳硬编码 ~/.dsh → **不兼容**（见 M12）；
  5. "看门狗自动拉起" / "一键重启服务" → 见 H3/H2。
- **修复**：README 删除 --port/多开声称或实现参数解析并移除单实例限制；按钮点击打开 html_url；其余随对应条目修复。

#### M11（DC-05 / TM-08 / F5 前端）· 更新检查逻辑缺陷：字符串相等比较、无超时、无跳转
- **位置**：`ui/app.js:143-167`；`README.md:19`
- (1) `rel.tag_name === 'v'+cur` 纯字符串相等：旧版/rc 标签会被误报"发现新版本"；info 命令失败时 cur=undefined → 恒报"发现新版本"；(2) fetch 无 AbortController 超时：网络挂起时按钮永久"检查中…"且 disabled；(3) 匿名 API 限流 60 次/h，超限一律"检查失败"；(4) 无签名/哈希校验，仅展示层（✅ textContent 无 XSS）；(5) 实测仓库尚无 Release → 当前恒 404"检查失败"（发布前属预期）。
- **修复**：semver 比较；空 cur 显示"版本未知"；加超时与限流处理；点击新版本按钮调用 open_browser。

#### M12（DC-06 / TM-05 部分）· 数据目录硬编码 ~/.dsh，忽略官方 DSH_HOME 覆盖
- **位置**：`src-tauri/src/lib.rs:87, 105`；官方包 `dsh-home-paths/lib/index.js:73-76`
- 官方解析优先级：显式配置 > `$DSH_HOME` > `~/.dsh`；壳的 info/open_data_dir 硬编码 home_dir()/.dsh（且 home_dir() 为 deprecated，None 时退化为相对路径 ".dsh"）。用户设置 DSH_HOME 时：服务读写 $DSH_HOME，控制台却显示/打开 ~/.dsh 并在错误位置 create_dir_all 空目录。
- **修复**：按 DSH_HOME → ~/.dsh 顺序解析；弃用 home_dir()。

#### M13（TM-05）· ~/.dsh 目录权限 0755——其他本地用户可枚举凭证/会话清单
- **位置**：`src-tauri/src/lib.rs:106`（create_dir_all 默认 umask 0755）；官方 dsh 凭证文件强制 0600、会话目录 0700（缓解内容泄露）
- ✅ 实测 ~/.dsh 为 drwxr-xr-x（0755），内含 .credentials.yaml(0600)、settings.yaml(0600)、sessions/(0700)、**.anonymous-user-id(0644)**。共享机器上其他用户可列出目录结构/文件名（内容因 0600/0700 不可读，属信息泄露降级）。
- **修复**：创建后 chmod 0700；.anonymous-user-id 改 0600。

#### M14（DC-03 合并见 F1 / 此处为 attach 的 stop 空操作面）· 被劫持/attach 后"停止服务"无效
- `server.rs:345-365`。attach 模式 pid=None 无子进程可杀，stop 对伪监听器完全无效——被 F1 劫持后无法通过 UI 终止攻击者监听器。**修复**：同 F1；UI 对 unmanaged 服务明确提示。

### Low（22）

| 编号（原始） | 标题 | 位置 |
|---|---|---|
| rust-09 | 日志环形缓冲只限条数不限字节，LLM 无换行流可撑爆内存 | server.rs:128-141, 262-278 |
| rust-10 | 所有锁操作 unwrap——Mutex 一旦毒化连锁 panic（H1 修复后此风险消失） | server.rs 多处；lib.rs:34 |
| rust-11 | 只杀直接子进程不杀进程组/树（推测性：依赖 dsh 是否 spawn 孙进程） | server.rs:237-241, 354-357 |
| TAURI-05 | dsh-page 未显式 local:false——剪贴板能力同时授予本地控制台页 ✅ | capabilities/dsh-page.json:5-8（✅ 生成 capabilities.json 含 local:true） |
| TAURI-06 | 本地页无 CSP 且含出站 fetch（api.github.com） | tauri.conf.json:23；ui/app.js:147 |
| TAURI-07 | stop 与 start 交错可静默吞掉停止 | server.rs:195-222, 345-365 |
| F2(前端) | 日志增量判断以"行数"为键，缓冲 ≥400 行后日志视图永久冻结；"清空"只清 DOM 不清后端 ✅ | ui/app.js:95-96, 127-130；server.rs:128-141 |
| F3(前端) | 每 2.5s 全量重建 400 行日志 DOM 并强制钉底，用户无法上翻阅读；托盘隐藏仍轮询 | ui/app.js:97-105, 174-175 |
| F4(前端) | stop 后端口未复位，状态栏/浏览器/重启按钮与"已停止"矛盾 ✅ | ui/app.js:65-68；server.rs:345-365 |
| F6(前端) | IPC 错误被 call() 静默吞掉，轮询失败时 UI 冻结在陈旧状态 | ui/app.js:34-41, 51-81 |
| SC-08 | Cargo 依赖浮点范围 + 无 cargo audit/dependabot；CI 构建无 --locked（当前锁定 tauri 2.11.5 已覆盖 origin-confusion 公告，风险过程性） | Cargo.toml:15, 18-23；release.yml:46 |
| SC-09 | DSH_DSH_VERSION env 可注入任意 npm spec（git/file: URL） | prepare-runtime.mjs:15, 22 |
| SC-10 | CI 用 npm install 而非 npm ci（根锁文件已提交） | release.yml:40 |
| SC-11 | fetch-node tmp 路径确定且写入非原子，同用户 TOCTOU/符号链接覆写 | fetch-node.mjs:33-34, 43, 45-46, 52 |
| DC-08 | 关窗"one-time hint"是死代码：首次关窗无任何驻留提示（swap 语义正确但提示未实现） | lib.rs:274-287 |
| DC-09 | start/restart fire-and-forget：错误被吞、UI 靠轮询滞后感知；restart 逻辑命令/菜单两处复制 | lib.rs:52-68, 154-161 |
| DC-10 | 窗口记忆仅在正常退出时持久化：关窗隐藏/强杀丢失状态（与"关窗不退出"设计矛盾） | lib.rs:227；tauri.conf.json:11-21 |
| TM-06 | 开机自启 LaunchAgent plist 字符串拼接无 XML 转义（auto-launch crate）；卸载残留自启项 | auto-launch macos.rs:97-107；lib.rs:226 |
| TM-07 | 不同构建（不同 identifier）可绕过单实例并发写同一 ~/.dsh（推测性） | lib.rs:221-223；server.rs:111-116 |
| TM-08 | 更新检查无签名/固定校验（已并入 M11；此处记录供应链残余面） | ui/app.js:147-167 |
| TM-09 | 服务日志无脱敏且无访问分级（2000 条内存缓冲；✅ 远程页无法调 logs，暴露面仅本地控制台页） | server.rs:128-141；lib.rs:47-50 |

### Info（3）

| 编号 | 标题 | 说明 |
|---|---|---|
| TAURI-08 / TM-10 | 远程页当前无法调用任何自定义命令；webview 沙箱边界紧致（正向确认）✅ | ACL 无 app 键 + 无远程能力引用 → start/stop/open_data_dir 等对远程页全部拒绝；残余面仅剪贴板。建议在 CI 固化该不变量 |
| F7(前端) | window.__TAURI__ 顶层解引用，非 Tauri 环境整页白屏 | ui/app.js:3 |
| F8(前端) | DOM 注入面审计：全部外部输入经 textContent/title 写入，未发现 XSS 路径（正向确认）✅ | LEVEL_CLASS 裸键查找建议 Object.hasOwn 加固 |

---

## 三、交叉核验记录（关键声明 → 证据）

| 声明 | 核验方式 | 结果 |
|---|---|---|
| watchdog 持锁调 log() 死锁 | 逐行对照 server.rs:309-316/325-330 | ✅ 确认（std Mutex 不可重入） |
| watchdog 重启被 Starting 守卫挡住 | 对照 server.rs:196-201 vs 319-341 | ✅ 确认 |
| stop() 杀不掉子进程（watchdog 独占句柄） | 对照 server.rs:289-293 vs 345-365 | ✅ 确认 |
| dsh /api 无认证（"fence is not an auth layer"） | grep 官方包 dsh-client-connection/lib/index.js:118-119 | ✅ 确认 |
| dsh 拒绝 0.0.0.0 绑定 | 官方包 startup.js:39 | ✅ 确认 |
| `npm run prepare` 不存在 | 实际执行 | ✅ 退出码 1 |
| dsh-page 能力 local:true（控制台页也获剪贴板） | 读生成产物 out/capabilities.json | ✅ 确认 |
| ACL 无 app 权限键 | 读生成产物 acl-manifests.json（14 个插件键，无 __app__）；desktop-schema.json 无 allow-* 条目 | ✅ 确认 |
| 前端 bundle 不用 readText | grep 打包产物 index-Dqw48FrP.js：writeText×8、readText×0 | ✅ 确认 |
| clipboard read/write 无手势校验 | 读插件 2.3.2 源码 commands.rs（_app 忽略、直接读写） | ✅ 确认 |
| 运行时锁文件被 gitignore | git check-ignore + 本地 lockfile（588 包 367KB） | ✅ 确认 |
| 锁定依赖版本 | Cargo.lock：tauri 2.11.5 / wry 0.55.1 / clipboard 2.3.2 | ✅ 确认 |
| --port CLI 参数未实现 | grep src/ 无 env::args/clap，仅 DSH_DESKTOP_PORT env | ✅ 确认（README 声称不实） |
| 更新检查端点状态 | 实测 api.github.com 返回 404（仓库尚无 Release） | ✅ 确认 |
| ~/.dsh 数据与权限 | 实测：.credentials.yaml(0600)、settings.yaml(0600)、sessions/(0700)、.anonymous-user-id(0644)、目录本身 0755 | ✅ 确认 |
| 前端无 XSS 注入路径 | grep ui/ 无 innerHTML/eval/document.write；全部 textContent/title | ✅ 确认 |
| 更新按钮从不打开 html_url | 对照 app.js:143-158（点击恒重查、仅写 title） | ✅ 确认 |
| DSH_HOME 官方优先解析 | 官方包 dsh-home-paths/lib/index.js:73-76（子代理引用，未亲自复核） | 引用待复核 |

---

## 四、修复优先级路线图

**P0（发布阻断，先修）**
1. H1：watchdog 持锁调 log 的死锁（挪出锁作用域）
2. H2：stop()/退出/重启杀不掉子进程（kill+wait 归属修复；杜绝孤儿）
3. H3：崩溃后自动重启失效（Starting 守卫问题）
4. H7：CI `npm run prepare` → `npm run setup`
5. M9：macOS 产物（dmg/zip .app）

**P1（安全，随下个版本）**
6. F1/H6：端口身份握手 + remote.urls 收窄到 3080 + attach 警告 + 禁止自动导航
7. H5：删除 allow-read-text（零成本）+ TAURI-05 local:false
8. F2/H12：dsh API 鉴权（per-instance token 或 UNIX socket；至少 H2 修复前不残留实例）
9. H10：Node 下载加校验和
10. M7：提交 runtime 锁文件 + npm ci

**P2（供应链/加固）**
11. H8：CI actions 钉 SHA、per-job 最小权限
12. H9：签名 + 公证 + 校验和
13. M8：补丁脚本失败报警 + bundle 哈希
14. M5：本地页 CSP

**P3（健壮性/体验）**
15. M1/M2/M3/M4：崩溃计数、TOCTOU、就绪导航、backoff 取消
16. M10/M11/M12/M13：README 一致性、更新检查、DSH_HOME、目录权限
17. H4/H11：attach 健康监测、console_url 可靠捕获
18. 其余 Low 按序跟进

---

## 五、审计范围与免责

- **覆盖**：`src-tauri/src/*.rs`、`tauri.conf.json`、`capabilities/*`、`ui/*`、`scripts/*.mjs`、`.github/workflows/release.yml`、`package.json`、`README.md`、生成产物（capabilities.json/acl-manifests.json/desktop-schema.json）、打包的 dsh 运行时（仅交叉验证接口与鉴权面）、插件源码（tauri-plugin-clipboard-manager 2.3.2、auto-launch 0.5.0）。
- **未覆盖**：官方 `@deepseek-ai/dsh*` 包的完整安全审计（超出本仓库范围，仅评估与本壳交互的接口）；WKWebView/WebView2 本身；Windows 平台运行时行为（本机为 macOS，Windows 结论为代码推演）。
- 推测性发现已标注；未标注的均经源码/产物/执行核验。F2/H12（dsh API 无认证）为上游设计缺陷，本壳是暴露面放大者，修复需上游配合或外壳侧拦截。

---

## 六、正式版修复记录（v1.0.0，DeepSeek Harness Desktop Client）

正式版开发按 P0+P1 路线图落地，同时完成三项产品需求（改名 / 官方版本检测与在线更新 / 汉化）。

| 编号 | 发现 | 修复状态 | 修复内容 |
|---|---|---|---|
| H1 | watchdog 持锁二次加锁死锁 | ✅ 已修复 | `server.rs` 重写：`log()` 永不持锁调用；watchdog 状态机在解锁后写日志 |
| H2 | stop/退出/重启杀不掉子进程 | ✅ 已修复 | `inner.child` 单一所有权：`stop()` 直接 kill+wait；watchdog 停止分支同样 kill+wait；退出路径可靠终止 |
| H3 | 崩溃后自动重启失效 | ✅ 已修复 | 崩溃分支重置状态为 Stopped 放行 `start()` 守卫；backoff 期间尊重 stop 请求（M4） |
| H4 | attach 模式无健康监测 | ✅ 已修复 | attach 也启动 watchdog，周期探测外部服务（连续 3 次失败回落 Stopped）；UI 标注「外部进程」 |
| H5 | 远程页剪贴板读权限死权限 | ✅ 已修复 | `dsh-page.json` 删除 `allow-read-text`，仅保留写；`local: false`（TAURI-05） |
| H6 | remote.urls 通配所有端口 | ✅ 已修复 | 静态收窄为 `http://127.0.0.1:3080/*`；自定义端口经 `dynamic-acl` 运行时按实际端口授权 |
| H7 | CI 调用不存在的 npm script | ✅ 已修复 | `release.yml` 改用 `npm run setup`；`npm ci`；`tauri build --locked` |
| H8 | CI actions 未钉死 SHA + 权限过大 | ✅ 已修复 | 各 action 钉 40 位 SHA（Dependabot 维护）；permissions per-job 最小化（build 仅 read） |
| H9 | 发布物无签名/校验和 | ◐ 部分修复 | 发布物附 SHA256SUMS.txt；签名/公证需开发者证书（README「发布」节说明） |
| H10 | Node 下载无校验和 | ✅ 已修复 | `fetch-node.mjs`：下载同版本 SHASUMS256.txt 并比对 sha256；`--proto '=https'`；随机临时目录 |
| F1/H4 | 端口抢占劫持主窗口 | ◐ 缓解 | attach 身份校验（响应必须含官方 dsh 特征，非 dsh 服务报错拒绝）；能力收窄 + 只写剪贴板 + 外部进程标注。残余：可伪造特征的恶意进程仍需上游 API 鉴权（F2）根治 |
| M1 | crash_count 永不重置 | ✅ 已修复 | 稳定运行 30s 后清零；端口占用不计入崩溃 |
| M2 | start TOCTOU 双 spawn | ✅ 已修复 | 检查+置状态单锁完成 |
| M3 | 导航到未就绪端口 | ✅ 已修复 | 导航前就绪探测（最长 20s）；前端自导航移除，后端为单一导航源 |
| M6 | core:default 过度授权 / ACL 隐式 | ✅ 已修复 | `permissions/app.toml` 显式定义 14 个应用命令权限；`default.json` 显式引用，移除 core:default |
| M7 | runtime 锁文件未提交 | ✅ 已修复 | `.gitignore` 例外提交 package-lock.json；`prepare-runtime.mjs` 有锁时 `npm ci`（实测 529 包复现） |
| M8 | patch 脚本静默失败 | ✅ 已废弃 | `patch-web-bundle.mjs` 删除；剪贴板桥改为运行时注入（每 3s 幂等），官方 bundle 实测已恢复原样（0 处补丁痕迹） |
| M9 | macOS 产物缺失 | ✅ 已修复 | targets 含 dmg；CI `--bundles "app,dmg"`；产物断言替代 `|| true` |
| M12 | DSH_HOME 被忽略 | ✅ 已修复 | `dsh_data_dir()` 统一解析（$DSH_HOME 优先）；info/open_data_dir 一致 |
| M13 | ~/.dsh 权限 0755 | ✅ 已修复 | 创建/打开数据目录时收紧 0700 |
| H11 | console_url 捕获不可靠 | ✅ 已修复 | 延迟捕获 + about:blank 守卫 + expect 改安全解析 |
| F2(前端) | 日志视图冻结/清空无效 | ✅ 已修复 | 指纹增量判断；新增 `clear_logs` 命令（后端真清空） |
| F4(前端) | stop 后端口状态矛盾 | ✅ 已修复 | 非运行状态忽略 port 字段 |
| TAURI-06 | 本地页无 CSP | ✅ 已修复 | `tauri.conf.json` 配置 CSP（connect-src 仅放行 api.github.com / registry.npmjs.org） |
| SC-09 | DSH_DSH_VERSION 注入 | ✅ 已修复 | prepare-runtime/fetch-node 版本号正则校验 |
| SC-10 | CI npm install 非 ci | ✅ 已修复 | `npm ci` |
| SC-11 | tmp 路径 TOCTOU | ✅ 已修复 | `mkdtempSync` 独占临时目录 |

### 正式版新增功能
- **改名**：DeepSeek Harness Desktop Client（productName / identifier / 窗口标题 / 菜单 / 托盘 / 仓库链接 Cnkore007/dsh-Desktop-Client），版本 1.0.0；
- **官方版本检测与在线更新**：控制台检测 npm registry 最新版 → `update_dsh` 命令 → `update-runtime.mjs`（integrity sha512 校验、staging 原子替换用户级 runtime `~/.dsh/desktop-runtime`）→ 自动重启生效；用户级 runtime 优先于 bundle；
- **汉化**：控制台「界面语言」开关写入官方 `locale.preference`（zh/en/auto，官方原生支持，覆盖 web UI 与右键菜单）；「官方命令参考（中文）」内置 dsh/dsh web CLI 帮助对照；
- **绝不改动官方源码**：删除补丁脚本，剪贴板桥运行时注入；在线更新安装官方 npm 原包。

### 遗留（后续版本）
- F2/H12：官方 dsh `/api` 无认证（上游设计，需官方引入 per-instance token 或 UNIX socket；壳侧已通过不再残留实例 + 端口身份校验缓解）；
- H9 签名/公证（需开发者证书）；自动更新（签名版）；TM-06 LaunchAgent plist 转义（依赖上游 auto-launch crate）。
