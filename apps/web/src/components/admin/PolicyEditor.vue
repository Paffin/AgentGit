<script setup lang="ts">
import { computed, nextTick, ref, watch } from 'vue'
import { api } from '@/api/instance'
import { ApiError, describeError } from '@/api/errors'
import type { PolicyVersion } from '@/api/types'
import { useResource } from '@/composables/useResource'
import { diffText, diffStats } from '@/lib/linediff'
import { analyzePolicy, POLICY_STARTER, prettyPolicy } from '@/lib/policy'
import type { PolicyIssue } from '@/lib/policy'
import { shortId } from '@/lib/format'
import { useAuth } from '@/stores/auth'
import { useBoard } from '@/stores/board'
import { useToasts } from '@/stores/toasts'
import Alert from '@/ui/Alert.vue'
import Badge from '@/ui/Badge.vue'
import Button from '@/ui/Button.vue'
import Card from '@/ui/Card.vue'
import EmptyState from '@/ui/EmptyState.vue'
import ErrorState from '@/ui/ErrorState.vue'
import Icon from '@/ui/Icon.vue'
import RelativeTime from '@/ui/RelativeTime.vue'
import SelectField from '@/ui/SelectField.vue'

const props = defineProps<{ repo: string }>()
const auth = useAuth()
const board = useBoard()
const toasts = useToasts()

const stream = ref('')
watch(
  () => board.streams,
  (list) => {
    if (!stream.value || !list.some((s) => s.name === stream.value))
      stream.value = board.repo?.default_stream ?? list[0]?.name ?? ''
  },
  { immediate: true },
)
const streamOptions = computed(() => board.streams.map((s) => ({ value: s.name, label: s.name })))

// The server needs a stream name; wait until the repository's streams are known.
const known = () => stream.value !== ''
const current = useResource<PolicyVersion>(() => api.admin.policy(props.repo, stream.value), {
  watch: [() => props.repo, stream],
  enabled: known,
})
const history = useResource<PolicyVersion[]>(
  () => api.admin.policyHistory(props.repo, stream.value),
  { watch: [() => props.repo, stream], enabled: known },
)

const text = ref('')
const loadedVersion = ref(-1)
watch(
  () => current.data.value,
  (c) => {
    if (!c) return
    // Do not clobber what the person is typing when the same version is re-read after an event.
    if (c.version !== loadedVersion.value || text.value === '') {
      loadedVersion.value = c.version
      text.value =
        c.version === 0 || !c.document ? POLICY_STARTER : (prettyPolicy(c.document) ?? c.document)
    }
  },
  { immediate: true },
)
watch(stream, () => {
  loadedVersion.value = -1
  text.value = ''
})

const baseline = computed(() => {
  const c = current.data.value
  return !c || c.version === 0 || !c.document ? '' : (prettyPolicy(c.document) ?? c.document)
})
const dirty = computed(() => text.value.trim() !== baseline.value.trim())
const issues = computed<PolicyIssue[]>(() => analyzePolicy(text.value, { me: auth.identity }))
const worst = computed(() =>
  issues.value.some((i) => i.level === 'error')
    ? 'error'
    : issues.value.some((i) => i.level === 'warning')
      ? 'warning'
      : 'ok',
)
const lineLevels = computed(() => {
  const m = new Map<number, PolicyIssue['level']>()
  for (const i of issues.value) {
    if (i.line === null) continue
    const prev = m.get(i.line)
    if (
      !prev ||
      (prev !== 'error' && i.level === 'error') ||
      (prev === 'hint' && i.level === 'warning')
    )
      m.set(i.line, i.level)
  }
  return m
})
const lines = computed(() => Math.max(1, text.value.split('\n').length))

// ---- editor mechanics -----------------------------------------------------------------------------------
const area = ref<HTMLTextAreaElement | null>(null)
const gutter = ref<HTMLElement | null>(null)
function syncScroll() {
  if (gutter.value && area.value) gutter.value.scrollTop = area.value.scrollTop
}
async function jump(i: PolicyIssue) {
  if (i.line === null || !area.value) return
  const rows = text.value.split('\n')
  let offset = 0
  for (let n = 0; n < i.line - 1; n++) offset += (rows[n]?.length ?? 0) + 1
  offset += Math.max(0, (i.col ?? 1) - 1)
  area.value.focus()
  area.value.setSelectionRange(offset, offset)
  await nextTick()
  area.value.scrollTop = Math.max(0, (i.line - 3) * 20)
  syncScroll()
}

