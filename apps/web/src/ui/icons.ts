// The icon set: 24×24 outline glyphs drawn as SVG path data (stroke 1.75, round caps). They are constants in
// this file; nothing from the server ever reaches an icon.

const circle = (cx: number, cy: number, r: number): string =>
  `M${cx - r} ${cy}a${r} ${r} 0 1 0 ${2 * r} 0a${r} ${r} 0 1 0 ${-2 * r} 0`

const rect = (x: number, y: number, w: number, h: number, r = 2): string =>
  `M${x + r} ${y}h${w - 2 * r}a${r} ${r} 0 0 1 ${r} ${r}v${h - 2 * r}a${r} ${r} 0 0 1 ${-r} ${r}h${-(w - 2 * r)}a${r} ${r} 0 0 1 ${-r} ${-r}v${-(h - 2 * r)}a${r} ${r} 0 0 1 ${r} ${-r}z`

export const ICONS = {
  check: 'M5 12.5l4.5 4.5L19 7.5',
  x: 'M6 6l12 12M18 6L6 18',
  alert: 'M12 4L3 19.5h18L12 4zM12 10v4.5M12 17.2v.01',
  info: `${circle(12, 12, 9)}M12 11v5.5M12 7.8v.01`,
  clock: `${circle(12, 12, 9)}M12 7v5l3.5 2`,
  'chevron-right': 'M9 6l6 6-6 6',
  'chevron-down': 'M6 9l6 6 6-6',
  'chevron-up': 'M6 15l6-6 6 6',
  'chevron-left': 'M15 6l-6 6 6 6',
  plus: 'M12 5v14M5 12h14',
  minus: 'M5 12h14',
  search: `${circle(10.5, 10.5, 6.5)}M15.5 15.5L20 20`,
  branch: `${circle(6, 5.5, 2.2)}${circle(6, 18.5, 2.2)}${circle(18, 8, 2.2)}M6 7.7v8.6M18 10.2c0 4-12 2.5-12 6.1`,
  merge: `${circle(6, 5.5, 2.2)}${circle(6, 18.5, 2.2)}${circle(18, 15, 2.2)}M6 7.7v8.6M6 7.7c0 5 12 2 12 5.1`,
  commit: `${circle(12, 12, 3.5)}M3 12h5.5M15.5 12H21`,
  proposal: `${circle(6, 5.5, 2.2)}${circle(6, 18.5, 2.2)}${circle(18, 18.5, 2.2)}M6 7.7v8.6M18 16.3V9.5a3 3 0 0 0-3-3h-3.5M13.8 4.2l-2.3 2.3 2.3 2.3`,
  lock: `${rect(5, 11, 14, 9.5, 2)}M8 11V8a4 4 0 0 1 8 0v3`,
  unlock: `${rect(5, 11, 14, 9.5, 2)}M8 11V8a4 4 0 0 1 7.6-1.8`,
  key: `${circle(8, 15.5, 4)}M10.9 12.6L20 3.5M17 6.5l3 3M14.5 9l2.5 2.5`,
  user: `${circle(12, 8.5, 3.8)}M4.5 20c0-4 3.4-6.5 7.5-6.5s7.5 2.5 7.5 6.5`,
  bot: `${rect(5, 8, 14, 11, 3)}M12 8V5${circle(12, 3.8, 1.1)}${circle(9.3, 13.2, 1.1)}${circle(14.7, 13.2, 1.1)}M9.5 16.5h5`,
  server: `${rect(4, 4, 16, 6.5, 2)}${rect(4, 13.5, 16, 6.5, 2)}M8 7.2v.01M8 16.8v.01`,
  system: 'M12 3l9 9-9 9-9-9z',
  list: 'M8.5 6h11.5M8.5 12h11.5M8.5 18h11.5M4 6v.01M4 12v.01M4 18v.01',
  grid: `${rect(4, 4, 6.5, 6.5, 1.5)}${rect(13.5, 4, 6.5, 6.5, 1.5)}${rect(4, 13.5, 6.5, 6.5, 1.5)}${rect(13.5, 13.5, 6.5, 6.5, 1.5)}`,
  activity: 'M3 12h4l3-8 4 16 3-8h4',
  bolt: 'M13 3L5 13.5h6L10 21l8-10.5h-6L13 3z',
  eye: `M2.5 12S6 5.5 12 5.5 21.5 12 21.5 12 18 18.5 12 18.5 2.5 12 2.5 12z${circle(12, 12, 3)}`,
  'eye-off': `M2.5 12S6 5.5 12 5.5 21.5 12 21.5 12 18 18.5 12 18.5 2.5 12 2.5 12z${circle(12, 12, 3)}M4 4l16 16`,
  copy: `${rect(9, 9, 11, 11, 2)}M5 15V6a2 2 0 0 1 2-2h8`,
  external: 'M14 4h6v6M20 4l-9 9M18 14v4a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h4',
  trash: 'M4 7h16M10 7V4.5h4V7M6.5 7l1 13h9l1-13M10 11v6M14 11v6',
  edit: 'M4 20l1-4L16.5 4.5a2 2 0 0 1 3 3L8 19l-4 1z',
  play: 'M7 4.5l12 7.5-12 7.5z',
  pause: 'M8 5v14M16 5v14',
  refresh:
    'M20 11a8 8 0 0 0-14.5-3.5L4 9.5M4 5v4.5h4.5M4 13a8 8 0 0 0 14.5 3.5l1.5-2M20 19v-4.5h-4.5',
  sun: `${circle(12, 12, 4)}M12 2.5v2.5M12 19v2.5M2.5 12H5M19 12h2.5M5.3 5.3l1.8 1.8M16.9 16.9l1.8 1.8M5.3 18.7l1.8-1.8M16.9 7.1l1.8-1.8`,
  moon: 'M20 14.5A8 8 0 0 1 9.5 4 8 8 0 1 0 20 14.5z',
  monitor: `${rect(3, 4, 18, 12, 2)}M8 20h8M12 16v4`,
  shield: 'M12 3l8 3v6c0 4.5-3.2 7.8-8 9-4.8-1.2-8-4.5-8-9V6l8-3zM8.8 12l2.4 2.4 4.2-4.6',
  file: 'M6 3h8l5 5v11a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2zM14 3v5h5',
  folder: 'M3 7a2 2 0 0 1 2-2h4l2 2.5h8a2 2 0 0 1 2 2V18a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V7z',
  download: 'M12 4v11M7.5 10.5L12 15l4.5-4.5M5 20h14',
  filter: 'M4 5h16l-6 8v5.5l-4 1.5V13L4 5z',
  logout: 'M10 4H6a2 2 0 0 0-2 2v12a2 2 0 0 0 2 2h4M15 8l4 4-4 4M19 12H9',
  home: 'M4 11l8-7 8 7M6 10v9a1 1 0 0 0 1 1h3.5v-5h3V20H17a1 1 0 0 0 1-1v-9',
  task: `${rect(4, 4, 16, 16, 3)}M8.5 12.2l2.6 2.6 4.6-5`,
  flag: 'M5 21V4M5 4h11l-2 4 2 4H5',
  link: 'M10 14a4 4 0 0 0 5.7 0l3-3a4 4 0 0 0-5.7-5.7l-1 1M14 10a4 4 0 0 0-5.7 0l-3 3a4 4 0 0 0 5.7 5.7l1-1',
  history: 'M4 12a8 8 0 1 0 2.5-5.8L4 8.5M4 4v4.5h4.5M12 8v4.5l3 2',
  pin: 'M9 3h6l-1 6 3.5 3.5V14h-11v-1.5L10 9 9 3zM12 14v7',
  database:
    'M4 6c0-1.7 3.6-3 8-3s8 1.3 8 3-3.6 3-8 3-8-1.3-8-3zM4 6v12c0 1.7 3.6 3 8 3s8-1.3 8-3V6M4 12c0 1.7 3.6 3 8 3s8-1.3 8-3',
  terminal: `${rect(3, 4, 18, 16, 2.5)}M7.5 9.5l3 2.5-3 2.5M13 15h4`,
  sparkle:
    'M12 3l1.8 5.2L19 10l-5.2 1.8L12 17l-1.8-5.2L5 10l5.2-1.8L12 3zM19 16l.8 2.2L22 19l-2.2.8L19 22l-.8-2.2L16 19l2.2-.8L19 16z',
  circle: circle(12, 12, 8),
  dot: circle(12, 12, 3),
  'circle-dot': `${circle(12, 12, 8.5)}${circle(12, 12, 2.5)}`,
  'arrow-right': 'M5 12h14M13 6l6 6-6 6',
  'arrow-down': 'M12 5v14M6 13l6 6 6-6',
  menu: 'M4 7h16M4 12h16M4 17h16',
  sliders: `M5 7h7M18 7h1${circle(15, 7, 2)}M5 17h1M12 17h7${circle(9, 17, 2)}`,
  comment:
    'M4 5.5A1.5 1.5 0 0 1 5.5 4h13A1.5 1.5 0 0 1 20 5.5v9a1.5 1.5 0 0 1-1.5 1.5H10l-5 4v-4H5.5A1.5 1.5 0 0 1 4 14.5v-9z',
  diff: 'M12 4.5v8M8 8.5h8M8 18.5h8',
  stack: 'M4 7.5l8-4 8 4-8 4-8-4zM4 12l8 4 8-4M4 16.5l8 4 8-4',
  wifi: 'M3 9a14 14 0 0 1 18 0M6 12.5a9.5 9.5 0 0 1 12 0M9 16a5 5 0 0 1 6 0M12 19.5v.01',
  'wifi-off':
    'M3 9a14 14 0 0 1 4-2.5M21 9a14 14 0 0 0-6-3.2M6 12.5a9.5 9.5 0 0 1 3-1.8M18 12.5a9.5 9.5 0 0 0-2.5-1.6M9 16a5 5 0 0 1 6 0M12 19.5v.01M4 4l16 16',
  heart: 'M12 20s-7.5-4.6-7.5-10A4.3 4.3 0 0 1 12 7.6 4.3 4.3 0 0 1 19.5 10c0 5.4-7.5 10-7.5 10z',
  layers: 'M12 3.5l9 5-9 5-9-5 9-5zM3 14l9 5 9-5',
  tree: 'M12 3v18M12 8H7a2 2 0 0 0-2 2v2M12 14h5a2 2 0 0 1 2 2v1',
} as const satisfies Record<string, string>

export type IconName = keyof typeof ICONS
