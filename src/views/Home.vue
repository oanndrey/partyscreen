<script setup>
import { ref } from 'vue'
import { useRouter } from 'vue-router'
import Icon from '../components/Icon.vue'
import Fundo from '../components/Fundo.vue'

const router = useRouter()
const code = ref('')
// Ainda não há página de download própria; o repositório tem as instruções
const DOWNLOAD = 'https://github.com/oanndrey/partyscreen'

function join() {
  // Aceita tanto o código quanto o link inteiro colado
  const value = code.value.trim()
  const match = value.match(/sala\/([A-Za-z0-9]+)/)
  const room = (match ? match[1] : value).toUpperCase()
  if (room) router.push(`/sala/${room}`)
}
</script>

<template>
  <div class="page">
    <Fundo />

    <header class="topbar">
      <router-link to="/" class="brand"><img src="/logo.svg" alt="" /><span>PartyScreen</span></router-link>
      <div class="grow"></div>
      <a class="btn secondary" :href="DOWNLOAD" target="_blank" rel="noopener"><Icon name="download" :size="18" /> Baixar o app</a>
    </header>

    <main class="home">
      <h1 class="display title">Assista à tela dos amigos.</h1>
      <p class="sub">Digite o código da sala ou cole o link que você recebeu. Sem cadastro.</p>

      <form class="join glass" @submit.prevent="join">
        <input v-model="code" class="code-input" placeholder="Código ou link da sala" autocomplete="off" spellcheck="false" />
        <button type="submit" class="primary" :disabled="!code.trim()"><Icon name="arrowRight" :size="18" /> Entrar</button>
      </form>

      <div class="cards">
        <div class="card-glass glass">
          <div class="bubble orange"><Icon name="screenShare" /></div>
          <div>
            <h3 class="display">Quer transmitir?</h3>
            <p>Use o app PartyScreen no computador para transmitir a tela inteira ou uma janela, com som.</p>
          </div>
          <div class="grow"></div>
          <a class="btn primary" :href="DOWNLOAD" target="_blank" rel="noopener"><Icon name="download" :size="18" /> Baixar o app</a>
        </div>
        <div class="card-glass glass">
          <div class="bubble"><Icon name="monitorPlay" /></div>
          <div>
            <h3 class="display">Sem instalar nada</h3>
            <p>Dá para transmitir direto pelo navegador. A qualidade fica menor que no app.</p>
          </div>
          <div class="grow"></div>
          <button class="secondary" @click="router.push('/compartilhar')">Transmitir pelo navegador</button>
        </div>
      </div>
    </main>
  </div>
</template>

<style scoped>
.page { position: relative; isolation: isolate; height: 100%; display: flex; flex-direction: column; overflow-y: auto; }
.home { width: 100%; max-width: 760px; margin: 0 auto; padding: 96px 20px 64px; display: flex; flex-direction: column; align-items: center; text-align: center; }
.title { font-size: clamp(30px, 5vw, 44px); line-height: 1.15; letter-spacing: -0.03em; }
.sub { margin-top: 12px; font-size: 17px; line-height: 1.55; color: rgb(255 255 255 / 82%); }

.join { display: flex; gap: 8px; width: 100%; max-width: 520px; margin-top: 40px; padding: 8px; border-radius: 14px; background: rgb(255 255 255 / 10%); border-color: rgb(255 255 255 / 18%); }
.code-input { flex: 1; height: 48px; padding: 0 14px; border: 0; background: transparent; font-size: 16px; }
.code-input:focus { box-shadow: none; }
.join button { height: 48px; }

.cards { display: grid; grid-template-columns: repeat(auto-fit, minmax(260px, 1fr)); gap: 16px; width: 100%; margin-top: 72px; text-align: left; }
.card-glass { display: flex; flex-direction: column; gap: 16px; padding: 24px; border-radius: 16px; }
.card-glass h3 { font-size: 15px; }
.card-glass p { margin-top: 4px; font-size: 14px; line-height: 1.5; color: rgb(255 255 255 / 66%); }
.card-glass .btn, .card-glass button { align-self: flex-start; }
.bubble { width: 44px; height: 44px; border-radius: 12px; display: grid; place-items: center; background: var(--raised); color: var(--text-strong); }
.bubble.orange { background: var(--primary-soft); color: var(--primary); }

@media (max-width: 640px) {
  .topbar .btn span, .topbar .btn { font-size: 13px; }
  .home { padding-top: 56px; }
  .cards { margin-top: 48px; }
}
</style>
