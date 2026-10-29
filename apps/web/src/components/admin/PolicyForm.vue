<script setup lang="ts">
import { computed } from 'vue'
import {
  PRESETS,
  applyPreset,
  describeChange,
  describePolicy,
  detectPreset,
  docFromForm,
  formFromDoc,
  parseDoc,
  stringifyDoc,
} from '@/lib/policy-plain'
import type { PolicyForm, PresetId } from '@/lib/policy-plain'
import Alert from '@/ui/Alert.vue'
import Button from '@/ui/Button.vue'
import Checkbox from '@/ui/Checkbox.vue'
import Icon from '@/ui/Icon.vue'
import TextField from '@/ui/TextField.vue'

/**
 * The rules of a branch as a form: a preset to start from, plain fields to adjust it, and sentences that say what
 * the result allows and denies. It edits the same JSON text as the raw editor under "Advanced" (v-model:text), and
 * anything it does not show is kept as it is.
 */
const props = defineProps<{
  /** The JSON document being edited. */
  text: string
  /** The document in force, to describe what a change does. */
  baseline: string
  disabled?: boolean
}>()
const emit = defineEmits<{ 'update:text': [text: string] }>()

const doc = computed(() => parseDoc(props.text))
const base = computed(() => parseDoc(props.baseline) ?? {})
const form = computed(() => (doc.value ? formFromDoc(doc.value) : null))
const preset = computed(() => (doc.value ? detectPreset(doc.value) : null))
const meaning = computed(() => (doc.value ? describePolicy(doc.value) : []))
const effects = computed(() =>
  doc.value && props.text.trim() !== props.baseline.trim()
    ? describeChange(base.value, doc.value)
    : [],
)

function choose(id: PresetId) {
  emit('update:text', stringifyDoc(applyPreset(id, doc.value ?? {})))
}
function set(patch: Partial<PolicyForm>) {
  if (!form.value || !doc.value) return
  emit('update:text', stringifyDoc(docFromForm({ ...form.value, ...patch }, doc.value)))
}
const int = (v: string, max = 9999) => Math.max(0, Math.min(max, Math.floor(Number(v) || 0)))

function toggleAutonomy(on: boolean) {
  if (!form.value) return
  set({
    agentsMayLand: on,
    agentPaths: form.value.agentPaths || 'docs/**',
    agentMaxFiles: form.value.agentMaxFiles || 10,
  })
}
function setProtected(i: number, patch: Partial<PolicyForm['protectedPaths'][number]>) {
  if (!form.value) return
  const rows = form.value.protectedPaths.map((r, n) => (n === i ? { ...r, ...patch } : r))
  set({ protectedPaths: rows })
}
function setTiming(index: number, patch: Partial<PolicyForm['checkTiming'][number]>) {
  if (!form.value) return
  set({
    checkTiming: form.value.checkTiming.map((row, i) => (i === index ? { ...row, ...patch } : row)),
  })
}
function addProtected() {
  if (!form.value) return
  // An empty row would be dropped when the document is written, so start with a real example.
  set({
    protectedPaths: [
      ...form.value.protectedPaths,
      { glob: 'src/security/**', minApprovals: 2, approvers: '' },
    ],
  })
}
function removeProtected(i: number) {
  if (!form.value) return
  set({ protectedPaths: form.value.protectedPaths.filter((_, n) => n !== i) })
}
</script>

