import { test, expect } from './fixtures.ts'
import { freshRepo, agentKey, commitFiles } from './support/world.ts'

test('a task keeps its full linked history when the repository has newer unrelated work', async ({
  page,
  cast,
}, info) => {
  const { repo } = await freshRepo(cast, 'task-history')
  const admin = cast.as('admin')
  const base = `/v1/repos/${repo}`
  const task = await admin.ok_(
    admin.post(`${base}/tasks`, { title: 'Task to audit' }),
    'create task',
  )
  await admin.ok_(
    admin.post(`${base}/agents`, {
      name: 'history-bot',
      public_key: agentKey('history-bot').publicHex,
    }),
    'register agent',
  )
  for (let n = 0; n < 51; n++) {
    const run = await admin.ok_(
      admin.post(`${base}/runs`, { agent: 'history-bot', task_id: task.id }),
      'start run',
    )
    await admin.ok_(
      admin.post(`${base}/runs/${run.run_id}/complete`, {
        status: 'completed',
        summary: `Historical session ${n}`,
      }),
      'complete run',
    )
  }
  const change = await commitFiles(
    cast.as('alice'),
    repo,
    { 'history.txt': 'History\n' },
    'History fixture',
  )
  for (let n = 0; n < 51; n++) {
    await admin.ok_(
      admin.post(`${base}/proposals`, {
        stream: 'main',
        head: change.head,
        title: `History change ${n}`,
        task_id: task.id,
      }),
      'link proposal',
    )
  }
  for (let n = 0; n < 5; n++) {
    await admin.ok_(admin.post(`${base}/runs`, { agent: 'history-bot' }), 'unrelated run')
    await admin.ok_(
      admin.post(`${base}/proposals`, {
        stream: 'main',
        head: change.head,
        title: `Unrelated change ${n}`,
      }),
      'unrelated proposal',
    )
  }
  await page.goto(`/repos/${repo}/tasks/${task.id}`)
  const changes = page.getByRole('region', { name: 'Related change requests', exact: true })
  const runs = page.getByRole('region', { name: 'Related agent sessions', exact: true })
  await expect(changes.getByRole('link', { name: /^History change / })).toHaveCount(50)
  await expect(changes.getByText(/Unrelated change/)).toHaveCount(0)
  await expect(runs.getByRole('link', { name: /^run_/ })).toHaveCount(50)
  await changes.getByRole('button', { name: 'Load older change requests' }).click()
  await runs.getByRole('button', { name: 'Load older sessions' }).click()
  await expect(changes.getByRole('link', { name: /^History change / })).toHaveCount(51)
  await expect(runs.getByRole('link', { name: /^run_/ })).toHaveCount(51)
  await expect(changes.getByRole('button', { name: 'Load older change requests' })).toHaveCount(0)
  await expect(runs.getByRole('button', { name: 'Load older sessions' })).toHaveCount(0)
  await page.screenshot({ path: info.outputPath('task-history.png'), fullPage: true })
})
