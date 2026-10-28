import { migrationWarnings } from '@/api/migration'
import type { MigrationPlan } from '@/api/migration'

export const migrationScope = {
  imported: ['File history, commit authors and messages.', 'Branches and commit-based tags.'],
  separate: [
    'Issues and pull or merge request discussions.',
    'Permissions, CI settings and release assets.',
    'External Git LFS files.',
  ],
} as const

/** Plain text keeps source ref names literal, including any Markdown or control characters. */
export function migrationSummary(plan: MigrationPlan): string {
  const quote = (value: string) => JSON.stringify(value)
  const report = plan.report
  const warnings = migrationWarnings(report)
  return [
    'AgentGit migration summary',
    '',
    `Repository: ${plan.name}`,
    `Status: ${plan.state === 'imported' ? 'Published in AgentGit' : 'Awaiting review; not published'}`,
    `Commits: ${report.commits}`,
    `Branches: ${report.branches.length}`,
    `Tags: ${report.tags.length}`,
    `Default branch: ${quote(report.default_branch)}`,
    `Source bundle SHA-256: ${plan.source_sha256}`,
    '',
    'Branches (Git ref -> AgentGit stream; Git commit -> AgentGit changeset)',
    ...report.branches.map(
      (ref) =>
        `${quote(ref.name)} -> ${quote(ref.destination)}; ${ref.git_commit} -> ${ref.changeset}`,
    ),
    '',
    'Tags (Git tag -> AgentGit pin; Git commit -> AgentGit changeset)',
    ...report.tags.map(
      (ref) =>
        `${quote(ref.name)} -> ${quote(ref.destination)}; ${ref.git_commit} -> ${ref.changeset}`,
    ),
    '',
    'Import warnings',
    ...(warnings.length ? warnings : ['No content warnings reported.']),
    ...report.skipped_paths.map(([path, reason]) => `${quote(path)}: ${quote(reason)}`),
    '',
    'Before your team switches',
    ...(plan.state === 'ready' ? ['[ ] Review this import and publish it when ready.'] : []),
    '[ ] Check the imported files, branches and tags.',
    ...migrationScope.separate.map((item) => `[ ] Move separately: ${item}`),
    '[ ] Set up team access and confirm the first change can be reviewed and integrated.',
    '[ ] Keep the original repository available until the team has checked the result.',
    '',
    'AgentGit changesets have their own IDs. Original Git IDs and signed tag objects remain',
    'in the retained source bundle, which is included in repository backups.',
    '',
  ].join('\n')
}
