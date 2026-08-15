// Downloads the official Node.js binary (executable only) into
// src-tauri/resources/runtime/ so the packaged app can run the official
// `dsh web` server without a system Node.js.
//
// Usage: node scripts/fetch-node.mjs [version]   (default: v24.9.0)

import { execFileSync } from 'node:child_process'
import { copyFileSync, existsSync, mkdirSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = dirname(dirname(fileURLToPath(import.meta.url)))
const outDir = join(root, 'src-tauri', 'resources', 'runtime')
mkdirSync(outDir, { recursive: true })

const version = process.argv[2] ?? 'v24.9.0'

const PLATFORM = {
  darwin: { plat: 'darwin', ext: 'tar.gz', bin: 'node' },
  win32: { plat: 'win', ext: 'zip', bin: 'node.exe' },
}
const ARCH = { arm64: 'arm64', x64: 'x64' }
const p = PLATFORM[process.platform]
const arch = ARCH[process.arch]
if (!p || !arch) {
  console.error(`Unsupported platform/arch: ${process.platform}/${process.arch}`)
  process.exit(1)
}

const base = `node-${version}-${p.plat}-${arch}`
const url = `https://nodejs.org/dist/${version}/${base}.${p.ext}`
const archive = join(tmpdir(), `${base}.${p.ext}`)
const extractDir = join(tmpdir(), `dsh-node-${base}`)
const outExe = join(outDir, p.bin)

if (existsSync(outExe)) {
  console.log(`${p.bin} already present: ${outExe}`)
  process.exit(0)
}

console.log(`Downloading ${url}`)
execFileSync('curl', ['-fsSL', '-o', archive, url], { stdio: 'inherit' })

rmSync(extractDir, { recursive: true, force: true })
mkdirSync(extractDir, { recursive: true })
if (process.platform === 'win32') {
  execFileSync('powershell', ['-NoProfile', '-NonInteractive', '-Command',
    `Expand-Archive -LiteralPath '${archive}' -DestinationPath '${extractDir}' -Force`], { stdio: 'inherit' })
  copyFileSync(join(extractDir, base, 'node.exe'), outExe)
} else {
  execFileSync('tar', ['-xzf', archive, '-C', extractDir], { stdio: 'inherit' })
  copyFileSync(join(extractDir, base, 'bin', 'node'), outExe)
  execFileSync('chmod', ['+x', outExe])
}
rmSync(archive, { force: true })

execFileSync(outExe, ['--version'], { stdio: 'inherit' })
console.log(`${p.bin} -> ${outExe}`)
