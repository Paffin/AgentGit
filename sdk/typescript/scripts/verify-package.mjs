#!/usr/bin/env node
// Checks the built package the way a consumer would use it (run `npm run build` first):
// the ESM and CJS entry points load and agree, the declarations compile for ESM and CJS consumers
// under Node's own resolution rules, nothing from node_modules is imported at runtime, and the
// tarball contains what it should.

import { execFileSync } from 'node:child_process';
import { createRequire } from 'node:module';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const pkg = JSON.parse(readFileSync(path.join(root, 'package.json'), 'utf8'));
const failures = [];
// one entry per operation of the OpenAPI document (the number grows with the API; it is not written down here)
const spec = JSON.parse(readFileSync(path.join(root, '../../docs/protocol/openapi.json'), 'utf8'));
const operationCount = Object.values(spec.paths).flatMap((item) => Object.values(item)).length;
const check = (ok, what) => {
  console.log(`${ok ? 'ok  ' : 'FAIL'} ${what}`);
  if (!ok) failures.push(what);
};

// 1. both entry points load and give the same answers
const esm = await import(pathToFileURL(path.join(root, 'dist/index.js')).href);
const cjs = createRequire(import.meta.url)(path.join(root, 'dist/index.cjs'));
for (const [name, m] of [
  ['ESM', esm],
  ['CJS', cjs],
]) {
  check(typeof m.AgentGitClient === 'function', `${name}: AgentGitClient is exported`);
  check(
    m.blobId('hello\n') === '6fdfa63ccd7aa33065dcfab8c0f7b1b605226c87833b50de3345f921122a4545',
    `${name}: blobId matches the conformance vector`,
  );
  const key = m.generateAgentKey();
  const record = { changeset: 'ab'.repeat(32), agent: 'agent:bot', created_at_ms: 1 };
  check(
    m.verifyProvenance(record, m.signProvenance(record, key.privateKey), key.publicKey),
    `${name}: provenance signatures verify`,
  );
  check(Object.keys(m.OPERATIONS).length === operationCount, `${name}: ${operationCount} operations`);
  check(
    new m.AgentGitClient({ baseUrl: 'http://127.0.0.1:1' }).proposals.create instanceof Function,
    `${name}: namespaced methods exist`,
  );
}
check(esm.AgentGitError !== undefined && cjs.AgentGitError !== undefined, 'AgentGitError is exported by both');

// 2. no runtime dependencies, and nothing is imported from node_modules or node:
check(Object.keys(pkg.dependencies ?? {}).length === 0, 'package.json has no dependencies');
for (const f of ['dist/index.js', 'dist/index.cjs']) {
  const text = readFileSync(path.join(root, f), 'utf8');
  check(!/(from|require\()\s*["'](@noble|node:)/.test(text), `${f} imports nothing from @noble or node:`);
}

// 3. declarations compile for consumers of both module formats
const tmp = mkdtempSync(path.join(tmpdir(), 'agentgit-ts-consumer-'));
try {
  mkdirSync(path.join(tmp, 'node_modules/@agentgit'), { recursive: true });
  symlinkSync(root, path.join(tmp, 'node_modules/@agentgit/client'), 'dir');
  const source = `
import { AgentGitClient, AgentGitError, blobId, signProvenance, type Req, type Res } from '@agentgit/client';
const c = new AgentGitClient({ baseUrl: 'http://x', token: 't' });
export async function f(): Promise<string> {
  const p: Res<'proposal.create'> = await c.proposals.create({ repo: 'a/b', stream: 'main', head: blobId('x'), title: 't' });
  const r: Req<'proposal.review'> = { repo: 'a/b', id: p.id, decision: 'approve' };
  await c.proposals.review(r);
  try { await c.repos.describe({ repo: 'a/b' }); } catch (e) { if (e instanceof AgentGitError) return e.code + String(e.retryable) + e.retryAfterMs; }
  // @ts-expect-error - id is required
  await c.proposals.get({ repo: 'a/b' });
  return signProvenance.name;
}
`;
  writeFileSync(path.join(tmp, 'consumer.mts'), source);
  writeFileSync(path.join(tmp, 'consumer.cts'), source); // same source, resolved through the `require` condition
  writeFileSync(
    path.join(tmp, 'tsconfig.json'),
    JSON.stringify({
      compilerOptions: {
        module: 'node16',
        moduleResolution: 'node16',
        target: 'es2022',
        strict: true,
        noEmit: true,
        skipLibCheck: false,
        lib: ['es2023', 'dom'],
        types: [],
      },
      files: ['consumer.mts', 'consumer.cts'],
    }),
  );
  try {
    execFileSync(path.join(root, 'node_modules/.bin/tsc'), ['-p', tmp], { stdio: 'pipe' });
    check(true, 'type declarations compile for ESM (.mts) and CJS (.cts) consumers, with skipLibCheck off');
  } catch (e) {
    check(false, `type declarations compile:\n${e.stdout?.toString() ?? e}`);
  }
} finally {
  rmSync(tmp, { recursive: true, force: true });
}

// 4. the tarball
const packed = JSON.parse(
  execFileSync('npm', ['pack', '--dry-run', '--json', '--ignore-scripts'], {
    cwd: root,
    stdio: ['ignore', 'pipe', 'ignore'],
  }).toString(),
);
const files = packed[0].files.map((f) => f.path).sort();
check(
  files.includes('dist/index.js') &&
    files.includes('dist/index.cjs') &&
    files.includes('dist/index.d.ts') &&
    files.includes('README.md') &&
    files.includes('LICENSE'),
  'tarball has dist, README and LICENSE',
);
check(
  !files.some((f) => f.startsWith('src/') || f.startsWith('test/') || f.startsWith('node_modules/')),
  'tarball has no sources, tests or node_modules',
);
check(
  !('postinstall' in (pkg.scripts ?? {})) &&
    !('preinstall' in (pkg.scripts ?? {})) &&
    !('install' in (pkg.scripts ?? {})),
  'no install scripts',
);

if (failures.length > 0) {
  console.error(`\n${failures.length} check(s) failed`);
  process.exit(1);
}
console.log('\npackage verified');
