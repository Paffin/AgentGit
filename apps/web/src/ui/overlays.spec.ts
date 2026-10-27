import { afterEach, describe, expect, it, vi } from 'vitest'
import { mount } from '@vue/test-utils'
import { createPinia, setActivePinia } from 'pinia'
import { defineComponent, h, nextTick, ref } from 'vue'
import Dialog from './Dialog.vue'
import Menu from './Menu.vue'
import ToastHost from './ToastHost.vue'
import Tooltip from './Tooltip.vue'
import { useToasts } from '@/stores/toasts'

afterEach(() => {
  document.body.replaceChildren()
})

function appRoot() {
  const root = document.createElement('div')
  root.id = 'app'
  document.body.appendChild(root)
  return root
}

const Host = defineComponent({
  props: { dismissable: { type: Boolean, default: true } },
  setup(props) {
    const open = ref(false)
    return () =>
      h('div', {}, [
        h('button', { id: 'opener', onClick: () => (open.value = true) }, 'open'),
        h(
          Dialog,
          {
            modelValue: open.value,
            'onUpdate:modelValue': (v: boolean) => (open.value = v),
            title: 'Review',
            description: 'Be kind',
            dismissable: props.dismissable,
          },
          {
            default: () => [h('input', { id: 'first' }), h('button', { id: 'second' }, 'ok')],
            footer: () => h('button', { id: 'last' }, 'Done'),
          },
        ),
      ])
  },
})

describe('Dialog', () => {
  it('is a labelled, modal dialog; the page behind becomes inert', async () => {
    const root = appRoot()
    const w = mount(Host, { attachTo: root })
    await w.find('#opener').trigger('click')
    await nextTick()
    const dialog = document.body.querySelector('[role="dialog"]')!
    expect(dialog).toBeTruthy()
    expect(dialog.getAttribute('aria-modal')).toBe('true')
    expect(document.getElementById(dialog.getAttribute('aria-labelledby')!)!.textContent).toBe(
      'Review',
    )
    expect(document.getElementById(dialog.getAttribute('aria-describedby')!)!.textContent).toBe(
      'Be kind',
    )
    expect(root.hasAttribute('inert')).toBe(true)
    w.unmount()
    expect(root.hasAttribute('inert')).toBe(false)
  })

  it('moves focus in (past the close button) and returns it to the opener on close', async () => {
    const root = appRoot()
    const w = mount(Host, { attachTo: root })
    const opener = w.find('#opener').element as HTMLButtonElement
    opener.focus()
    await w.find('#opener').trigger('click')
    await nextTick()
    await nextTick()
    expect(document.activeElement?.id).toBe('first')
    document.body
      .querySelector('[role="dialog"]')!
      .dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))
    await nextTick()
    await nextTick()
    expect(document.body.querySelector('[role="dialog"]')).toBeNull()
    expect(document.activeElement).toBe(opener)
    expect(root.hasAttribute('inert')).toBe(false)
    w.unmount()
  })

  it('keeps Tab inside the dialog, wrapping both ways', async () => {
    const root = appRoot()
    const w = mount(Host, { attachTo: root })
    await w.find('#opener').trigger('click')
    await nextTick()
    await nextTick()
    const dialog = document.body.querySelector('[role="dialog"]') as HTMLElement
    const last = document.getElementById('last') as HTMLElement
    const closeBtn = dialog.querySelector('[data-dialog-close]') as HTMLElement
    last.focus()
    const tab = new KeyboardEvent('keydown', { key: 'Tab', bubbles: true, cancelable: true })
    dialog.dispatchEvent(tab)
    expect(tab.defaultPrevented).toBe(true)
    expect(document.activeElement).toBe(closeBtn)
    closeBtn.focus()
    const back = new KeyboardEvent('keydown', {
      key: 'Tab',
      shiftKey: true,
      bubbles: true,
      cancelable: true,
    })
    dialog.dispatchEvent(back)
    expect(document.activeElement).toBe(last)
    w.unmount()
  })

  it('closes from the close button and the backdrop, but not while work is in flight', async () => {
    const root = appRoot()
    const w = mount(Host, { attachTo: root })
    await w.find('#opener').trigger('click')
    await nextTick()
    ;(document.body.querySelector('[data-dialog-close]') as HTMLElement).click()
    await nextTick()
    expect(document.body.querySelector('[role="dialog"]')).toBeNull()
    w.unmount()

    const busy = mount(Host, { props: { dismissable: false }, attachTo: appRoot() })
    await busy.find('#opener').trigger('click')
    await nextTick()
    document.body
      .querySelector('[role="dialog"]')!
      .dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))
    await nextTick()
    expect(document.body.querySelector('[role="dialog"]')).toBeTruthy()
    expect(document.body.querySelector('[data-dialog-close]')).toBeNull()
    busy.unmount()
  })

  it('keeps the page locked when one of two dialogs closes', async () => {
    const root = appRoot()
    const first = mount(Dialog, { props: { modelValue: true, title: 'First' }, attachTo: root })
    const second = mount(Dialog, { props: { modelValue: true, title: 'Second' }, attachTo: root })
    await nextTick()
    expect(root.hasAttribute('inert')).toBe(true)
    await first.setProps({ modelValue: false })
    expect(root.hasAttribute('inert')).toBe(true)
    second.unmount()
    expect(root.hasAttribute('inert')).toBe(false)
    first.unmount()
  })

  it('renders hostile titles as text', async () => {
    const root = appRoot()
    const w = mount(Dialog, {
      props: { modelValue: true, title: '<img src=x onerror=alert(1)>' },
      attachTo: root,
    })
    await nextTick()
    expect(document.body.querySelector('img')).toBeNull()
    expect(document.body.querySelector('h2')!.textContent).toContain('<img')
    w.unmount()
  })
})

