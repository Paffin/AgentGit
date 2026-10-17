import { describe, expect, it, vi } from 'vitest'
import { mount } from '@vue/test-utils'
import { createMemoryHistory, createRouter } from 'vue-router'
import Alert from './Alert.vue'
import Avatar from './Avatar.vue'
import Badge from './Badge.vue'
import Button from './Button.vue'
import Card from './Card.vue'
import EmptyState from './EmptyState.vue'
import Icon from './Icon.vue'
import IdentityLabel from './IdentityLabel.vue'
import RelativeTime from './RelativeTime.vue'
import Skeleton from './Skeleton.vue'
import Spinner from './Spinner.vue'
import { ICONS } from './icons'

const router = createRouter({
  history: createMemoryHistory(),
  routes: [
    { path: '/', component: { template: '<i/>' } },
    { path: '/x', component: { template: '<i/>' } },
  ],
})

describe('Icon', () => {
  it('is decorative by default and named when it has a label', () => {
    const decorative = mount(Icon, { props: { name: 'check' } })
    expect(decorative.attributes('aria-hidden')).toBe('true')
    expect(decorative.attributes('role')).toBeUndefined()
    const named = mount(Icon, { props: { name: 'check', label: 'Done' } })
    expect(named.attributes('role')).toBe('img')
    expect(named.attributes('aria-label')).toBe('Done')
    expect(named.attributes('aria-hidden')).toBeUndefined()
  })
  it('draws the registered path at the requested size', () => {
    const w = mount(Icon, { props: { name: 'lock', size: 22 } })
    expect(w.find('path').attributes('d')).toBe(ICONS.lock)
    expect(w.attributes('width')).toBe('22')
  })
  it('has a well-formed path for every icon', () => {
    for (const [name, d] of Object.entries(ICONS)) {
      expect(d, name).toMatch(/^[MmLlHhVvCcSsQqAaZz0-9\s.,-]+$/)
    }
  })
})

describe('Spinner', () => {
  it('announces its label to assistive tech only', () => {
    const w = mount(Spinner, { props: { label: 'Loading proposals' } })
    expect(w.attributes('role')).toBe('status')
    expect(w.find('.sr-only').text()).toBe('Loading proposals')
  })
})

describe('Button', () => {
  it('renders a real button that emits click', async () => {
    const w = mount(Button, { slots: { default: 'Save' } })
    expect(w.element.tagName).toBe('BUTTON')
    expect(w.attributes('type')).toBe('button')
    await w.trigger('click')
    expect(w.emitted('click')).toHaveLength(1)
  })
  it('does nothing while disabled or loading, and says so to assistive tech', async () => {
    const disabled = mount(Button, {
      props: { disabled: true, disabledReason: 'Requires the admin role' },
      slots: { default: 'Delete' },
    })
    await disabled.trigger('click')
    expect(disabled.emitted('click')).toBeUndefined()
    expect(disabled.attributes('aria-disabled')).toBe('true')
    expect(disabled.attributes('title')).toBe('Requires the admin role')
    expect(disabled.find('.sr-only').text()).toBe('Requires the admin role')
    expect(disabled.attributes('aria-describedby')).toBe(disabled.find('.sr-only').attributes('id'))

    const loading = mount(Button, { props: { loading: true }, slots: { default: 'Go' } })
    await loading.trigger('click')
    expect(loading.emitted('click')).toBeUndefined()
    expect(loading.attributes('aria-busy')).toBe('true')
  })
  it('stays focusable when disabled so the reason can be read', () => {
    const w = mount(Button, {
      props: { disabled: true, disabledReason: 'why' },
      slots: { default: 'x' },
    })
    expect(w.attributes('disabled')).toBeUndefined()
  })
  it('icon-only buttons carry their accessible name', () => {
    const w = mount(Button, { props: { icon: 'x', label: 'Close' } })
    expect(w.attributes('aria-label')).toBe('Close')
    expect(w.classes()).toContain('icon-only')
  })
  it('can be a link', async () => {
    const w = mount(Button, {
      props: { to: '/x' },
      slots: { default: 'Go' },
      global: { plugins: [router] },
    })
    expect(w.element.tagName).toBe('A')
    expect(w.attributes('href')).toBe('/x')
  })
})

describe('Badge', () => {
  it('renders its slot with the tone class', () => {
    const w = mount(Badge, {
      props: { tone: 'warn', icon: 'alert', dot: true },
      slots: { default: 'Quiet' },
    })
    expect(w.text()).toBe('Quiet')
    expect(w.classes()).toContain('t-warn')
    expect(w.find('.dot').exists()).toBe(true)
    expect(w.findComponent(Icon).exists()).toBe(true)
  })
})

describe('Card', () => {
  it('names its region by its title and renders header, body and footer', () => {
    const w = mount(Card, {
      props: { title: 'Agent runs', subtitle: '2 active', icon: 'bot' },
      slots: { default: '<p>body</p>', footer: 'foot', actions: '<button>act</button>' },
    })
    const section = w.find('section')
    const heading = w.find('h2')
    expect(heading.text()).toContain('Agent runs')
    expect(section.attributes('aria-labelledby')).toBe(heading.attributes('id'))
    expect(w.text()).toContain('2 active')
    expect(w.text()).toContain('foot')
    expect(w.find('button').text()).toBe('act')
  })
  it('honours the heading level and skips an empty header', () => {
    expect(
      mount(Card, { props: { title: 't', level: 3 } })
        .find('h3')
        .exists(),
    ).toBe(true)
    expect(
      mount(Card, { slots: { default: 'x' } })
        .find('header')
        .exists(),
    ).toBe(false)
  })
})

