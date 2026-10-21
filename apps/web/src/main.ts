import { createApp } from 'vue'
import { createPinia } from 'pinia'
import App from './App.vue'
import { takeLoginFragment } from './lib/login-link'
import { createAppRouter } from './router'
import { useAuth } from './stores/auth'
import './styles/tokens.css'
import './styles/base.css'

// A sign-in link (`…#token=…`, printed by `ag-server dev`) carries its credential in the fragment. Take it out of
// the address bar before anything else runs, and sign in before the first navigation, so the router's guard sees a
// signed-in session and the link lands where its path says.
const credential = takeLoginFragment(window.location, window.history)

const app = createApp(App)
const pinia = createPinia()
app.use(pinia)
if (credential) await useAuth(pinia).signInFromLink(credential)
app.use(createAppRouter())
app.mount('#app')
