// Is this copy of the UI the one that belongs to the server it talks to? The UI is built from the same workspace
// version as the server (a Rust test keeps `package.json` equal to it); the build writes it next to the app as
// `ui-version.json`, and the server reports its own in `GET /v1/capabilities` (`server_version`).

/** The protocol this UI speaks. A server that selected another one cannot be driven by it. */
export const SUPPORTED_PROTOCOL = 'agwp.v1'

export interface Semver {
  major: number
  minor: number
  patch: number
  pre: string
}

/** `1.2.3`, `1.2.3-rc.1`, `1.2.3+build` (build metadata ignored). Anything else is not a version. */
export function parseVersion(text: string): Semver | null {
  const m = /^v?(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?(?:\+[0-9A-Za-z.-]+)?$/.exec(text.trim())
  if (!m) return null
  return { major: Number(m[1]), minor: Number(m[2]), patch: Number(m[3]), pre: m[4] ?? '' }
}

export type Fit =
  /** Same version. */
  | { kind: 'same' }
  /** Different, but only in the patch level or a pre-release tag: compatible, nothing to say. */
  | { kind: 'compatible' }
  /** A different major or minor version: the screens may not match the API. */
  | { kind: 'mismatch'; ui: string; server: string; uiNewer: boolean }
  /** One of the two is not a version we can read: say nothing rather than guess. */
  | { kind: 'unknown' }

export function compareBuilds(ui: string | null, server: string | null): Fit {
  const a = ui ? parseVersion(ui) : null
  const b = server ? parseVersion(server) : null
  if (!a || !b || !ui || !server) return { kind: 'unknown' }
  if (a.major === b.major && a.minor === b.minor) {
    return a.patch === b.patch && a.pre === b.pre ? { kind: 'same' } : { kind: 'compatible' }
  }
  const uiNewer = a.major !== b.major ? a.major > b.major : a.minor > b.minor
  return { kind: 'mismatch', ui, server, uiNewer }
}

/** Whether the server selected a protocol this UI does not speak (an empty answer says nothing). */
export function protocolMismatch(selected: string | null | undefined): string | null {
  return selected && selected !== SUPPORTED_PROTOCOL ? selected : null
}
