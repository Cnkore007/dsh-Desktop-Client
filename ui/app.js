// DSh Desktop 控制台 —— 与服务外壳通过 Tauri IPC 通信（原创实现）
/* global window */
const invoke = window.__TAURI__.core.invoke

const $ = (id) => document.getElementById(id)
const els = {
  statusDot: $('status-dot'),
  statusLabel: $('status-label'),
  kvPort: $('kv-port'),
  kvPid: $('kv-pid'),
  kvDsh: $('kv-dsh'),
  kvData: $('kv-data'),
  errorBox: $('error-box'),
  btnStart: $('btn-start'),
  btnRestart: $('btn-restart'),
  btnStop: $('btn-stop'),
  btnOpen: $('btn-open'),
  btnBrowser: $('btn-browser'),
  btnData: $('btn-data'),
  btnClearLog: $('btn-clear-log'),
  btnCheckUpdate: $('check-update'),
  autostart: $('autostart'),
  logView: $('log-view'),
  version: $('version'),
  footStatus: $('foot-status'),
}

let state = { status: 'unknown', port: null, pid: null }
let lastLogCount = -1
let autoNavTimer = null
let appInfo = null

// ---- IPC helpers ------------------------------------------------------------
async function call(cmd, args = {}) {
  try {
    return await invoke(cmd, args)
  } catch (err) {
    console.error(`invoke ${cmd} failed:`, err)
    return null
  }
}

// ---- status ----------------------------------------------------------------
const STATUS_META = {
  running: { label: '运行中', cls: 'running' },
  starting: { label: '启动中…', cls: 'starting' },
  stopped: { label: '已停止', cls: 'stopped' },
  error: { label: '服务异常', cls: 'error' },
}

async function refresh() {
  const st = await call('status')
  if (!st) return
  const meta = STATUS_META[st.state] ?? { label: st.state, cls: 'stopped' }
  els.statusDot.className = `dot ${meta.cls}`
  els.statusLabel.textContent = meta.label
  els.kvPort.textContent = st.port ?? '—'
  els.kvPid.textContent = st.pid ?? '—'
  els.errorBox.classList.toggle('hidden', !(st.message))
  if (st.message) els.errorBox.textContent = st.message

  const running = st.state === 'running'
  const starting = st.state === 'starting'
  els.btnStart.disabled = running || starting
  els.btnRestart.disabled = !running && !st.port
  els.btnStop.disabled = !running && !starting
  els.btnOpen.disabled = !running
  els.btnBrowser.disabled = !st.port

  // auto-navigate to the web UI when the service comes up
  if (running && st.port) {
    if (state.status !== 'running') {
      scheduleNav(st.port)
    }
  } else {
    clearTimeout(autoNavTimer)
    autoNavTimer = null
  }
  state = { status: st.state, port: st.port, pid: st.pid }
  els.footStatus.textContent = `${new Date().toLocaleTimeString()} · 状态已刷新`
}

function scheduleNav(port) {
  clearTimeout(autoNavTimer)
  autoNavTimer = setTimeout(() => {
    window.location.href = `http://127.0.0.1:${port}`
  }, 1500)
}

// ---- logs ------------------------------------------------------------------
const LEVEL_CLASS = { info: 'l-info', ok: 'l-ok', warn: 'l-warn', error: 'l-error' }

async function refreshLogs() {
  const lines = await call('logs', { n: 400 })
  if (!lines || lines.length === lastLogCount) return
  lastLogCount = lines.length
  const frag = document.createDocumentFragment()
  for (const line of lines) {
    const span = document.createElement('span')
    span.className = LEVEL_CLASS[line.level] ?? 'l-info'
    span.textContent = `[${line.time}] ${line.text}\n`
    frag.appendChild(span)
  }
  els.logView.replaceChildren(frag)
  els.logView.scrollTop = els.logView.scrollHeight
}

// ---- info ------------------------------------------------------------------
async function loadInfo() {
  const info = await call('info')
  if (!info) return
  appInfo = info
  if (info.version) els.version.textContent = `v${info.version}`
  els.kvDsh.textContent = info.dshVersion ?? '—'
  els.kvData.textContent = info.dataDir ?? '—'
}

// ---- actions ---------------------------------------------------------------
els.btnStart.addEventListener('click', async () => { await call('start'); refresh() })
els.btnRestart.addEventListener('click', async () => { await call('restart'); refresh() })
els.btnStop.addEventListener('click', async () => { await call('stop'); refresh() })
els.btnOpen.addEventListener('click', () => {
  if (state.port) window.location.href = `http://127.0.0.1:${state.port}`
})
els.btnBrowser.addEventListener('click', () => call('open_browser'))
els.btnData.addEventListener('click', () => call('open_data_dir'))
els.btnClearLog.addEventListener('click', () => {
  els.logView.replaceChildren()
  lastLogCount = -1
})

els.autostart.addEventListener('change', async (e) => {
  const ok = await call('autostart_set', { enabled: e.target.checked })
  if (ok === null) e.target.checked = !e.target.checked
})

async function loadAutostart() {
  const enabled = await call('autostart_enabled')
  if (enabled !== null) els.autostart.checked = enabled
}

// ---- update check ----------------------------------------------------------
els.btnCheckUpdate.addEventListener('click', async () => {
  els.btnCheckUpdate.textContent = '检查中…'
  els.btnCheckUpdate.disabled = true
  try {
    const res = await fetch('https://api.github.com/repos/Cnkore007/dsh-desktop-client/releases/latest', {
      headers: { Accept: 'application/vnd.github+json' },
    })
    if (!res.ok) throw new Error(String(res.status))
    const rel = await res.json()
    const cur = appInfo?.version
    if (rel.tag_name === `v${cur}`) {
      els.btnCheckUpdate.textContent = '已是最新版本'
    } else {
      els.btnCheckUpdate.textContent = `发现新版本 ${rel.tag_name}`
    }
    els.btnCheckUpdate.title = rel.html_url
  } catch {
    els.btnCheckUpdate.textContent = '检查失败'
  } finally {
    setTimeout(() => {
      els.btnCheckUpdate.textContent = '检查更新'
      els.btnCheckUpdate.disabled = false
    }, 4000)
  }
})

// ---- boot ------------------------------------------------------------------
async function boot() {
  await Promise.all([loadInfo(), loadAutostart()])
  await refresh()
  await refreshLogs()
  setInterval(refresh, 1200)
  setInterval(refreshLogs, 2500)
}
boot()
