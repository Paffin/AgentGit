// Turns the audit-backed event feed into sentences. Topics are the audit actions
// (docs/protocol/agwp-v1.md "Events"); unknown topics still render, just less prettily.
import type { ServerEvent } from '@/api/types'
import type { IconName } from '@/ui/icons'

export type EventTone = 'neutral' | 'ok' | 'warn' | 'bad' | 'info' | 'agent'
export type ObjectKind = 'proposal' | 'stream' | 'lease' | 'run' | 'task' | 'changeset' | 'other'

export interface EventView {
  seq: number
  timestamp: number
  topic: string
  actor: string
  icon: IconName
  tone: EventTone
  /** Sentence fragment between the actor and the object: "opened proposal". */
  verb: string
  /** The thing acted upon, as plain text. */
  object: string
  objectKind: ObjectKind
  /** Extra plain-text context: "src/auth/**", "approve", … */
  note: string
  /** Housekeeping the feed can dim or hide. */
  minor: boolean
  /** Ids this event relates to (proposal id, stream name…) for filtering a per-proposal timeline. */
  refs: string[]
  /** When it arrived over the live stream (ms); 0 for events loaded as history. */
  liveAt: number
}

type Details = Record<string, unknown>

const s = (v: unknown): string =>
  typeof v === 'string' ? v : typeof v === 'number' ? String(v) : ''

function payloadOf(ev: ServerEvent): { actor: string; target: string; details: Details } {
  const p = (ev.payload ?? {}) as Record<string, unknown>
  const details = p.details && typeof p.details === 'object' ? (p.details as Details) : {}
  return { actor: s(p.actor), target: s(p.target), details }
}

const REVIEW_VERB: Record<string, string> = {
  approve: 'approved',
  request_changes: 'requested changes on',
  comment: 'commented on',
}

