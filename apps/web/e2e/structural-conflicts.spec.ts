import { test, expect } from './fixtures.ts'
import { freshRepo, commitFiles, openProposal, integrate } from './support/world.ts'

for (const keepFolder of [true, false])
  for (const directorySide of ['proposal', 'branch']) {
    test(`chooses ${keepFolder ? 'folder' : 'file'} against ${directorySide} folder and preserves independent clean edits`, async ({
      cast,
      openAs,
    }, info) => {
      const { repo } = await freshRepo(cast, 'structural-conflict')
      const alice = cast.as('alice'),
        dana = cast.as('dana')
      const folder = { 'clash/nested/run': 'run\n', 'clash/link': 'target\n' }
      const ours = await commitFiles(
        alice,
        repo,
        directorySide === 'proposal'
          ? { ...folder, 'mine-clean.txt': 'mine clean\n' }
          : { clash: 'my file\n', 'mine-clean.txt': 'mine clean\n' },
        'Proposal layout',
      )
      const theirs = await commitFiles(
        dana,
        repo,
        directorySide === 'branch'
          ? { ...folder, 'branch-clean.txt': 'branch clean\n' }
          : { clash: 'branch file\n', 'branch-clean.txt': 'branch clean\n' },
        'Branch layout',
      )
      const owner = directorySide === 'proposal' ? alice : dana,
        source = directorySide === 'proposal' ? ours : theirs
      const exact = await owner.ok_(
        owner.post(`/v1/repos/${repo}/workspaces/${source.workspace}/changes`, {
          expected_head: source.head,
          message: 'Preserve type and mode',
          changes: [
            {
              path: 'clash/nested/run',
              op: 'put',
              content: Buffer.from('run\n').toString('base64'),
              executable: true,
            },
            {
              path: 'clash/link',
              op: 'put',
              content: Buffer.from('target').toString('base64'),
              file_type: 'symlink',
            },
          ],
        }),
        'set exact entries',
      )
      const proposal = await openProposal(alice, repo, {
        head: directorySide === 'proposal' ? exact.head : ours.head,
        title: 'Choose a whole folder',
      })
      const upstream = await openProposal(dana, repo, {
        head: directorySide === 'branch' ? exact.head : theirs.head,
        title: 'Branch structure',
      })
      await integrate(cast.as('carol'), repo, upstream.id)
      const page = await openAs('alice')
      await page.goto(`/repos/${repo}/proposals/${proposal.id}`)
      await page.getByTestId('fix-conflict').click()
      const row = page.getByTestId('resolver').locator('[data-path="clash"]')
      await expect(row).toBeVisible()
      await expect(page.getByTestId('resolve-save')).toBeDisabled()
      await row
        .getByRole('button', {
          name: keepFolder
            ? directorySide === 'proposal'
              ? 'Keep mine (2 files)'
              : 'Keep the branch’s (2 files)'
            : directorySide === 'proposal'
              ? 'Keep the branch’s (1 file)'
              : 'Keep mine (1 file)',
          exact: true,
        })
        .click()
      await page.screenshot({ path: info.outputPath('structural-conflict.png') })
      await page.getByTestId('resolve-save').click()
      await expect(page.getByRole('dialog')).toHaveCount(0)
      const detail = await alice.ok_(
        alice.get(`/v1/repos/${repo}/proposals/${proposal.id}`),
        'resolved proposal',
      )
      const head = detail.proposal.head
      if (keepFolder)
        for (const path of ['clash/nested/run', 'clash/link']) {
          const result = await alice.ok_(
            alice.get(`/v1/repos/${repo}/tree/resolve?rev=${head}&path=${path}`),
            'preserved entry',
          )
          const original = await alice.ok_(
            alice.get(`/v1/repos/${repo}/tree/resolve?rev=${exact.head}&path=${path}`),
            'source entry',
          )
          expect(result.entry).toEqual(original.entry)
        }
      for (const path of ['mine-clean.txt', 'branch-clean.txt'])
        expect(
          (
            await alice.ok_(
              alice.get(`/v1/repos/${repo}/tree/resolve?rev=${head}&path=${path}`),
              'independent edit',
            )
          ).entry,
        ).not.toBeNull()
      expect(
        (
          await alice.ok_(
            alice.get(`/v1/repos/${repo}/tree/resolve?rev=${head}&path=clash`),
            'folder chosen',
          )
        ).kind,
      ).toBe(keepFolder ? 'dir' : 'file')
      if (!keepFolder)
        for (const path of ['clash/link', 'clash/nested/run'])
          expect(
            (
              await alice.ok_(
                alice.get(`/v1/repos/${repo}/tree/resolve?rev=${head}&path=${path}`),
                'removed child',
              )
            ).kind,
          ).toBe('missing')
      await page.close()
    })
  }
