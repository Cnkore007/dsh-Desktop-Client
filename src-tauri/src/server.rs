//! DSh Desktop — server lifecycle management.
//!
//! Spawns the official `dsh web` server with the bundled Node runtime,
//! attaches to an already-running instance on the default port, captures
//! logs into a ring buffer, and runs a watchdog that restarts the service
//! after unexpected crashes (with a bounded backoff).

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
    last_crash: Option<Instant>,
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
            last_crash: None,
            resource_dir: None,
        }
    }
}

/// Handle shared between the UI layer and the watchdog thread.
#[derive(Clone)]
pub struct DshServer {
    inner: Arc<Mutex<Inner>>,
}

fn now() -> String {
    let d = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = d.as_secs() % 86_400;
    format!("{:02}:{:02}:{:02}", secs / 3600, (secs % 3600) / 60, secs % 60)
}

impl DshServer {
    pub fn new() -> Self {
        Self { inner: Arc::new(Mutex::new(Inner::default())) }
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
        ServerStatus {
            state: inner.state.as_str().to_string(),
            port: (inner.port != 0).then_some(inner.port),
            pid: inner.pid,
            message: inner.message.clone(),
        }
    }

    pub fn logs(&self, n: usize) -> Vec<LogLine> {
        let inner = self.inner.lock().unwrap();
        let skip = inner.logs.len().saturating_sub(n);
        inner.logs.iter().skip(skip).cloned().collect()
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

    /// True when something already answers on the port.
    pub fn probe(&self, port: u16) -> bool {
        let addr = format!("127.0.0.1:{port}");
        let Ok(mut stream) = TcpStream::connect_timeout(&addr.parse().unwrap(), Duration::from_millis(600))
        else { return false };
        let _ = stream.set_read_timeout(Some(Duration::from_millis(600)));
        let _ = stream.write_all(b"GET / HTTP/1.0\r\n\r\n");
        let mut buf = [0u8; 64];
        let Ok(n) = stream.read(&mut buf) else { return false };
        let head = String::from_utf8_lossy(&buf[..n]);
        head.starts_with("HTTP/1.1 200") || head.starts_with("HTTP/1.0 200")
    }

    /// Resolve the bundled node executable, with dev-mode fallbacks.
    fn resolve_node(&self) -> Option<PathBuf> {
        let name = if cfg!(windows) { "node.exe" } else { "node" };
        for base in self.runtime_paths() {
            let p = base.join(name);
            if p.is_file() {
                return Some(p);
            }
        }
        None
    }

    fn resolve_dsh_bin(&self) -> Option<PathBuf> {
        for base in self.runtime_paths() {
            let p = base.join("node_modules").join("@deepseek-ai").join("dsh").join("lib").join("bin.js");
            if p.is_file() {
                return Some(p);
            }
        }
        None
    }

    pub fn runtime_paths(&self) -> Vec<PathBuf> {
        let mut list = Vec::new();
        if let Some(dir) = self.inner.lock().unwrap().resource_dir.clone() {
            list.push(dir.join("runtime"));
        }
        // dev mode: repo-local runtime
        list.push(PathBuf::from("src-tauri/resources/runtime"));
        list
    }

    /// Start (or attach to) the dsh web server. Never blocks for long.
    pub fn start(&self) -> Result<(), String> {
        {
            let inner = self.inner.lock().unwrap();
            if matches!(inner.state, ServerState::Running | ServerState::Starting) {
                return Ok(());
            }
        }

        let port = Self::port_override().unwrap_or(DEFAULT_PORT);
        {
            let mut inner = self.inner.lock().unwrap();
            inner.port = port;
            inner.stop_requested = false;
            inner.message = None;
            inner.state = ServerState::Starting;
        }
        self.log("info", format!("准备启动 dsh 服务（端口 {port}）"));

        // attach to an already-running instance instead of double-spawning
        if self.probe(port) {
            {
                let mut inner = self.inner.lock().unwrap();
                inner.state = ServerState::Running;
                inner.pid = None;
            }
            self.log("ok", format!("检测到已在运行的 dsh 服务（127.0.0.1:{port}），直接使用"));
            return Ok(());
        }

        let Some(node) = self.resolve_node() else {
            self.set_state(ServerState::Error, Some("未找到 Node 运行时（runtime/node 缺失，请运行 npm run prepare）".into()));
            return Err("node runtime not found".into());
        };
        let Some(bin) = self.resolve_dsh_bin() else {
            self.set_state(ServerState::Error, Some("dsh 运行时缺失（请运行 npm run prepare）".into()));
            return Err("dsh runtime not found".into());
        };

        let mut cmd = Command::new(&node);
        cmd.arg(&bin).arg("web").arg("--port").arg(port.to_string());
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
        {
            let mut inner = self.inner.lock().unwrap();
            inner.child = Some(child);
            inner.pid = Some(pid);
            inner.state = ServerState::Running;
        }
        self.log("ok", format!("dsh 服务已启动（pid {pid}，端口 {port}）"));

        // log capture threads
        let stdout = self.inner.lock().unwrap().child.as_mut().and_then(|c| c.stdout.take());
        let stderr = self.inner.lock().unwrap().child.as_mut().and_then(|c| c.stderr.take());
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

    /// Watch the child; restart it on unexpected exit (bounded backoff).
    fn watchdog(&self) {
        let server = self.clone();
        thread::spawn(move || {
            let child = {
                let mut inner = server.inner.lock().unwrap();
                inner.child.take()
            };
            let Some(mut child) = child else { return };

            // wait for the process to exit (or a requested stop)
            let status = loop {
                match child.try_wait() {
                    Ok(Some(status)) => break Some(status),
                    Ok(None) => {
                        if server.inner.lock().unwrap().stop_requested {
                            break None;
                        }
                        thread::sleep(Duration::from_millis(400));
                    }
                    Err(_) => break None,
                }
            };

            if server.inner.lock().unwrap().stop_requested {
                let mut inner = server.inner.lock().unwrap();
                inner.pid = None;
                inner.state = ServerState::Stopped;
                inner.message = None;
                server.log("info", "服务已停止".into());
                return;
            }

            // unexpected exit → restart with backoff
            let code = status.map(|s| s.code().unwrap_or(-1)).unwrap_or(-1);
            {
                let mut inner = server.inner.lock().unwrap();
                inner.pid = None;
                inner.state = ServerState::Starting;
                inner.crash_count += 1;
                if inner.crash_count >= MAX_CONSECUTIVE_CRASHES {
                    inner.state = ServerState::Error;
                    inner.message = Some(format!("服务连续崩溃 {MAX_CONSECUTIVE_CRASHES} 次，已停止自动重启"));
                    server.log("error", "连续崩溃，已停止自动重启（可在控制台手动启动）".into());
                    return;
                }
                if let Some(last) = inner.last_crash {
                    if last.elapsed() < Duration::from_secs(10) {
                        thread::sleep(CRASH_BACKOFF);
                    }
                }
                inner.last_crash = Some(Instant::now());
            }
            server.log("warn", format!("dsh 服务异常退出（code {code}），自动重启…"));
            thread::sleep(CRASH_BACKOFF);
            let _ = server.start();
        });
    }

    /// Request a stop: kills the child process and marks the state.
    pub fn stop(&self) {
        {
            let mut inner = self.inner.lock().unwrap();
            inner.stop_requested = true;
        }
        let child = {
            let mut inner = self.inner.lock().unwrap();
            inner.child.take()
        };
        if let Some(mut child) = child {
            let _ = child.kill();
            let _ = child.wait();
        }
        {
            let mut inner = self.inner.lock().unwrap();
            inner.pid = None;
            inner.state = ServerState::Stopped;
            inner.message = None;
        }
        self.log("info", "服务已停止".into());
    }
}
