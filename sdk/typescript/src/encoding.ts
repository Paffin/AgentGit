const B64 = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/';
const B64_REV = new Int16Array(128).fill(-1);
for (let i = 0; i < B64.length; i++) B64_REV[B64.charCodeAt(i)] = i;

const encoder = new TextEncoder();
const decoder = new TextDecoder('utf-8', { fatal: true });

/** UTF-8 encode. */
export function utf8Encode(text: string): Uint8Array {
  return encoder.encode(text);
}

/** UTF-8 decode (throws on invalid input). */
export function utf8Decode(bytes: Uint8Array): string {
  return decoder.decode(bytes);
}

/** Standard base64 with padding, as used by `bytes` fields in the JSON mapping. */
export function encodeBase64(bytes: Uint8Array): string {
  let out = '';
  let i = 0;
  for (; i + 2 < bytes.length; i += 3) {
    const n = (bytes[i]! << 16) | (bytes[i + 1]! << 8) | bytes[i + 2]!;
    out += B64[(n >> 18) & 63]! + B64[(n >> 12) & 63]! + B64[(n >> 6) & 63]! + B64[n & 63]!;
  }
  const rest = bytes.length - i;
  if (rest === 1) {
    const n = bytes[i]! << 16;
    out += B64[(n >> 18) & 63]! + B64[(n >> 12) & 63]! + '==';
  } else if (rest === 2) {
    const n = (bytes[i]! << 16) | (bytes[i + 1]! << 8);
    out += B64[(n >> 18) & 63]! + B64[(n >> 12) & 63]! + B64[(n >> 6) & 63]! + '=';
  }
  return out;
}

/** Decodes standard base64 (padding optional, ASCII whitespace ignored). Throws `TypeError` on bad input. */
export function decodeBase64(text: string): Uint8Array {
  const clean = text.replace(/[\s]+/g, '').replace(/=+$/, '');
  if (clean.length % 4 === 1) throw new TypeError('invalid base64 length');
  const out = new Uint8Array(Math.floor((clean.length * 3) / 4));
  let o = 0;
  let acc = 0;
  let bits = 0;
  for (let i = 0; i < clean.length; i++) {
    const code = clean.charCodeAt(i);
    const v = code < 128 ? B64_REV[code]! : -1;
    if (v < 0) throw new TypeError(`invalid base64 character at offset ${i}`);
    acc = (acc << 6) | v;
    bits += 6;
    if (bits >= 8) {
      bits -= 8;
      out[o++] = (acc >> bits) & 0xff;
      acc &= (1 << bits) - 1;
    }
  }
  return out.subarray(0, o);
}

/** Lowercase hex. */
export function toHex(bytes: Uint8Array): string {
  let out = '';
  for (const b of bytes) out += (b < 16 ? '0' : '') + b.toString(16);
  return out;
}

/** Parses hex (either case). Throws `TypeError` on bad input. */
export function fromHex(text: string): Uint8Array {
  if (text.length % 2 !== 0 || /[^0-9a-fA-F]/.test(text)) throw new TypeError('invalid hex string');
  const out = new Uint8Array(text.length / 2);
  for (let i = 0; i < out.length; i++) out[i] = parseInt(text.slice(i * 2, i * 2 + 2), 16);
  return out;
}

/** Concatenates byte arrays. */
export function concatBytes(...parts: Uint8Array[]): Uint8Array {
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let o = 0;
  for (const p of parts) {
    out.set(p, o);
    o += p.length;
  }
  return out;
}

/** Fixed-width big-endian integer. */
export function uint(value: number | bigint, width: 1 | 2 | 8): Uint8Array {
  let v = BigInt(value);
  if (v < 0n && width !== 8) throw new RangeError('negative value');
  const out = new Uint8Array(width);
  for (let i = width - 1; i >= 0; i--) {
    out[i] = Number(v & 0xffn);
    v >>= 8n;
  }
  return out;
}

/** Signed 64-bit big-endian (two's complement). */
export function int64(value: number | bigint): Uint8Array {
  return uint(BigInt.asUintN(64, BigInt(value)), 8);
}

/** Unsigned LEB128 (minimal encoding), as used for lengths and counts in the object formats. */
export function varint(value: number | bigint): Uint8Array {
  let v = BigInt(value);
  if (v < 0n) throw new RangeError('negative varint');
  const out: number[] = [];
  for (;;) {
    const byte = Number(v & 0x7fn);
    v >>= 7n;
    if (v === 0n) {
      out.push(byte);
      return Uint8Array.from(out);
    }
    out.push(byte | 0x80);
  }
}

/** Length-prefixed UTF-8 string (`varint length || bytes`). */
export function lengthPrefixedString(text: string): Uint8Array {
  const raw = utf8Encode(text);
  return concatBytes(varint(raw.length), raw);
}

/** Random bytes from the platform CSPRNG. */
export function randomBytes(length: number): Uint8Array {
  const out = new Uint8Array(length);
  globalThis.crypto.getRandomValues(out);
  return out;
}
