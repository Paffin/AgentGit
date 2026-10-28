import { describe, expect, it } from 'vitest'
import { mount } from '@vue/test-utils'
import { nextTick } from 'vue'
import type { Component } from 'vue'
import { ApiError } from '@/api/errors'
import DataTable from './DataTable.vue'
import ErrorState from './ErrorState.vue'
import KeyValue from './KeyValue.vue'
import LoadMore from './LoadMore.vue'
import Tabs from './Tabs.vue'
import Timeline from './Timeline.vue'
import VirtualList from './VirtualList.vue'
import type { Column, KvItem, TabItem } from './types'

// Generic components lose their type parameters when mounted from a test; the props are still checked at runtime.
const DataTableC = DataTable as unknown as Component
const VirtualListC = VirtualList as unknown as Component
const TimelineC = Timeline as unknown as Component
const TabsC = Tabs as unknown as Component

describe('Tabs', () => {
  const tabs: TabItem<'a' | 'b' | 'c'>[] = [
    { id: 'a', label: 'Changes', count: 3 },
    { id: 'b', label: 'Reviews' },
    { id: 'c', label: 'Timeline', disabled: true },
  ]
  const make = (modelValue: 'a' | 'b' | 'c' = 'a') =>
    mount(TabsC, {
      props: { modelValue, tabs, label: 'Proposal details' },
      slots: { a: '<p>panel A</p>', b: '<p>panel B</p>' },
      attachTo: document.body,
    })

  it('follows the ARIA tabs pattern', () => {
    const w = make()
    expect(w.find('[role="tablist"]').attributes('aria-label')).toBe('Proposal details')
    const t = w.findAll('[role="tab"]')
    expect(t.map((x) => x.attributes('aria-selected'))).toEqual(['true', 'false', 'false'])
    expect(t.map((x) => x.attributes('tabindex'))).toEqual(['0', '-1', '-1'])
    const panel = w.find('[role="tabpanel"]')
    expect(panel.attributes('aria-labelledby')).toBe(t[0]!.attributes('id'))
    expect(t[0]!.attributes('aria-controls')).toBe(panel.attributes('id'))
    expect(panel.text()).toBe('panel A')
    expect(t[0]!.text()).toContain('3')
    w.unmount()
  })
  it('selects on click and moves with arrow keys, skipping disabled tabs and wrapping', async () => {
    const w = make()
    await w.findAll('[role="tab"]')[1]!.trigger('click')
    expect(w.emitted('update:modelValue')![0]).toEqual(['b'])
    await w.findAll('[role="tab"]')[0]!.trigger('keydown', { key: 'ArrowRight' })
    expect(w.emitted('update:modelValue')![1]).toEqual(['b'])
    await w.findAll('[role="tab"]')[1]!.trigger('keydown', { key: 'ArrowRight' })
    expect(w.emitted('update:modelValue')![2]).toEqual(['a'])
    await w.findAll('[role="tab"]')[0]!.trigger('keydown', { key: 'End' })
    expect(w.emitted('update:modelValue')![3]).toEqual(['b'])
    await w.findAll('[role="tab"]')[0]!.trigger('keydown', { key: 'Home' })
    expect(w.emitted('update:modelValue')![4]).toEqual(['a'])
    w.unmount()
  })
  it('renders only the active panel', async () => {
    const w = make('b')
    expect(w.find('[role="tabpanel"]').text()).toBe('panel B')
    w.unmount()
  })
})

describe('DataTable', () => {
  interface Row {
    id: string
    name: string
    n: number
  }
  const columns: Column[] = [
    { key: 'name', label: 'Name' },
    { key: 'n', label: 'Count', align: 'end' },
  ]
  const rows: Row[] = [
    { id: '1', name: 'alpha', n: 3 },
    { id: '2', name: '<b>beta</b>', n: 5 },
  ]
  it('is a real table with a caption, column headers and a cell per column', () => {
    const w = mount(DataTableC, {
      props: { columns, rows, rowKey: (r: Row) => r.id, caption: 'Things' },
    })
    expect(w.find('caption').text()).toBe('Things')
    expect(w.findAll('th').map((x) => x.text())).toEqual(['Name', 'Count'])
    expect(w.findAll('tbody tr')).toHaveLength(2)
    expect(w.findAll('th').every((x) => x.attributes('scope') === 'col')).toBe(true)
  })
  it('shows server text as text', () => {
    const w = mount(DataTableC, {
      props: { columns, rows, rowKey: (r: Row) => r.id, caption: 'Things' },
    })
    expect(w.find('b').exists()).toBe(false)
    expect(w.text()).toContain('<b>beta</b>')
  })
  it('renders custom cells through slots', () => {
    const w = mount(DataTableC, {
      props: { columns, rows, rowKey: (r: Row) => r.id, caption: 'Things' },
      slots: {
        'cell-name': `<template #cell-name="{ row }"><a href="#">{{ row.name.toUpperCase() }}</a></template>`,
      },
    })
    expect(w.find('a').text()).toBe('ALPHA')
  })
  it('keeps its height while loading (skeleton rows) and marks itself busy', () => {
    const w = mount(DataTableC, {
      props: {
        columns,
        rows: [] as Row[],
        rowKey: (r: Row) => r.id,
        caption: 'Things',
        loading: true,
        skeletonRows: 3,
      },
    })
    expect(w.findAll('tbody tr')).toHaveLength(3)
    expect(w.find('table').attributes('aria-busy')).toBe('true')
    expect(w.find('.empty').exists()).toBe(false)
  })
  it('says what is missing when empty, not while loading', () => {
    const w = mount(DataTableC, {
      props: {
        columns,
        rows: [] as Row[],
        rowKey: (r: Row) => r.id,
        caption: 'Things',
        emptyText: 'No things.',
      },
    })
    expect(w.find('.empty').text()).toBe('No things.')
  })
  it('highlights the selected row', () => {
    const w = mount(DataTableC, {
      props: { columns, rows, rowKey: (r: Row) => r.id, caption: 'Things', selectedKey: '2' },
    })
    expect(w.findAll('tbody tr')[1]!.classes()).toContain('selected')
  })
  it('can hide an action column header visually but keep it for screen readers', () => {
    const w = mount(DataTableC, {
      props: {
        columns: [...columns, { key: 'x', label: 'Actions', srOnlyHeader: true }],
        rows,
        rowKey: (r: Row) => r.id,
        caption: 'Things',
      },
    })
    expect(w.findAll('th')[2]!.classes()).toContain('sr-only')
  })
})

