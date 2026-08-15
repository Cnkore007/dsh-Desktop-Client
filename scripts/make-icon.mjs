// Generates the DSh Desktop brand icon (1024x1024 RGBA PNG, pure Node, no deps).
// Design: rounded square with an indigo→violet gradient and a white terminal
// prompt glyph (">_" chevron + cursor bar).
//
// Usage: node scripts/make-icon.mjs <out.png>

import { deflateSync } from 'node:zlib'
import { writeFileSync } from 'node:fs'

const SIZE = 1024

// ---- tiny PNG encoder -------------------------------------------------------
const CRC_TABLE = (() => {
  const t = new Uint32Array(256)
  for (let n = 0; n < 256; n++) {
    let c = n
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1
    t[n] = c >>> 0
  }
  return t
})()

function crc32(buf) {
  let c = 0xffffffff
  for (let i = 0; i < buf.length; i++) c = CRC_TABLE[(c ^ buf[i]) & 0xff] ^ (c >>> 8)
  return (c ^ 0xffffffff) >>> 0
}

function chunk(type, data) {
  const len = Buffer.alloc(4)
  len.writeUInt32BE(data.length)
  const body = Buffer.concat([Buffer.from(type, 'ascii'), data])
  const crc = Buffer.alloc(4)
  crc.writeUInt32BE(crc32(body))
  return Buffer.concat([len, body, crc])
}

function encodePNG(size, rgba) {
  const sig = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a])
  const ihdr = Buffer.alloc(13)
  ihdr.writeUInt32BE(size, 0)
  ihdr.writeUInt32BE(size, 4)
  ihdr[8] = 8 // bit depth
  ihdr[9] = 6 // color type RGBA
  const raw = Buffer.alloc(size * (1 + size * 4))
  for (let y = 0; y < size; y++) {
    const rowStart = y * (1 + size * 4)
    raw[rowStart] = 0 // filter: none
    rgba.copy(raw, rowStart + 1, y * size * 4, (y + 1) * size * 4)
  }
  return Buffer.concat([sig, chunk('IHDR', ihdr), chunk('IDAT', deflateSync(raw)), chunk('IEND', Buffer.alloc(0))])
}

// ---- geometry ---------------------------------------------------------------
const clamp = (v, a, b) => Math.max(a, Math.min(b, v))

/** Signed distance inside a rounded-rect: <=0 means inside. */
function roundedRect(x, y, x0, y0, x1, y1, r) {
  const cx = clamp(x, x0 + r, x1 - r)
  const cy = clamp(y, y0 + r, y1 - r)
  const dx = x - cx
  const dy = y - cy
  return Math.hypot(dx, dy) - r
}

/** Signed distance to a thick diagonal bar from p0 to p1 with half-width w. */
function bar(x, y, x0, y0, x1, y1, halfW) {
  const vx = x1 - x0
  const vy = y1 - y0
  const len = Math.hypot(vx, vy)
  const ux = vx / len
  const uy = vy / len
  const t = clamp((x - x0) * ux + (y - y0) * uy, 0, len)
  const px = x0 + t * ux
  const py = y0 + t * uy
  return Math.hypot(x - px, y - py) - halfW
}

// ---- render -----------------------------------------------------------------
const px = Buffer.alloc(SIZE * SIZE * 4)
const CX = SIZE / 2
const c1 = [0x1e, 0x1b, 0x4b] // indigo-950
const c2 = [0x6d, 0x28, 0xd9] // violet-700
const WHITE = [0xf8, 0xfa, 0xfc]

for (let y = 0; y < SIZE; y++) {
  const t = y / SIZE
  const bg = [
    Math.round(c1[0] + (c2[0] - c1[0]) * t),
    Math.round(c1[1] + (c2[1] - c1[1]) * t),
    Math.round(c1[2] + (c2[2] - c1[2]) * t),
  ]
  for (let x = 0; x < SIZE; x++) {
    const i = (y * SIZE + x) * 4
    const d = roundedRect(x + 0.5, y + 0.5, 64, 64, SIZE - 64, SIZE - 64, 210)
    if (d > 0) {
      px[i + 3] = 0 // transparent outside
      continue
    }
    // glyph: terminal prompt chevron ">" (two diagonal bars) + cursor bar
    const glyph = Math.min(
      bar(x, y, 350, 340, 610, 600, 44),
      bar(x, y, 610, 600, 350, 860, 44),
      bar(x, y, 350, 830, 660, 830, 44),
    )
    const a = clamp(-glyph / 2, 0, 1)
    if (a > 0) {
      px[i] = Math.round(bg[0] + (WHITE[0] - bg[0]) * a)
      px[i + 1] = Math.round(bg[1] + (WHITE[1] - bg[1]) * a)
      px[i + 2] = Math.round(bg[2] + (WHITE[2] - bg[2]) * a)
      px[i + 3] = 255
    } else {
      px[i] = bg[0]
      px[i + 1] = bg[1]
      px[i + 2] = bg[2]
      px[i + 3] = 255
    }
  }
}

const out = process.argv[2] ?? 'app-icon.png'
writeFileSync(out, encodePNG(SIZE, px))
console.log(`icon written: ${out} (${SIZE}x${SIZE})`)
