<script setup lang="ts">
import { onMounted } from 'vue'
import { useRouter } from 'vue-router'
import { safeNext } from '@/router/redirect'
import { useAuth } from '@/stores/auth'
import { usePageTitle } from '@/composables/usePageTitle'
import Spinner from '@/ui/Spinner.vue'

// The identity provider sends the browser here with `?code=…&state=…` (or `?error=…`). This page finishes the
// sign-in and moves on; if it cannot, the sign-in page says why.
const auth = useAuth()
const router = useRouter()
usePageTitle(() => 'Signing in')

onMounted(async () => {
  const search = window.location.search
  // The one-time code must not stay in the address bar, the history or a copied link.
  window.history.replaceState(window.history.state, '', window.location.pathname)
  try {
    const next = await auth.completeSso(search)
    await router.replace(safeNext(next))
  } catch {
    // `auth.ssoMessage` holds the reason; the sign-in page shows it.
    await router.replace({ name: 'signin' })
  }
})
</script>

<template>
  <div class="wrap" data-testid="auth-callback">
    <Spinner :size="20" />
    <h1>Signing you in…</h1>
  </div>
</template>

<style scoped>
.wrap {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: var(--s-3);
  margin-top: var(--s-7);
  color: var(--text-2);
}
h1 {
  font-size: var(--fs-lg);
  font-weight: 560;
}
</style>