function format() {
  const p = prettyPolicy(text.value)
  if (p) text.value = p
  else toasts.warn('Cannot format', 'Fix the JSON syntax error first.')
}
function reset() {
  text.value = baseline.value || POLICY_STARTER
  saveError.value = null
}

// ---- save -------------------------------------------------------------------------------------------
const saving = ref(false)
const saveError = ref<ApiError | null>(null)
async function save() {
  saving.value = true
  saveError.value = null
  try {
    const v = await api.admin.setPolicy(props.repo, stream.value, text.value)
    toasts.ok(
      `Policy version ${v.version} saved`,
      'Attestations made under an earlier version no longer count, on purpose.',
    )
    loadedVersion.value = -1
    await Promise.all([current.reload(), history.reload()])
    board.touched(['policy', 'streams', 'prepare-all'])
  } catch (e) {
    saveError.value =
      e instanceof ApiError
        ? e
        : new ApiError({ status: 0, code: 'internal', message: describeError(e) })
  } finally {
    saving.value = false
  }
}

// ---- version history & diff ---------------------------------------------------------------------------
const versions = computed(() =>
  [...(history.data.value ?? [])].sort((a, b) => b.version - a.version),
)
const versionOptions = computed(() =>
  versions.value.map((v) => ({ value: String(v.version), label: `v${v.version}` })),
)
const from = ref('')
const to = ref('')
watch(
  versions,
  (list) => {
    if (list.length === 0) return
    if (!list.some((v) => String(v.version) === to.value)) to.value = String(list[0]?.version)
    if (!list.some((v) => String(v.version) === from.value))
      from.value = String(list[1]?.version ?? list[0]?.version)
  },
  { immediate: true },
)
const docOf = (v: string) => {
  const found = versions.value.find((x) => String(x.version) === v)
  return found ? (prettyPolicy(found.document) ?? found.document) : ''
}
const ops = computed(() =>
  from.value && to.value ? diffText(docOf(from.value), docOf(to.value)) : [],
)
const stats = computed(() => diffStats(ops.value))
// Show changed lines with two lines of context.
const shown = computed(() => {
  const keep = new Set<number>()
  ops.value.forEach((o, i) => {
    if (o.kind !== 'eq')
      for (let k = Math.max(0, i - 2); k <= Math.min(ops.value.length - 1, i + 2); k++) keep.add(k)
  })
  const out: { op: (typeof ops.value)[number]; gap: boolean }[] = []
  let last = -2
  ops.value.forEach((o, i) => {
    if (!keep.has(i)) return
    out.push({ op: o, gap: i !== last + 1 && out.length > 0 })
    last = i
  })
  return out
})
function loadVersion(v: PolicyVersion) {
  text.value = prettyPolicy(v.document) ?? v.document
  toasts.info(`Loaded version ${v.version} into the editor`, 'Saving it creates a new version.')
}
const SIGIL = { add: '+', del: '−', eq: ' ' } as const
</script>