describe('VirtualList', () => {
  it('renders only the rows near the viewport', async () => {
    const items = Array.from({ length: 5000 }, (_, i) => ({ id: i }))
    const w = mount(VirtualListC, {
      props: {
        items,
        rowHeight: 40,
        itemKey: (x: { id: number }) => x.id,
        height: '400px',
        label: 'Events',
        overscan: 2,
      },
      slots: { default: `<template #default="{ item }">row {{ item.id }}</template>` },
      attachTo: document.body,
    })
    await nextTick()
    const rendered = w.findAll('[role="listitem"]')
    expect(rendered.length).toBeLessThan(30)
    expect(rendered[0]!.attributes('aria-posinset')).toBe('1')
    expect(rendered[0]!.attributes('aria-setsize')).toBe('5000')
    expect(w.find('.space').attributes('style')).toContain('height: 200000px')
    const scroller = w.find('[role="list"]')
    expect(scroller.attributes('tabindex')).toBe('0')
    expect(scroller.attributes('aria-label')).toBe('Events')
    ;(scroller.element as HTMLElement).scrollTop = 40 * 1000
    await scroller.trigger('scroll')
    ;(scroller.element as HTMLElement).scrollTop = 40 * 1001
    await scroller.trigger('scroll')
    await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()))
    await nextTick()
    const after = w.findAll('[role="listitem"]')
    expect(after[0]!.text()).toBe('row 999')
    expect(after.length).toBeLessThan(30)
    w.unmount()
  })
})

describe('Timeline / KeyValue / LoadMore', () => {
  it('timeline is an ordered, named list', () => {
    const w = mount(TimelineC, {
      props: {
        items: [
          { id: 1, t: 'a' },
          { id: 2, t: 'b' },
        ],
        label: 'Events',
        tone: (i: { id: number }) => (i.id === 1 ? 'ok' : 'warn'),
      },
      slots: { default: `<template #default="{ item }">{{ item.t }}</template>` },
    })
    expect(w.find('ol').attributes('aria-label')).toBe('Events')
    expect(w.findAll('li')).toHaveLength(2)
    expect(w.findAll('li')[0]!.classes()).toContain('t-ok')
    expect(w.findAll('li')[1]!.text()).toBe('b')
  })
  it('key-value is a description list; empty values show a dash', () => {
    const items: KvItem[] = [
      { key: 'a', label: 'Name', value: 'x' },
      { key: 'b', label: 'Empty', value: '' },
      { key: 'c', label: 'Custom' },
    ]
    const w = mount(KeyValue, { props: { items }, slots: { 'value-c': '<em>custom</em>' } })
    expect(w.findAll('dt').map((d) => d.text())).toEqual(['Name', 'Empty', 'Custom'])
    expect(w.findAll('dd').map((d) => d.text())).toEqual(['x', '–', 'custom'])
  })
  it('load-more shows progress and only offers more when there is more', async () => {
    const w = mount(LoadMore, { props: { hasMore: true, shown: 50, noun: 'changesets' } })
    expect(w.text()).toContain('50 changesets so far')
    await w.find('button').trigger('click')
    expect(w.emitted('more')).toHaveLength(1)
    const done = mount(LoadMore, { props: { hasMore: false, shown: 7, noun: 'changesets' } })
    expect(done.find('button').exists()).toBe(false)
    expect(done.text()).toBe('7 changesets')
  })
})

describe('ErrorState', () => {
  const api = (code: string, status: number, message = 'm') =>
    new ApiError({ status, code, message, requestId: 'req_7' })
  it('says plainly what is wrong for each class of failure', () => {
    const net = mount(ErrorState, { props: { error: api('network_error', 0) } })
    expect(net.text()).toContain('Cannot reach the server')
    const forbidden = mount(ErrorState, {
      props: { error: api('permission_denied', 403, 'role missing') },
    })
    expect(forbidden.text()).toContain('You do not have access')
    expect(forbidden.text()).toContain('role missing')
    expect(forbidden.find('button').exists()).toBe(false) // retrying a 403 is pointless
    const missing = mount(ErrorState, {
      props: { error: api('not_found', 404), what: 'this proposal' },
    })
    expect(missing.text()).toContain('Could not find this proposal')
    const other = mount(ErrorState, {
      props: { error: api('internal', 500, 'kaput'), what: 'the board' },
    })
    expect(other.text()).toContain('Could not load the board')
    expect(other.text()).toContain('kaput')
  })
  it('quotes the request id and lets the user retry', async () => {
    const w = mount(ErrorState, { props: { error: api('internal', 500) } })
    expect(w.text()).toContain('request req_7')
    expect(w.attributes('role')).toBe('alert')
    await w.find('button').trigger('click')
    expect(w.emitted('retry')).toHaveLength(1)
  })
  it('handles errors that are not ApiErrors', () => {
    expect(mount(ErrorState, { props: { error: new Error('weird') } }).text()).toContain('weird')
  })
})
