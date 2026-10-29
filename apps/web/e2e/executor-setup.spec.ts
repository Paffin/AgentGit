import { mkdtemp, readFile, chmod, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { spawn, execFile } from 'node:child_process'
import { promisify } from 'node:util'
const execFileAsync = promisify(execFile)
import { test, expect } from './fixtures.ts'
import { freshRepo } from './support/world.ts'
import { Api } from './support/http.ts'

test('executor setup streams live output through the native runtime and can revoke access', async ({
  page,
  cast,
  openAs,
}, info) => {
  const { repo } = await freshRepo(cast, 'executor-setup')
  const directory = await mkdtemp(join(tmpdir(), 'agentgit-executor-setup-'))
  let process: ReturnType<typeof spawn> | undefined
  const progress = join(directory, 'show-progress')
  const release = join(directory, 'finish-execution')
  const script = `echo executor-output; while [ ! -f "${progress}" ]; do sleep 0.05; done; echo executor-progress; while [ ! -f "${release}" ]; do sleep 0.05; done; echo executor-done`
  try {
    await page.goto(`/repos/${repo}/agents?tab=agents`)
    await page.getByRole('button', { name: 'Connect an executor…', exact: true }).click()
    const dialog = page.getByRole('dialog', { name: 'Connect an executor', exact: true })
    await dialog.getByLabel('Agent name', { exact: true }).fill('setup-bot')
    await dialog.getByLabel('Executable on the executor host', { exact: true }).fill('/bin/sh')
    await dialog
      .getByLabel('Arguments (JSON array)', { exact: true })
      .fill(JSON.stringify(['-c', script]))
    await dialog.getByLabel('Save diagnostic output in AgentGit', { exact: true }).check()
    await dialog.getByLabel('Show output while the agent is running', { exact: true }).check()
    await dialog.getByRole('button', { name: 'Prepare access for 24 hours', exact: true }).click()
    await expect(dialog.getByText('Waiting for the executor', { exact: true })).toBeVisible()
    const download = async (label: string, filename: string) => {
      const waiting = page.waitForEvent('download')
      await dialog.getByRole('button', { name: label, exact: true }).click()
      const result = await waiting
      await result.saveAs(join(directory, filename))
    }
    await download('Download configuration', 'config.json')
    await download('Download private token', 'token')
    await chmod(join(directory, 'token'), 0o600)
    const token = (await readFile(join(directory, 'token'), 'utf8')).trim()
    const configText = await readFile(join(directory, 'config.json'), 'utf8')
    expect(configText.includes(token)).toBe(false)
    const config = JSON.parse(configText)
    expect(config.repos[0].name).toBe(repo)
    expect(config.executors[0].command).toEqual(['/bin/sh', '-c', script])
    expect(config.executors[0].publish_output).toBe(true)
    expect(config.executors[0].output_interval).toBe('2s')
    const executor = new Api(cast.baseURL, token)
    const identity = await executor.ok_(executor.get('/v1/whoami'), 'executor identity')
    expect(identity.identity).toMatch(/^service:exec-setup-bot-[a-f0-9]{16}$/)
    expect(identity.scope.repos).toEqual([repo])
    expect(identity.scope.operations).not.toContain('integrate.run')
    // Local qualification also starts the exact downloaded profile with the real native binary.
    const binary = globalThis.process.env.AG_ORCH_BIN
    if (binary) {
      config.state_dir = join(directory, 'state')
      config.listen = '127.0.0.1:0'
      await writeFile(join(directory, 'config.json'), JSON.stringify(config))
      await execFileAsync(binary, [
        'check',
        '-config',
        join(directory, 'config.json'),
        '-token-file',
        join(directory, 'token'),
      ])
      process = spawn(
        binary,
        ['run', '-config', join(directory, 'config.json'), '-token-file', join(directory, 'token')],
        { stdio: 'ignore' },
      )
      await expect(dialog.getByText('Executor connected', { exact: true })).toBeVisible({
        timeout: 25000,
      })
      const task = await cast.as('admin').ok_(
        cast.as('admin').post(`/v1/repos/${repo}/tasks`, {
          title: 'Run the connected executor',
          assignee: 'setup-bot',
        }),
        'assigned task',
      )
      let runId = ''
      await expect
        .poll(
          async () => {
            const rows = await cast
              .as('admin')
              .ok_(cast.as('admin').get(`/v1/repos/${repo}/runs`), 'started runs')
            runId =
              rows.runs.find((run: { task_id: string }) => run.task_id === task.id)?.run_id || ''
            return runId
          },
          { timeout: 15000 },
        )
        .not.toBe('')
      const observer = await openAs('dana')
      try {
        await observer.goto(`/repos/${repo}/runs/${runId}`)
        await expect(
          observer.getByLabel('Captured standard output', { exact: true }),
        ).toContainText('executor-output', { timeout: 10000 })
        await writeFile(progress, 'next')
        await expect(
          observer.getByLabel('Captured standard output', { exact: true }),
        ).toContainText('executor-progress', { timeout: 10000 })
        const running = await cast
          .as('admin')
          .ok_(cast.as('admin').get(`/v1/repos/${repo}/runs/${runId}`), 'still running')
        expect(running.status).toBe('running')
        await observer.screenshot({
          path: info.outputPath('live-native-output.png'),
          fullPage: true,
        })
        await writeFile(release, 'finish')
        await expect(
          observer.getByLabel('Captured standard output', { exact: true }),
        ).toContainText('executor-done', { timeout: 10000 })
      } finally {
        await observer.close()
      }
      await expect
        .poll(
          async () => {
            const runs = await cast
              .as('admin')
              .ok_(cast.as('admin').get(`/v1/repos/${repo}/runs`), 'runs')
            return runs.runs.some(
              (run: { task_id: string; status: string }) =>
                run.task_id === task.id && run.status === 'completed',
            )
          },
          { timeout: 45000 },
        )
        .toBe(true)
    }
    if (binary) {
      const runs = await cast
        .as('admin')
        .ok_(cast.as('admin').get(`/v1/repos/${repo}/runs`), 'completed runs')
      const output = await cast
        .as('admin')
        .ok_(
          cast.as('admin').get(`/v1/repos/${repo}/runs/${runs.runs[0].run_id}/output`),
          'saved native output',
        )
      expect(output.stdout).toContain('executor-output')
    }
    await page.screenshot({ path: info.outputPath('executor-setup.png'), fullPage: true })
    await dialog.getByRole('button', { name: 'Revoke this executor token', exact: true }).click()
    await expect(
      dialog.getByRole('button', { name: 'Prepare access for 24 hours', exact: true }),
    ).toBeVisible()
    expect((await executor.get('/v1/whoami')).status).toBe(401)
  } finally {
    await writeFile(progress, 'next').catch(() => {})
    await writeFile(release, 'finish').catch(() => {})
    process?.kill('SIGTERM')
    if (process && process.exitCode === null)
      await new Promise((resolve) => process!.once('exit', resolve))
    await rm(directory, { recursive: true, force: true })
  }
})
