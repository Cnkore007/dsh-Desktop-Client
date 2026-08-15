// DeepSeek Harness Desktop Client 控制台 —— 与服务外壳通过 Tauri IPC 通信（原创实现）
/* global window */
const invoke = window.__TAURI__.core.invoke

const $ = (id) => document.getElementById(id)
const els = {
  statusDot: $('status-dot'),
  statusLabel: $('status-label'),
  kvPort: $('kv-port'),
  kvPid: $('kv-pid'),
  kvManaged: $('kv-managed'),
  kvDsh: $('kv-dsh'),
  kvData: $('kv-data'),
  errorBox: $('error-box'),
  conflictActions: $('conflict-actions'),
  btnKillRestart: $('btn-kill-restart'),
  btnAttach: $('btn-attach'),
  btnStart: $('btn-start'),
  btnRestart: $('btn-restart'),
  btnStop: $('btn-stop'),
  btnOpen: $('btn-open'),
  enterHint: $('enter-hint'),
  btnUpdate: $('btn-update'),
  btnBrowser: $('btn-browser'),
  btnData: $('btn-data'),
  btnClearLog: $('btn-clear-log'),
  btnCheckUpdate: $('check-update'),
  autostart: $('autostart'),
  locale: $('locale'),
  port: $('port'),
  btnSavePort: $('btn-save-port'),
  logView: $('log-view'),
  version: $('version'),
  kvLatest: $('kv-latest'),
  kvWorkspace: $('kv-workspace'),
  btnPickWorkspace: $('btn-pick-workspace'),
  btnOpenWorkspace: $('btn-open-workspace'),
  btnResetWorkspace: $('btn-reset-workspace'),
  sessionStatBadge: $('session-stat-badge'),
  btnBackupSessions: $('btn-backup-sessions'),
  btnCleanSessions: $('btn-clean-sessions'),
  footStatus: $('foot-status'),
}

let state = { status: 'unknown', port: null, pid: null, managed: true }
let lastLogKey = null
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
  const running = st.state === 'running'
  // 非运行状态忽略 port 字段（停止后端口不再展示/可用）
  els.kvPort.textContent = running && st.port ? st.port : (st.port ?? '—')
  els.kvPid.textContent = st.pid ?? (st.occupant_pid ? `${st.occupant_pid} (占用)` : '—')
  // managed=false：attach 到的外部/残留 dsh 实例
  els.kvManaged.textContent = running ? (st.managed ? '本应用托管' : '外部进程') : '—'
  if (st.workspace) {
    els.kvWorkspace.textContent = st.workspace
    els.kvWorkspace.title = `当前工作区: ${st.workspace}`
  }
  els.errorBox.classList.toggle('hidden', !(st.message))
  if (st.message) els.errorBox.textContent = st.message

  const isConflict = st.state === 'error' && (st.message && (st.message.includes('占用') || st.message.includes('occupied')))
  els.conflictActions.classList.toggle('hidden', !isConflict)

  const starting = st.state === 'starting'
  els.btnStart.disabled = running || starting
  els.btnRestart.disabled = !running
  els.btnStop.disabled = !running && !starting
  els.btnOpen.disabled = !running
  // 服务就绪后，「进入界面」成为主操作，并显示就绪提示（手动进入）
  els.btnOpen.classList.toggle('primary', running)
  els.enterHint.classList.toggle('hidden', !running)
  if (running) {
    els.enterHint.textContent = `服务已就绪（127.0.0.1:${st.port}）——点击「进入界面」手动进入 DeepSeek Harness`
  }
  els.btnBrowser.disabled = !running
  state = { status: st.state, port: st.port, pid: st.pid, managed: st.managed }
  els.footStatus.textContent = `${new Date().toLocaleTimeString()} · 状态已刷新`
}

// ---- logs ------------------------------------------------------------------
const LEVEL_CLASS = { info: 'l-info', ok: 'l-ok', warn: 'l-warn', error: 'l-error' }
const LEVEL_KEYS = Object.keys(LEVEL_CLASS)

