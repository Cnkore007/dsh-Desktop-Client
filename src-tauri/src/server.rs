//! DeepSeek Harness Desktop Client — server lifecycle management.
//!
//! Spawns the official `dsh web` server with the bundled Node runtime,
//! attaches to an already-running instance on the default port, captures
//! logs into a ring buffer, and runs a watchdog that restarts the service
//! after unexpected crashes (with a bounded backoff).
//!
//! Lifecycle invariants (audit-hardened):
//! - `inner.child` is the single owner of the spawned process; `stop()` and
//!   the watchdog both terminate through it — no orphaned services.
//! - `log()` is never called while holding the state mutex (no re-entrant
//!   locking / deadlock).
//! - `start()` checks and transitions state in one lock scope (no TOCTOU
//!   double-spawn), and the crash-restart path resets state so the guard
//!   never blocks its own restart.
//! - Attach mode requires the responder to carry the dsh page identity
//!   marker; any other HTTP responder is rejected (port-hijack mitigation).
//! - The crash counter resets after the service stays stable; attach-mode
//!   services get periodic health probes.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;

pub const DEFAULT_PORT: u16 = 3080;
const MAX_CONSECUTIVE_CRASHES: u32 = 6;
const CRASH_BACKOFF: Duration = Duration::from_millis(1500);
const POLL_INTERVAL: Duration = Duration::from_millis(400);
/// 服务稳定运行超过该时长后重置崩溃计数（"连续崩溃"名副其实）
const STABLE_WINDOW: Duration = Duration::from_secs(30);
/// attach 模式健康探测：连续失败达到该次数才判定外部服务已死（容忍瞬时抖动）
const ATTACH_FAIL_LIMIT: u32 = 3;
/// attach 身份特征：官方 dsh 首页 HTML 的 title（防止把任意 HTTP 200 服务当 dsh 接管）
const DSH_BODY_MARKER: &str = "DeepSeek Harness";
/// 合法端口下限（<1024 需要特权，避免误用）
const MIN_PORT: u16 = 1024;

#[derive(Clone, Debug, Serialize, PartialEq)]
pub enum ServerState {
    Stopped,
    Starting,
    Running,
    Error,
}

