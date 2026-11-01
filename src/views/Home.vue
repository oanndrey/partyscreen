<script setup>
import { ref } from 'vue'
import { useRouter } from 'vue-router'
import Icon from '../components/Icon.vue'

const router = useRouter()
const code = ref('')

function join() {
  // Aceita tanto o código quanto o link inteiro colado
  const value = code.value.trim()
  const match = value.match(/sala\/([A-Za-z0-9]+)/)
  const room = (match ? match[1] : value).toUpperCase()
  if (room) router.push(`/sala/${room}`)
}
</script>

<template>
  <header class="topbar">
    <Icon name="monitorPlay" class="muted" />
    <h1>Início</h1>
  </header>
  <section class="home">
    <h2 class="title">Bem-vindo ao PartyScreen</h2>
    <p class="muted">Assista à tela dos amigos com um código. Sem cadastro.</p>

    <div class="cards">
      <form class="card" @submit.prevent="join">
        <div class="bubble green"><Icon name="monitorPlay" :size="24" /></div>
        <div>
          <div class="card-title">Entrar numa sala</div>
          <div class="muted small">Digite o código ou cole o link que você recebeu.</div>
        </div>
        <div class="row">
          <input v-model="code" class="code-input" placeholder="Ex.: K7QX2M" autocomplete="off" />
          <button type="submit" class="primary" :disabled="!code.trim()">Entrar</button>
        </div>
      </form>

      <div class="card">
        <div class="bubble orange"><Icon name="screenShare" :size="24" /></div>
        <div>
          <div class="card-title">Quer transmitir?</div>
          <div class="muted small">Para transmitir a tela, com som e boa qualidade, use o app PartyScreen no computador.</div>
        </div>
        <button @click="router.push('/compartilhar')">Transmitir pelo navegador</button>
      </div>
    </div>
  </section>
</template>

<style scoped>
.topbar {
  display: flex;
  align-items: center;
  gap: 8px;
  height: 48px;
  padding: 0 16px;
  flex-shrink: 0;
  border-bottom: 1px solid var(--border);
  box-shadow: 0 1px 2px rgb(0 0 0 / 20%);
}
.topbar h1 { font-size: 15px; font-weight: 600; }
.home { width: 100%; max-width: 768px; margin: 0 auto; padding: 40px 16px; }
.title { font-size: 1.5rem; font-weight: 700; }
.home > .muted { margin: 4px 0 0; }
.cards { display: grid; grid-template-columns: repeat(auto-fit, minmax(280px, 1fr)); gap: 16px; margin-top: 32px; }
.card { gap: 16px; }
.card-title { font-weight: 600; color: var(--text-strong); }
.small { font-size: .875rem; margin-top: 2px; }
.bubble { width: 48px; height: 48px; border-radius: 50%; display: grid; place-items: center; }
.bubble.green { background: rgb(35 165 90 / 15%); color: var(--success); }
.bubble.orange { background: rgb(253 90 16 / 15%); color: var(--primary); }
.code-input { font-family: ui-monospace, monospace; font-weight: 600; letter-spacing: .2em; text-transform: uppercase; }
.code-input::placeholder { letter-spacing: normal; font-family: Inter, sans-serif; font-weight: 400; text-transform: none; }
</style>
