// Writes a Brotli (`x.br`) and a gzip (`x.gz`) copy next to every compressible file of the build, so that
// `ag-server` (embedded or from a directory) can answer with the pre-compressed bytes and never spends CPU on
// compression at request time. A variant is kept only when it is smaller than the original. Node's own zlib is
// used, so the build needs nothing extra.
import { brotliCompressSync, constants, gzipSync } from 'node:zlib'
import { readdirSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs'
import { extname, join, resolve } from 'node:path'

const root = resolve(process.argv[2] ?? join(import.meta.dirname, '..', 'dist'))
const compressible = new Set([
  '.html',
  '.js',
  '.mjs',
  '.css',
  '.json',
  '.svg',
  '.txt',
  '.map',
  '.webmanifest',
])
const MIN_BYTES = 256

function* walk(dir) {
  for (const name of readdirSync(dir)) {
    const path = join(dir, name)
    if (statSync(path).isDirectory()) yield* walk(path)
    else yield path
  }
}

let files = 0
let raw = 0
let brotli = 0
for (const path of walk(root)) {
  // Start clean: a stale variant of a file that changed must never be served.
  if (path.endsWith('.br') || path.endsWith('.gz')) {
    rmSync(path)
    continue
  }
  if (!compressible.has(extname(path))) continue
  const data = readFileSync(path)
  if (data.length < MIN_BYTES) continue
  const br = brotliCompressSync(data, {
    params: {
      [constants.BROTLI_PARAM_QUALITY]: constants.BROTLI_MAX_QUALITY,
      [constants.BROTLI_PARAM_SIZE_HINT]: data.length,
    },
  })
  const gz = gzipSync(data, { level: 9 })
  if (br.length < data.length) writeFileSync(`${path}.br`, br)
  if (gz.length < data.length) writeFileSync(`${path}.gz`, gz)
  files += 1
  raw += data.length
  brotli += Math.min(br.length, data.length)
}
process.stdout.write(
  `precompress: ${files} files, ${(raw / 1024).toFixed(0)} KiB -> ${(brotli / 1024).toFixed(0)} KiB brotli\n`,
)
