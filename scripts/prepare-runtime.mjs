// Installs the official @deepseek-ai/dsh production tree into
// src-tauri/resources/runtime so packaged builds can serve the harness
// without a network connection or a system npm.
//
// Usage: node scripts/prepare-runtime.mjs
// Env:   DSH_DSH_VERSION   npm version spec (default 0.1.0-rc.6)
//
// Reproducibility (audit M7 / SC-05): package-lock.json is committed to the
// repo (.gitignore exception) and `npm ci` is used when it is present, so
// every build resolves the exact same tree with integrity checks. Run
// `npm install` once after changing DSH_DSH_VERSION and commit the lockfile.

import { execFileSync } from 'node:child_process'
import { existsSync, mkdirSync, writeFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = dirname(dirname(fileURLToPath(import.meta.url)))
const runtimeDir = join(root, 'src-tauri', 'resources', 'runtime')
const version = process.env.DSH_DSH_VERSION ?? '0.1.0-rc.6'
if (!/^\d+\.\d+\.\d+([-.][0-9A-Za-z.-]+)?$/.test(version)) {
  console.error(`非法版本号: ${version}`)
  process.exit(1)
}
mkdirSync(runtimeDir, { recursive: true })

const pkg = {
  name: 'dsh-desktop-runtime',
  private: true,
  version: '0.0.0',
  // npm >= 11 blocks dependency lifecycle scripts unless declared here
  // (project-scoped installs reject the user-level `allow-scripts` config,
  // error EALLOWSCRIPTS). Keep in sync with ~/.npmrc allow-scripts list.
  allowScripts: {
    '@deepseek-ai/dsh-subprocess-local': true,
    koffi: true,
    'node-pty': true,
    '@google/genai': true,
    protobufjs: true,
  },
  dependencies: { '@deepseek-ai/dsh': version },
}
writeFileSync(join(runtimeDir, 'package.json'), JSON.stringify(pkg, null, 2) + '\n')

const lockfile = join(runtimeDir, 'package-lock.json')
console.log(`Installing @deepseek-ai/dsh@${version} -> ${runtimeDir}`)
// 有锁文件时用 npm ci（严格按锁解析 + integrity 校验）；无锁（首次/换版本）用 npm install
const cmd = existsSync(lockfile) ? 'ci' : 'install'
// 外层 `npm run` 会把有效配置导出为 npm_config_* 环境变量；其中
// npm_config_allow_scripts 会让内层 npm 在 project-scoped 安装时直接报错
// EALLOWSCRIPTS（env 层优先级高于 package.json#allowScripts），故显式剔除。
// 本项目允许的安装脚本已通过 package.json#allowScripts 声明。
const env = { ...process.env }
delete env.npm_config_allow_scripts
delete env.npm_config_allow_scripts_pending
execFileSync('npm', [cmd, '--no-audit', '--no-fund', '--prefix', runtimeDir], {
  stdio: 'inherit',
  shell: process.platform === 'win32',
  env,
})

const bin = join(runtimeDir, 'node_modules', '@deepseek-ai', 'dsh', 'lib', 'bin.js')
if (!existsSync(bin)) {
  console.error('runtime install failed: dsh bin.js missing')
  process.exit(1)
}
console.log('Runtime ready.')
