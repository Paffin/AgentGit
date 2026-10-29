# AgentGit Web design

The application is a working tool for reviewing changes and directing agents.
Keep work, its state and the next action visible together. Preserve the existing
routes, permission checks, keyboard behavior and server-backed data.

## System

- Genre: modern-minimal application, with the existing AgentGit brand.
- Material: ruled records and a quiet workspace, informed by Hallmark's Furrow.
- Keep the forest-green accent and the existing logo. No marketing hero, decorative
  imagery, invented metrics or external font dependency.
- Use one capped container; Home is a filter rail and queue, Repositories a table,
  Overview a summary strip with separate work and activity regions.
- Shared tokens live in `src/styles/tokens.css`; foundational rules in
  `src/styles/base.css`. Every color and font references those tokens.

## Typography and controls

- Use the system sans face for text and controls, monospace for code and identifiers.
- Body text is 16px. Secondary text and controls use the shared 12–14px steps.
  Page headings use 28px; section headings use 20px.
- Labels and table headers use sentence case, without tracking or all caps.
- Primary buttons use the green accent with its paired foreground. Standard
  controls are 40px, compact buttons 32px; touch targets remain at least 44px.
- Focus is immediate and visible. Disabled controls retain their explanation.
  Keep existing explicit confirmations for executable and irreversible actions.

## Layout and states

- Keep page gutters consistent and cap the container at 1360px. On phones use
  16px gutters and wrap action groups without cutting off their labels.
- Use horizontal rules for table and queue rows. Avoid a card inside a card;
  reserve panel surfaces for independent work regions, menus and dialogs.
- All figures come from the server. A failed read remains an error, not zero.
- Preserve loading, empty, failed and populated states and URL-based filters.
- Dark mode uses the existing paired tokens, not inversion. Motion only explains
  state changes and obeys `prefers-reduced-motion`.

## Review workspace

Lead with the change title and the two primary review/integration actions. Keep the
four independent readiness states compact; do not put a large status card between
the title and changed code. Details, requested reviewers and secondary actions live
in a quiet side panel, after the code on phones. Preserve visible permission guidance
for unavailable actions and keep file navigation compact on narrow screens.

## Verification

Run the existing type, lint, unit and real-server browser checks after changes.
Inspect light and dark, desktop and phone. Use Hallmark's app-scope `sloplint`
with a before/kept inventory when changing composition; generated QA artifacts
stay outside version control. No route or test is removed to satisfy a redesign.
