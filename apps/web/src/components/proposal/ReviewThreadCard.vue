<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { RouterLink } from 'vue-router'
import type { Proposal, ReviewThread } from '@/api/types'
import { api } from '@/api/instance'
import { randomHex } from '@/api/client'
import { describeError } from '@/api/errors'
import { useAuth } from '@/stores/auth'
import { usePagedResource } from '@/composables/usePagedResource'
import { useCommentDraft } from '@/composables/useCommentDraft'
import { samePrincipal } from '@/lib/identity'
import { shortId } from '@/lib/format'
import Alert from '@/ui/Alert.vue'
import Badge from '@/ui/Badge.vue'
import Button from '@/ui/Button.vue'
import ErrorState from '@/ui/ErrorState.vue'
import TextArea from '@/ui/TextArea.vue'
import IdentityLabel from '@/ui/IdentityLabel.vue'
import RelativeTime from '@/ui/RelativeTime.vue'

const props = defineProps<{ repo: string; proposal: Proposal; thread: ReviewThread }>()
const emit = defineEmits<{ changed: [] }>()
const auth = useAuth()
const sourceRevision = computed(() =>
  props.thread.anchor?.side === 'old' ? props.thread.anchor.base : props.thread.anchor?.head,
)
const sourceLink = computed(() => {
  const [owner, name] = props.repo.split('/')
  return {
    name: 'files',
    params: { owner, name },
    query: {
      rev: sourceRevision.value,
      path: props.thread.anchor?.path,
      line: String(props.thread.anchor?.line || 1),
    },
  }
})
const expanded = ref(false)
const allowed = computed(() => auth.can(props.repo, 'commentProposal'))
const canResolve = computed(
  () =>
    allowed.value &&
    (samePrincipal(props.thread.author, auth.identity) ||
      samePrincipal(props.proposal.author, auth.identity) ||
      auth.can(props.repo, 'review') ||
      auth.can(props.repo, 'integrate')),
)
const observedVersion = ref(0)
const replies = usePagedResource(
  async (cursor, signal) => {
    const version = props.thread.version
    const threadId = props.thread.id
    const page = await api.proposals.threadComments(
      props.repo,
      props.proposal.id,
      threadId,
      { page_size: 50, page_token: cursor },
      { signal },
    )
    if (
      !signal.aborted &&
      threadId === props.thread.id &&
      version === props.thread.version &&
      !page.nextPageToken
    )
      observedVersion.value = version
    return page
  },
  [() => props.repo, () => props.proposal.id, () => props.thread.id, () => auth.token, expanded],
  () => expanded.value,
)
const resolutionReady = computed(
  () =>
    props.thread.resolved ||
    (expanded.value &&
      replies.settled.value &&
      !replies.loading.value &&
      !replies.error.value &&
      !replies.next.value &&
      observedVersion.value === props.thread.version),
)
const draft = useCommentDraft(
  () => JSON.stringify([auth.identity, props.repo, props.proposal.id, props.thread.id]),
  () => props.thread.anchor?.head || '',
)
const busy = ref(false),
  error = ref('')
let generation = 0,
  controller: AbortController | undefined,
  resolutionKey = randomHex()
watch(
  [() => props.repo, () => props.proposal.id, () => props.thread.id, () => auth.token],
  () => {
    generation++
    controller?.abort()
    busy.value = false
    error.value = ''
    resolutionKey = randomHex()
  },
  { flush: 'sync' },
)
watch(
  () => props.thread.version,
  () => {
    resolutionKey = randomHex()
    observedVersion.value = 0
    if (expanded.value) void replies.reload()
  },
)
onBeforeUnmount(() => {
  generation++
  controller?.abort()
})
async function mutate(resolve: boolean) {
  if (
    busy.value ||
    (resolve
      ? !canResolve.value || !resolutionReady.value
      : !allowed.value || props.thread.resolved || !draft.body.value.trim())
  )
    return
  const mine = generation
  controller = new AbortController()
  busy.value = true
  error.value = ''
  const options = {
    signal: controller.signal,
    token: auth.token,
    idempotencyKey: resolve ? resolutionKey : draft.requestId.value,
    quietUnauthenticated: true,
  }
  try {
    if (resolve)
      await api.proposals.resolveThread(
        props.repo,
        props.proposal.id,
        props.thread.id,
        !props.thread.resolved,
        props.thread.version,
        options,
      )
    else
      await api.proposals.replyThread(
        props.repo,
        props.proposal.id,
        props.thread.id,
        draft.body.value,
        options,
      )
    if (mine !== generation) return
    if (!resolve) draft.clear()
    emit('changed')
    if (expanded.value) await replies.reload()
  } catch (cause) {
    if (mine === generation) {
      error.value = describeError(cause)
      emit('changed')
    }
  } finally {
    if (mine === generation) busy.value = false
  }
}
</script>