async function refreshLogs() {
  const lines = await call('logs', { n: 400 })
  if (!lines) return
  // 增量判断：以最后一行 time+text 为指纹（行数会被环形缓冲封顶，不可用作增量键）
  const last = lines[lines.length - 1]
  const key = last ? `${last.time}|${last.text}` : ''
  if (key === lastLogKey) return
  lastLogKey = key
  const frag = document.createDocumentFragment()
  for (const line of lines) {
    const span = document.createElement('span')
    span.className = LEVEL_KEYS.includes(line.level) ? LEVEL_CLASS[line.level] : 'l-info'
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
  if (info.port) els.port.value = info.port
}

// ---- actions ---------------------------------------------------------------
els.btnStart.addEventListener('click', async () => { await call('start'); refresh() })
els.btnRestart.addEventListener('click', async () => { await call('restart'); refresh() })
els.btnStop.addEventListener('click', async () => { await call('stop'); refresh() })

els.btnKillRestart.addEventListener('click', async () => {
  els.btnKillRestart.disabled = true
  els.btnKillRestart.textContent = '终止中…'
  await call('kill_port_process', { port: parseInt(els.port.value, 10) || null })
  setTimeout(() => {
    els.btnKillRestart.disabled = false
    els.btnKillRestart.textContent = '终止占用进程并重新拉起'
    refresh()
  }, 1000)
})

els.btnAttach.addEventListener('click', async () => {
  els.btnAttach.disabled = true
  els.btnAttach.textContent = '接管中…'
  await call('attach_port', { port: parseInt(els.port.value, 10) || null })
  setTimeout(() => {
    els.btnAttach.disabled = false
    els.btnAttach.textContent = '接管现有服务'
    refresh()
  }, 600)
})
// 进入官方界面：由后端权威导航（enter_ui 命令）。应用打开后不会自动进入，
// 必须用户手动点击「进入界面」
els.btnOpen.addEventListener('click', async () => {
  if (state.status !== 'running') return
  try {
    await invoke('enter_ui')
  } catch (err) {
    alert(`进入界面失败：${err}`)
  }
})
els.btnBrowser.addEventListener('click', () => call('open_browser'))
els.btnData.addEventListener('click', () => call('open_data_dir'))
els.btnClearLog.addEventListener('click', async () => {
  await call('clear_logs')
  els.logView.replaceChildren()
  lastLogKey = null
})

els.autostart.addEventListener('change', async (e) => {
  const ok = await call('autostart_set', { enabled: e.target.checked })
  if (ok === null) e.target.checked = !e.target.checked
})

async function loadAutostart() {
  const enabled = await call('autostart_enabled')
  if (enabled !== null) els.autostart.checked = enabled
}

// ---- 界面语言（官方 locale） ------------------------------------------------
async function loadLocale() {
  const pref = await call('get_locale')
  if (pref && els.locale.value !== pref) els.locale.value = pref
}

els.locale.addEventListener('change', async (e) => {
  const ok = await call('set_locale', { preference: e.target.value })
  if (ok === null) {
    e.target.value = await call('get_locale') ?? 'auto'
  }
})

// ---- 服务端口 ----------------------------------------------------------------
els.btnSavePort.addEventListener('click', async () => {
  const p = parseInt(els.port.value, 10)
  if (!Number.isInteger(p) || p < 1024 || p > 65535) {
    els.port.value = appInfo?.port ?? ''
    alert('端口需为 1024-65535 之间的整数')
    return
  }
  els.btnSavePort.disabled = true
  els.btnSavePort.textContent = '保存中…'
  const ok = await call('set_port', { port: p })
  if (ok === null) {
    alert('保存端口失败，请查看日志')
    els.port.value = appInfo?.port ?? ''
  }
  // 保存成功：后端会自动重启服务（新端口见状态卡片）
  els.btnSavePort.disabled = false
  els.btnSavePort.textContent = '保存'
  setTimeout(refresh, 2500)
})

// ---- update check ----------------------------------------------------------
els.btnCheckUpdate.addEventListener('click', async () => {
  els.btnCheckUpdate.textContent = '检查中…'
  els.btnCheckUpdate.disabled = true
  try {
    const res = await fetch('https://api.github.com/repos/Cnkore007/dsh-Desktop-Client/releases/latest', {
      headers: { Accept: 'application/vnd.github+json' },
    })
    if (!res.ok) throw new Error(String(res.status))
    const rel = await res.json()
    const cur = appInfo?.version
    if (cur && rel.tag_name === `v${cur}`) {
      els.btnCheckUpdate.textContent = '已是最新版本'
    } else {
      els.btnCheckUpdate.textContent = `发现新版本 ${rel.tag_name}`
      // 点击跳转下载页（README 声称的"一键跳转"）
      if (rel.html_url) els.btnCheckUpdate.title = rel.html_url
    }
  } catch {
    els.btnCheckUpdate.textContent = '检查失败'
  } finally {
    setTimeout(() => {
      els.btnCheckUpdate.textContent = '检查更新'
      els.btnCheckUpdate.disabled = false
    }, 4000)
  }
})

// ---- 官方 dsh 版本检测与在线更新 ------------------------------------------
async function checkDshLatest() {
  // 1. 优先调用后端命令（系统级网络请求，带多源解析）
  const info = await call('check_official_dsh')
  if (info && info.latest_version) {
    return info.latest_version
  }

  // 2. 前端 fetch npm registry（标准 json 头）
  try {
    const res = await fetch('https://registry.npmjs.org/@deepseek-ai/dsh', {
      headers: { Accept: 'application/json' },
    })
    if (res.ok) {
      const meta = await res.json()
      const ver = meta['dist-tags']?.latest || meta['dist-tags']?.next
      if (ver) return ver
    }
  } catch (e) {
    console.warn('fetch npm registry failed:', e)
  }

  // 3. 兜底尝试 GitHub Releases
  try {
    const res = await fetch('https://api.github.com/repos/deepseek-ai/deepseek-harness/releases/latest', {
      headers: { Accept: 'application/vnd.github+json' },
    })
    if (res.ok) {
      const rel = await res.json()
      if (rel.tag_name) return rel.tag_name.replace(/^v/, '')
    }
  } catch (e) {
    console.warn('fetch github releases failed:', e)
  }

  return null
}

async function refreshDshUpdate() {
  const latest = await checkDshLatest()
  els.kvLatest.textContent = latest ?? '—'
  const cur = appInfo?.dshVersion
  const outdated = Boolean(latest && cur && latest !== cur)
  els.btnUpdate.disabled = !outdated || updating
  if (outdated) {
    els.btnUpdate.textContent = `更新官方 dsh (v${latest})`
    els.btnUpdate.title = `官方发布了新版本 ${latest}（当前 ${cur}），点击一键在线更新`
  } else if (latest) {
    els.btnUpdate.textContent = `官方 dsh 已是最新 (v${latest})`
    els.btnUpdate.title = `官方仓库: https://github.com/deepseek-ai/deepseek-harness`
  } else {
    els.btnUpdate.textContent = '更新官方 dsh'
    els.btnUpdate.title = '检查官方版本中…'
  }
}

let updating = false
els.btnUpdate.addEventListener('click', async () => {
  const latest = els.kvLatest.textContent
  if (updating || !latest || latest === '—') return
  updating = true
  els.btnUpdate.disabled = true
  els.btnUpdate.textContent = '更新中…（进度见日志）'
  await call('update_dsh', { version: latest })
  // 更新由后端线程执行（含自动重启），UI 定期恢复并刷新
  const poll = setInterval(async () => {
    const lines = await call('logs', { n: 5 })
    const done = Array.isArray(lines) && lines.some((l) => l.text.includes('更新完成'))
    if (done || !els.btnUpdate) {
      clearInterval(poll)
      updating = false
      els.btnUpdate.textContent = '更新到最新'
      await Promise.all([refresh(), refreshDshUpdate()])
    }
  }, 3000)
})

els.kvLatest.style.cursor = 'pointer'
els.kvLatest.addEventListener('click', () => {
  call('open_official_repo')
})

// ---- 工作区目录管理 --------------------------------------------------------
els.btnPickWorkspace.addEventListener('click', async () => {
  els.btnPickWorkspace.disabled = true
  const dir = await call('pick_workspace_dir')
  if (dir) {
    await call('set_workspace', { path: dir })
    await refresh()
  }
  els.btnPickWorkspace.disabled = false
})

els.btnOpenWorkspace.addEventListener('click', () => {
  call('open_workspace_dir')
})

els.btnResetWorkspace.addEventListener('click', async () => {
  if (confirm('确定要重置工作区为默认目录吗？（服务将自动重启生效）')) {
    await call('reset_workspace')
    await refresh()
  }
})

// ---- 会话统计与备份清理 ----------------------------------------------------
async function refreshSessionStats() {
  const stats = await call('get_session_stats')
  if (!stats) return
  els.sessionStatBadge.textContent = `${stats.count} 个会话 · ${stats.size_formatted}`
  if (stats.backup_count > 0) {
    els.sessionStatBadge.title = `会话目录: ${stats.sessions_dir}\n已有 ${stats.backup_count} 个历史备份（存储于 backups/）`
  } else {
    els.sessionStatBadge.title = `会话目录: ${stats.sessions_dir}`
  }
}

els.btnBackupSessions.addEventListener('click', async () => {
  els.btnBackupSessions.disabled = true
  els.btnBackupSessions.textContent = '备份中…'
  const path = await call('backup_sessions')
  if (path) {
    els.btnBackupSessions.textContent = '备份完成！'
    setTimeout(async () => {
      els.btnBackupSessions.disabled = false
      els.btnBackupSessions.textContent = '一键备份会话'
      await refreshSessionStats()
    }, 2000)
  } else {
    els.btnBackupSessions.disabled = false
    els.btnBackupSessions.textContent = '一键备份会话'
  }
})

let cleaningSessions = false
els.btnCleanSessions.addEventListener('click', async () => {
  if (cleaningSessions) return
  if (!confirm('确定要清理历史会话缓存吗？\n（系统会在清理前自动在 backups 目录生成快照备份，安全防丢失）')) {
    return
  }
  cleaningSessions = true
  els.btnCleanSessions.disabled = true
  els.btnCleanSessions.textContent = '清理中…'
  const res = await call('clear_session_cache')
  if (res) {
    alert(res)
  }
  cleaningSessions = false
  els.btnCleanSessions.disabled = false
  els.btnCleanSessions.textContent = '清理历史缓存'
  await refreshSessionStats()
})

// ---- boot ------------------------------------------------------------------
async function boot() {
  await Promise.all([loadInfo(), loadAutostart(), loadLocale(), refreshSessionStats()])
  await refresh()
  await refreshLogs()
  await refreshDshUpdate()
  setInterval(refresh, 1200)
  setInterval(refreshLogs, 2500)
  setInterval(refreshSessionStats, 5000)
}
boot()

