import { describe, expect, it } from 'vitest'
import {
  ADMIN_TOKEN_VAR,
  TOKEN_SLOT,
  connectSteps,
  directoryFor,
  shellQuote,
  subjectProblem,
} from './onboarding'

const input = { origin: 'http://127.0.0.1:8080', repo: 'acme/webapp', subject: 'agent:my-agent' }

describe('connect steps', () => {
  it('builds the three steps with this server and repository in them', () => {
    const steps = connectSteps(input)
    expect(steps?.map((s) => s.id)).toEqual(['role', 'credential', 'work'])
    expect(steps?.map((s) => s.admin)).toEqual([true, true, false])
    const [role, credential, work] = steps ?? []
    expect(role?.lines[0]).toContain("'http://127.0.0.1:8080/v1/roles'")
    expect(role?.lines[0]).toContain(`$${ADMIN_TOKEN_VAR}`)
    expect(role?.lines[0]).toContain(
      `-d '{"subject":"agent:my-agent","repo":"acme/webapp","role":"writer"}'`,
    )
    expect(credential?.lines[0]).toContain(
      `-d '{"subject":"agent:my-agent","ttl_secs":3600,"scope":{"repos":["acme/webapp"]}}'`,
    )
    expect(work?.lines).toEqual([
      `export AG_TOKEN=${TOKEN_SLOT}`,
      "ag clone 'http://127.0.0.1:8080/acme/webapp' 'webapp'",
      "cd 'webapp'",
      'ag remote whoami',
      'echo hi > hello.txt',
      'ag snapshot -m "First change"',
      'ag push -t "First change"',
    ])
  })

  it('ignores a trailing slash of the origin', () => {
    expect(connectSteps({ ...input, origin: 'https://ag.example.com/' })?.[0]?.lines[0]).toContain(
      "'https://ag.example.com/v1/roles'",
    )
  })

  it('refuses what could not be put safely into a shell line', () => {
    for (const subject of [
      "agent:x'; rm -rf ~; '",
      'agent:',
      'bot',
      'agent:a b',
      'agent:$(id)',
      '',
    ]) {
      expect(subjectProblem(subject), subject).not.toBeNull()
      expect(connectSteps({ ...input, subject }), subject).toBeNull()
    }
    expect(connectSteps({ ...input, repo: "a/b'c" })).toBeNull()
    expect(connectSteps({ ...input, repo: 'nowhere' })).toBeNull()
    expect(connectSteps({ ...input, origin: "http://x'y" })).toBeNull()
    expect(subjectProblem('agent:ci.bot-2_x@corp')).toBeNull()
  })

  it('quotes for a POSIX shell', () => {
    expect(shellQuote("it's")).toBe(`'it'\\''s'`)
    expect(directoryFor('acme/webapp')).toBe('webapp')
  })
})