<template>
  <article
    class="thread"
    :aria-label="`Discussion on ${thread.anchor?.path || 'file'} line ${thread.anchor?.line || 0}`"
  >
    <header class="row">
      <strong
        >{{ thread.anchor?.path }}:{{ thread.anchor?.line }}
        <span class="muted">({{ thread.anchor?.side }})</span></strong
      >
      <Badge :tone="thread.resolved ? 'ok' : 'warn'">{{
        thread.resolved ? 'Resolved' : 'Open'
      }}</Badge>
      <Badge v-if="thread.anchor?.head !== proposal.head" tone="neutral">Earlier head</Badge>
    </header>
    <p class="muted">
      Revision
      <code>{{
        shortId(
          thread.anchor?.side === 'old' ? thread.anchor?.base || '' : thread.anchor?.head || '',
          12,
        )
      }}</code>
      · <IdentityLabel :identity="thread.author" /> · <RelativeTime :ms="thread.created_at_ms" />
    </p>
    <RouterLink :to="sourceLink">View source at this revision</RouterLink>
    <pre class="source">{{ thread.anchor?.text }}</pre>
    <p class="body">{{ thread.body }}</p>
    <div class="row">
      <Button size="sm" :aria-expanded="expanded" @click="expanded = !expanded">{{
        expanded ? 'Hide replies' : 'Replies and actions'
      }}</Button>
      <Button
        v-if="canResolve"
        size="sm"
        :loading="busy"
        :disabled="!resolutionReady"
        :label="thread.resolved ? 'Reopen discussion' : 'Resolve discussion'"
        disabled-reason="Read all replies before resolving this discussion"
        @click="mutate(true)"
        >{{ thread.resolved ? 'Reopen discussion' : 'Resolve discussion' }}</Button
      >
    </div>
    <p v-if="canResolve && !thread.resolved && !resolutionReady" class="muted">
      Read all replies before resolving this discussion.
    </p>
    <Alert v-if="error" tone="bad" live="alert">{{ error }}</Alert>
    <template v-if="expanded">
      <ErrorState
        v-if="replies.error.value"
        compact
        what="thread replies"
        :error="replies.error.value"
        @retry="replies.reload()"
      />
      <ol class="replies">
        <li v-for="reply in replies.data.value" :key="reply.id">
          <p class="muted">
            <IdentityLabel :identity="reply.author" /> · <RelativeTime :ms="reply.created_at_ms" />
          </p>
          <p class="body">{{ reply.body }}</p>
        </li>
      </ol>
      <p v-if="replies.loading.value" role="status">Loading replies…</p>
      <Button
        v-if="replies.next.value"
        size="sm"
        :loading="replies.loading.value"
        @click="replies.more"
        >Load more replies</Button
      >
      <form v-if="allowed && !thread.resolved" class="reply-form" @submit.prevent="mutate(false)">
        <TextArea
          v-model="draft.body.value"
          label="Reply to this discussion"
          :rows="3"
          :disabled="busy"
          hint="Saved in this tab until posted."
        />
        <Alert v-if="draft.error.value" tone="warn">{{ draft.error.value }}</Alert>
        <Button type="submit" :loading="busy" :disabled="!draft.body.value.trim()"
          >Post reply</Button
        >
      </form>
      <p v-else-if="thread.resolved" class="muted">
        Reopen this discussion before adding another reply.
      </p>
    </template>
  </article>
</template>

<style scoped>
.thread {
  display: grid;
  gap: var(--s-3);
  border: 1px solid var(--border);
  border-radius: var(--r-md);
  padding: var(--s-4);
  min-width: 0;
}
.row {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: var(--s-2);
}
.row strong {
  overflow-wrap: anywhere;
}
.source {
  white-space: pre-wrap;
  overflow-wrap: anywhere;
  padding: var(--s-3);
  background: var(--surface-2);
}
.body {
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}
.muted {
  color: var(--text-2);
}
.replies {
  list-style: none;
  margin: 0;
  padding: 0;
  display: grid;
  gap: var(--s-3);
}
.replies li {
  border-left: 2px solid var(--border);
  padding-left: var(--s-3);
}
.reply-form {
  display: grid;
  gap: var(--s-3);
  justify-items: start;
}
.reply-form :deep(.field) {
  width: 100%;
}
</style>