export function describeEvent(ev: ServerEvent): EventView {
  const { actor, target, details } = payloadOf(ev)
  const view: EventView = {
    seq: ev.seq,
    timestamp: ev.timestamp_ms,
    topic: ev.topic,
    actor,
    icon: 'dot',
    tone: 'neutral',
    verb: ev.topic,
    object: target,
    objectKind: 'other',
    note: '',
    minor: false,
    refs: [target].filter(Boolean),
    liveAt: 0,
  }
  const set = (v: Partial<EventView>) => Object.assign(view, v)

  switch (ev.topic) {
    case 'repo.init':
      set({ icon: 'database', verb: 'created the repository' })
      break
    case 'stream.create':
      set({ icon: 'branch', verb: 'created stream', objectKind: 'stream' })
      break
    case 'stream.integrate':
      set({
        icon: 'merge',
        tone: 'ok',
        verb: 'integrated',
        object: s(details.proposal) || target,
        objectKind: 'proposal',
        note: `into ${target}${details.version ? ` (v${s(details.version)})` : ''}`,
        refs: [s(details.proposal), target, s(details.operation_id)].filter(Boolean),
      })
      break
    case 'stream.cas':
    case 'stream.adopt':
      set({
        icon: 'alert',
        tone: 'warn',
        verb: 'moved stream',
        objectKind: 'stream',
        note: s(details.reason),
      })
      break
    case 'proposal.create':
      set({
        icon: 'proposal',
        tone: 'info',
        verb: 'opened proposal',
        objectKind: 'proposal',
        note: s(details.stream) ? `→ ${s(details.stream)}` : '',
      })
      break
    case 'proposal.update':
      set({
        icon: 'refresh',
        tone: 'info',
        verb: 'updated proposal',
        objectKind: 'proposal',
        note: 'new head',
      })
      break
    case 'proposal.close':
      set({ icon: 'x', verb: 'closed proposal', objectKind: 'proposal' })
      break
    case 'proposal.review': {
      const d = s(details.decision)
      set({
        icon: d === 'approve' ? 'check' : d === 'request_changes' ? 'alert' : 'comment',
        tone: d === 'approve' ? 'ok' : d === 'request_changes' ? 'warn' : 'neutral',
        verb: REVIEW_VERB[d] ?? 'reviewed',
        objectKind: 'proposal',
      })
      break
    }
    case 'integrate.blocked':
      set({
        icon: 'alert',
        tone: 'warn',
        verb: 'tried to integrate; blocked by policy',
        object: '',
        objectKind: 'other',
        note: blockerNote(details),
        refs: [],
      })
      break
    case 'integrate.denied':
      set({
        icon: 'shield',
        tone: 'bad',
        verb: 'was refused integration by policy',
        object: '',
        note: blockerNote(details),
        refs: [],
      })
      break
    case 'integrate.abort':
      set({ icon: 'x', verb: 'aborted an integration', object: '', refs: [] })
      break
    case 'queue.enqueue':
      set({
        icon: 'stack',
        tone: 'info',
        verb: 'queued',
        objectKind: 'proposal',
        note: s(details.stream) ? `for ${s(details.stream)}` : '',
      })
      break
    case 'attestation.submit': {
      const pass = s(details.result) === 'pass'
      set({
        icon: pass ? 'shield' : 'x',
        tone: pass ? 'ok' : 'bad',
        verb: `reported check “${s(details.check)}” ${pass ? 'passed' : 'failed'} on tree`,
        object: target.slice(0, 10),
        refs: [target],
      })
      break
    }
    case 'job.enqueue':
      set({
        icon: 'terminal',
        tone: 'info',
        verb: `queued validation “${s(details.check)}” for`,
        objectKind: 'proposal',
        object: s(details.proposal) || target,
        refs: [s(details.proposal), target, s(details.tree)].filter(Boolean),
      })
      break
    case 'job.claim':
      set({
        icon: 'play',
        tone: 'info',
        verb: 'picked up validation job',
        object: target,
        minor: true,
        note: s(details.worker),
      })
      break
    case 'job.complete': {
      const result = (details.result ?? {}) as Record<string, unknown>
      const pass = s(result.outcome) !== 'fail'
      set({
        icon: pass ? 'shield' : 'x',
        tone: pass ? 'ok' : 'bad',
        verb: `finished validation job (${pass ? 'passed' : 'failed'})`,
        object: target,
      })
      break
    }
    case 'job.fail':
      set({
        icon: 'x',
        tone: 'bad',
        verb: 'failed validation job',
        object: target,
        note: s(details.error),
      })
      break
    case 'provenance.attach':
      set({
        icon: 'sparkle',
        tone: 'agent',
        verb: 'attached provenance to',
        objectKind: 'changeset',
        object: target.slice(0, 10),
        note: s(details.model),
        refs: [target],
      })
      break
    case 'lease.acquire':
      set({
        icon: details.exclusive === true ? 'lock' : 'unlock',
        tone: details.exclusive === true ? 'warn' : 'info',
        verb: `took a${details.exclusive === true ? 'n exclusive' : ' soft'} lease on`,
        objectKind: 'lease',
        object: s(details.scope) || target,
        note:
          details.exclusive === true && details.token !== undefined
            ? `token ${s(details.token)}`
            : '',
        refs: [target],
      })
      break
    case 'lease.release':
      set({ icon: 'unlock', verb: 'released lease', objectKind: 'lease' })
      break
    case 'run.start':
      set({
        icon: 'play',
        tone: 'agent',
        verb: 'started run',
        objectKind: 'run',
        note: s(details.task) ? 'on a task' : '',
      })
      break
    case 'run.finish':
      set({
        icon: 'check',
        tone: s(details.status) === 'completed' ? 'ok' : 'warn',
        verb: `finished run (${s(details.status) || 'done'})`,
        objectKind: 'run',
      })
      break
    case 'run.abandon':
      set({
        icon: 'alert',
        tone: 'warn',
        verb: 'abandoned run',
        objectKind: 'run',
        note: 'no heartbeat',
      })
      break
    case 'task.create':
      set({
        icon: 'task',
        verb: 'created task',
        objectKind: 'task',
        object: s(details.title) || target,
        refs: [target],
      })
      break
    case 'task.depend':
      set({
        icon: 'link',
        verb: 'made task',
        objectKind: 'task',
        note: 'depend on another',
        minor: true,
      })
      break
    case 'task.finish':
      set({
        icon: 'task',
        tone: s(details.state) === 'done' ? 'ok' : 'neutral',
        verb: `marked task ${s(details.state) || 'finished'}`,
        objectKind: 'task',
      })
      break
    case 'agent.register':
      set({ icon: 'bot', tone: 'agent', verb: 'registered agent', object: target })
      break
    case 'agent.disable':
    case 'agent.enable':
      set({
        icon: 'bot',
        tone: 'warn',
        verb: ev.topic === 'agent.disable' ? 'disabled agent' : 'enabled agent',
      })
      break
    case 'policy.set':
      set({
        icon: 'shield',
        tone: 'info',
        verb: 'set the policy of',
        objectKind: 'stream',
        note: details.version !== undefined ? `version ${s(details.version)}` : '',
      })
      break
    case 'changeset.publish':
      set({
        icon: 'commit',
        verb: 'published changeset',
        objectKind: 'changeset',
        object: target.slice(0, 10),
        minor: true,
        refs: [target],
      })
      break
    case 'publish.reject':
      set({
        icon: 'alert',
        tone: 'bad',
        verb: 'had a publication rejected',
        note: s(details.reason),
      })
      break
    case 'pin.set':
      set({ icon: 'pin', verb: 'pinned', object: target })
      break
    case 'pin.remove':
      set({ icon: 'pin', verb: 'removed pin', object: target })
      break
    case 'gc.run':
      set({
        icon: 'trash',
        verb: 'ran garbage collection',
        object: '',
        note: details.deleted !== undefined ? `${s(details.deleted)} objects removed` : '',
        minor: true,
      })
      break
    case 'workspace.create':
    case 'workspace.modify':
    case 'workspace.snapshot':
    case 'workspace.sync':
    case 'workspace.sync_conflict':
    case 'workspace.remove':
      set({
        icon: 'edit',
        verb: ev.topic.replace('workspace.', 'workspace · '),
        object: target,
        minor: true,
      })
      break
    default:
      break
  }
  return view
}

