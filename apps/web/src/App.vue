<script setup lang="ts">
import { computed, nextTick, ref, watch } from 'vue'
import { RouterView, useRoute, useRouter } from 'vue-router'
import AppHeader from '@/shell/AppHeader.vue'
import { useAuth } from '@/stores/auth'
import { usePrefs } from '@/stores/prefs'
import Alert from '@/ui/Alert.vue'
import ToastHost from '@/ui/ToastHost.vue'
import { useServerNow } from '@/composables/useNow'
import { formatDuration } from '@/lib/format'

const route = useRoute()
const router = useRouter()
const auth = useAuth()
const prefs = usePrefs()
prefs.applyTheme()

const main = ref<HTMLElement | null>(null)
const announcement = ref('')
const now = useServerNow()

const expiresIn = computed(() => formatDuration(Math.max(0, auth.expiresAt - now.value)))

// After a navigation, tell assistive tech where we are and move focus to the new content.
watch(
  () => route.fullPath,
  async () => {
    await nextTick()
    announcement.value = document.title
    if (route.hash) return
    main.value?.focus({ preventScroll: true })
  },
)

// When the session ends (expiry, revocation, sign-out elsewhere) go to the sign-in page.
watch(
  () => auth.isAuthenticated,
  (ok, was) => {
    if (was && !ok && !route.meta.public) {
      void router.replace({ name: 'signin', query: { next: route.fullPath } })
    }
  },
)
</script>

<template>
  <a class="skip-link" href="#main" @click.prevent="main?.focus()">Skip to content</a>
  <AppHeader />
  <div v-if="auth.isAuthenticated && auth.expiryWarned" class="expiry">
    <Alert tone="warn" live="status" title="Your credential is about to expire">
      You will be signed out in {{ expiresIn }}. Mint a new token or sign in again to keep working.
    </Alert>
  </div>
  <main id="main" ref="main" tabindex="-1" class="main">
    <RouterView />
  </main>
  <div class="sr-only" role="status" aria-live="polite" data-testid="route-announcer">
    {{ announcement }}
  </div>
  <ToastHost />
</template>

<style scoped>
.main {
  max-width: var(--content-max);
  margin: 0 auto;
  padding: var(--s-5) var(--s-4) var(--s-7);
  outline: none;
}
.expiry {
  max-width: var(--content-max);
  margin: var(--s-3) auto 0;
  padding: 0 var(--s-4);
}
</style>
