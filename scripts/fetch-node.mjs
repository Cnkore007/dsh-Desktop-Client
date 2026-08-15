// Downloads the official Node.js binary (executable only) into
// src-tauri/resources/runtime/ so the packaged app can run the official
// `dsh web` server without a system Node.js.
//
// Supply-chain hardening (audit H10 / SC-04 / SC-11):
// - downloads SHASUMS256.txt for the same version and verifies the archive
//   sha256 before extraction (nodejs.org publishes the file next to dist);
// - pins the protocol to https (no redirect downgrade);
// - uses a fresh random temp directory (no predictable-path TOCTOU).
//
// Usage: node scripts/fetch-node.mjs [version]   (default: v24.9.0)

import { execFileSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import { copyFileSync, existsSync, mkdtempSync, mkdirSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = dirname(dirname(fileURLToPath(import.meta.url)))
const outDir = join(root, 'src-tauri', 'resources', 'runtime')
mkdirSync(outDir, { recursive: true })

const version = process.argv[2] ?? 'v24.9.0'
if (!/^v\d+\.\d+\.\d+$/.test(version)) {
  console.error(`非法版本号: ${version}`)
  process.exit(1)
}

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
const outExe = join(outDir, p.bin)

if (existsSync(outExe)) {
  console.log(`${p.bin} already present: ${outExe}`)
  process.exit(0)
}

// 独占临时目录（审计 SC-11：不再使用可预测路径）
const workDir = mkdtempSync(join(tmpdir(), 'dsh-node-'))
const archive = join(workDir, `${base}.${p.ext}`)
const shasumFile = join(workDir, 'SHASUMS256.txt')
const extractDir = join(workDir, 'extract')

const curlArgs = ['-fsSL', '--proto', '=https', '--proto-redir', '=https', '--tlsv1.2']

try {
  // 1) 下载同版本 SHASUMS256.txt（含各平台归档的 sha256）
  console.log(`Downloading checksums ${version}/SHASUMS256.txt`)
  execFileSync('curl', [...curlArgs, '-o', shasumFile, `https://nodejs.org/dist/${version}/SHASUMS256.txt`], { stdio: 'inherit' })
  const shasums = readFileSync(shasumFile, 'utf8')
  const line = shasums.split('\n').find((l) => l.trim().endsWith(`  ${base}.${p.ext}`))
  if (!line) {
    console.error(`SHASUMS256.txt 中找不到 ${base}.${p.ext} 的校验和`)
    process.exit(1)
  }
  const expected = line.trim().split(/\s+/)[0]
  if (!/^[0-9a-f]{64}$/.test(expected)) {
    console.error(`校验和格式异常: ${expected}`)
    process.exit(1)
  }

  // 2) 下载归档（协议钉死 https，禁止重定向降级）
  const url = `https://nodejs.org/dist/${version}/${base}.${p.ext}`
  console.log(`Downloading ${url}`)
  execFileSync('curl', [...curlArgs, '-o', archive, url], { stdio: 'inherit' })

  // 3) sha256 比对，失败即中止（不落盘）
  const digest = createHash('sha256').update(readFileSync(archive)).digest('hex')
  if (digest !== expected) {
    console.error(`sha256 校验失败：期望 ${expected}，实际 ${digest}（已中止，可能是下载被篡改）`)
    process.exit(1)
  }
  console.log('sha256 校验通过 ✓')

  // 4) 解压并安装
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
} finally {
  rmSync(workDir, { recursive: true, force: true })
}

execFileSync(outExe, ['--version'], { stdio: 'inherit' })
console.log(`${p.bin} -> ${outExe}`)