impl ServerState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Stopped => "stopped",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Error => "error",
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct LogLine {
    pub time: String,
    pub level: String,
    pub text: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct ServerStatus {
    pub state: String,
    pub port: Option<u16>,
    pub pid: Option<u32>,
    pub message: Option<String>,
    /// true = 由本应用 spawn 并完全托管；false = attach 到的外部/残留实例
    pub managed: bool,
    /// 占用该端口的进程 PID（若检测到端口冲突或外部进程）
    pub occupant_pid: Option<u32>,
    /// 当前工作区目录路径
    pub workspace: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct SessionStats {
    pub count: usize,
    pub size_bytes: u64,
    pub size_formatted: String,
    pub backup_count: usize,
    pub sessions_dir: String,
    pub backups_dir: String,
}

/// 端口探测结果：空闲 / 应答为 dsh 服务 / 应答为非 dsh 服务
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ProbeResult {
    Free,
    Dsh,
    Foreign,
}

struct Inner {
    state: ServerState,
    port: u16,
    pid: Option<u32>,
    message: Option<String>,
    logs: VecDeque<LogLine>,
    child: Option<Child>,
    stop_requested: bool,
    crash_count: u32,
    running_since: Option<Instant>,
    attach_failures: u32,
    resource_dir: Option<PathBuf>,
}

impl Default for Inner {
    fn default() -> Self {
        Self {
            state: ServerState::Stopped,
            port: 0,
            pid: None,
            message: None,
            logs: VecDeque::new(),
            child: None,
            stop_requested: false,
            crash_count: 0,
            running_since: None,
            attach_failures: 0,
            resource_dir: None,
        }
    }
}

/// Handle shared between the UI layer and the watchdog thread.
#[derive(Clone)]
pub struct DshServer {
    inner: Arc<Mutex<Inner>>,
    /// 端口变化回调（lib.rs 注册：动态授权远程页 capability）
    port_callback: Arc<Mutex<Option<Box<dyn Fn(u16) + Send + Sync>>>>,
}

/// 用户数据目录：与官方 CLI 一致（$DSH_HOME 优先，缺省 ~/.dsh）。
pub fn dsh_data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("DSH_HOME") {
        let dir = dir.trim();
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    std::env::home_dir()
        .map(|h| h.join(".dsh"))
        .unwrap_or_else(|| PathBuf::from(".dsh"))
}

/// 在线更新后的用户级 runtime（~/.dsh/desktop-runtime，可写；bundle 内 runtime 只读）。
pub fn user_runtime_dir() -> PathBuf {
    dsh_data_dir().join("desktop-runtime")
}

/// 桌面客户端自身配置（端口等；不写入官方 settings.yaml，避免与官方 schema 冲突）。
pub fn desktop_settings_path() -> PathBuf {
    dsh_data_dir().join("desktop-settings.json")
}

/// 读取桌面配置中的工作区路径（若未设置或不存在则返回 None）。
pub fn read_workspace() -> Option<PathBuf> {
    let text = std::fs::read_to_string(desktop_settings_path()).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    v.get("workspace")
        .and_then(|w| w.as_str())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
}

/// 默认工作区路径（优先使用用户主目录或当前目录）
pub fn default_workspace_dir() -> PathBuf {
    std::env::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

/// 有效工作区路径
pub fn effective_workspace() -> PathBuf {
    read_workspace().unwrap_or_else(default_workspace_dir)
}

/// 持久化工作区路径到桌面配置（原子写，传入 None 则清除自定义设置）
pub fn write_workspace(workspace: Option<&str>) -> Result<(), String> {
    let path = desktop_settings_path();
    let dir = path.parent().ok_or("desktop-settings 路径无父目录")?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let mut doc: serde_json::Value =
        std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_else(|| serde_json::json!({}));
    if let Some(obj) = doc.as_object_mut() {
        if let Some(ws) = workspace {
            let trimmed = ws.trim();
            if trimmed.is_empty() {
                obj.remove("workspace");
            } else {
                obj.insert("workspace".into(), serde_json::json!(trimmed));
            }
        } else {
            obj.remove("workspace");
        }
    }
    let text = serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    Ok(())
}

/// 读取桌面配置中的端口（未设置返回 None）。
fn read_settings_port() -> Option<u16> {
    let text = std::fs::read_to_string(desktop_settings_path()).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    v.get("port")
        .and_then(|p| p.as_u64())
        .and_then(|p| u16::try_from(p).ok())
        .filter(|p| *p >= MIN_PORT)
}

/// 持久化端口到桌面配置（原子写，保留其他字段）。
pub fn write_settings_port(port: u16) -> Result<(), String> {
    let path = desktop_settings_path();
    let dir = path.parent().ok_or("desktop-settings 路径无父目录")?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let mut doc: serde_json::Value =
        std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_else(|| serde_json::json!({}));
    if let Some(obj) = doc.as_object_mut() {
        obj.insert("port".into(), serde_json::json!(port));
    }
    let text = serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    Ok(())
}

fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}

/// 获取会话统计信息
pub fn get_session_stats() -> SessionStats {
    let dsh_dir = dsh_data_dir();
    let sessions_dir = dsh_dir.join("sessions");
    let backups_dir = dsh_dir.join("backups");

    let mut count = 0usize;
    let mut size_bytes = 0u64;

    if sessions_dir.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&sessions_dir) {
            for entry in entries.flatten() {
                count += 1;
                if let Ok(meta) = entry.metadata() {
                    if meta.is_file() {
                        size_bytes += meta.len();
                    } else if meta.is_dir() {
                        if let Ok(sub) = std::fs::read_dir(entry.path()) {
                            for sub_entry in sub.flatten() {
                                if let Ok(m) = sub_entry.metadata() {
                                    size_bytes += m.len();
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    let mut backup_count = 0usize;
    if backups_dir.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&backups_dir) {
            backup_count = entries.flatten().filter(|e| e.path().is_file()).count();
        }
    }

    SessionStats {
        count,
        size_bytes,
        size_formatted: format_size(size_bytes),
        backup_count,
        sessions_dir: sessions_dir.display().to_string(),
        backups_dir: backups_dir.display().to_string(),
    }
}

/// 一键将会话打包备份至 ~/.dsh/backups/
pub fn backup_sessions() -> Result<PathBuf, String> {
    let dsh_dir = dsh_data_dir();
    let sessions_dir = dsh_dir.join("sessions");
    let backups_dir = dsh_dir.join("backups");
    std::fs::create_dir_all(&backups_dir).map_err(|e| format!("创建备份目录失败: {e}"))?;

    let timestamp = {
        let now = std::time::SystemTime::now();
        let dur = now.duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
        dur.as_secs()
    };
    let file_name = format!("dsh-sessions-backup-{timestamp}.tar.gz");
    let dest_file = backups_dir.join(&file_name);

    if !sessions_dir.exists() {
        std::fs::create_dir_all(&sessions_dir).map_err(|e| format!("创建会话目录失败: {e}"))?;
    }

    #[cfg(unix)]
    {
        let output = Command::new("tar")
            .args(["-czf", dest_file.to_str().unwrap(), "-C", dsh_dir.to_str().unwrap(), "sessions"])
            .output()
            .map_err(|e| format!("执行 tar 备份失败: {e}"))?;
        if !output.status.success() {
            let err = String::from_utf8_lossy(&output.stderr);
            return Err(format!("备份会话失败: {err}"));
        }
    }
    #[cfg(windows)]
    {
        let script = format!(
            "Compress-Archive -Path '{}' -DestinationPath '{}' -Force",
            sessions_dir.display(),
            dest_file.with_extension("zip").display()
        );
        let output = Command::new("powershell")
            .args(["-Command", &script])
            .output()
            .map_err(|e| format!("执行 PowerShell 压缩失败: {e}"))?;
        if !output.status.success() {
            let err = String::from_utf8_lossy(&output.stderr);
            return Err(format!("备份会话失败: {err}"));
        }
    }

    Ok(dest_file)
}

/// 清理会话与临时存储（清理前自动备份）
pub fn clear_session_cache() -> Result<String, String> {
    let backup_path = backup_sessions()?;
    let dsh_dir = dsh_data_dir();
    let sessions_dir = dsh_dir.join("sessions");
    let storages_dir = dsh_dir.join("storages");

    if sessions_dir.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&sessions_dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    let _ = std::fs::remove_dir_all(&p);
                } else {
                    let _ = std::fs::remove_file(&p);
                }
            }
        }
    }

    if storages_dir.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&storages_dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    let _ = std::fs::remove_dir_all(&p);
                } else {
                    let _ = std::fs::remove_file(&p);
                }
            }
        }
    }

    Ok(format!("已清理会话缓存（已自动备份至 {}）", backup_path.display()))
}

