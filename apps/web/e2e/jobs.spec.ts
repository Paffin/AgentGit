import { rmSync } from 'node:fs'
import type { Page } from '@playwright/test'
import { expect, test } from './fixtures.ts'
import { newShell } from './support/cli.ts'
import { expectAccessible, verdicts } from './support/ui.ts'
import {
  commitFiles,
  freshRepo,
  integrate,
  openProposal,
  policyDocument,
  setPolicy,
  suffix,
} from './support/world.ts'

// Task-based tests: a person who has not read the docs does a job using ONLY what the screen says. Each step is
// something the page offered (a button, a link, a line of text); the server and the command line are real. The API is
// used to arrange the situation (someone else's work already there), never to do the job itself.

const strip = (page: Page) => page.getByRole('list', { name: /separate counts|four independent/ })
const pill = (page: Page, key: string) => strip(page).locator(`[data-state="${key}"]`)
const item = (page: Page, title: string) =>
  page.getByTestId('attention-item').filter({ hasText: title })

test.describe('job 1: start a repository', () => {
  test('from the home page to a repository with rules and a next step', async ({
    page,
  }, testInfo) => {
    const name = `acme/start-${suffix()}`
    await page.goto('/')
    await expect(page.getByRole('heading', { name: 'Home', level: 1 })).toBeVisible()
    await page.getByTestId('home-new-repo').click()

    // 1. Name it. The page says what a name looks like.
    await expect(page.getByTestId('new-repo-steps')).toContainText('Name it')
    await expectAccessible(page, testInfo, 'new repository, step 1')
    await page.getByRole('textbox', { name: 'Name', exact: true }).fill(name)
    await page.getByRole('button', { name: 'Next: choose the rules' }).click()

    // 2. Rules: four choices, each in plain words.
    for (const id of ['solo', 'team', 'agents', 'strict'])
      await expect(page.getByTestId(`new-preset-${id}`)).toBeVisible()
    await expectAccessible(page, testInfo, 'new repository, step 2')
    await page.getByTestId('new-preset-team').click()
    await page.getByRole('button', { name: 'Next: review' }).click()

    // 3. Review says what the choice means before anything is created.
    await expect(page.getByText('What that means')).toBeVisible()
    await expect(page.getByText(/needs 1 approval of its latest version/)).toBeVisible()
    await page.getByTestId('create-repo').click()

    // The overview says what is done and what is next, with the button for it.
    const start = page.getByTestId('getting-started')
    await expect(start).toContainText('2 of 5 done')
    await expect(start.getByTestId('start-work')).toHaveText('Give an agent work')
    await expectAccessible(page, testInfo, 'overview of a new repository')
    await start.getByTestId('start-work').click()
    await expect(page.getByRole('heading', { name: 'Agents & work', level: 1 })).toBeVisible()
    await expect(page.getByTestId('give-work')).toBeVisible()
  })
})

