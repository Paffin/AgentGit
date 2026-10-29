// @vitest-environment node
import { describe, expect, it } from 'vitest'
import { analyzePolicy } from './policy'
import {
  PRESETS,
  applyPreset,
  describeChange,
  describePolicy,
  detectPreset,
  docFromForm,
  formFromDoc,
  parseDoc,
  stringifyDoc,
} from './policy-plain'

describe('presets', () => {
  it('are the four the roadmap names, each valid for the editor’s own checks', () => {
    expect(PRESETS.map((p) => p.id)).toEqual(['solo', 'team', 'agents', 'strict'])
    for (const p of PRESETS) {
      const errors = analyzePolicy(stringifyDoc(p.doc)).filter((i) => i.level === 'error')
      expect(errors, p.id).toEqual([])
    }
  })

  it('keep the checks, trusted runners and who-may lists a branch already has', () => {
    const base = {
      trusted_issuers: [{ identity: 'service:ci', public_key: 'ab'.repeat(32) }],
      required_validations: [{ check: 'tests', issuers: ['service:ci'] }],
      integrators: ['user:carol'],
      approvals: { min: 5 },
    }
    const doc = applyPreset('team', base)
    expect(doc.trusted_issuers).toEqual(base.trusted_issuers)
    expect(doc.required_validations).toEqual(base.required_validations)
    expect(doc.integrators).toEqual(['user:carol'])
    expect((doc.approvals as { min: number }).min).toBe(1)
  })

  it('are recognised again, and a hand-edited document is custom', () => {
    for (const p of PRESETS)
      expect(detectPreset(applyPreset(p.id, { admins: ['user:x'] }))).toBe(p.id)
    expect(detectPreset({ approvals: { min: 7 } })).toBeNull()
  })

  it('differ in what matters: solo needs no review, strict needs two, only guardrails lets agents land', () => {
    const f = (id: 'solo' | 'team' | 'agents' | 'strict') => formFromDoc(applyPreset(id))
    expect(f('solo').approvalsMin).toBe(0)
    expect(f('team').approvalsMin).toBe(1)
    expect(f('strict').approvalsMin).toBe(2)
    expect(PRESETS.filter((p) => formFromDoc(p.doc).agentsMayLand).map((p) => p.id)).toEqual([
      'agents',
    ])
    expect(f('agents').requireProvenance).toBe(true)
  })
})

describe('form <-> document', () => {
  it('keeps protected approvers when a path changes and edits check timing without replacing the runner', () => {
    const base = {
      protected_paths: [{ glob: 'src/old/**', min_approvals: 2, approvers: ['user:security-*'] }],
      required_validations: [
        {
          check: 'tests',
          issuers: ['service:ci'],
          max_age_secs: 600,
          run: { argv: ['cargo', 'test'], timeout_secs: 300, memory_mb: 2048, network: false },
        },
      ],
    }
    const form = formFromDoc(base)
    form.protectedPaths[0]!.glob = 'src/new/**'
    expect(docFromForm(form, base).protected_paths).toEqual([
      { glob: 'src/new/**', min_approvals: 2, approvers: ['user:security-*'] },
    ])
    form.checkTiming[0]!.maxAgeSecs = '900'
    form.checkTiming[0]!.timeoutSecs = '120'
    expect(
      describeChange(base, docFromForm(form, base))
        .map((effect) => effect.text)
        .join(' '),
    ).toContain('maximum result age 900s, command timeout 120s')
    expect(docFromForm(form, base).required_validations).toEqual([
      {
        ...base.required_validations[0],
        max_age_secs: 900,
        run: { ...base.required_validations[0]!.run, timeout_secs: 120 },
      },
    ])
  })

  it('clears timing limits without creating a command for external checks', () => {
    const base = {
      required_validations: [
        { check: 'local', max_age_secs: 60, run: { argv: ['true'], timeout_secs: 30 } },
        { check: 'external', max_age_secs: 120 },
      ],
    }
    const form = formFromDoc(base)
    form.checkTiming[0]!.maxAgeSecs = ''
    form.checkTiming[0]!.timeoutSecs = ''
    expect(form.checkTiming[1]!.timeoutSecs).toBeNull()
    expect(docFromForm(form, base).required_validations).toEqual([
      { check: 'local', run: { argv: ['true'] } },
      base.required_validations[1],
    ])
  })

  it('round-trips a preset and keeps fields the form does not show', () => {
    const base = {
      ...applyPreset('agents'),
      trusted_issuers: [{ identity: 'service:ci', public_key: 'cd'.repeat(32) }],
      protected_paths: [{ glob: '.github/**', min_approvals: 2, approvers: ['user:admin*'] }],
      some_future_field: { keep: true },
    }
    const form = formFromDoc(base)
    const back = docFromForm({ ...form, approvalsMin: 2 }, base)
    expect((back.approvals as { min: number }).min).toBe(2)
    expect(back.some_future_field).toEqual({ keep: true })
    expect(back.trusted_issuers).toEqual(base.trusted_issuers)
    // a protected path keeps its own approvers while its number is edited
    expect(back.protected_paths).toEqual([
      { glob: '.github/**', min_approvals: 2, approvers: ['user:admin*'] },
    ])
  })

  it('drops what is switched off instead of writing zeros that would still mean something', () => {
    const base = applyPreset('agents')
    const form = formFromDoc(base)
    const off = docFromForm(
      {
        ...form,
        agentsMayLand: false,
        requireProvenance: false,
        protectedPaths: [],
        maxChangedFiles: 0,
      },
      base,
    )
    expect(off.agent_autonomy).toBeUndefined()
    expect(off.require_provenance_for_agents).toBeUndefined()
    expect(off.protected_paths).toBeUndefined()
    expect(off.max_changed_files).toBeUndefined()
  })

  it('reads the server’s defaults for absent fields: self-approval is forbidden, agents need one approval', () => {
    const f = formFromDoc({})
    expect(f.forbidSelfApproval).toBe(true)
    expect(f.approvalsMinForAgents).toBe(1)
    expect(f.approvalsMin).toBe(0)
  })

  it('parses only objects', () => {
    expect(parseDoc('[]')).toBeNull()
    expect(parseDoc('{')).toBeNull()
    expect(parseDoc('{"a":1}')).toEqual({ a: 1 })
  })
})

