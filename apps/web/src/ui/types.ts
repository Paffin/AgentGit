import type { IconName } from './icons'

export interface TabItem<I extends string = string> {
  id: I
  label: string
  count?: number | string
  icon?: IconName
  disabled?: boolean
}

export interface MenuItem {
  id: string
  label: string
  icon?: IconName
  danger?: boolean
  disabled?: boolean
}

export interface KvItem {
  key: string
  label: string
  value?: string | number | null
  mono?: boolean
}

export interface Column {
  /** Explanation of a domain term without adding a tab stop. */
  tooltip?: string
  key: string
  label: string
  /** CSS width, e.g. `120px` or `2fr`. */
  width?: string
  align?: 'start' | 'end' | 'center'
  /** Hide the column below this viewport width (px). */
  hideBelow?: number
  mono?: boolean
  /** Visually hide the header (action columns); still read by assistive tech. */
  srOnlyHeader?: boolean
}