<template>
  <div class="policy" data-testid="policy-editor">
    <Card
      title="Policy"
      icon="shield"
      :subtitle="`Versioned and append-only. ${current.data.value ? `Version ${current.data.value.version} is in force${current.data.value.version === 0 ? ' (the built-in default)' : ''}.` : ''}`"
    >
      <template #actions>
        <SelectField
          v-if="streamOptions.length > 1"
          v-model="stream"
          label="Stream"
          :options="streamOptions"
          hide-label
          class="stream-select"
        />
        <Badge v-else mono icon="branch">{{ stream }}</Badge>
      </template>
      <ErrorState
        v-if="current.error.value && !current.data.value"
        what="the policy"
        :error="current.error.value"
        compact
        @retry="current.reload()"
      />
      <div v-else class="edit">
        <div class="toolbar">
          <Button size="sm" icon="sliders" @click="format">Format</Button>
          <Button size="sm" variant="ghost" icon="refresh" :disabled="!dirty" @click="reset"
            >Revert edits</Button
          >
          <span class="spacer" />
          <span class="state faint" aria-live="polite">{{
            dirty ? 'Unsaved changes' : 'No changes'
          }}</span>
          <Button
            variant="primary"
            icon="check"
            :loading="saving"
            :disabled="!dirty"
            data-testid="policy-save"
            @click="save"
            >Save new version</Button
          >
        </div>
        <div class="editor" :class="`w-${worst}`">
          <div ref="gutter" class="gutter" aria-hidden="true">
            <div v-for="n in lines" :key="n" class="ln" :class="lineLevels.get(n)">
              {{ n }}
            </div>
          </div>
          <textarea
            ref="area"
            v-model="text"
            class="ta mono"
            spellcheck="false"
            autocomplete="off"
            wrap="off"
            aria-label="Policy document, JSON"
            aria-describedby="policy-issues"
            data-testid="policy-text"
            @scroll="syncScroll"
          />
        </div>
        <section
          id="policy-issues"
          class="issues"
          aria-label="Checks on this document"
          data-testid="policy-issues"
        >
          <h3 class="sr-only">Checks on this document</h3>
          <ul role="list">
            <li v-for="(i, n) in issues" :key="n" :class="`l-${i.level}`">
              <Icon
                :name="i.level === 'error' ? 'x' : i.level === 'warning' ? 'alert' : 'info'"
                :size="14"
              />
              <button v-if="i.line !== null" type="button" class="where" @click="jump(i)">
                line {{ i.line }}
              </button>
              <span>{{ i.message }}</span>
            </li>
          </ul>
        </section>
        <Alert
          v-if="saveError"
          tone="bad"
          live="alert"
          title="The server did not accept this policy"
          data-testid="policy-server-error"
        >
          <p>{{ saveError.message }}</p>
          <p class="faint">
            The document was not stored. The server is strict: unknown fields and policies that can
            never be satisfied are refused.
          </p>
        </Alert>
      </div>
    </Card>

    <Card
      title="Version history"
      icon="history"
      :subtitle="
        versions.length
          ? `${versions.length} version${versions.length === 1 ? '' : 's'}`
          : undefined
      "
    >
      <ErrorState
        v-if="history.error.value && !history.data.value"
        what="the history"
        :error="history.error.value"
        compact
        @retry="history.reload()"
      />
      <EmptyState
        v-else-if="versions.length === 0"
        icon="history"
        compact
        title="No policy has been stored"
        description="The built-in default applies: anyone but agents may integrate, agents need a human approval."
      />
      <template v-else>
        <ul class="versions" role="list">
          <li v-for="v in versions" :key="v.version">
            <Badge :tone="v.version === versions[0]?.version ? 'accent' : 'neutral'"
              >v{{ v.version }}</Badge
            >
            <span class="faint">{{ v.created_by }} · <RelativeTime :ms="v.created_at_ms" /></span>
            <code class="mono faint">{{ shortId(v.doc_hash, 10) }}</code>
            <Button size="sm" variant="ghost" @click="loadVersion(v)">Load into editor</Button>
          </li>
        </ul>
        <div v-if="versions.length > 1" class="cmp">
          <div class="pick">
            <SelectField v-model="from" label="Compare" :options="versionOptions" />
            <SelectField v-model="to" label="with" :options="versionOptions" />
            <p class="faint">+{{ stats.additions }} −{{ stats.deletions }}</p>
          </div>
          <div
            class="diff"
            role="group"
            aria-label="Difference between the two versions"
            data-testid="policy-diff"
          >
            <p v-if="shown.length === 0" class="faint pad">The two versions are identical.</p>
            <template v-for="(s, i) in shown" :key="i">
              <div v-if="s.gap" class="gapline faint">⋯</div>
              <div class="drow mono" :class="`k-${s.op.kind}`">
                <span class="dno" aria-hidden="true">{{ s.op.aNo ?? '' }}</span>
                <span class="dno" aria-hidden="true">{{ s.op.bNo ?? '' }}</span>
                <span class="dsg" aria-hidden="true">{{ SIGIL[s.op.kind] }}</span>
                <span v-if="s.op.kind === 'add'" class="sr-only">added: </span>
                <span v-else-if="s.op.kind === 'del'" class="sr-only">removed: </span>
                <span class="dtx">{{ s.op.text }}</span>
              </div>
            </template>
          </div>
        </div>
      </template>
    </Card>
  </div>
