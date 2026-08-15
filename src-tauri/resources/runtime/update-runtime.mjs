#!/usr/bin/env node
// DeepSeek Harness Desktop Client — 在线更新官方 @deepseek-ai/dsh 运行时。
//
// 原则：绝不修改官方 DeepSeek Harness 源码——本脚本只负责把官方 npm 包
// 原样安装到用户级 runtime（默认 ~/.dsh/desktop-runtime，可用 DSH_HOME 覆盖），
// 完整性通过 npm registry 的 dist.integrity（sha512）校验。
//
// 用法: node update-runtime.mjs <version>
// 流程:
//   1. 确保 npm CLI 缓存（~/.dsh/.npm-cli/，下载时校验 integrity）
//   2. 在临时目录安装 @deepseek-ai/dsh@<version>（npm install --prefix <staging>）
//   3. 校验 bin.js 存在后原子替换用户 runtime（失败不影响现有版本）

import { execFileSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import { existsSync, mkdirSync, rmSync, renameSync, writeFileSync } from 'node:fs'
import { homedir } from 'node:os'
import { join } from 'node:path'

const version = process.argv[2]
if (!/^\d+\.\d+\.\d+([-.][0-9A-Za-z.-]+)?$/.test(version ?? '')) {
  console.error(`非法版本号: ${version}`)
  process.exit(1)
}

const dshDataDir = process.env.DSH_HOME?.trim() || join(homedir(), '.dsh')
const runtimeDir = join(dshDataDir, 'desktop-runtime')
const stagingDir = join(dshDataDir, '.desktop-runtime-staging')
const npmCliDir = join(dshDataDir, '.npm-cli')

async function fetchJson(url) {
  const res = await fetch(url, { headers: { Accept: 'application/json' } })
  if (!res.ok) throw new Error(`HTTP ${res.status} ${url}`)
  return res.json()
}

// ── 1. 确保 npm CLI ────────────────────────────────────────────────────────
async function ensureNpmCli() {
  const tag = await fetchJson('https://registry.npmjs.org/-/package/npm/dist-tags')
  const npmVersion = tag.latest
  const dir = join(npmCliDir, `npm-${npmVersion}`)
  const cliJs = join(dir, 'bin', 'npm-cli.js')
  if (existsSync(cliJs)) return cliJs

  console.log(`下载 npm CLI v${npmVersion} …`)
  const meta = await fetchJson(`https://registry.npmjs.org/npm/${npmVersion}`)
  const tarballUrl = meta.dist?.tarball
  const integrity = meta.dist?.integrity
  if (!tarballUrl || !integrity) throw new Error('npm 包元数据缺少 tarball/integrity')

  const buf = Buffer.from(await (await fetch(tarballUrl)).arrayBuffer())
  const [algo, expected] = integrity.split('-')
  if (!['sha512', 'sha384', 'sha1'].includes(algo)) throw new Error(`未知完整性算法: ${algo}`)
  const digest = createHash(algo).update(buf).digest('base64')
  if (digest !== expected) throw new Error('npm CLI 完整性校验失败，已中止')

  mkdirSync(npmCliDir, { recursive: true })
  const tmp = join(npmCliDir, `npm-${npmVersion}.tmp`)
  rmSync(tmp, { recursive: true, force: true })
  writeFileSync(join(npmCliDir, `npm-${npmVersion}.tgz`), buf)
  execFileSync('tar', ['-xzf', join(npmCliDir, `npm-${npmVersion}.tgz`), '-C', npmCliDir], { stdio: 'inherit' })
  renameSync(join(npmCliDir, 'package'), tmp)
  rmSync(dir, { recursive: true, force: true })
  renameSync(tmp, dir)
  console.log(`npm CLI 就绪: ${cliJs}`)
  return cliJs
}

// ── 2. 安装到 staging ─────────────────────────────────────────────────────
async function install(version, npmCliJs) {
  console.log(`安装 @deepseek-ai/dsh@${version} …`)
  rmSync(stagingDir, { recursive: true, force: true })
  mkdirSync(stagingDir, { recursive: true })
  writeFileSync(
    join(stagingDir, 'package.json'),
    JSON.stringify(
      { name: 'dsh-desktop-runtime', private: true, version: '0.0.0', dependencies: { '@deepseek-ai/dsh': version } },
      null,
      2,
    ),
  )
  execFileSync(process.execPath, [npmCliJs, 'install', '--no-audit', '--no-fund', '--prefix', stagingDir], {
    stdio: 'inherit',
    shell: process.platform === 'win32',
  })
  const bin = join(stagingDir, 'node_modules', '@deepseek-ai', 'dsh', 'lib', 'bin.js')
  if (!existsSync(bin)) throw new Error('安装失败: dsh lib/bin.js 缺失')
}

// ── 3. 原子替换 ────────────────────────────────────────────────────────────
function activate() {
  const backup = join(dshDataDir, '.desktop-runtime-old')
  rmSync(backup, { recursive: true, force: true })
  if (existsSync(runtimeDir)) renameSync(runtimeDir, backup)
  renameSync(stagingDir, runtimeDir)
  rmSync(backup, { recursive: true, force: true })
  console.log(`已更新用户 runtime → ${runtimeDir}`)
}

try {
  const npmCliJs = await ensureNpmCli()
  await install(version, npmCliJs)
  activate()
  console.log(`@deepseek-ai/dsh@${version} 更新完成 ✓（重启服务后生效）`)
} catch (err) {
  console.error(`更新失败: ${err.message}`)
  process.exit(1)
}
