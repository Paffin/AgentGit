import { test, expect } from './fixtures.ts'
import { freshRepo, commitFiles, openProposal } from './support/world.ts'

test('a reviewer proposes code and the author applies the exact replacement without losing file mode', async ({
  cast,
  openAs,
}, info) => {
  const { repo } = await freshRepo(cast, 'suggestions')
  const alice = cast.as('alice')
  const initial = await commitFiles(
    alice,
    repo,
    { 'script.sh': '#!/bin/sh\r\nprintf old\r\n' },
    'Script',
  )
  const executable = await alice.ok_(
    alice.post(`/v1/repos/${repo}/workspaces/${initial.workspace}/changes`, {
      changes: [
        {
          op: 'put',
          path: 'script.sh',
          content: Buffer.from('#!/bin/sh\r\nprintf old\r\n').toString('base64'),
          executable: true,
        },
      ],
      expected_head: initial.head,
      message: 'Keep script executable',
    }),
    'set executable mode',
  )
  const proposal = await openProposal(alice, repo, {
    head: executable.head,
    title: 'Review the script',
  })
  const url = `/repos/${repo}/proposals/${proposal.id}`
  const reviewer = await openAs('bob')
  await reviewer.goto(url)
  await reviewer
    .getByRole('button', { name: 'Discuss new line 2 of script.sh', exact: true })
    .click()
  await reviewer.getByRole('button', { name: 'Suggest a replacement', exact: true }).click()
  await reviewer
    .getByRole('textbox', { name: 'Explain this suggestion', exact: true })
    .fill('Use the localized message.')
  await reviewer
    .getByRole('textbox', { name: 'Replacement code', exact: true })
    .fill("printf 'новое'")
  await reviewer.reload()
  await expect(
    reviewer.getByRole('textbox', { name: 'Replacement code', exact: true }),
  ).toHaveValue("printf 'новое'")
  await reviewer.getByRole('button', { name: 'Start discussion', exact: true }).click()
  await expect(reviewer.getByRole('region', { name: 'Suggested replacement' })).toContainText(
    "printf 'новое'",
  )
  await expect(reviewer.getByRole('button', { name: 'Review and apply suggestion' })).toHaveCount(0)
  await cast.as('bob').ok_(
    cast.as('bob').post(`/v1/repos/${repo}/proposals/${proposal.id}/reviews`, {
      decision: 'approve',
      expected_head: executable.head,
    }),
    'review original revision',
  )

  const author = await openAs('alice')
  await author.goto(`${url}?tab=reviews`)
  await author.getByRole('button', { name: 'Review and apply suggestion', exact: true }).click()
  const dialog = author.getByRole('dialog', { name: 'Apply this suggestion?' })
  await expect(dialog).toContainText('printf old')
  await expect(dialog).toContainText("printf 'новое'")
  await expect(dialog.getByRole('button', { name: 'Apply suggestion', exact: true })).toBeEnabled()
  await author.screenshot({ path: info.outputPath('suggestion-preview.png'), fullPage: true })
  await dialog.getByRole('button', { name: 'Apply suggestion', exact: true }).click()
  await expect(dialog).toHaveCount(0)
  await expect(author.getByText('Earlier head', { exact: true })).toBeVisible()
  await expect(author.getByRole('button', { name: 'Review and apply suggestion' })).toHaveCount(0)
  const detail = await alice.ok_(
    alice.get(`/v1/repos/${repo}/proposals/${proposal.id}`),
    'read updated proposal',
  )
  expect(detail.proposal.head).not.toBe(executable.head)
  expect(detail.reviews[0].head).toBe(executable.head)
  const file = await alice.ok_(
    alice.get(`/v1/repos/${repo}/files/script.sh?rev=${detail.proposal.head}`),
    'read exact resulting bytes',
  )
  expect(file).toBe("#!/bin/sh\r\nprintf 'новое'\r\n")
  const location = await alice.ok_(
    alice.get(`/v1/repos/${repo}/tree/resolve?path=script.sh&rev=${detail.proposal.head}`),
    'read executable metadata',
  )
  expect(location.entry.executable).toBe(true)
  await author.reload()
  await expect(author.getByRole('region', { name: 'Suggested replacement' })).toContainText(
    "printf 'новое'",
  )
  const reader = await openAs('erin')
  await reader.goto(`${url}?tab=reviews`)
  await expect(reader.getByRole('region', { name: 'Suggested replacement' })).toBeVisible()
  await expect(reader.getByRole('button', { name: 'Review and apply suggestion' })).toHaveCount(0)
  await reader.close()
  await author.close()
  await reviewer.close()
})