function blockerNote(details: Details): string {
  const blockers = Array.isArray(details.blockers)
    ? (details.blockers as Array<Record<string, unknown>>)
    : []
  const codes = blockers.map((b) => s(b.code)).filter(Boolean)
  return codes.join(', ')
}

/** Does this event belong in the timeline of a given proposal? */
export function eventConcernsProposal(v: EventView, proposalId: string, head?: string): boolean {
  if (v.refs.includes(proposalId)) return true
  if (head && v.refs.includes(head)) return true
  return false
}

/** Topics that can change the board; used to decide what to refresh. */
export type Section =
  | 'proposals'
  | 'streams'
  | 'runs'
  | 'leases'
  | 'tasks'
  | 'queue'
  | 'predict'
  | 'policy'
  | 'prepare-all'
  | 'agents'

export function sectionsFor(topic: string): Section[] {
  const t = topic.split('.')[0] ?? topic
  switch (t) {
    case 'proposal':
      return ['proposals', 'predict', 'queue']
    case 'stream':
    case 'integrate':
      return ['streams', 'proposals', 'prepare-all', 'predict', 'queue']
    case 'changeset':
      return ['prepare-all']
    case 'attestation':
    case 'provenance':
    case 'job':
      return ['prepare-all']
    case 'policy':
      return ['policy', 'streams', 'prepare-all']
    case 'queue':
      return ['queue', 'proposals']
    case 'lease':
      return ['leases', 'predict', 'prepare-all']
    case 'run':
      return ['runs', 'tasks', 'leases']
    case 'agent':
      return ['agents', 'runs']
    case 'task':
      return ['tasks', 'predict']
    default:
      return []
  }
}
