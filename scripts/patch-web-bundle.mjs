// Applies the clipboard-hardening patch to the bundled dsh-web-frontend
// asset inside src-tauri/resources/runtime. This is what makes copy buttons
// actually write to the system pasteboard inside WKWebView (macOS): the
// patch makes writeClipboard prefer the Tauri native clipboard bridge, then
// navigator.clipboard.writeText, then ALWAYS run the execCommand('copy')
// textarea fallback (the only write that lands in WKWebView).
//
// The patch is content-addressed: it only touches the exact minified helper
// bodies it knows. If upstream dsh already ships the fix (or renames the
// helpers), the script detects it and skips — it never corrupts an unknown
// bundle.
//
// Usage: node scripts/patch-web-bundle.mjs

import { readdirSync, readFileSync, writeFileSync, copyFileSync, mkdirSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = dirname(dirname(fileURLToPath(import.meta.url)))
const assetsDir = join(root, 'src-tauri', 'resources', 'runtime', 'node_modules',
  '@deepseek-ai', 'dsh-web-frontend', 'dist', 'assets')

if (!readdirSafe(assetsDir)) {
  console.log('dsh-web-frontend assets not found — skipping patch')
  process.exit(0)
}

const entries = readdirSync(assetsDir).filter((f) => /^index-.*\.js$/.test(f))
if (entries.length === 0) {
  console.log('no index-*.js bundle found — skipping patch')
  process.exit(0)
}

const file = join(assetsDir, entries[0])
let src = readFileSync(file, 'utf8')

// Already patched (this build or a future upstream one ships the bridge)?
if (src.includes('window.__TAURI__.clipboard') || !src.includes('navigator.clipboard')) {
  console.log('bundle already has native clipboard support or no clipboard code — skipping')
  process.exit(0)
}

// --- patch 1: shared writeClipboard helper (minified `async function br`) ---
const OLD1 = /async function (\w+)\(n\)\{var s;if\(\(s=navigator\.clipboard\)!=null&&s\.writeText\)try\{return await navigator\.clipboard\.writeText\(n\),!0\}catch\{return!1\}const r=typeof document\.execCommand=="function"\?document\.execCommand\.bind\(document\):void 0;/
const m1 = src.match(OLD1)
if (!m1) {
  console.error('patch 1 pattern not found — bundle shape changed; skipping (safe)')
  process.exit(0)
}
const fnName = m1[1]
const NEW1 = `async function ${fnName}(n){try{if(typeof window!="undefined"&&window.__TAURI__&&window.__TAURI__.clipboard&&window.__TAURI__.clipboard.writeText)return await window.__TAURI__.clipboard.writeText(n),!0}catch{}if(navigator.clipboard&&navigator.clipboard.writeText)try{await navigator.clipboard.writeText(n)}catch{}const r=typeof document.execCommand=="function"?document.execCommand.bind(document):void 0;`
src = src.replace(m1[0], NEW1)

// --- patch 2: JsonTree inline copy handler (minified `Se=async O=>...`) ---
const OLD2 = /Se=async O=>\{if\(T!==void 0\)\{try\{await navigator\.clipboard\.writeText\(od\(T,O\)\),z\("copied"\)\}catch\{z\("failed"\)\}E\.current!==void 0&&clearTimeout\(E\.current\),E\.current=setTimeout\(\(\)=>\{z\("idle"\)\},1500\)\}\}/
const m2 = src.match(OLD2)
if (!m2) {
  console.error('patch 2 pattern not found — bundle shape changed; skipping (safe)')
  process.exit(0)
}
const NEW2 = `Se=async O=>{if(T!==void 0){const W=async()=>{try{if(typeof window!="undefined"&&window.__TAURI__&&window.__TAURI__.clipboard&&window.__TAURI__.clipboard.writeText)return await window.__TAURI__.clipboard.writeText(od(T,O)),!0}catch{}if(navigator.clipboard&&navigator.clipboard.writeText)try{await navigator.clipboard.writeText(od(T,O))}catch{}const r=typeof document.execCommand=="function"?document.execCommand.bind(document):void 0;if(r===void 0)return!1;const i=document.createElement("textarea");i.value=od(T,O),i.setAttribute("readonly",""),i.style.position="fixed",i.style.left="-9999px",document.body.appendChild(i),i.select();try{return r("copy")}catch{return!1}finally{i.remove()}};try{(await W())&&z("copied")}catch{z("failed")}E.current!==void 0&&clearTimeout(E.current),E.current=setTimeout(()=>{z("idle")},1500)}}`
src = src.replace(m2[0], NEW2)

const backupDir = join(dirname(file), '.patch-backup')
mkdirSync(backupDir, { recursive: true })
copyFileSync(file, join(backupDir, `${entries[0]}.orig`))
writeFileSync(file, src)
console.log(`patched ${entries[0]} (backup in ${backupDir})`)

function readdirSafe(dir) {
  try {
    return readdirSync(dir).length >= 0
  } catch {
    return null
  }
}
