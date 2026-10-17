// Documentation cannot drift from the code: every ```ts block in the READMEs and in the SDK guide is a
// verbatim copy of a region of a program under examples/ (which examples.test.ts runs), announced by
//   <!-- example: sdk/typescript/examples/quickstart.ts#quickstart -->

import { existsSync, readFileSync } from 'node:fs';
import path from 'node:path';
import { describe, expect, it } from 'vitest';
import { REPO_ROOT } from './harness/server.js';

function regionText(file: string, name: string): string {
  const lines = readFileSync(path.join(REPO_ROOT, file), 'utf8').split('\n');
  const start = lines.findIndex((l) => l.trim() === `// #region ${name}`);
  const end = lines.findIndex((l) => l.trim() === `// #endregion ${name}`);
  if (start < 0 || end < start) throw new Error(`${file}: no region "${name}"`);
  const body = lines.slice(start + 1, end);
  const indent = Math.min(...body.filter((l) => l.trim()).map((l) => /^ */.exec(l)![0].length));
  return body.map((l) => l.slice(indent)).join('\n');
}

interface Block {
  doc: string;
  line: number;
  lang: string;
  marker: string | undefined;
  text: string;
}

function blocks(doc: string): Block[] {
  const lines = readFileSync(path.join(REPO_ROOT, doc), 'utf8').split('\n');
  const out: Block[] = [];
  for (let i = 0; i < lines.length; i++) {
    const open = /^```(\w+)\s*$/.exec(lines[i]!);
    if (!open) continue;
    let j = i + 1;
    while (j < lines.length && !/^```\s*$/.test(lines[j]!)) j++;
    let k = i - 1;
    while (k >= 0 && lines[k]!.trim() === '') k--;
    const marker = /^<!-- example: (\S+) -->$/.exec(lines[k] ?? '')?.[1];
    out.push({ doc, line: i + 1, lang: open[1]!, marker, text: lines.slice(i + 1, j).join('\n') });
    i = j;
  }
  return out;
}

const DOCS = ['sdk/typescript/README.md', 'docs/guides/sdks.md'];
const TS = new Set(['ts', 'typescript']);

describe('documentation samples', () => {
  for (const doc of DOCS) {
    it(`${doc}: every TypeScript block is a verbatim region of an example`, () => {
      expect(existsSync(path.join(REPO_ROOT, doc)), `${doc} exists`).toBe(true);
      const ts = blocks(doc).filter((b) => TS.has(b.lang));
      for (const b of ts) {
        expect(b.marker, `${doc}:${b.line} needs an <!-- example: path#region --> marker`).toBeDefined();
        const [file, region] = b.marker!.split('#');
        expect(file, `${doc}:${b.line}`).toMatch(/^sdk\/typescript\/examples\/[\w-]+\.ts$/);
        expect(b.text, `${doc}:${b.line} differs from ${b.marker}`).toBe(regionText(file!, region!));
      }
      if (doc === 'sdk/typescript/README.md') expect(ts.length).toBeGreaterThanOrEqual(6);
    });
  }

  it('the other languages in the guide are shell or output only', () => {
    for (const b of blocks('docs/guides/sdks.md')) {
      expect(
        ['ts', 'typescript', 'python', 'bash', 'sh', 'text', 'json', 'toml'],
        `${b.doc}:${b.line} ${b.lang}`,
      ).toContain(b.lang);
    }
  });

  it('every example region quoted somewhere exists', () => {
    for (const doc of DOCS) {
      for (const b of blocks(doc)) {
        if (b.marker?.startsWith('sdk/typescript/')) {
          const [file, region] = b.marker.split('#');
          expect(() => regionText(file!, region!)).not.toThrow();
        }
      }
    }
  });
});
