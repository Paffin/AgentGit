import { createRouter, createWebHistory, START_LOCATION } from 'vue-router'
import type { RouteRecordRaw, Router } from 'vue-router'
import { useAuth } from '@/stores/auth'
import { ApiError } from '@/api/errors'
import { safeNext } from './redirect'

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
    // Where the identity provider sends the browser back to after a redirect sign-in.
    path: '/auth/callback',
    name: 'auth-callback',
    component: () => import('@/views/AuthCallbackView.vue'),
    meta: { public: true, title: 'Signing in' },
  },
  {
    // Where a session that ended while it was in use ends up: says why, then leads back to the sign-in.
    path: '/unauthorized',
    name: 'unauthorized',
    component: () => import('@/views/UnauthorizedView.vue'),
    meta: { public: true, title: 'Session ended' },
  },
  {
    path: '/',
    name: 'home',
    component: () => import('@/views/HomeView.vue'),
    meta: { title: 'Home' },
  },
  {
    path: '/repos',
    name: 'repos',
    component: () => import('@/views/ReposView.vue'),
    meta: { title: 'Repositories' },
  },
  {
    path: '/repos/new',
    name: 'new-repo',
    component: () => import('@/views/NewRepoView.vue'),
    meta: { title: 'Start a repository' },
  },
  {
    path: '/import',
    name: 'import-repo',
    component: () => import('@/views/ImportRepoView.vue'),
    meta: { title: 'Import a repository' },
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
        path: 'search',
        name: 'code-search',
        component: () => import('@/views/CodeSearchView.vue'),
        meta: { title: 'Search code' },
      },
      {
        path: 'files',
        name: 'files',
        component: () => import('@/views/FilesView.vue'),
        meta: { title: 'Files' },
      },
      {
        path: 'edit',
        name: 'change-editor',
        component: () => import('@/views/ChangeEditorView.vue'),
        meta: { title: 'Edit files' },
      },
      {
        path: 'compare',
        name: 'compare',
        component: () => import('@/views/CompareView.vue'),
        meta: { title: 'Compare' },
      },
      {
        path: 'issues',
        name: 'issues',
        component: () => import('@/views/IssuesView.vue'),
        meta: { title: 'Issues' },
      },
      {
        path: 'issues/:number',
        name: 'issue',
        component: () => import('@/views/IssueView.vue'),
        props: true,
        meta: { title: 'Issue' },
      },
      {
        path: 'runs/:id',
        name: 'run',
        props: true,
        component: () => import('@/views/RunView.vue'),
        meta: { title: 'Agent run' },
      },
      {
        path: 'tasks/:id',
        name: 'task',
        component: () => import('@/views/TaskView.vue'),
        props: true,
        meta: { title: 'Task' },
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
    // The callback finishes a sign-in itself; resuming one there would start another.
    if (to.name !== 'auth-callback' && auth.status === 'anonymous' && auth.token === null) {
      try {
        await auth.restore(to.fullPath)
      } catch (e) {
        // The server is unreachable while resuming a saved session: say so instead of showing a sign-in form.
        if (e instanceof ApiError && e.isNetwork && to.name !== 'offline') {
          return { name: 'offline', query: { next: to.fullPath } }
        }
      }
    }
    if (to.meta.public) {
      if ((to.name === 'signin' || to.name === 'unauthorized') && auth.isAuthenticated)
        return typeof to.query.next === 'string' ? safeNext(to.query.next) : { name: 'home' }
      return true
    }
    if (!auth.isAuthenticated) {
      return { name: 'signin', query: to.fullPath === '/' ? {} : { next: to.fullPath } }
    }
    return true
  })

  router.afterEach((to, from) => {
    if (from !== START_LOCATION && to.path === from.path) return
    const base = typeof to.meta.title === 'string' ? to.meta.title : 'AgentGit'
    document.title = `${base} · AgentGit`
  })
  return router
}
