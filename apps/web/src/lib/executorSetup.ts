/** A task-only orchestrator profile. Tokens are always kept in a separate private file. */
export const EXECUTOR_OPERATIONS = [
  ...new Set([
    'repo.read',
    'stream.read',
    'task.read',
    'lease.read',
    'agent.read',
    'agent.register',
    'agent.run',
    'proposal.read',
    'integrate.prepare',
    'predict',
    'events.read',
    'tree.read',
    'history.read',
    'jobs.read',
    'queue.read',
    'credentials.delegate',
    'agent.operate',
    'task.write',
    'objects.read',
    'objects.write',
    'publish',
    'operation.read',
    'workspace.read',
    'workspace.write',
    'proposal.create',
    'proposal.update',
    'lease.write',
    'provenance.read',
    'provenance.write',
  ]),
].sort()

export interface ExecutorSetup {
  origin: string
  repo: string
  agent: string
  command: string
  arguments: string
  environment: string
  slots: number
}

export function executorSetupProblem(input: ExecutorSetup): string | null {
  const serverProblem = serverAddressProblem(input.origin)
  if (serverProblem) return serverProblem
  if (!/^[A-Za-z0-9][A-Za-z0-9._-]{0,47}$/.test(input.agent))
    return 'Use 1–48 letters, digits, dots, dashes or underscores for the agent name.'
  if (!input.command.trim() || input.command.includes('\0'))
    return 'Enter the executable installed on the executor host.'
  const argsProblem = argumentsProblem(input.arguments)
  if (argsProblem) return argsProblem
  if (!validSlots(input.slots)) return 'Choose 1–32 concurrent tasks.'
  if (!validEnvironment(input.environment)) return 'Enter environment variable names, not values.'
  return null
}

export function executorConfiguration(input: ExecutorSetup): string {
  const problem = executorSetupProblem(input)
  if (problem) throw new Error(problem)
  return (
    JSON.stringify(
      {
        server: { url: input.origin, token_env: 'AG_ORCH_TOKEN' },
        state_dir: `./agentgit-executor-state/${input.repo}/${input.agent}`,
        listen: '127.0.0.1:0',
        repos: [
          {
            name: input.repo,
            stream: 'main',
            schedule: { slots: input.slots, default_executor: input.agent },
            validations: { enabled: false },
            advisor: { enabled: false },
            review: { enabled: false },
            index: { enabled: false },
          },
        ],
        executors: [
          {
            name: input.agent,
            agent: input.agent,
            type: 'command',
            slots: input.slots,
            command: [input.command.trim(), ...JSON.parse(input.arguments)],
            pass_env: [...new Set(['PATH', ...environmentNames(input.environment)])],
          },
        ],
      },
      null,
      2,
    ) + '\n'
  )
}

export function downloadSetupFile(name: string, contents: string, type = 'text/plain') {
  const url = URL.createObjectURL(new Blob([contents], { type }))
  const link = document.createElement('a')
  link.href = url
  link.download = name
  link.click()
  setTimeout(() => URL.revokeObjectURL(url), 0)
}

function argumentsProblem(text: string): string | null {
  try {
    const args: unknown = JSON.parse(text)
    if (!Array.isArray(args) || args.some((a) => typeof a !== 'string' || a.includes('\0')))
      return 'Arguments must be a JSON array of strings.'
  } catch {
    return 'Arguments must be a JSON array of strings, for example ["--mode", "agent"].'
  }
  return null
}

function serverAddressProblem(address: string): string | null {
  try {
    const url = new URL(address)
    if (url.username || url.password || url.search || url.hash)
      return 'Use the server address without credentials, query parameters or a fragment.'
    if (url.protocol === 'https:') return null
    if (url.protocol === 'http:' && isLoopback(url.hostname)) return null
    return 'Use HTTPS for a remote executor connection. HTTP is supported only for loopback addresses.'
  } catch {
    return 'Enter a valid AgentGit server URL.'
  }
}

function environmentNames(value: string): string[] {
  return value.split(/[\s,]+/).filter(Boolean)
}
function validEnvironment(value: string): boolean {
  return environmentNames(value).every((name) => /^[A-Za-z_][A-Za-z0-9_]*$/.test(name))
}
function isLoopback(hostname: string): boolean {
  return ['localhost', '[::1]'].includes(hostname) || /^127(?:\.\d{1,3}){3}$/.test(hostname)
}

function validSlots(slots: number): boolean {
  return Number.isInteger(slots) && slots >= 1 && slots <= 32
}
