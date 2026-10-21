# Handoff: Web UI for a first-time user

Written 2026-10-21. Review and rationale: `docs/design/web-ux-review.md`. User-facing description: `docs/guides/web-ui.md`.

## What was built (all in `apps/web`)

| Slice | Where |
| --- | --- |
| Home ("needs your attention") and navigation by job | `views/HomeView.vue`, `composables/useAttention.ts`, `lib/attention.ts`; `views/RepoLayout.vue` (Overview, Change requests, Agents & work, Code + sub-row, Settings); `/` is Home, `/repos` the list |
| Plain labels + glossary tooltips | `lib/glossary.ts` (the only source), `ui/Term.vue`, `Card term=`; table printed in the guide, checked by `tests/glossary-docs.spec.ts` |
| Blockers with their fix as an action | `lib/fixes.ts` (role-aware), `components/BlockerList.vue` (`plan`, `fix` event), `views/ProposalView.vue` (`doFix`, `?do=` deep links, "Next:" line from `lib/nextstep.ts`, visible reasons for disabled buttons) |
| Guided flows | `views/NewRepoView.vue` (wizard), `components/GettingStarted.vue`, `components/agents/GiveAgentWork.vue` (task, grant role + mint credential, snippet, watches for the run/proposal), review/land from Home |
| Rules editor with presets | `lib/policy-plain.ts`, `components/admin/PolicyForm.vue`; raw JSON under "Advanced" in `PolicyEditor.vue` |
| Edit and propose from the browser, conflict resolver | `lib/edit.ts`, `components/files/FileEditor.vue`, `components/proposal/ConflictResolver.vue`; API wrappers `api.workspaces.*`, `api.proposals.update` |
| Empty states | board, agents, sessions, reservations, Home, repository list say what to do next |

The server API already supported all of it (virtual workspaces, `PATCH /proposals/{id}`, roles, credentials): no server change.

## Tests

* Unit: `src/lib/guidance.spec.ts`, `policy-plain.spec.ts`, `edit-next.spec.ts`, `src/components/guidance.spec.ts`, `tests/glossary-docs.spec.ts`.
* End to end (real server, real `ag`): `e2e/jobs.spec.ts` — a fresh user does each of the five jobs from what the screen says, plus edit/propose, conflict resolution, rules presets, empty states. `screens.ts` (axe + responsive) has Home, the wizard and the getting-started screen.
* Old specs were updated for the new words (Land, Change requests, Overview, Settings, Branches) and for Home being the landing page.

## Decisions worth knowing

* Home reads the 30 newest repositories and re-reads every 15 s (one event stream per repository would be too many). It never invents: a proposal whose preparation could not be read is skipped.
* "Run the check" is a button on the page, not on Home: arriving on a page never starts work (`?do=validate` is ignored).
* A preset keeps `trusted_issuers`, `required_validations`, `integrators`, `admins`, `allowed_authors`. The form shows required checks but adding one stays under Advanced (needs a trusted key).
* The server's default for an absent `forbid_self_approval` is true; the form and sentences follow it.
* The draft workspace of an edit is removed after the change request is opened.

## Not done / next

* No Firefox/WebKit run of the new specs locally (CI runs them); tablet/phone projects were run for layout and axe specs only where noted in the report.
* Tooltips (`Term`) are used on card titles and state pills; tab labels, table headers and some dialog text carry the plain word without a tooltip.
* Editing: one text file per change request; no rename/delete/multi-file; no editing of a conflict that is not text.
* Home has no live updates and no per-user "dismiss".
* "Agents waiting" is shown as a mark on agent-authored rows, not as its own section.
* Policy form: protected-path approvers, `approvers`, `max_age_secs`, environment digests stay in the JSON.
* Not tested with a screen reader.
