// @vitest-environment node
import { describe, expect, it } from 'vitest'
import type { Run, ServerEvent } from '@/api/types'
import { describeEvent, eventConcernsProposal, sectionsFor } from './events'
import { parseJsonWithPositions } from './jsonpos'
import { analyzePolicy, levenshtein, prettyPolicy, suggestKey, summarizePolicy } from './policy'
import { DEFAULT_STALE_AFTER_MS, isActive, runHealth } from './runs'

describe('JSON with positions', () => {
  it('records where keys are', () => {
    const r = parseJsonWithPositions('{\n  "approvals": {\n    "min": 1\n  }\n}')
    expect(r.error).toBeUndefined()
    expect(r.positions.get('approvals')).toMatchObject({ line: 2, col: 3 })
    expect(r.positions.get('approvals.min')).toMatchObject({ line: 3, col: 5 })
  })
  it('points at the exact place of a syntax error', () => {
    const r = parseJsonWithPositions('{\n  "a": 1,\n  "b": ,\n}')
    expect(r.error).toMatchObject({ line: 3 })
    expect(r.error!.message).toMatch(/expected a value/)
  })
  it('reports trailing commas, unterminated strings and trailing junk', () => {
    expect(parseJsonWithPositions('{"a":1,}').error).toBeDefined()
    expect(parseJsonWithPositions('{"a":"x').error?.message).toMatch(/not closed/)
    expect(parseJsonWithPositions('{} x').error?.message).toMatch(/after the end/)
    expect(parseJsonWithPositions('').error?.message).toMatch(/empty/)
  })
  it('finds duplicate keys', () => {
    expect(parseJsonWithPositions('{"a":1,"a":2}').duplicates.map((d) => d.path)).toEqual(['a'])
  })
  it('parses escapes, numbers and nesting', () => {
    const r = parseJsonWithPositions('{"s":"a\\n\\u0041","n":-1.5e2,"l":[true,null,{"x":[]}]}')
    expect(r.value).toEqual({ s: 'a\nA', n: -150, l: [true, null, { x: [] }] })
  })
})

describe('policy hints', () => {
  const levels = (text: string, me?: string) =>
    analyzePolicy(text, { me }).map((i) => `${i.level}:${i.path}`)

  it('accepts a sound policy', () => {
    const issues = analyzePolicy('{"approvals":{"min":1,"forbid_self_approval":true}}')
    expect(issues.every((i) => i.level === 'hint')).toBe(true)
  })

  it('names unknown fields and suggests the closest known one (the server rejects them)', () => {
    const issues = analyzePolicy('{\n  "approvals": {\n    "mim": 1\n  }\n}')
    const e = issues.find((i) => i.level === 'error')!
    expect(e.message).toContain('Unknown field “mim”')
    expect(e.message).toContain('did you mean “min”')
    expect(e.line).toBe(3)
    expect(levels('{"nope": 1}')).toEqual(['error:nope'])
  })

  it('checks types', () => {
    expect(levels('{"approvals":{"min":"one"}}')).toEqual(['error:approvals.min'])
    expect(levels('{"integrators":"user:*"}')).toEqual(['error:integrators'])
    expect(levels('{"max_changed_files":-1}')).toEqual(['error:max_changed_files'])
  })

  it('catches policies that could never be satisfied', () => {
    expect(levels('{"required_validations":[{"check":"tests"}]}')).toContain(
      'error:required_validations',
    )
    expect(levels('{"agent_autonomy":{"auto_integrate":true}}')).toContain(
      'error:agent_autonomy.auto_integrate',
    )
    expect(
      levels('{"agent_autonomy":{"auto_integrate":true,"allowed_paths":["docs/**"]}}'),
    ).toContain('warning:agent_autonomy.max_files')
  })

  it('checks issuer keys and protected paths', () => {
    expect(levels('{"trusted_issuers":[{"identity":"service:ci","public_key":"xyz"}]}')).toContain(
      'error:trusted_issuers[0].public_key',
    )
    expect(levels('{"protected_paths":[{"glob":".github/**","min_approvals":0}]}')).toContain(
      'warning:protected_paths[0].min_approvals',
    )
  })

  it('warns before the author locks themselves out', () => {
    expect(levels('{"admins":["user:root"]}', 'user:alice')).toContain('warning:admins')
    expect(levels('{"admins":["user:*"]}', 'user:alice')).not.toContain('warning:admins')
  })

  it('reports a syntax error with its line', () => {
    const [issue] = analyzePolicy('{\n "a": }')
    expect(issue).toMatchObject({ level: 'error', line: 2 })
  })

  it('suggests keys by edit distance and formats documents', () => {
    expect(levenshtein('kitten', 'sitting')).toBe(3)
    expect(suggestKey('min_for_agent', ['min', 'min_for_agents'])).toBe('min_for_agents')
    expect(suggestKey('zzzzzz', ['min'])).toBeNull()
    expect(prettyPolicy('{"a":1}')).toBe('{\n  "a": 1\n}\n')
    expect(prettyPolicy('{')).toBeNull()
  })

  it('summarizes what the board needs', () => {
    const s = summarizePolicy(
      JSON.stringify({
        approvals: { min: 1, min_for_agents: 2 },
        required_validations: [{ check: 'tests' }, { check: 'lint' }],
        agent_autonomy: { auto_integrate: true },
      }),
    )
    expect(s).toEqual({
      approvalsMin: 1,
      approvalsMinForAgents: 2,
      requiredChecks: ['tests', 'lint'],
      autoIntegrate: true,
    })
    expect(summarizePolicy('garbage').requiredChecks).toEqual([])
  })
})

