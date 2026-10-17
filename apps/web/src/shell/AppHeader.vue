<script setup lang="ts">
import { computed } from 'vue'
import { RouterLink, useRouter } from 'vue-router'
import { useAuth } from '@/stores/auth'
import { useLive } from '@/stores/live'
import { usePrefs } from '@/stores/prefs'
import Avatar from '@/ui/Avatar.vue'
import Button from '@/ui/Button.vue'
import Icon from '@/ui/Icon.vue'
import LiveIndicator from '@/ui/LiveIndicator.vue'
import Menu from '@/ui/Menu.vue'
import type { MenuItem } from '@/ui/types'

const auth = useAuth()
const live = useLive()
const prefs = usePrefs()
const router = useRouter()

const themeIcon = computed(() =>
  prefs.theme === 'system' ? 'monitor' : prefs.theme === 'light' ? 'sun' : 'moon',
)
const themeLabel = computed(
  () => `Theme: ${prefs.theme === 'system' ? 'follow system' : prefs.theme}. Press to change.`,
)

const items = computed<MenuItem[]>(() => [
  { id: 'access', label: 'Credentials and roles', icon: 'key' },
  { id: 'signout', label: 'Sign out', icon: 'logout', danger: true },
])

async function onSelect(id: string) {
  if (id === 'signout') {
    auth.signOut()
    await router.replace({ name: 'signin' })
  } else if (id === 'access') {
    await router.push({ name: 'access' })
  }
}
</script>

<template>
  <header class="top">
    <div class="inner">
      <RouterLink to="/" class="brand" aria-label="AgentGit, repositories">
        <svg viewBox="0 0 32 32" width="26" height="26" aria-hidden="true" focusable="false">
          <rect width="32" height="32" rx="8" fill="var(--accent)" />
          <path
            d="M10 22V10m0 0a3 3 0 1 0 0-.01M10 22a3 3 0 1 0 0 .01M22 12a3 3 0 1 0 0-.01M22 12c0 6-12 3-12 10"
            fill="none"
            stroke="var(--accent-fg)"
            stroke-width="2.2"
            stroke-linecap="round"
            stroke-linejoin="round"
          />
        </svg>
        <span class="word">AgentGit</span>
      </RouterLink>

      <nav v-if="auth.isAuthenticated" class="nav" aria-label="Main">
        <RouterLink to="/" class="navlink" exact-active-class="current">Repositories</RouterLink>
      </nav>

      <span class="spacer" />

      <LiveIndicator v-if="live.repo" :status="live.status" :retry-in-ms="live.retryInMs" />
      <Button
        variant="ghost"
        size="md"
        :icon="themeIcon"
        :label="themeLabel"
        data-testid="theme-toggle"
        @click="prefs.cycleTheme()"
      />
      <Menu v-if="auth.isAuthenticated" label="Account" :items="items" @select="onSelect">
        <template #trigger="{ open, toggle, menuId }">
          <button
            type="button"
            class="user"
            aria-haspopup="menu"
            :aria-expanded="open"
            :aria-controls="open ? menuId : undefined"
            data-testid="user-menu"
            @click="toggle"
          >
            <Avatar :identity="auth.identity" :size="24" decorative />
            <span class="who">{{ auth.identity }}</span>
            <Icon name="chevron-down" :size="14" />
          </button>
        </template>
      </Menu>
    </div>
  </header>
</template>

<style scoped>
.top {
  position: sticky;
  top: 0;
  z-index: var(--z-sticky);
  height: var(--header-h);
  background: color-mix(in srgb, var(--surface) 92%, transparent);
  backdrop-filter: saturate(1.4) blur(8px);
  border-bottom: 1px solid var(--border);
}
.inner {
  display: flex;
  align-items: center;
  gap: var(--s-3);
  height: 100%;
  max-width: var(--content-max);
  margin: 0 auto;
  padding: 0 var(--s-4);
}
.brand {
  display: inline-flex;
  align-items: center;
  gap: var(--s-2);
  color: var(--text);
  font-weight: 700;
  letter-spacing: -0.01em;
}
.brand:hover {
  text-decoration: none;
}
.nav {
  display: flex;
  gap: var(--s-1);
  margin-left: var(--s-3);
}
.navlink {
  padding: 6px var(--s-3);
  border-radius: var(--r-sm);
  color: var(--text-2);
  font-weight: 560;
}
.navlink:hover {
  background: var(--surface-3);
  color: var(--text);
  text-decoration: none;
}
.navlink.current {
  color: var(--text);
  background: var(--surface-3);
}
.spacer {
  flex: 1;
}
.user {
  display: inline-flex;
  align-items: center;
  gap: var(--s-2);
  height: 34px;
  padding: 0 var(--s-2);
  background: transparent;
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  color: var(--text);
  max-width: 260px;
}
.user:hover {
  background: var(--surface-2);
}
.who {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-weight: 560;
}
@media (max-width: 720px) {
  .word,
  .who {
    display: none;
  }
}
</style>