</template>

<style scoped>
.policy {
  display: flex;
  flex-direction: column;
  gap: var(--s-4);
}
.stream-select {
  width: 10rem;
}
.edit {
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
}
.toolbar {
  display: flex;
  align-items: center;
  gap: var(--s-2);
  flex-wrap: wrap;
}
.spacer {
  flex: 1;
}
.state {
  font-size: var(--fs-sm);
}
.editor {
  display: flex;
  border: 1px solid var(--border-strong);
  border-radius: var(--r-sm);
  background: var(--code-bg);
  height: 420px;
  overflow: hidden;
}
.editor:focus-within {
  outline: 2px solid var(--focus);
  outline-offset: 1px;
}
.editor.w-error {
  border-color: var(--bad);
}
.editor.w-warning {
  border-color: var(--warn);
}
.gutter {
  flex: none;
  width: 3.6rem;
  overflow: hidden;
  padding: var(--s-2) 0;
  background: var(--surface-2);
  border-right: 1px solid var(--border);
  font-family: var(--font-mono);
  font-size: var(--fs-sm);
  line-height: 20px;
  text-align: right;
  color: var(--text-3);
  user-select: none;
}
.ln {
  padding-right: var(--s-2);
  height: 20px;
}
.ln.error {
  background: var(--bad-soft);
  color: var(--bad);
  font-weight: 700;
}
.ln.warning {
  background: var(--warn-soft);
  color: var(--warn);
  font-weight: 700;
}
.ta {
  flex: 1;
  border: none;
  background: transparent;
  resize: none;
  padding: var(--s-2) var(--s-3);
  font-size: var(--fs-sm);
  line-height: 20px;
  tab-size: 2;
  white-space: pre;
  overflow: auto;
  outline: none;
  min-width: 0;
}
.issues ul {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 6px;
  font-size: var(--fs-sm);
}
.issues li {
  display: flex;
  gap: 8px;
  align-items: baseline;
}
.issues li :deep(.icon) {
  align-self: center;
  flex: none;
}
.l-error {
  color: var(--bad);
}
.l-warning {
  color: var(--warn);
}
.l-hint {
  color: var(--text-2);
}
.where {
  background: none;
  border: 1px solid currentColor;
  border-radius: var(--r-sm);
  padding: 0 6px;
  color: inherit;
  font-size: var(--fs-xs);
  font-weight: 600;
}
.versions {
  list-style: none;
  margin: 0 0 var(--s-4);
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.versions li {
  display: flex;
  align-items: center;
  gap: var(--s-3);
  flex-wrap: wrap;
  font-size: var(--fs-sm);
}
.cmp {
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
}
.pick {
  display: flex;
  gap: var(--s-3);
  align-items: flex-end;
}
.diff {
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  overflow: auto;
  max-height: 360px;
  font-size: var(--fs-sm);
  line-height: 20px;
}
.drow {
  display: flex;
  white-space: pre;
  min-width: max-content;
}
.dno {
  flex: none;
  width: 3.4ch;
  padding: 0 6px;
  text-align: right;
  color: var(--text-3);
  background: var(--surface-2);
  user-select: none;
}
.dsg {
  flex: none;
  width: 2ch;
  text-align: center;
  user-select: none;
}
.dtx {
  padding-right: var(--s-4);
}
.k-add {
  background: var(--diff-add-bg);
}
.k-del {
  background: var(--diff-del-bg);
}
.gapline {
  text-align: center;
  background: var(--diff-hunk-bg);
  font-size: var(--fs-xs);
}
.pad {
  padding: var(--s-3);
}
</style>