test.describe('job 2: give an agent work', () => {
  test('describe the task, connect an agent, run what the page shows, see its change request', async ({
    page,
    cast,
    state,
  }, testInfo) => {
    test.setTimeout(120_000)
    const repo = `acme/work-${suffix()}`
    const admin = cast.as('admin')
    await admin.ok_(admin.post('/v1/repos', { name: repo }), 'create the repository')

    await page.goto(`/repos/${repo}`)
    await page.getByRole('link', { name: 'Agents & work' }).click()
    const box = page.getByTestId('give-work')
    await expect(box).toBeVisible()
    await expectAccessible(page, testInfo, 'give an agent work')

    // 1. The task, in the person's words.
    await box.getByLabel('What should the agent do?').fill('Add a greeting file')
    await box.getByTestId('create-task').click()
    await expect(box.getByTestId('task-made')).toContainText('Add a greeting file')

    // 2. The agent: the page grants the role and mints the credential itself.
    await expect(box.getByLabel('Agent name')).toHaveValue('my-agent')
    await box.getByTestId('connect-agent-go').click()
    await expect(box.getByTestId('agent-token-box')).toContainText('shown once')
    const commands = await box.getByTestId('agent-commands').innerText()
    expect(commands).toContain(`${state.baseURL}/${repo}`)
    expect(commands).toMatch(/AG_TOKEN='agt_/)
    await expect(box.getByTestId('agent-prompt')).toContainText('Add a greeting file')
    await expect(box.getByTestId('agent-waiting')).toBeVisible()

    // The agent runs what the page printed (the real `ag`, the real server), making its change where the page says.
    const shell = newShell()
    try {
      const out = await shell.run(
        commands.replace('# ...the agent makes its changes here...', 'echo hello > hello.txt'),
      )
      expect(out).toContain('agent:my-agent')
    } finally {
      rmSync(shell.dir, { recursive: true, force: true })
    }

    // The page notices by itself and says what to do next.
    await expect(box.getByTestId('agent-proposed')).toContainText('opened', { timeout: 20_000 })
    await expect(box.getByTestId('agent-proposed')).toContainText('Add a greeting file')
    await expect(box.getByTestId('give-work-progress')).toContainText('Change request opened')
    await box.getByTestId('agent-proposed').getByRole('link').click()
    await expect(page.getByTestId('proposal-title')).toHaveText('Add a greeting file')
  })
})

test.describe('jobs 3 and 5: review a change, then land it', () => {
  test('the reviewer finds it on the home page and approves; the integrator lands it', async ({
    cast,
    openAs,
  }, testInfo) => {
    const { repo } = await freshRepo(cast, 'review')
    await setPolicy(
      cast.as('dana'),
      repo,
      policyDocument({ approvals: { min: 1, forbid_self_approval: true }, requireTests: false }),
    )
    const title = `Fix a typo ${suffix()}`
    const alice = cast.as('alice')
    const c = await commitFiles(alice, repo, { 'README.md': '# demo\n\nA tiny repo.\n' }, title)
    await openProposal(alice, repo, { head: c.head, title, description: 'One word.' })

    // bob opens the app. The first page says it is waiting for him.
    const bob = await openAs('bob')
    await bob.goto('/')
    const row = item(bob, title)
    await expect(row).toBeVisible()
    await expectAccessible(bob, testInfo, 'home with work waiting')
    await expect(bob.getByTestId('attention-review')).toContainText('Waiting for your review')
    await expect(row).toContainText('Needs 1 more approval')
    await row.getByTestId('attention-action-review').click()

    // The review dialog is already open; the person reads the change behind it, then decides.
    const dialog = bob.getByRole('dialog', { name: 'Write a review' })
    await expect(dialog).toBeVisible()
    await dialog.getByLabel('Approve').check()
    await dialog.getByRole('textbox', { name: 'Comment' }).fill('Looks right.')
    await dialog.getByRole('button', { name: 'Submit review' }).click()
    await expect(dialog).toHaveCount(0)
    await expect(pill(bob, 'approved')).toHaveAttribute('data-tone', 'ok', { timeout: 15_000 })
    // The page says what is left and who can do it.
    await expect(
      bob.getByText('Nothing is in the way. This change request can land now.'),
    ).toBeVisible()
    await expect(bob.getByTestId('unavailable')).toContainText('Land: requires the integrator role')

    // carol, who may land, finds it under "Ready to land".
    const carol = await openAs('carol')
    await carol.goto('/')
    const ready = item(carol, title)
    await expect(ready).toBeVisible()
    await expect(carol.getByTestId('attention-ready')).toContainText('Ready to land')
    await ready.getByTestId('attention-action-land').click()
    const land = carol.getByRole('dialog', { name: 'Land this change request' })
    await expect(land).toBeVisible()
    await land.getByRole('button', { name: 'Land', exact: true }).click()
    await expect(land.getByText('Landed', { exact: true })).toBeVisible()
    await land.getByRole('button', { name: 'Close', exact: true }).click()
    await expect(carol.getByTestId('proposal-state')).toHaveText('landed')
    await expect(carol.getByRole('link', { name: 'See it in history' })).toBeVisible()

    // And it is gone from everyone's list.
    await carol.goto('/')
    await expect(item(carol, title)).toHaveCount(0)
  })
})

test.describe('job 4: unblock a blocked change', () => {
  test('the reason and the fix are on the row; the fix is a button the role may press', async ({
    cast,
    openAs,
  }, testInfo) => {
    const { repo } = await freshRepo(cast, 'unblock')
    await setPolicy(cast.as('dana'), repo, policyDocument({ requireTests: true }))
    const title = `Needs its checks ${suffix()}`
    const alice = cast.as('alice')
    const c = await commitFiles(alice, repo, { 'src/app.ts': 'export const answer = 42\n' }, title)
    await openProposal(alice, repo, { head: c.head, title })

    const carol = await openAs('carol')
    await carol.goto('/')
    const row = item(carol, title)
    await expect(carol.getByTestId('attention-blocked')).toContainText('Blocked')
    await expect(row).toContainText('has not passed on these exact files')
    await row.getByRole('link', { name: title }).click()

    // On the change request: the reason, what it means, and the fix, as a button.
    const blockers = carol.getByTestId('blockers')
    await expect(blockers).toContainText('A trusted runner must report a passing, signed result')
    await expectAccessible(carol, testInfo, 'a blocked change with its fix')
    await blockers.getByTestId('fix-validate').click()
    await expect(pill(carol, 'validated')).toHaveAttribute('data-tone', 'ok', { timeout: 40_000 })
    expect(await verdicts(strip(carol))).toMatchObject({ validated: 'ok', mergeable: 'ok' })
    await expect(
      carol.getByText('Nothing is in the way. This change request can land now.'),
    ).toBeVisible()
  })

  test('a reader sees the reason but is told who can fix it, not given buttons that fail', async ({
    cast,
    openAs,
  }) => {
    const { repo } = await freshRepo(cast, 'readerfix')
    await setPolicy(cast.as('dana'), repo, policyDocument({ requireTests: true }))
    const alice = cast.as('alice')
    const c = await commitFiles(alice, repo, { 'src/app.ts': 'export const answer = 7\n' }, 'Seven')
    const p = await openProposal(alice, repo, { head: c.head, title: 'Seven' })

    const erin = await openAs('erin')
    await erin.goto(`/repos/${repo}/proposals/${p.id}`)
    const blockers = erin.getByTestId('blockers')
    await expect(blockers).toContainText('has not passed on these exact files')
    await expect(blockers.getByTestId('fix-validate')).toHaveCount(0)
    await expect(blockers.getByTestId('ask-missing_validation')).toContainText(
      'Ask someone with the writer or integrator role',
    )
    await expect(erin.getByTestId('unavailable')).toContainText(
      'Land: requires the integrator role',
    )
  })
})

test.describe('editing in the browser', () => {
  test('open a file, edit it, propose the change; it is a change request like any other', async ({
    page,
    cast,
  }, testInfo) => {
    const { repo } = await freshRepo(cast, 'edit')
    const headBefore = (await cast.as('dana').get(`/v1/repos/${repo}/streams/head?stream=main`))
      .json.head
    await page.goto(`/repos/${repo}`)
    await page.getByRole('link', { name: 'Code' }).click()
    await page.getByRole('link', { name: 'README.md' }).click()
    await page.getByTestId('edit-file').click()
    const editor = page.getByTestId('file-editor')
    await expect(editor).toBeVisible()
    await expect(editor.getByRole('button', { name: 'Propose change…' })).toBeDisabled()
    await expectAccessible(page, testInfo, 'file editor')
    await editor.getByLabel('Contents of README.md').fill('# demo\n\nEdited in the browser.\n')
    await editor.getByTestId('propose-change').click()
    const dialog = page.getByRole('dialog', { name: 'Propose this change' })
    await expect(dialog.getByLabel('Title')).toHaveValue('Update README.md')
    await dialog.getByLabel('Title').fill('Say that it was edited in the browser')
    await dialog.getByLabel('Description (optional)').fill('A small edit made without a clone.')
    await dialog.getByTestId('propose-submit').click()

    await expect(page.getByTestId('proposal-title')).toHaveText(
      'Say that it was edited in the browser',
    )
    await expect(page.getByTestId('proposal-state')).toHaveText('open')
    await expect(page.getByText('Edited in the browser.').first()).toBeVisible()
    // The branch did not move: nothing lands without going through review.
    const after = (await cast.as('dana').get(`/v1/repos/${repo}/streams/head?stream=main`)).json
      .head
    expect(after).toBe(headBefore)
  })

  test('a new file; a reader is not offered editing', async ({ page, cast, openAs }) => {
    const { repo } = await freshRepo(cast, 'newfile')
    await page.goto(`/repos/${repo}/files`)
    await page.getByTestId('new-file').click()
    const editor = page.getByTestId('file-editor')
    await editor.getByLabel('File path').fill('/bad')
    await expect(editor.getByText('Leave out the leading slash.')).toBeVisible()
    await editor.getByLabel('File path').fill('docs/hello.md')
    await editor.getByLabel('Contents').fill('hello\n')
    await editor.getByTestId('propose-change').click()
    const dialog = page.getByRole('dialog', { name: 'Propose this change' })
    await expect(dialog.getByLabel('Title')).toHaveValue('Add docs/hello.md')
    await dialog.getByTestId('propose-submit').click()
    await expect(page.getByTestId('proposal-title')).toHaveText('Add docs/hello.md')

    const erin = await openAs('erin')
    await erin.goto(`/repos/${repo}/files`)
    await expect(erin.getByTestId('new-file')).toHaveCount(0)
    await erin.getByRole('link', { name: 'README.md' }).click()
    await expect(erin.getByTestId('file-viewer')).toBeVisible()
    await expect(erin.getByTestId('edit-file')).toHaveCount(0)
  })

  test('a conflict is resolved by its author, file by file, and the change request moves on', async ({
    cast,
    openAs,
  }, testInfo) => {
    const { repo } = await freshRepo(cast, 'resolve')
    const alice = cast.as('alice')
    // alice and dana both change the same line of the same file; dana's lands first.
    const mine = await commitFiles(
      alice,
      repo,
      { 'src/app.ts': 'export const answer = 42\n' },
      'Answer is 42',
    )
    const p = await openProposal(alice, repo, { head: mine.head, title: 'The answer is 42' })
    const dana = cast.as('dana')
    const theirs = await commitFiles(
      dana,
      repo,
      { 'src/app.ts': 'export const answer = 43\n' },
      'Answer is 43',
    )
    const other = await openProposal(dana, repo, { head: theirs.head, title: 'The answer is 43' })
    await integrate(cast.as('carol'), repo, other.id)

    const page = await openAs('alice')
    await page.goto(`/repos/${repo}/proposals/${p.id}`)
    await expect(pill(page, 'mergeable')).toHaveAttribute('data-tone', 'failed')
    await expect(page.getByTestId('conflicts')).toContainText('src/app.ts')
    await page.getByTestId('fix-conflict').click()

    const resolver = page.getByTestId('resolver')
    await expect(resolver).toBeVisible()
    await expect(resolver.getByLabel('The branch’s version of src/app.ts')).toContainText(
      'answer = 43',
    )
    await expect(resolver.getByLabel('Your version of src/app.ts')).toContainText('answer = 42')
    await expectAccessible(page, testInfo, 'conflict resolver')
    await resolver.getByLabel('Result for src/app.ts').fill('export const answer = 42 + 1\n')
    await page.getByTestId('resolve-save').click()

    await expect(page.getByRole('dialog')).toHaveCount(0)
    await expect(pill(page, 'mergeable')).toHaveAttribute('data-tone', 'ok', { timeout: 15_000 })
    await expect(page.getByTestId('conflicts')).toHaveCount(0)
    const detail = await alice.get(`/v1/repos/${repo}/proposals/${p.id}`)
    expect(detail.json.proposal.head).not.toBe(mine.head)
  })

  test('someone who is not the author is told to pass the link on, not offered the resolver', async ({
    cast,
    openAs,
  }) => {
    const { repo } = await freshRepo(cast, 'resolve2')
    const alice = cast.as('alice')
    const mine = await commitFiles(
      alice,
      repo,
      { 'src/app.ts': 'export const answer = 1\n' },
      'one',
    )
    const p = await openProposal(alice, repo, { head: mine.head, title: 'One' })
    const dana = cast.as('dana')
    const theirs = await commitFiles(
      dana,
      repo,
      { 'src/app.ts': 'export const answer = 2\n' },
      'two',
    )
    const other = await openProposal(dana, repo, { head: theirs.head, title: 'Two' })
    await integrate(cast.as('carol'), repo, other.id)

    const bob = await openAs('bob')
    await bob.goto(`/repos/${repo}/proposals/${p.id}`)
    await expect(bob.getByTestId('conflicts')).toBeVisible()
    await expect(bob.getByTestId('fix-conflict')).toHaveCount(0)
    await expect(bob.getByTestId('fix-copy-link')).toBeVisible()
  })
})

test.describe('the rules, in plain words', () => {
  test('choose a preset, read what it changes, save; the raw JSON is still there under Advanced', async ({
    page,
    cast,
  }, testInfo) => {
    const { repo } = await freshRepo(cast, 'rules')
    await page.goto(`/repos/${repo}/admin`)
    const form = page.getByTestId('policy-form')
    await expect(form).toBeVisible()
    await expect(page.getByTestId('policy-meaning')).toContainText('No checks are required')
    // No rules stored yet: the editor starts from a suggestion, and says what saving it would change from the default.
    await expect(page.getByTestId('policy-effects')).toContainText('Landing now needs 1 approval')

    await page.getByTestId('preset-strict').click()
    const effects = page.getByTestId('policy-effects')
    await expect(effects).toContainText('Landing now needs 2 approvals (was 0)')
    await expect(effects).toContainText('Asks for more')
    await expect(page.getByTestId('policy-meaning')).toContainText('at most 200 files')
    await expectAccessible(page, testInfo, 'rules form')

    // The form is just the document: a field and the raw JSON agree.
    await page.getByLabel('Approvals needed for a change').fill('3')
    await page.getByTestId('policy-advanced').locator('summary').click()
    await expect(page.getByLabel('Policy document, JSON')).toHaveValue(/"min": 3/)
    await page.getByTestId('policy-save').click()
    await expect(page.getByText('Policy version 1 saved')).toBeVisible()

    const stored = await cast.as('dana').get(`/v1/repos/${repo}/policy?stream=main`)
    const doc = JSON.parse(stored.json.document)
    expect(doc.approvals.min).toBe(3)
    expect(doc.max_changed_files).toBe(200)
    await expect(page.getByTestId('policy-effects')).toHaveCount(0)
  })
})

test.describe('empty states say what to do next', () => {
  test('a new repository: change requests, agent sessions and reservations', async ({
    page,
    cast,
  }) => {
    const repo = `acme/empty-${suffix()}`
    await cast.as('admin').ok_(cast.as('admin').post('/v1/repos', { name: repo }), 'create')
    await page.goto(`/repos/${repo}/proposals`)
    await expect(page.getByText('No open change requests').first()).toBeVisible()
    await expect(page.getByText(/edit a file under Code|give an agent work/i).first()).toBeVisible()
    await page.goto(`/repos/${repo}/agents`)
    await expect(page.getByText('No active agent sessions')).toBeVisible()
    await expect(page.getByText(/create a task and connect an agent/)).toBeVisible()
  })
})

test.describe('a person who sees nothing', () => {
  test.use({ persona: null })

  test('is told how to get a repository, not shown an error', async ({ page, cast }) => {
    const admin = cast.as('admin')
    const minted = await admin.ok_(
      admin.post('/v1/credentials', { subject: 'user:nobody', ttl_secs: 600 }),
      'mint',
    )
    await page.addInitScript((t) => sessionStorage.setItem('agentgit.token', t), minted.token)
    await page.goto('/')
    await expect(page.getByText('Nothing to look at yet')).toBeVisible()
    await expect(page.getByText(/administrator gives you a role/)).toBeVisible()
  })
})