fn now() -> String {
    let d = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = d.as_secs() % 86_400;
    format!("{:02}:{:02}:{:02}", secs / 3600, (secs % 3600) / 60, secs % 60)
}

/// 查找监听在指定端口的进程 PID（跨平台）
pub fn find_port_pid(port: u16) -> Option<u32> {
    #[cfg(unix)]
    {
        if let Ok(output) = Command::new("lsof")
            .args(["-t", &format!("-iTCP:{port}"), "-sTCP:LISTEN"])
            .output()
        {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                for line in stdout.lines() {
                    if let Ok(pid) = line.trim().parse::<u32>() {
                        if pid > 1 && pid != std::process::id() {
                            return Some(pid);
                        }
                    }
                }
            }
        }
        if let Ok(output) = Command::new("lsof")
            .args(["-t", &format!("-i:{port}")])
            .output()
        {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                for line in stdout.lines() {
                    if let Ok(pid) = line.trim().parse::<u32>() {
                        if pid > 1 && pid != std::process::id() {
                            return Some(pid);
                        }
                    }
                }
            }
        }
    }

    #[cfg(windows)]
    {
        if let Ok(output) = Command::new("cmd")
            .args(["/c", &format!("netstat -ano | findstr :{port}")])
            .output()
        {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                for line in stdout.lines() {
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    if parts.len() >= 5 && parts.contains(&"LISTENING") {
                        if let Some(last) = parts.last() {
                            if let Ok(pid) = last.parse::<u32>() {
                                if pid > 0 && pid != std::process::id() {
                                    return Some(pid);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    None
}

/// 终止占用指定端口的进程（优雅 SIGTERM → 超时 SIGKILL）
pub fn kill_port_occupant(port: u16) -> Result<(), String> {
    let pid = find_port_pid(port).ok_or_else(|| format!("未找到占用端口 {port} 的进程"))?;
    if pid <= 1 || pid == std::process::id() {
        return Err("无法终止系统关键进程或当前应用自身".into());
    }

    #[cfg(unix)]
    {
        let _ = Command::new("kill").arg("-15").arg(pid.to_string()).output();
        thread::sleep(Duration::from_millis(400));
        if find_port_pid(port) == Some(pid) {
            let _ = Command::new("kill").arg("-9").arg(pid.to_string()).output();
            thread::sleep(Duration::from_millis(300));
        }
    }

    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/F", "/PID", &pid.to_string()])
            .output();
        thread::sleep(Duration::from_millis(500));
    }

    for _ in 0..10 {
        let addr = format!("127.0.0.1:{port}");
        if TcpStream::connect_timeout(&addr.parse().unwrap(), Duration::from_millis(150)).is_err() {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(150));
    }

    Ok(())
}

impl DshServer {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner::default())),
            port_callback: Arc::new(Mutex::new(None)),
        }
    }

    /// 注册端口变化回调（用于动态授权远程页 capability）。
    pub fn on_port_change(&self, cb: Box<dyn Fn(u16) + Send + Sync>) {
        *self.port_callback.lock().unwrap() = Some(cb);
    }

    fn notify_port(&self, port: u16) {
        if let Some(cb) = self.port_callback.lock().unwrap().as_ref() {
            cb(port);
        }
    }

    /// 有效端口：DSH_DESKTOP_PORT 环境变量（临时覆盖）> 桌面配置（持久化）> 默认。
    pub fn effective_port() -> u16 {
        Self::port_override()
            .or_else(read_settings_port)
            .unwrap_or(DEFAULT_PORT)
    }

    /// Record where the bundled runtime lives (app resource dir).
    pub fn set_resource_dir(&self, dir: PathBuf) {
        self.inner.lock().unwrap().resource_dir = Some(dir);
    }

    pub fn port_override() -> Option<u16> {
        std::env::var("DSH_DESKTOP_PORT")
            .ok()
            .and_then(|v| v.parse::<u16>().ok())
            .filter(|p| *p > 0)
    }

    pub fn status(&self) -> ServerStatus {
        let inner = self.inner.lock().unwrap();
        let current_port = if inner.port != 0 { inner.port } else { Self::effective_port() };
        let occupant_pid = if inner.state == ServerState::Error {
            find_port_pid(current_port)
        } else {
            None
        };
        ServerStatus {
            state: inner.state.as_str().to_string(),
            port: (inner.port != 0).then_some(inner.port),
            pid: inner.pid,
            message: inner.message.clone(),
            managed: inner.pid.is_some() && inner.child.is_some(),
            occupant_pid,
            workspace: effective_workspace().display().to_string(),
        }
    }

    pub fn logs(&self, n: usize) -> Vec<LogLine> {
        let inner = self.inner.lock().unwrap();
        let n = n.min(2000);
        let skip = inner.logs.len().saturating_sub(n);
        inner.logs.iter().skip(skip).cloned().collect()
    }

    /// 清空日志环形缓冲（控制台"清空"按钮）。
    pub fn clear_logs(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.logs.clear();
    }

    fn log(&self, level: &str, text: String) {
        let mut inner = self.inner.lock().unwrap();
        inner.logs.push_back(LogLine { time: now(), level: level.to_string(), text });
        let overflow = inner.logs.len().saturating_sub(2000);
        if overflow > 0 {
            inner.logs.drain(..overflow);
        }
    }

    fn set_state(&self, state: ServerState, message: Option<String>) {
        let mut inner = self.inner.lock().unwrap();
        inner.state = state;
        inner.message = message;
    }

    /// 探测 127.0.0.1:port：Free=无服务 / Dsh=应答且带 dsh 特征 / Foreign=有应答但非 dsh。
    /// 发送完整 HTTP/1.1 请求并循环读取响应体，避免单次 read 只读取头部导致特征误判。
    pub fn probe_port(&self, port: u16) -> ProbeResult {
        let addr = format!("127.0.0.1:{port}");
        let Ok(mut stream) =
            TcpStream::connect_timeout(&addr.parse().unwrap(), Duration::from_millis(800))
        else {
            return ProbeResult::Free;
        };
        let _ = stream.set_read_timeout(Some(Duration::from_millis(800)));
        let _ = stream.set_write_timeout(Some(Duration::from_millis(800)));
        let req = format!("GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nUser-Agent: DeepSeek-Harness-Desktop\r\nConnection: close\r\nAccept: text/html,*/*\r\n\r\n");
        if stream.write_all(req.as_bytes()).is_err() {
            return ProbeResult::Foreign;
        }

        let mut resp = Vec::with_capacity(8192);
        let mut buf = [0u8; 1024];
        while resp.len() < 16384 {
            match stream.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    resp.extend_from_slice(&buf[..n]);
                    let body = String::from_utf8_lossy(&resp);
                    if body.contains(DSH_BODY_MARKER)
                        || body.contains("<title>DeepSeek Harness</title>")
                        || body.contains("deepseek-harness")
                        || body.contains("dsh-web")
                        || body.contains("manifest.webmanifest")
                    {
                        return ProbeResult::Dsh;
                    }
                }
                Err(_) => break,
            }
        }

        if resp.is_empty() {
            return ProbeResult::Foreign;
        }
        let text = String::from_utf8_lossy(&resp);
        let is_http_ok = text.starts_with("HTTP/1.1 200")
            || text.starts_with("HTTP/1.0 200")
            || text.starts_with("HTTP/1.1 304")
            || text.starts_with("HTTP/1.0 304");

        if is_http_ok && (
            text.contains(DSH_BODY_MARKER)
            || text.contains("<title>DeepSeek Harness</title>")
            || text.contains("manifest.webmanifest")
            || text.contains("/assets/index-")
            || text.contains("/assets/vendor-")
        ) {
            ProbeResult::Dsh
        } else {
            ProbeResult::Foreign
        }
    }

    // ── runtime 路径解析 ────────────────────────────────────────────────

    /// bundle 内置 runtime（只读）：资源目录 + dev 模式 repo-local。
    fn bundle_runtime_paths(&self) -> Vec<PathBuf> {
        let mut list = Vec::new();
        if let Some(dir) = self.inner.lock().unwrap().resource_dir.clone() {
            list.push(dir.join("runtime"));
        }
        list.push(PathBuf::from("src-tauri/resources/runtime"));
        list
    }

    /// 用户级 runtime（在线更新目标，优先于 bundle）。
    fn user_runtime_paths(&self) -> Vec<PathBuf> {
        vec![user_runtime_dir()]
    }

    /// 供外部读取 dsh 包版本/元数据的路径：用户 runtime 优先，bundle 兜底。
    pub fn runtime_paths_for_version(&self) -> Vec<PathBuf> {
        let mut bases = self.user_runtime_paths();
        bases.extend(self.bundle_runtime_paths());
        bases
    }

    /// 解析 node 可执行文件：bundle 内置优先（用户 runtime 不含 node 二进制）。
    fn resolve_node(&self) -> Option<PathBuf> {
        let name = if cfg!(windows) { "node.exe" } else { "node" };
        for base in self.bundle_runtime_paths() {
            let p = base.join(name);
            if p.is_file() {
                return Some(p);
            }
        }
        None
    }

    /// 解析 dsh 入口：用户 runtime（可能已在线更新）优先，bundle 兜底。
    fn resolve_dsh_bin(&self) -> Option<PathBuf> {
        let mut bases = self.user_runtime_paths();
        bases.extend(self.bundle_runtime_paths());
        for base in bases {
            let p = base
                .join("node_modules")
                .join("@deepseek-ai")
                .join("dsh")
                .join("lib")
                .join("bin.js");
            if p.is_file() {
                return Some(p);
            }
        }
        None
    }

    // ── 生命周期 ────────────────────────────────────────────────────────

    /// Start (or attach to) the dsh web server. Never blocks for long.
    pub fn start(&self) -> Result<(), String> {
        let port = Self::effective_port();
        {
            // 检查 + 状态写入在同一锁作用域内完成（无 TOCTOU 双 spawn）
            let mut inner = self.inner.lock().unwrap();
            if matches!(inner.state, ServerState::Running | ServerState::Starting) {
                return Ok(());
            }
            inner.port = port;
            inner.stop_requested = false;
            inner.message = None;
            inner.state = ServerState::Starting;
            inner.attach_failures = 0;
        }
        self.log("info", format!("准备启动 dsh 服务（端口 {port}）"));
        self.start_impl(port)
    }

    fn start_impl(&self, port: u16) -> Result<(), String> {
        match self.probe_port(port) {
            ProbeResult::Free => self.spawn(port),
            ProbeResult::Dsh => {
                let occ_pid = find_port_pid(port);
                {
                    let mut inner = self.inner.lock().unwrap();
                    inner.state = ServerState::Running;
                    inner.port = port;
                    inner.pid = occ_pid;
                    inner.running_since = Some(Instant::now());
                    inner.attach_failures = 0;
                    inner.message = None;
                }
                let pid_str = occ_pid.map(|p| format!("（PID: {p}）")).unwrap_or_default();
                self.log("ok", format!("检测到已在运行的 dsh 服务（127.0.0.1:{port}{pid_str}），已接管健康监控"));
                self.notify_port(port);
                self.watchdog();
                Ok(())
            }
            ProbeResult::Foreign => {
                let occ_pid = find_port_pid(port);
                let pid_str = occ_pid.map(|p| format!(" (PID: {p})")).unwrap_or_default();
                self.set_state(
                    ServerState::Error,
                    Some(format!(
                        "端口 {port} 已被占用{pid_str}。若为残留的 dsh 服务，可点击「接管现有服务」；或点击「终止占用进程并重新拉起」。"
                    )),
                );
                self.log("warn", format!("端口 {port} 已被占用{pid_str}，等待用户处理"));
                Err(format!("port {port} occupied"))
            }
        }
    }

    /// 手动接管已有端口的服务
    pub fn attach_port(&self, port: u16) -> Result<(), String> {
        let occ_pid = find_port_pid(port);
        {
            let mut inner = self.inner.lock().unwrap();
            inner.state = ServerState::Running;
            inner.port = port;
            inner.pid = occ_pid;
            inner.running_since = Some(Instant::now());
            inner.attach_failures = 0;
            inner.message = None;
        }
        let pid_str = occ_pid.map(|p| format!("（PID: {p}）")).unwrap_or_default();
        self.log("ok", format!("已手动接管 127.0.0.1:{port} 上的服务{pid_str}"));
        self.notify_port(port);
        self.watchdog();
        Ok(())
    }

    /// 终止占用指定端口的进程并重新拉起 dsh 服务
    pub fn kill_and_restart(&self, port: u16) -> Result<(), String> {
        self.log("info", format!("正在终止占用端口 {port} 的进程…"));
        if let Err(e) = kill_port_occupant(port) {
            self.log("warn", format!("终止端口 {port} 占用进程提示: {e}"));
        } else {
            self.log("ok", format!("端口 {port} 占用进程已成功终止，端口已释放"));
        }
        self.stop();
        thread::sleep(Duration::from_millis(500));
        self.start()
    }

    fn spawn(&self, port: u16) -> Result<(), String> {
        let Some(node) = self.resolve_node() else {
            self.set_state(
                ServerState::Error,
                Some("未找到 Node 运行时（runtime/node 缺失，请运行 npm run setup）".into()),
            );
            return Err("node runtime not found".into());
        };
        let Some(bin) = self.resolve_dsh_bin() else {
            self.set_state(
                ServerState::Error,
                Some("dsh 运行时缺失（请运行 npm run setup）".into()),
            );
            return Err("dsh runtime not found".into());
        };

        let mut cmd = Command::new(&node);
        cmd.arg(&bin).arg("web").arg("--port").arg(port.to_string());
        let ws = effective_workspace();
        if ws.is_dir() {
            cmd.current_dir(&ws);
        }
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        cmd.stdin(Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        let child = cmd.spawn().map_err(|e| {
            self.set_state(ServerState::Error, Some(format!("启动失败: {e}")));
            format!("spawn failed: {e}")
        })?;
        let pid = child.id();

        let (stdout, stderr) = {
            let mut inner = self.inner.lock().unwrap();
            inner.child = Some(child);
            inner.pid = Some(pid);
            inner.state = ServerState::Running;
            inner.running_since = Some(Instant::now());
            inner.attach_failures = 0;
            let out = inner.child.as_mut().and_then(|c| c.stdout.take());
            let err = inner.child.as_mut().and_then(|c| c.stderr.take());
            (out, err)
        };
        self.log("ok", format!("dsh 服务已启动（pid {pid}，端口 {port}）"));
        self.notify_port(port);

        // log capture threads
        if let Some(out) = stdout {
            let server = self.clone();
            thread::spawn(move || {
                let reader = BufReader::new(out);
                for line in reader.lines().map_while(Result::ok) {
                    if !line.trim().is_empty() {
                        server.log("info", line);
                    }
                }
            });
        }
        if let Some(err) = stderr {
            let server = self.clone();
            thread::spawn(move || {
                let reader = BufReader::new(err);
                for line in reader.lines().map_while(Result::ok) {
                    if !line.trim().is_empty() {
                        server.log("error", line);
                    }
                }
            });
        }

        self.watchdog();
        Ok(())
    }

    /// Watch the child / attached service; restart the child on unexpected
    /// exit (bounded backoff, cancellable by stop).
    fn watchdog(&self) {
        let server = self.clone();
        thread::spawn(move || loop {
            // ── 1. 主动停止：kill + wait 子进程后退出线程 ──
            {
                let mut inner = server.inner.lock().unwrap();
                if inner.stop_requested {
                    let port = if inner.port != 0 { inner.port } else { Self::effective_port() };
                    let pid = inner.pid;
                    if let Some(mut child) = inner.child.take() {
                        let _ = child.kill();
                        let _ = child.wait();
                    }
                    if let Some(p) = pid {
                        if p > 1 && p != std::process::id() {
                            #[cfg(unix)]
                            {
                                let _ = Command::new("kill").args(["-15", &format!("-{p}")]).output();
                                let _ = Command::new("kill").args(["-9", &format!("-{p}")]).output();
                                let _ = Command::new("kill").args(["-15", &p.to_string()]).output();
                                let _ = Command::new("kill").args(["-9", &p.to_string()]).output();
                            }
                            #[cfg(windows)]
                            {
                                let _ = Command::new("taskkill")
                                    .args(["/F", "/T", "/PID", &p.to_string()])
                                    .output();
                            }
                        }
                    }
                    inner.pid = None;
                    inner.state = ServerState::Stopped;
                    inner.message = None;
                    inner.running_since = None;
                    drop(inner);
                    if port > 0 {
                        let _ = kill_port_occupant(port);
                    }
                    server.log("info", "服务已停止，进程已退出，端口已释放".into());
                    return;
                }
            }

            // ── 2. 稳定窗口：运行足够久则重置崩溃计数 ──
            {
                let mut inner = server.inner.lock().unwrap();
                if inner.crash_count > 0
                    && inner.running_since.is_some_and(|t| t.elapsed() >= STABLE_WINDOW)
                {
                    inner.crash_count = 0;
                    inner.running_since = None;
                }
            }

            // ── 3. spawn 模式：子进程退出检测 ──
            let exited: Option<(i32, u16)> = {
                let mut inner = server.inner.lock().unwrap();
                match inner.child.as_mut().and_then(|c| c.try_wait().ok().flatten()) {
                    Some(st) => Some((st.code().unwrap_or(-1), inner.port)),
                    None => None,
                }
            };
            if let Some((code, _port)) = exited {
                let restart = {
                    let mut inner = server.inner.lock().unwrap();
                    inner.child = None;
                    inner.pid = None;
                    inner.running_since = None;
                    if inner.crash_count + 1 >= MAX_CONSECUTIVE_CRASHES {
                        inner.state = ServerState::Error;
                        inner.message = Some(format!(
                            "服务连续崩溃 {MAX_CONSECUTIVE_CRASHES} 次，已停止自动重启（可在控制台手动启动）"
                        ));
                        false
                    } else {
                        inner.crash_count += 1;
                        inner.state = ServerState::Stopped; // 放行 start() 的状态守卫
                        true
                    }
                };
                if restart {
                    server.log("warn", format!("dsh 服务异常退出（code {code}），准备自动重启…"));
                    thread::sleep(CRASH_BACKOFF);
                    // backoff 期间用户可能请求了停止：尊重停止请求
                    if server.inner.lock().unwrap().stop_requested {
                        server.log("info", "已取消自动重启（用户请求停止）".into());
                    } else {
                        let _ = server.start();
                    }
                } else {
                    server.log(
                        "error",
                        "dsh 服务停止（崩溃次数超限，可在控制台手动处理）".into(),
                    );
                }
                continue;
            }

            // ── 4. attach 模式：外部服务健康探测 ──
            let attach_probe: Option<(u16, u32)> = {
                let mut inner = server.inner.lock().unwrap();
                if inner.child.is_none() && inner.state == ServerState::Running {
                    inner.attach_failures += 1;
                    Some((inner.port, inner.attach_failures))
                } else {
                    None
                }
            };
            if let Some((port, fails)) = attach_probe {
                match server.probe_port(port) {
                    ProbeResult::Dsh => {
                        server.inner.lock().unwrap().attach_failures = 0;
                    }
                    _ if fails >= ATTACH_FAIL_LIMIT => {
                        server.set_state(ServerState::Stopped, Some("外部 dsh 服务已退出".into()));
                        server.log("warn", format!("外部 dsh 服务（127.0.0.1:{port}）已退出，已回到控制台"));
                        return;
                    }
                    _ => {}
                }
                thread::sleep(POLL_INTERVAL);
                continue;
            }

            thread::sleep(POLL_INTERVAL);
        });
    }

    /// Request a stop: kills the child process and marks the state.
    /// Idempotent; safe to call while the watchdog is running or backing off.
    pub fn stop(&self) {
        let port = {
            let mut inner = self.inner.lock().unwrap();
            inner.stop_requested = true;
            let port = if inner.port != 0 { inner.port } else { Self::effective_port() };
            let pid = inner.pid;
            if let Some(mut child) = inner.child.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
            if let Some(p) = pid {
                if p > 1 && p != std::process::id() {
                    #[cfg(unix)]
                    {
                        let _ = Command::new("kill").args(["-15", &format!("-{p}")]).output();
                        let _ = Command::new("kill").args(["-9", &format!("-{p}")]).output();
                        let _ = Command::new("kill").args(["-15", &p.to_string()]).output();
                        let _ = Command::new("kill").args(["-9", &p.to_string()]).output();
                    }
                    #[cfg(windows)]
                    {
                        let _ = Command::new("taskkill")
                            .args(["/F", "/T", "/PID", &p.to_string()])
                            .output();
                    }
                }
            }
            inner.pid = None;
            inner.state = ServerState::Stopped;
            inner.message = None;
            inner.running_since = None;
            port
        };
        if port > 0 {
            let _ = kill_port_occupant(port);
        }
        self.log("info", "服务已停止，进程已退出，端口已释放".into());
    }

    /// 在线更新官方 @deepseek-ai/dsh 到指定版本（用户级 runtime；绝不修改
    /// 官方源码）。通过打包的 update-runtime.mjs 执行，输出进入日志流；
    /// 成功后自动重启服务使新版本生效。阻塞调用线程（由命令线程调用）。
    pub fn update_dsh(&self, version: &str) -> Result<(), String> {
        let Some(node) = self.resolve_node() else {
            self.log("error", "在线更新失败：未找到 Node 运行时".into());
            return Err("node runtime not found".into());
        };
        let Some(script) = self.resolve_update_script() else {
            self.log("error", "在线更新失败：update-runtime.mjs 缺失".into());
            return Err("update script not found".into());
        };
        self.log("info", format!("开始在线更新 @deepseek-ai/dsh@{version} …"));

        let mut cmd = Command::new(&node);
        cmd.arg(&script).arg(version);
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        cmd.stdin(Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        let mut child = cmd.spawn().map_err(|e| {
            self.log("error", format!("更新进程启动失败: {e}"));
            format!("spawn failed: {e}")
        })?;

        if let Some(out) = child.stdout.take() {
            let server = self.clone();
            thread::spawn(move || {
                let reader = BufReader::new(out);
                for line in reader.lines().map_while(Result::ok) {
                    if !line.trim().is_empty() {
                        server.log("info", line);
                    }
                }
            });
        }
        if let Some(err) = child.stderr.take() {
            let server = self.clone();
            thread::spawn(move || {
                let reader = BufReader::new(err);
                for line in reader.lines().map_while(Result::ok) {
                    if !line.trim().is_empty() {
                        server.log("error", line);
                    }
                }
            });
        }
        let status = child.wait().map_err(|e| {
            self.log("error", format!("等待更新进程失败: {e}"));
            format!("wait failed: {e}")
        })?;

        if status.success() {
            self.log("ok", "官方 dsh 运行时更新完成，正在重启服务…".into());
            thread::sleep(Duration::from_millis(500));
            self.stop();
            thread::sleep(Duration::from_millis(600));
            let _ = self.start();
            Ok(())
        } else {
            self.log("error", format!("更新失败（退出码 {:?}），已回退到原版本", status.code()));
            Err("update failed".into())
        }
    }

    fn resolve_update_script(&self) -> Option<PathBuf> {
        for base in self.bundle_runtime_paths() {
            let p = base.join("update-runtime.mjs");
            if p.is_file() {
                return Some(p);
            }
        }
        None
    }

    /// 查询官方 @deepseek-ai/dsh 最新版本（npm registry 优先，GitHub releases 兜底）
    pub fn fetch_latest_dsh_version() -> Option<String> {
        if let Ok(output) = Command::new("curl")
            .args(["-s", "--max-time", "4", "-H", "Accept: application/json", "https://registry.npmjs.org/@deepseek-ai/dsh"])
            .output()
        {
            if output.status.success() {
                if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&output.stdout) {
                    if let Some(ver) = json.get("dist-tags")
                        .and_then(|dt| dt.get("latest").or_else(|| dt.get("next")))
                        .and_then(|v| v.as_str())
                    {
                        return Some(ver.to_string());
                    }
                }
            }
        }

        if let Ok(output) = Command::new("curl")
            .args(["-s", "--max-time", "4", "-H", "Accept: application/vnd.github+json", "https://api.github.com/repos/deepseek-ai/deepseek-harness/releases/latest"])
            .output()
        {
            if output.status.success() {
                if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&output.stdout) {
                    if let Some(tag) = json.get("tag_name").and_then(|t| t.as_str()) {
                        return Some(tag.trim_start_matches('v').to_string());
                    }
                }
            }
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn test_effective_port_default() {
        assert_eq!(DEFAULT_PORT, 3080);
    }

    #[test]
    fn test_probe_free_port() {
        let server = DshServer::new();
        // 查找一个当前没有监听的临时端口
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener); // 释放端口

        assert_eq!(server.probe_port(port), ProbeResult::Free);
    }

    #[test]
    fn test_probe_dsh_mock() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 512];
                let _ = stream.read(&mut buf);
                let html = "<!doctype html><html><head><title>DeepSeek Harness</title></head><body></body></html>";
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    html.len(),
                    html
                );
                let _ = stream.write_all(resp.as_bytes());
            }
        });

        let server = DshServer::new();
        assert_eq!(server.probe_port(port), ProbeResult::Dsh);
    }

    #[test]
    fn test_probe_foreign_mock() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 512];
                let _ = stream.read(&mut buf);
                let resp = "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 12\r\nConnection: close\r\n\r\nHello World!";
                let _ = stream.write_all(resp.as_bytes());
            }
        });

        let server = DshServer::new();
        assert_eq!(server.probe_port(port), ProbeResult::Foreign);
    }

    #[test]
    fn test_format_size() {
        assert_eq!(format_size(500), "500 B");
        assert_eq!(format_size(1024), "1.0 KB");
        assert_eq!(format_size(1024 * 1024 * 3), "3.0 MB");
    }

    #[test]
    fn test_get_session_stats() {
        let stats = get_session_stats();
        assert!(!stats.sessions_dir.is_empty());
        assert!(!stats.backups_dir.is_empty());
    }
}
