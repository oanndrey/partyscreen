import { createApp } from 'vue'
import { createRouter, createWebHashHistory } from 'vue-router'
import App from './App.vue'
import Home from './views/Home.vue'
import Host from './views/Host.vue'
import Viewer from './views/Viewer.vue'
import './style.css'

const router = createRouter({
  history: createWebHashHistory(),
  routes: [
    { path: '/', component: Home },
    { path: '/compartilhar', component: Host },
    { path: '/sala/:code', component: Viewer, props: true },
  ],
})

createApp(App).use(router).mount('#app')
