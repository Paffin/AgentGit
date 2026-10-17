// Deciding whether bytes are text, and turning them into lines. Never produces markup.

const SNIFF = 8000

/** Binary if a NUL byte shows up early, or if the bytes are not valid UTF-8. */
export function looksBinary(bytes: Uint8Array): boolean {
  const head = bytes.subarray(0, SNIFF)
  if (head.includes(0)) return true
  try {
    new TextDecoder('utf-8', { fatal: true }).decode(bytes)
    return false
  } catch {
    return true
  }
}

export function decodeUtf8(bytes: Uint8Array): string {
  return new TextDecoder('utf-8').decode(bytes)
}

export function splitLines(text: string): string[] {
  if (text === '') return []
  const lines = text.split('\n')
  if (lines.length > 0 && lines[lines.length - 1] === '') lines.pop()
  return lines
}

const IMAGE_TYPES: Record<string, string> = {
  png: 'image/png',
  jpg: 'image/jpeg',
  jpeg: 'image/jpeg',
  gif: 'image/gif',
  webp: 'image/webp',
}

/** Raster images only (SVG is deliberately excluded). */
export function imageMime(path: string): string | null {
  const ext = path.slice(path.lastIndexOf('.') + 1).toLowerCase()
  return IMAGE_TYPES[ext] ?? null
}

export function toDataUrl(bytes: Uint8Array, mime: string): string {
  let bin = ''
  const chunk = 0x8000
  for (let i = 0; i < bytes.length; i += chunk) {
    bin += String.fromCharCode(...bytes.subarray(i, i + chunk))
  }
  return `data:${mime};base64,${btoa(bin)}`
}

export function baseName(path: string): string {
  const i = path.lastIndexOf('/')
  return i === -1 ? path : path.slice(i + 1)
}

export function dirName(path: string): string {
  const i = path.lastIndexOf('/')
  return i === -1 ? '' : path.slice(0, i)
}
