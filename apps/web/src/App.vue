<script setup lang="ts">
import { computed, nextTick, ref, watch } from 'vue'
import { RouterView, START_LOCATION, useRoute, useRouter } from 'vue-router'
import AppHeader from '@/shell/AppHeader.vue'
import VersionBanner from '@/shell/VersionBanner.vue'
import { useAuth } from '@/stores/auth'
import { usePrefs } from '@/stores/prefs'
import Alert from '@/ui/Alert.vue'
import Button from '@/ui/Button.vue'
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

const reauthBusy = ref(false)
async function reauthenticate() {
  reauthBusy.value = true
  try {
    await auth.reauthenticate(route.fullPath)
  } catch {
    reauthBusy.value = false
  }
}

// After a page navigation, announce it and focus the new content. Query/hash changes retain focus. The page's
// first load keeps focus where the browser put it, so the first Tab reaches the "Skip to content" link.
router.afterEach(async (to, from) => {
  await nextTick()
  if (to.path === from.path) return
  announcement.value = document.title
  if (from === START_LOCATION || to.hash) return
  main.value?.focus({ preventScroll: true })
})

// When the session ends while it is in use (the server answered 401, or the credential ran out) say so on a page
// of its own; a sign-out the person chose goes to the sign-in page.
watch(
  () => auth.isAuthenticated,
  (ok, was) => {
    if (was && !ok && !route.meta.public) {
      void router.replace({
        name: auth.endedBecause === 'expired' ? 'unauthorized' : 'signin',
        query: { next: route.fullPath },
      })
    }
  },
)
</script>

<template>
  <a class="skip-link" href="#main" @click.prevent="main?.focus()">Skip to content</a>
  <AppHeader />
  <VersionBanner />
  <div v-if="auth.isAuthenticated && auth.expiryWarned" class="expiry">
    <Alert tone="warn" live="status" title="Your credential is about to expire">
      You will be signed out in {{ expiresIn }}.
      <template v-if="auth.method === 'oidc'">
        Sign in again to keep working; you come back to this page.
        <Button size="sm" data-testid="reauth" :loading="reauthBusy" @click="reauthenticate">
          Sign in again
        </Button>
      </template>
      <template v-else>Mint a new token or sign in again to keep working.</template>
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
