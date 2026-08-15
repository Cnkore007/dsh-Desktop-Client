// Installs the official @deepseek-ai/dsh production tree into
// src-tauri/resources/runtime so packaged builds can serve the harness
// without a network connection or a system npm.
//
// Usage: node scripts/prepare-runtime.mjs
// Env:   DSH_DSH_VERSION   npm version spec (default 0.1.0-rc.6)

import { execFileSync } from 'node:child_process'
import { existsSync, mkdirSync, writeFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = dirname(dirname(fileURLToPath(import.meta.url)))
const runtimeDir = join(root, 'src-tauri', 'resources', 'runtime')
const version = process.env.DSH_DSH_VERSION ?? '0.1.0-rc.6'
mkdirSync(runtimeDir, { recursive: true })

const pkg = {
  name: 'dsh-desktop-runtime',
  private: true,
  version: '0.0.0',
  dependencies: { '@deepseek-ai/dsh': version },
}
writeFileSync(join(runtimeDir, 'package.json'), JSON.stringify(pkg, null, 2) + '\n')

console.log(`Installing @deepseek-ai/dsh@${version} -> ${runtimeDir}`)
execFileSync('npm', ['install', '--no-audit', '--no-fund', '--prefix', runtimeDir], {
  stdio: 'inherit',
  shell: process.platform === 'win32',
})

const bin = join(runtimeDir, 'node_modules', '@deepseek-ai', 'dsh', 'lib', 'bin.js')
if (!existsSync(bin)) {
  console.error('runtime install failed: dsh bin.js missing')
  process.exit(1)
}
console.log('Runtime ready.')