test('the author previews and applies selected suggestions across files in one revision', async ({
  cast,
  openAs,
}, info) => {
  const { repo } = await freshRepo(cast, 'batch-suggestions')
  const alice = cast.as('alice')
  const initial = await commitFiles(
    alice,
    repo,
    {
      'script.sh': '\uFEFFfirst\r\nsecond\r\nthird\r\n',
      'note.txt': 'old\n',
    },
    'Files for review',
  )
  const proposal = await openProposal(alice, repo, {
    head: initial.head,
    title: 'Apply reviewed fixes together',
  })
  const entries = [
    { path: 'script.sh', line: 1, replacement: 'one\nextra' },
    { path: 'script.sh', line: 3, replacement: 'three' },
    { path: 'note.txt', line: 1, replacement: 'new' },
  ]
  const bob = cast.as('bob')
  for (const entry of entries) {
    await bob.ok_(
      bob.post(`/v1/repos/${repo}/proposals/${proposal.id}/threads`, {
        expected_head: initial.head,
        anchor: {
          base: initial.base,
          head: initial.head,
          side: 'new',
          path: entry.path,
          line: entry.line,
        },
        body: `Use reviewed text.\n\n\`\`\`suggestion\n${entry.replacement}\n\`\`\``,
      }),
      'create suggestion',
    )
  }
  const url = `/repos/${repo}/proposals/${proposal.id}?tab=reviews`
  const reviewer = await openAs('bob')
  await reviewer.goto(url)
  await expect(reviewer.getByText('Use reviewed text.', { exact: true })).toHaveCount(3)
  await expect(reviewer.getByText('Apply several suggestions', { exact: true })).toHaveCount(0)
  const author = await openAs('alice')
  await author.goto(url)
  await author.getByText('Apply several suggestions', { exact: true }).click()
  for (const entry of entries)
    await author
      .getByRole('checkbox', { name: `${entry.path}, line ${entry.line}`, exact: true })
      .check()
  await author.getByRole('button', { name: 'Review selected suggestions', exact: true }).click()
  const dialog = author.getByRole('dialog', { name: 'Apply selected suggestions?' })
  await expect(
    dialog.getByRole('button', { name: 'Apply selected suggestions', exact: true }),
  ).toBeEnabled()
  await expect(dialog).toContainText('3 suggestions in 2 files')
  expect(
    (
      await alice.ok_(
        alice.get(`/v1/repos/${repo}/proposals/${proposal.id}`),
        'inspect original revision',
      )
    ).proposal.head,
  ).toBe(initial.head)
  await author.screenshot({ path: info.outputPath('batch-suggestion-preview.png'), fullPage: true })
  const mutations: string[] = []
  author.on('request', (request) => {
    if (request.method() === 'POST' && new URL(request.url()).pathname.endsWith('/changes'))
      mutations.push('changes')
    if (
      request.method() === 'PATCH' &&
      new URL(request.url()).pathname.endsWith(`/proposals/${proposal.id}`)
    )
      mutations.push('proposal')
  })
  await dialog.getByRole('button', { name: 'Apply selected suggestions', exact: true }).click()
  await expect(dialog).toHaveCount(0)
  expect(mutations).toEqual(['changes', 'proposal'])
  const current = await alice.ok_(
    alice.get(`/v1/repos/${repo}/proposals/${proposal.id}`),
    'read resulting revision',
  )
  expect(current.proposal.head).not.toBe(initial.head)
  for (const [path, text] of [
    ['script.sh', '\uFEFFone\r\nextra\r\nsecond\r\nthree\r\n'],
    ['note.txt', 'new\n'],
  ]) {
    const response = await fetch(
      `${cast.baseURL}/v1/repos/${repo}/files/${path}?rev=${current.proposal.head}`,
      { headers: { authorization: `Bearer ${cast.token('alice')}` } },
    )
    expect(response.status).toBe(200)
    expect(Array.from(new Uint8Array(await response.arrayBuffer()))).toEqual(
      Array.from(new TextEncoder().encode(text)),
    )
  }
  const threads = await bob.ok_(
    bob.get(`/v1/repos/${repo}/proposals/${proposal.id}/threads`),
    'read retained discussions',
  )
  expect(threads.threads).toHaveLength(3)
  expect(threads.threads.every((thread: { resolved: boolean }) => !thread.resolved)).toBe(true)
  await reviewer.close()
  await author.close()
})
