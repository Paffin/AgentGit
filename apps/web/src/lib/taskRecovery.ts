import type { Run, Task } from '@/api/types'

/** Keep recovery context inspectable by both a person and the next executor. */
export function followUpDescription(repo: string, task: Task, run: Run): string {
  const base = `/repos/${repo.split('/').map(encodeURIComponent).join('/')}`
  return [
    task.description,
    `Follow-up to task ${task.id}: ${base}/tasks/${encodeURIComponent(task.id)}`,
    `Source run (${run.status}): ${base}/runs/${encodeURIComponent(run.run_id)}`,
    run.summary
      ? `Previous run summary:\n${run.summary}`
      : 'The previous run did not provide a summary.',
  ]
    .filter(Boolean)
    .join('\n\n')
}