<template>
  <div class="pf" data-testid="policy-form">
    <Alert v-if="!doc || !form" tone="warn" title="These rules are not valid JSON right now">
      The form comes back when the document under Advanced parses again.
    </Alert>
    <template v-else>
      <fieldset class="presets" :disabled="disabled">
        <legend>Start from a preset</legend>
        <label
          v-for="p in PRESETS"
          :key="p.id"
          class="preset"
          :class="{ on: preset === p.id }"
          :data-testid="`preset-${p.id}`"
        >
          <input
            type="radio"
            name="preset"
            :value="p.id"
            :checked="preset === p.id"
            @change="choose(p.id)"
          />
          <span class="pname">{{ p.name }}</span>
          <span class="pfor">{{ p.forWhom }}</span>
          <span class="psum">{{ p.summary }}</span>
        </label>
        <p v-if="!preset" class="faint custom" data-testid="preset-custom">
          These rules are custom: they match none of the presets. Choosing one replaces the settings
          below (who may land and the required checks are kept).
        </p>
      </fieldset>

      <fieldset class="fields" :disabled="disabled">
        <legend>Adjust</legend>
        <div class="row2">
          <TextField
            :model-value="String(form.approvalsMin)"
            label="Approvals needed for a change"
            type="number"
            :min="0"
            :max="9"
            hint="0 means a person’s change can land without review."
            @update:model-value="(v) => set({ approvalsMin: int(v, 9) })"
          />
          <TextField
            :model-value="String(form.approvalsMinForAgents)"
            label="Approvals needed for an agent’s change"
            type="number"
            :min="0"
            :max="9"
            hint="Applies when the agent goes beyond the limits below."
            @update:model-value="(v) => set({ approvalsMinForAgents: int(v, 9) })"
          />
        </div>
        <Checkbox
          :model-value="form.forbidSelfApproval"
          label="Authors may not approve their own change"
          @update:model-value="(v) => set({ forbidSelfApproval: v })"
        />
        <Checkbox
          :model-value="form.requireProvenance"
          label="An agent’s change must say, with a signature, where it came from"
          hint="Model, reason and inputs, signed by the agent."
          @update:model-value="(v) => set({ requireProvenance: v })"
        />
        <Checkbox
          :model-value="form.agentsMayLand"
          label="Agents may land small changes by themselves"
          hint="Only inside the limits you set here. Everything else still needs approval."
          @update:model-value="toggleAutonomy"
        />
        <div v-if="form.agentsMayLand" class="row2 indent">
          <TextField
            :model-value="form.agentPaths"
            label="Only for these paths"
            mono
            hint="Separate with commas, for example docs/**, tests/**."
            @update:model-value="(v) => set({ agentPaths: v })"
          />
          <TextField
            :model-value="String(form.agentMaxFiles)"
            label="At most this many files"
            type="number"
            :min="1"
            @update:model-value="(v) => set({ agentMaxFiles: int(v) })"
          />
        </div>

        <div class="prot">
          <h3>Extra care for some paths</h3>
          <p v-if="!form.protectedPaths.length" class="faint">
            None. Add a path (like <code>.github/**</code>) to ask for more approvals when a change
            touches it.
          </p>
          <ul v-else role="list">
            <li v-for="(r, i) in form.protectedPaths" :key="i">
              <TextField
                :model-value="r.glob"
                :label="`Path ${i + 1}`"
                mono
                @update:model-value="(v) => setProtected(i, { glob: v })"
              />
              <TextField
                :model-value="String(r.minApprovals)"
                label="Approvals"
                type="number"
                :min="1"
                :max="9"
                @update:model-value="(v) => setProtected(i, { minApprovals: int(v, 9) || 1 })"
              />

              <Button size="sm" variant="ghost" icon="trash" @click="removeProtected(i)"
                >Remove path {{ i + 1 }}</Button
              >
              <TextField
                class="protected-approvers"
                :model-value="r.approvers"
                :label="`Who may approve path ${i + 1}`"
                mono
                hint="Identity patterns separated by commas, for example user:security-*. Empty uses the branch’s default approver rules. This does not grant repository access."
                @update:model-value="(v) => setProtected(i, { approvers: v })"
              />
            </li>
          </ul>
          <Button size="sm" icon="plus" @click="addProtected">Add a path</Button>
        </div>

        <TextField
          :model-value="String(form.maxChangedFiles)"
          label="Most files one change may touch"
          type="number"
          :min="0"
          hint="0 means no limit."
          @update:model-value="(v) => set({ maxChangedFiles: int(v) })"
        />
        <section v-for="(check, i) in form.checkTiming" :key="i" class="check-timing">
          <h3>Timing for {{ check.check }}</h3>
          <div class="row2">
            <TextField
              :model-value="check.maxAgeSecs"
              label="Maximum result age (seconds)"
              type="number"
              :min="0"
              hint="Empty means no age limit. Older attestations will not satisfy this check."
              @update:model-value="(v) => setTiming(i, { maxAgeSecs: v })"
            />
            <TextField
              v-if="check.timeoutSecs !== null"
              :model-value="check.timeoutSecs"
              label="Command timeout (seconds)"
              type="number"
              :min="1"
              :max="86400"
              placeholder="600"
              hint="Empty uses the policy default of 600 seconds. Maximum: 86400."
              @update:model-value="(v) => setTiming(i, { timeoutSecs: v })"
            />
          </div>
          <p v-if="check.timeoutSecs === null" class="faint">
            No worker command is configured; this check needs an externally submitted attestation.
          </p>
        </section>
        <p class="faint checks" data-testid="policy-checks">
          <template v-if="form.checks.length">
            Required checks: <strong>{{ form.checks.join(', ') }}</strong
            >.
          </template>
          <template v-else>No checks are required.</template>
          Add or remove checks and configure trusted runner keys under Advanced. Timing can be
          adjusted above.
        </p>
      </fieldset>

      <section class="mean" aria-labelledby="mean-h">
        <h3 id="mean-h">What these rules mean</h3>
        <ul role="list" data-testid="policy-meaning">
          <li v-for="s in meaning" :key="s">{{ s }}</li>
        </ul>
      </section>

      <section v-if="effects.length" class="mean change" aria-labelledby="chg-h">
        <h3 id="chg-h">What saving this changes</h3>
        <ul role="list" data-testid="policy-effects">
          <li v-for="e in effects" :key="e.text" :class="`k-${e.kind}`">
            <Icon
              :name="e.kind === 'allows' ? 'check' : e.kind === 'denies' ? 'x' : 'info'"
              :size="14"
            />
            <span class="tag">{{
              e.kind === 'allows'
                ? 'Allows more'
                : e.kind === 'denies'
                  ? 'Asks for more'
                  : 'Changes'
            }}</span>
            {{ e.text }}
          </li>
        </ul>
        <p class="faint">
          Check results made under the old rules stop counting, on purpose: the checks may have
          meant something different.
        </p>
      </section>
    </template>
  </div>
</template>

<style scoped>
.pf {
  display: flex;
  flex-direction: column;
  gap: var(--s-4);
}
fieldset {
  border: 0;
  padding: 0;
  margin: 0;
  min-width: 0;
}
legend {
  font-weight: 600;
  padding: 0;
  margin-bottom: var(--s-2);
}
.presets {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(210px, 1fr));
  gap: var(--s-3);
}
.presets legend {
  grid-column: 1 / -1;
}
.preset {
  position: relative;
  display: grid;
  gap: var(--s-1);
  padding: var(--s-3);
  border: 1px solid var(--border-strong);
  border-radius: var(--r-md);
  background: var(--surface);
  cursor: pointer;
  align-content: start;
}
.preset.on {
  border-color: var(--accent);
  box-shadow: 0 0 0 1px var(--accent);
  background: var(--accent-soft, var(--surface-2));
}
.preset:focus-within {
  outline: 2px solid var(--focus);
  outline-offset: 2px;
}
.preset input {
  position: absolute;
  opacity: 0;
  pointer-events: none;
}
.pname {
  font-weight: 650;
}
.pfor {
  font-size: var(--fs-sm);
  color: var(--text);
}
.psum {
  font-size: var(--fs-xs);
  color: var(--text-2);
}
.custom {
  grid-column: 1 / -1;
  font-size: var(--fs-sm);
}
.fields {
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
}
.row2 {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(220px, 1fr));
  gap: var(--s-3);
}
.indent {
  margin-left: var(--s-6);
}
.prot h3,
.mean h3 {
  font-size: var(--fs-md, 1rem);
  margin: 0 0 var(--s-2);
}
.prot ul {
  list-style: none;
  margin: 0 0 var(--s-2);
  padding: 0;
  display: grid;
  gap: var(--s-2);
}
.prot li {
  display: grid;
  grid-template-columns: minmax(0, 2fr) minmax(0, 1fr) auto;
  gap: var(--s-2);
  align-items: end;
}
.protected-approvers {
  grid-column: 1 / -1;
}
.check-timing h3 {
  font-size: var(--fs-sm);
  margin: 0 0 var(--s-2);
}
.checks {
  font-size: var(--fs-sm);
}
.mean {
  padding: var(--s-3) var(--s-4);
  background: var(--surface-2);
  border: 1px solid var(--border);
  border-radius: var(--r-md);
}
.mean ul {
  list-style: disc;
  margin: 0;
  padding-left: var(--s-4);
  display: grid;
  gap: var(--s-1);
  font-size: var(--fs-sm);
}
.change {
  border-color: var(--accent);
}
.change ul {
  list-style: none;
  padding: 0;
}
.change li {
  display: flex;
  gap: var(--s-2);
  align-items: baseline;
  flex-wrap: wrap;
}
.tag {
  font-weight: 650;
  font-size: var(--fs-xs);
}
.k-allows .tag {
  color: var(--ok-text, var(--ok));
}
.k-denies .tag {
  color: var(--bad-text, var(--bad));
}
@media (max-width: 640px) {
  .prot li {
    grid-template-columns: 1fr;
  }
  .indent {
    margin-left: 0;
  }
}
</style>