describe('Skeleton / EmptyState / Alert', () => {
  it('skeletons are hidden from assistive tech and keep their size', () => {
    const w = mount(Skeleton, { props: { height: 20, width: 120, lines: 3 } })
    expect(w.attributes('aria-hidden')).toBe('true')
    expect(w.findAll('.skel')).toHaveLength(3)
    expect(w.findAll('.skel')[0]!.attributes('style')).toContain('height: 20px')
    expect(w.findAll('.skel')[2]!.attributes('style')).toContain('width: 60%')
  })
  it('empty states explain themselves and offer actions', () => {
    const w = mount(EmptyState, {
      props: { title: 'No open proposals', description: 'Nothing yet' },
      slots: { default: '<button>New</button>' },
    })
    expect(w.text()).toContain('No open proposals')
    expect(w.text()).toContain('Nothing yet')
    expect(w.find('button').exists()).toBe(true)
  })
  it('alerts only interrupt when asked to', () => {
    expect(mount(Alert, { slots: { default: 'x' } }).attributes('role')).toBeUndefined()
    expect(
      mount(Alert, {
        props: { live: 'alert', tone: 'bad', title: 'Oops' },
        slots: { default: 'x' },
      }).attributes('role'),
    ).toBe('alert')
    expect(
      mount(Alert, { props: { live: 'status' }, slots: { default: 'x' } }).attributes('role'),
    ).toBe('status')
  })
})

describe('Avatar / IdentityLabel', () => {
  it('tells people, agents, services and the system apart by shape, not just colour', () => {
    const shapes = (id: string) => {
      const w = mount(Avatar, { props: { identity: id } })
      return {
        circle: w.find('circle').exists(),
        path: w.find('path.shape').exists(),
        rect: w.find('rect.shape').exists(),
        text: w.find('text').exists(),
        label: w.attributes('aria-label'),
      }
    }
    expect(shapes('user:alice')).toMatchObject({
      circle: true,
      text: true,
      label: 'user: user:alice',
    })
    expect(shapes('agent:refactor-bot')).toMatchObject({ path: true, circle: false, text: false })
    expect(shapes('service:ci')).toMatchObject({ rect: true, text: false })
    const system = mount(Avatar, { props: { identity: 'system:recovery' } })
    expect(system.find('path.shape').attributes('d')).not.toBe(
      mount(Avatar, { props: { identity: 'agent:x' } })
        .find('path.shape')
        .attributes('d'),
    )
  })
  it('shows initials for people', () => {
    expect(
      mount(Avatar, { props: { identity: 'user:alice-smith' } })
        .find('text')
        .text(),
    ).toBe('AS')
    expect(
      mount(Avatar, { props: { identity: 'user:bob' } })
        .find('text')
        .text(),
    ).toBe('BO')
  })
  it('is stable: the same identity always gets the same hue', () => {
    const hue = (id: string) => mount(Avatar, { props: { identity: id } }).attributes('style')
    expect(hue('user:alice')).toBe(hue('user:alice'))
  })
  it('can be decorative', () => {
    const w = mount(Avatar, { props: { identity: 'user:a', decorative: true } })
    expect(w.attributes('aria-hidden')).toBe('true')
    expect(w.find('title').exists()).toBe(false)
  })
  it('labels the identity with its name and, for agents, the run', () => {
    const w = mount(IdentityLabel, { props: { identity: 'agent:refactor-bot#run_8f78a1b2c3' } })
    expect(w.find('.name').text()).toBe('refactor-bot')
    expect(w.find('.run').text()).toBe('#8f78a1')
    expect(w.attributes('title')).toBe('agent:refactor-bot#run_8f78a1b2c3')
    expect(w.find('.sr-only').text()).toBe('agent')
    expect(
      mount(IdentityLabel, { props: { identity: 'agent:x#run_123456789', hideRun: true } })
        .find('.run')
        .exists(),
    ).toBe(false)
    expect(
      mount(IdentityLabel, { props: { identity: 'user:alice', showKind: true } })
        .find('.name')
        .text(),
    ).toBe('user:alice')
  })
  it('renders hostile names as text', () => {
    const w = mount(IdentityLabel, { props: { identity: 'user:<img src=x onerror=alert(1)>' } })
    expect(w.find('img').exists()).toBe(false)
    expect(w.find('.name').text()).toContain('<img')
  })
})

describe('RelativeTime', () => {
  it('shows a relative time with the exact instant as a tooltip', () => {
    vi.useFakeTimers()
    vi.setSystemTime(new Date('2026-10-17T12:00:00Z'))
    const w = mount(RelativeTime, { props: { ms: Date.parse('2026-10-17T11:55:00Z') } })
    expect(w.text()).toBe('5m ago')
    expect(w.attributes('datetime')).toBe('2026-10-17T11:55:00.000Z')
    expect(w.attributes('title')).toBe('2026-10-17T11:55:00.000Z')
    w.unmount()
  })
})
