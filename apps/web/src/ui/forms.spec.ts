import { describe, expect, it, vi } from 'vitest'
import { mount } from '@vue/test-utils'
import Checkbox from './Checkbox.vue'
import CopyButton from './CopyButton.vue'
import Segmented from './Segmented.vue'
import SelectField from './SelectField.vue'
import TextArea from './TextArea.vue'
import TextField from './TextField.vue'

describe('TextField', () => {
  it('associates its label with the input and updates v-model', async () => {
    const w = mount(TextField, { props: { modelValue: '', label: 'Name' } })
    const input = w.find('input')
    expect(w.find('label').attributes('for')).toBe(input.attributes('id'))
    await input.setValue('acme/web')
    expect(w.emitted('update:modelValue')![0]).toEqual(['acme/web'])
  })
  it('wires hints and errors for assistive tech', () => {
    const w = mount(TextField, {
      props: { modelValue: 'x', label: 'Key', hint: 'Hex only', error: 'Too short' },
    })
    const input = w.find('input')
    expect(input.attributes('aria-invalid')).toBe('true')
    const ids = input.attributes('aria-describedby')!.split(' ')
    expect(ids).toHaveLength(2)
    expect(w.find(`#${CSS.escape(ids[0]!)}`).text()).toBe('Hex only')
    expect(w.find('[role="alert"]').text()).toBe('Too short')
  })
  it('can hide the label visually without removing it', () => {
    const w = mount(TextField, { props: { modelValue: '', label: 'Search', hideLabel: true } })
    expect(w.find('label').classes()).toContain('sr-only')
  })
  it('marks required fields and never autocompletes secrets', () => {
    const w = mount(TextField, {
      props: { modelValue: '', label: 'Token', required: true, type: 'password' },
    })
    expect(w.find('input').attributes('required')).toBeDefined()
    expect(w.find('input').attributes('autocomplete')).toBe('off')
    expect(w.find('input').attributes('spellcheck')).toBe('false')
  })
  it('lets a dialog focus it first', () => {
    expect(
      mount(TextField, { props: { modelValue: '', label: 'a', autofocus: true } })
        .find('input')
        .attributes('data-autofocus'),
    ).toBeDefined()
  })
})

describe('TextArea / SelectField / Checkbox', () => {
  it('textarea: label, v-model, error', async () => {
    const w = mount(TextArea, { props: { modelValue: '', label: 'Comment', error: 'Required' } })
    expect(w.find('label').attributes('for')).toBe(w.find('textarea').attributes('id'))
    await w.find('textarea').setValue('hello')
    expect(w.emitted('update:modelValue')![0]).toEqual(['hello'])
    expect(w.find('textarea').attributes('aria-invalid')).toBe('true')
  })
  it('select: renders options and emits the chosen value', async () => {
    const w = mount(SelectField, {
      props: {
        modelValue: 'a',
        label: 'Role',
        options: [
          { value: 'a', label: 'Alpha' },
          { value: 'b', label: 'Beta' },
        ],
      },
    })
    expect(w.findAll('option').map((o) => o.text())).toEqual(['Alpha', 'Beta'])
    await w.find('select').setValue('b')
    expect(w.emitted('update:modelValue')![0]).toEqual(['b'])
  })
  it('checkbox: toggles and describes itself', async () => {
    const w = mount(Checkbox, {
      props: { modelValue: false, label: 'Include revoked', hint: 'Shows old ones' },
    })
    expect(w.find('label').attributes('for')).toBe(w.find('input').attributes('id'))
    await w.find('input').setValue(true)
    expect(w.emitted('update:modelValue')![0]).toEqual([true])
    expect(w.text()).toContain('Shows old ones')
  })
})

describe('Segmented', () => {
  const options = [
    { value: 'approve', label: 'Approve' },
    { value: 'comment', label: 'Comment' },
  ] as const
  it('is a labelled radio group', async () => {
    const w = mount(Segmented, { props: { modelValue: 'approve', legend: 'Decision', options } })
    expect(w.find('fieldset legend').text()).toBe('Decision')
    const radios = w.findAll('input[type="radio"]')
    expect(radios).toHaveLength(2)
    expect((radios[0]!.element as HTMLInputElement).checked).toBe(true)
    expect(new Set(radios.map((r) => r.attributes('name'))).size).toBe(1)
    await radios[1]!.setValue(true)
    expect(w.emitted('update:modelValue')![0]).toEqual(['comment'])
  })
})

describe('CopyButton', () => {
  it('copies, confirms visibly and to assistive tech, then resets', async () => {
    vi.useFakeTimers()
    const writeText = vi.fn().mockResolvedValue(undefined)
    Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true })
    const w = mount(CopyButton, { props: { text: 'agt_secret', label: 'Copy token' } })
    expect(w.find('button').attributes('aria-label')).toBe('Copy token')
    await w.find('button').trigger('click')
    await vi.advanceTimersByTimeAsync(0)
    expect(writeText).toHaveBeenCalledWith('agt_secret')
    expect(w.find('[role="status"]').text()).toBe('Copied to clipboard')
    await vi.advanceTimersByTimeAsync(2000)
    expect(w.find('[role="status"]').text()).toBe('')
  })
  it('reports failure when nothing can copy', async () => {
    vi.useFakeTimers()
    Object.defineProperty(navigator, 'clipboard', {
      value: { writeText: vi.fn().mockRejectedValue(new Error('denied')) },
      configurable: true,
    })
    ;(document as unknown as { execCommand: () => boolean }).execCommand = () => false
    const w = mount(CopyButton, { props: { text: 'x' } })
    await w.find('button').trigger('click')
    await vi.advanceTimersByTimeAsync(0)
    expect(w.find('[role="status"]').text()).toBe('Copy failed')
  })
})