const event = (topic: string, payload: Record<string, unknown>, seq = 1): ServerEvent => ({
  seq,
  topic,
  repo: 'a/b',
  timestamp_ms: 1,
  payload,
})

describe('describing events', () => {
  it('turns audit entries into sentences', () => {
    const v = describeEvent(
      event('proposal.review', {
        actor: 'user:bob',
        target: 'prop_1',
        details: { decision: 'approve' },
      }),
    )
    expect(v).toMatchObject({
      actor: 'user:bob',
      verb: 'approved',
      object: 'prop_1',
      objectKind: 'proposal',
      tone: 'ok',
    })
    const lease = describeEvent(
      event('lease.acquire', {
        actor: 'agent:bot#run_1',
        target: 'lease_1',
        details: { scope: 'src/**', exclusive: true, token: 7 },
      }),
    )
    expect(lease.verb).toContain('exclusive')
    expect(lease.object).toBe('src/**')
    expect(lease.note).toBe('token 7')
  })
  it('knows what integrated and what was blocked', () => {
    const done = describeEvent(
      event('stream.integrate', {
        actor: 'user:c',
        target: 'main',
        details: { proposal: 'prop_9', version: 4 },
      }),
    )
    expect(done).toMatchObject({ object: 'prop_9', note: 'into main (v4)', tone: 'ok' })
    const blocked = describeEvent(
      event('integrate.blocked', {
        actor: 'user:c',
        target: 'op_1',
        details: { blockers: [{ code: 'lease' }, { code: 'missing_validation' }] },
      }),
    )
    expect(blocked.tone).toBe('warn')
    expect(blocked.note).toBe('lease, missing_validation')
  })
  it('describes the execution plane', () => {
    expect(
      describeEvent(
        event('job.enqueue', {
          actor: 'user:a',
          target: 'job_1',
          details: { check: 'tests', proposal: 'prop_1' },
        }),
      ).verb,
    ).toContain('“tests”')
    expect(
      describeEvent(
        event('job.complete', {
          actor: 'service:ci',
          target: 'job_1',
          details: { result: { outcome: 'pass' } },
        }),
      ).tone,
    ).toBe('ok')
  })
  it('does not call a job without a verdict "passed"', () => {
    const done = (result: unknown) =>
      describeEvent(
        event('job.complete', { actor: 'service:ci', target: 'job_1', details: { result } }),
      )
    expect(done({ outcome: 'fail' })).toMatchObject({ tone: 'bad' })
    expect(done({ outcome: 'fail' }).verb).toContain('failed')
    const stale = done({ stale: true })
    expect(stale.tone).toBe('neutral')
    expect(stale.verb).toContain('stale')
    expect(stale.verb).not.toContain('passed')
    const none = done({})
    expect(none.tone).toBe('neutral')
    expect(none.verb).not.toMatch(/passed|failed/)
  })
  it('still renders topics it has never seen, and hostile payloads stay plain strings', () => {
    const v = describeEvent(
      event('future.thing', { actor: '<b>x</b>', target: '<script>', details: 'nope' }),
    )
    expect(v.verb).toBe('future.thing')
    expect(v.actor).toBe('<b>x</b>')
    expect(describeEvent(event('x', {})).actor).toBe('')
    expect(
      describeEvent({ seq: 1, topic: 'x', repo: '', timestamp_ms: 0, payload: {} }).refs,
    ).toEqual([])
  })
  it('flags housekeeping as minor', () => {
    expect(describeEvent(event('workspace.modify', { target: 'w' })).minor).toBe(true)
    expect(describeEvent(event('proposal.create', { target: 'prop_1' })).minor).toBe(false)
  })
  it('relates events to a proposal by id or head', () => {
    const v = describeEvent(
      event('stream.integrate', { target: 'main', details: { proposal: 'prop_9' } }),
    )
    expect(eventConcernsProposal(v, 'prop_9')).toBe(true)
    expect(eventConcernsProposal(v, 'prop_1')).toBe(false)
    const att = describeEvent(event('provenance.attach', { target: 'headhash' }))
    expect(eventConcernsProposal(att, 'prop_1', 'headhash')).toBe(true)
  })
  it('maps topics to the sections that must refresh', () => {
    expect(sectionsFor('proposal.review')).toContain('proposals')
    expect(sectionsFor('stream.integrate')).toEqual(
      expect.arrayContaining(['streams', 'prepare-all', 'queue']),
    )
    expect(sectionsFor('lease.acquire')).toEqual(expect.arrayContaining(['leases', 'predict']))
    expect(sectionsFor('job.complete')).toContain('prepare-all')
    expect(sectionsFor('gc.run')).toEqual([])
  })
})

describe('run liveness', () => {
  const run = (over: Partial<Run> = {}): Run => ({
    run_id: 'run_1',
    agent: 'bot',
    identity: 'agent:bot#run_1',
    task_id: '',
    started_at_ms: 1_000,
    finished_at_ms: 0,
    status: 'running',
    last_heartbeat_ms: 1_000,
    summary: '',
    ...over,
  })
  it('is fresh until the threshold and stale after it', () => {
    expect(runHealth(run(), 1_000 + 30_000).liveness).toBe('fresh')
    expect(runHealth(run(), 1_000 + DEFAULT_STALE_AFTER_MS).liveness).toBe('stale')
    expect(runHealth(run(), 1_000 + 31_000, 30_000)).toMatchObject({
      liveness: 'stale',
      ageMs: 31_000,
    })
  })
  it('falls back to the start time when it never heartbeated', () => {
    expect(runHealth(run({ last_heartbeat_ms: 0 }), 1_000 + 5_000).ageMs).toBe(5_000)
  })
  it('treats finished runs as finished', () => {
    expect(runHealth(run({ status: 'completed' }), 9e9).liveness).toBe('finished')
    expect(isActive(run())).toBe(true)
    expect(isActive(run({ status: 'completed' }))).toBe(false)
  })
})