describe('Tooltip', () => {
  it('describes its trigger, appears on focus, and Escape dismisses it', async () => {
    const w = mount(Tooltip, {
      props: { text: 'Requires the admin role' },
      slots: { default: '<button>Delete</button>' },
      attachTo: document.body,
    })
    const button = w.find('button')
    const tip = w.find('[role="tooltip"]')
    expect(button.attributes('aria-describedby')).toBe(tip.attributes('id'))
    expect((tip.element as HTMLElement).style.display).toBe('none')
    await button.trigger('focusin')
    expect((tip.element as HTMLElement).style.display).not.toBe('none')
    expect(tip.text()).toBe('Requires the admin role')
    await button.trigger('keydown', { key: 'Escape' })
    expect((tip.element as HTMLElement).style.display).toBe('none')
    w.unmount()
  })
  it('waits a moment before appearing on hover', async () => {
    vi.useFakeTimers()
    const w = mount(Tooltip, {
      props: { text: 't', delay: 300 },
      slots: { default: '<span>hover me</span>' },
    })
    await w.trigger('mouseenter')
    expect((w.find('[role="tooltip"]').element as HTMLElement).style.display).toBe('none')
    await vi.advanceTimersByTimeAsync(310)
    expect((w.find('[role="tooltip"]').element as HTMLElement).style.display).not.toBe('none')
    await w.trigger('mouseleave')
    expect((w.find('[role="tooltip"]').element as HTMLElement).style.display).toBe('none')
  })
  it('makes a non-interactive trigger focusable so keyboard users can read it', () => {
    const w = mount(Tooltip, { props: { text: 'x' }, slots: { default: '<span>badge</span>' } })
    expect(w.attributes('tabindex')).toBe('0')
  })
})

describe('ToastHost', () => {
  it('announces toasts in a labelled region; errors interrupt, others are polite', async () => {
    setActivePinia(createPinia())
    const toasts = useToasts()
    const w = mount(ToastHost, { attachTo: document.body })
    toasts.ok('Saved')
    toasts.error('Failed', new Error('boom'))
    await nextTick()
    const region = document.body.querySelector('[aria-label="Notifications"]')!
    expect(region.getAttribute('role')).toBe('region')
    expect(region.querySelector('[role="status"]')!.textContent).toContain('Saved')
    expect(region.querySelector('[role="alert"]')!.textContent).toContain('boom')
    w.unmount()
  })
  it('can be dismissed with a button', async () => {
    setActivePinia(createPinia())
    const toasts = useToasts()
    const w = mount(ToastHost, { attachTo: document.body })
    toasts.info('Hello')
    await nextTick()
    ;(document.body.querySelector('[aria-label="Dismiss notification"]') as HTMLElement).click()
    await nextTick()
    expect(toasts.items).toHaveLength(0)
    w.unmount()
  })
})

describe('Menu', () => {
  const items = [
    { id: 'a', label: 'First' },
    { id: 'b', label: 'Second', danger: true },
    { id: 'c', label: 'Third', disabled: true },
  ]
  const make = () =>
    mount(Menu, {
      props: { label: 'Account', items },
      slots: {
        trigger: `<template #trigger="{ open, toggle }"><button aria-haspopup="menu" :aria-expanded="open" @click="toggle">menu</button></template>`,
      },
      attachTo: document.body,
    })

  it('opens, navigates with arrows, selects with Enter/click, closes with Escape and returns focus', async () => {
    const w = make()
    const trigger = w.find('button[aria-haspopup]')
    await trigger.trigger('click')
    expect(w.find('[role="menu"]').exists()).toBe(true)
    expect(w.findAll('[role="menuitem"]')).toHaveLength(3)
    ;(w.findAll('[role="menuitem"]')[0]!.element as HTMLElement).focus()
    await w.find('.menu').trigger('keydown', { key: 'ArrowDown' })
    expect((document.activeElement as HTMLElement).textContent?.trim()).toBe('Second')
    await w.find('.menu').trigger('keydown', { key: 'ArrowDown' })
    expect((document.activeElement as HTMLElement).textContent?.trim()).toBe('First') // skips the disabled item and wraps
    await w.findAll('[role="menuitem"]')[1]!.trigger('click')
    expect(w.emitted('select')![0]).toEqual(['b'])
    expect(w.find('[role="menu"]').exists()).toBe(false)
    expect(document.activeElement).toBe(trigger.element)
    w.unmount()
  })
  it('ignores disabled items and closes on Escape', async () => {
    const w = make()
    await w.find('button[aria-haspopup]').trigger('click')
    await w.findAll('[role="menuitem"]')[2]!.trigger('click')
    expect(w.emitted('select')).toBeUndefined()
    await w.find('.menu').trigger('keydown', { key: 'Escape' })
    expect(w.find('[role="menu"]').exists()).toBe(false)
    w.unmount()
  })
})
