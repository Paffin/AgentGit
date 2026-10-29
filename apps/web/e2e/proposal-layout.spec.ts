import { test, expect } from './fixtures.ts'
import { freshRepo, commitFiles, openProposal } from './support/world.ts'
import { expectAccessible } from './support/ui.ts'

for (const viewport of [
  { name: 'desktop', width: 1440, height: 900 },
  { name: 'phone', width: 390, height: 844 },
]) {
  test(`the ${viewport.name} review workspace keeps changed code ahead of secondary actions`, async ({
    cast,
    openAs,
  }, info) => {
    const { repo } = await freshRepo(cast, 'review-layout')
    const alice = cast.as('alice')
    const revision = await commitFiles(
      alice,
      repo,
      {
        'src/session.ts':
          'export function createSession(user: string) {\n  return { user, expiresIn: 3600 }\n}\n',
        'test/session.test.ts': 'expect(createSession("alex").expiresIn).toBe(3600)\n',
      },
      'Bound session lifetime',
    )
    const proposal = await openProposal(alice, repo, {
      head: revision.head,
      title: 'Expire inactive sessions after one hour',
    })
    const page = await openAs('bob')
    await page.setViewportSize(viewport)
    await page.goto(`/repos/${repo}/proposals/${proposal.id}`)
    await expect(page.getByTestId('changes-tab')).toContainText('src/session.ts')
    await expect(
      page.getByRole('list', { name: /four independent states/ }).getByText('checking'),
    ).toHaveCount(0)
    await page.screenshot({
      path: info.outputPath(`proposal-${viewport.name}.png`),
      fullPage: true,
    })
    const tabs = await page.getByRole('tablist', { name: 'Change request details' }).boundingBox()
    expect(tabs!.y).toBeLessThan(viewport.width < 600 ? 650 : 500)
    const review = await page.getByTestId('act-review').boundingBox()
    expect(review!.y).toBeLessThan(tabs!.y)
    await page.getByTestId('act-review').click()
    await expect(page.getByRole('dialog', { name: 'Write a review' })).toBeVisible()
    await page.getByRole('button', { name: 'Keep draft and close' }).click()
    await expectAccessible(page, info, `${viewport.name} review workspace`)
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(
      viewport.width,
    )
    await page.close()
  })
}