describe('describePolicy', () => {
  it('says what the default and a strict document mean in sentences', () => {
    const d = describePolicy({})
    expect(d.join(' ')).toMatch(/Any person may land a change/)
    expect(d.join(' ')).toMatch(/No checks are required/)
    expect(d.join(' ')).toMatch(/Agents cannot land changes by themselves/)
    const s = describePolicy(applyPreset('strict')).join(' ')
    expect(s).toMatch(/needs 2 approvals/)
    expect(s).toMatch(/\.github\/\*\* need 2 approvals/)
    expect(s).toMatch(/at most 200 files/)
  })

  it('names the limits an agent may land within', () => {
    expect(describePolicy(applyPreset('agents')).join(' ')).toMatch(
      /Agents may land a change by themselves if it touches only docs\/\*\* and tests\/\*\* and at most 10 files/,
    )
  })
})

describe('describeChange', () => {
  it('turning on agent autonomy allows; raising approvals asks for more', () => {
    const e = describeChange(applyPreset('team'), applyPreset('agents'))
    expect(e.find((x) => /Agents may now land/.test(x.text))?.kind).toBe('allows')
    expect(e.find((x) => /signed record/.test(x.text))?.kind).toBe('denies')
    expect(e.find((x) => /\.github\/\*\* now need 2/.test(x.text))?.kind).toBe('denies')
    const s = describeChange(applyPreset('team'), applyPreset('strict'))
    expect(s.find((x) => /Landing now needs 2 approvals \(was 1\)/.test(x.text))?.kind).toBe(
      'denies',
    )
  })

  it('going from strict to solo allows more and says so per setting', () => {
    const e = describeChange(applyPreset('strict'), applyPreset('solo'))
    expect(e.every((x) => x.kind === 'allows')).toBe(true)
    expect(e.map((x) => x.text).join(' ')).toMatch(/no approval/)
    expect(e.map((x) => x.text).join(' ')).toMatch(/no longer a limit/)
  })

  it('reports checks added and removed, and nothing when nothing changed', () => {
    const a = { required_validations: [{ check: 'tests' }] }
    const b = { required_validations: [{ check: 'lint' }] }
    const e = describeChange(a, b)
    expect(e).toEqual([
      { kind: 'denies', text: 'Landing now also needs these checks to pass: lint.' },
      { kind: 'allows', text: 'These checks are no longer required: tests.' },
    ])
    expect(describeChange(applyPreset('team'), applyPreset('team'))).toEqual([])
  })

  it('treats an absent self-approval flag as the server does (forbidden)', () => {
    const e = describeChange(
      { approvals: { min: 1 } },
      { approvals: { min: 1, forbid_self_approval: false } },
    )
    expect(e).toEqual([{ kind: 'allows', text: 'Authors may now approve their own changes.' }])
  })
})
