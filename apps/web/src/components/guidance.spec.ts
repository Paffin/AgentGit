import { describe, expect, it } from 'vitest'
import { mount } from '@vue/test-utils'
import type { Blocker } from '@/api/types'
import { fixesForBlocker } from '@/lib/fixes'
import { mayDo, parseRoleGrants, rolesForRepo } from '@/lib/roles'
import type { Action } from '@/lib/roles'
import Term from '@/ui/Term.vue'
import BlockerList from './BlockerList.vue'

const may = (roles: string[]) => {
  const have = rolesForRepo(parseRoleGrants(roles), 'acme/app')
  return (a: Action) => mayDo(have, a)
}
const blocker = (code: string): Blocker => ({ code, message: code, detail: {} })

describe('Term', () => {
  it('shows the plain label and puts the AgentGit word in the tooltip', () => {
    const w = mount(Term, { props: { id: 'lease', plural: true } })
    expect(w.text()).toContain('Reservations')
    expect(w.find('[role="tooltip"]').text()).toContain('“lease”')
  })

  it('in a repeated place, is a plain word with a title and no extra tab stop', () => {
    const w = mount(Term, { props: { id: 'fencing', quiet: true } })
    expect(w.find('button').exists()).toBe(false)
    expect(w.find('span').attributes('title')).toContain('fencing token')
    expect(w.text()).toBe('Reservation key')
  })
})

describe('BlockerList with fixes', () => {
  const mountWith = (roles: string[], codes: string[]) =>
    mount(BlockerList, {
      props: {
        blockers: codes.map(blocker),
        hints: true,
        plan: (code: string) => fixesForBlocker(code, { may: may(roles), isAuthor: false }),
      },
    })

  it('offers a button only for what the role may do, and says who to ask otherwise', async () => {
    const writer = mountWith(['acme/*:writer'], ['missing_validation', 'insufficient_approvals'])
    expect(writer.find('[data-testid="fix-validate"]').exists()).toBe(true)
    expect(writer.find('[data-testid="fix-review"]').exists()).toBe(false)
    expect(writer.find('[data-testid="ask-insufficient_approvals"]').text()).toMatch(/reviewer/)

    const reviewer = mountWith(['acme/*:reviewer'], ['missing_validation'])
    expect(reviewer.find('[data-testid="fix-validate"]').exists()).toBe(false)
    expect(reviewer.find('[data-testid="ask-missing_validation"]').text()).toMatch(/writer/)
  })

  it('tells the page which fix was pressed', async () => {
    const w = mountWith(['acme/*:reviewer'], ['insufficient_approvals'])
    await w.find('[data-testid="fix-review"]').trigger('click')
    expect(w.emitted('fix')?.[0]).toEqual(['review', 'insufficient_approvals'])
  })

  it('without a plan it only explains, as before', () => {
    const w = mount(BlockerList, { props: { blockers: [blocker('lease')], hints: true } })
    expect(w.find('button').exists()).toBe(false)
    expect(w.text()).toMatch(/reservation/i)
  })
})
