import { createRouter, createWebHistory } from 'vue-router'
import type { RouteRecordRaw, Router } from 'vue-router'
import { useAuth } from '@/stores/auth'
import { ApiError } from '@/api/errors'

declare module 'vue-router' {
  interface RouteMeta {
    /** Reachable without a session. */
    public?: boolean
    /** Base of the document title; views refine it with `usePageTitle`. */
    title?: string
  }
}

export const routes: RouteRecordRaw[] = [
  {
    path: '/signin',
    name: 'signin',
    component: () => import('@/views/SignInView.vue'),
    meta: { public: true, title: 'Sign in' },
  },
  {
    path: '/',
    name: 'repos',
    component: () => import('@/views/ReposView.vue'),
    meta: { title: 'Repositories' },
  },
  {
    path: '/access',
    name: 'access',
    component: () => import('@/views/AccessView.vue'),
    meta: { title: 'Access' },
  },
  {
    path: '/repos/:owner/:name',
    component: () => import('@/views/RepoLayout.vue'),
    props: true,
    children: [
      {
        path: '',
        name: 'board',
        component: () => import('@/views/BoardView.vue'),
        meta: { title: 'Live board' },
      },
      {
        path: 'proposals',
        name: 'proposals',
        component: () => import('@/views/ProposalsView.vue'),
        meta: { title: 'Proposals' },
      },
      {
        path: 'proposals/:id',
        name: 'proposal',
        component: () => import('@/views/ProposalView.vue'),
        props: (route) => ({ id: String(route.params.id) }),
        meta: { title: 'Proposal' },
      },
      {
        path: 'streams',
        name: 'streams',
        component: () => import('@/views/StreamsView.vue'),
        meta: { title: 'Streams' },
      },
      {
        path: 'history',
        name: 'history',
        component: () => import('@/views/HistoryView.vue'),
        meta: { title: 'History' },
      },
      {
        path: 'files',
        name: 'files',
        component: () => import('@/views/FilesView.vue'),
        meta: { title: 'Files' },
      },
      {
        path: 'compare',
        name: 'compare',
        component: () => import('@/views/CompareView.vue'),
        meta: { title: 'Compare' },
      },
      {
        path: 'agents',
        name: 'agents',
        component: () => import('@/views/AgentsView.vue'),
        meta: { title: 'Agents' },
      },
      {
        path: 'admin',
        name: 'admin',
        component: () => import('@/views/RepoAdminView.vue'),
        meta: { title: 'Admin' },
      },
    ],
  },
  {
    path: '/forbidden',
    name: 'forbidden',
    component: () => import('@/views/ForbiddenView.vue'),
    meta: { title: 'No access' },
  },
  {
    path: '/offline',
    name: 'offline',
    component: () => import('@/views/OfflineView.vue'),
    meta: { public: true, title: 'Offline' },
  },
  {
    path: '/:pathMatch(.*)*',
    name: 'not-found',
    component: () => import('@/views/NotFoundView.vue'),
    meta: { public: true, title: 'Not found' },
  },
]

export function createAppRouter(history = createWebHistory()): Router {
  const router = createRouter({ history, routes })

  router.beforeEach(async (to) => {
    const auth = useAuth()
    if (auth.status === 'anonymous' && auth.token === null) {
      try {
        await auth.restore()
      } catch (e) {
        // The server is unreachable while resuming a saved session: say so instead of showing a sign-in form.
        if (e instanceof ApiError && e.isNetwork && to.name !== 'offline') {
          return { name: 'offline', query: { next: to.fullPath } }
        }
      }
    }
    if (to.meta.public) {
      if (to.name === 'signin' && auth.isAuthenticated)
        return typeof to.query.next === 'string' ? safeNext(to.query.next) : { name: 'repos' }
      return true
    }
    if (!auth.isAuthenticated) {
      return { name: 'signin', query: to.fullPath === '/' ? {} : { next: to.fullPath } }
    }
    return true
  })

  router.afterEach((to) => {
    const base = typeof to.meta.title === 'string' ? to.meta.title : 'AgentGit'
    document.title = `${base} · AgentGit`
  })
  return router
}

/** Only same-origin app paths may be used as a post-sign-in destination (no open redirects). */
export function safeNext(next: string): string {
  return next.startsWith('/') && !next.startsWith('//') && !next.startsWith('/\\') ? next : '/'
}
