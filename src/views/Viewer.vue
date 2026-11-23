<script setup>
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useRouter } from 'vue-router'
import { connectSignaling, getRtcConfig } from '../lib/signaling'
import Icon from '../components/Icon.vue'
import Avatar from '../components/Avatar.vue'
import Fundo from '../components/Fundo.vue'

const props = defineProps({ code: { type: String, required: true } })
const router = useRouter()
const room = computed(() => props.code.toUpperCase())

// Nick de quem assiste (lembrado neste navegador); a sala só conecta depois de informar
const nick = ref(localStorage.getItem('partyscreen-nick') || '')
const joined = ref(false)

const status = ref('connecting') // connecting | waiting | live | error
const message = ref('')
const members = ref([])
const myId = ref('')
const chat = ref([])
const draft = ref('')
const stats = ref({ fps: 0, mbps: 0, route: '' })

const volume = ref(Number(localStorage.getItem('partyscreen-volume') ?? 1))
const muted = ref(false)
const showPanel = ref(true)
const fullscreen = ref(false)
const toast = ref('')
const tab = ref('chat') // chat | pessoas
const isTouch = window.matchMedia?.('(pointer: coarse)').matches

const stage = ref(null)
const video = ref(null)
const chatList = ref(null)

const host = computed(() => members.value.find((m) => m.role === 'host'))
const link = computed(() => `${location.origin}/#/sala/${room.value}`)

let signaling = null
let pc = null
let queue = Promise.resolve()
let statsTimer = null

async function join() {
  const name = nick.value.trim().slice(0, 24)
  if (!name) return
  try {
    localStorage.setItem('partyscreen-nick', name)
  } catch {}
  joined.value = true
  const rtcConfig = await getRtcConfig()
  signaling = connectSignaling({
    room: room.value,
    role: 'viewer',
    name,
    onMessage: (msg) => handleMessage(msg, rtcConfig),
    onClose: () => {
      status.value = 'error'
      message.value = 'A conexão com o servidor caiu. Recarregue a página para entrar de novo.'
    },
  })
}

function handleMessage(msg, rtcConfig) {
  if (msg.type === 'joined') {
    myId.value = msg.id
    status.value = 'waiting'
  } else if (msg.type === 'members') {
    members.value = msg.members
  } else if (msg.type === 'chat') {
    chat.value.push(msg)
    if (chat.value.length > 200) chat.value.shift()
  } else if (msg.type === 'host-left') {
    closePeer()
    status.value = 'waiting'
  } else if (msg.type === 'error') {
    status.value = 'error'
    message.value = msg.message
  } else if (msg.type === 'signal') {
    // Uma nova offer significa nova sessão de quem transmite
    if (msg.data.sdp?.type === 'offer') createPeer(msg.from, rtcConfig)
    queue = queue.then(() => applySignal(msg.from, msg.data)).catch(console.error)
  }
}

// Caminho da conexão: mesma rede, direta pela internet ou retransmitida (TURN)
function routeKind(local, remote) {
  if (!local || !remote) return ''
  if (local.candidateType === 'relay' || remote.candidateType === 'relay') return 'relay'
  if (local.candidateType === 'host' && remote.candidateType === 'host') return 'local'
  return 'direct'
}
const ROUTES = { local: 'Direta · mesma rede', direct: 'Direta · internet', relay: 'Retransmitida (TURN)' }

function createPeer(hostId, rtcConfig) {
  closePeer()
  pc = new RTCPeerConnection(rtcConfig)
  pc.onicecandidate = (e) => e.candidate && signaling.signal(hostId, { candidate: e.candidate })
  pc.ontrack = (e) => {
    if (video.value.srcObject !== e.streams[0]) video.value.srcObject = e.streams[0]
    applyVolume()
    // Sem interação recente o navegador pode bloquear som: toca mudo e avisa
    video.value.play().catch(() => {
      muted.value = true
      applyVolume()
      video.value.play().catch(() => {})
    })
    status.value = 'live'
  }
  pc.onconnectionstatechange = () => {
    if (pc?.connectionState === 'failed') {
      status.value = 'error'
      message.value = 'A conexão falhou. A rede de um dos dois pode estar bloqueando.'
    }
  }
  let lastBytes = 0
  let lastTime = performance.now()
  statsTimer = setInterval(async () => {
    if (!pc) return
    const report = await pc.getStats()
    let inbound
    let pairId
    report.forEach((s) => {
      if (s.type === 'inbound-rtp' && s.kind === 'video') inbound = s
      if (s.type === 'transport' && s.selectedCandidatePairId) pairId = s.selectedCandidatePairId
    })
    if (!pairId) report.forEach((s) => {
      if (s.type === 'candidate-pair' && s.nominated && s.state === 'succeeded') pairId = s.id
    })
    const pair = pairId && report.get(pairId)
    const route = pair ? routeKind(report.get(pair.localCandidateId), report.get(pair.remoteCandidateId)) : ''
    if (inbound) {
      const now = performance.now()
      const mbps = ((inbound.bytesReceived - lastBytes) * 8) / ((now - lastTime) / 1000) / 1e6
      lastBytes = inbound.bytesReceived
      lastTime = now
      stats.value = { fps: inbound.framesPerSecond || 0, mbps, route }
    }
  }, 2000)
}

async function applySignal(hostId, data) {
  if (!pc) return
  if (data.sdp) {
    await pc.setRemoteDescription(data.sdp)
    await pc.setLocalDescription(await pc.createAnswer())
    signaling.signal(hostId, { sdp: pc.localDescription })
  } else if (data.candidate) {
    await pc.addIceCandidate(data.candidate)
  }
}

function closePeer() {
  clearInterval(statsTimer)
  pc?.close()
  pc = null
  queue = Promise.resolve()
  if (video.value) video.value.srcObject = null
}

function applyVolume() {
  if (!video.value) return
  video.value.volume = volume.value
  video.value.muted = muted.value || volume.value === 0
}
watch([volume, muted], () => {
  applyVolume()
  try {
    localStorage.setItem('partyscreen-volume', String(volume.value))
  } catch {}
})
function toggleMute() {
  if (muted.value || volume.value === 0) {
    muted.value = false
    if (volume.value === 0) volume.value = 0.5
  } else {
    muted.value = true
  }
}
const silent = computed(() => muted.value || volume.value === 0)

function toggleFullscreen() {
  if (document.fullscreenElement) document.exitFullscreen()
  else stage.value?.requestFullscreen()
}
const onFullscreen = () => (fullscreen.value = !!document.fullscreenElement)

function send() {
  const text = draft.value.trim()
  if (!text) return
  signaling?.chat(text)
  draft.value = ''
}

function copy(text, label) {
  navigator.clipboard?.writeText(text)
  toast.value = label
  setTimeout(() => (toast.value = ''), 1800)
}

function leave() {
  router.push('/')
}

function retry() {
  location.reload()
}

function time(ts) {
  const d = new Date(ts)
  return `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`
}
// Mensagens seguidas da mesma pessoa (até 5 min) ficam agrupadas, como no Discord
const grouped = computed(() =>
  chat.value.map((m, i) => {
    const prev = chat.value[i - 1]
    return { ...m, cont: !m.system && prev && !prev.system && prev.id === m.id && m.ts - prev.ts < 5 * 60_000 }
  }),
)
watch(
  () => chat.value.length,
  async () => {
    await nextTick()
    if (chatList.value) chatList.value.scrollTop = chatList.value.scrollHeight
  },
)

onMounted(() => document.addEventListener('fullscreenchange', onFullscreen))
onBeforeUnmount(() => {
  document.removeEventListener('fullscreenchange', onFullscreen)
  if (document.fullscreenElement) document.exitFullscreen()
  closePeer()
  signaling?.close()
})
</script>

<template>
  <!-- Antes de entrar: pede o nick -->
  <div v-if="!joined" class="gate">
    <Fundo />
    <header class="topbar">
      <router-link to="/" class="brand"><img src="/logo.svg" alt="" /><span>PartyScreen</span></router-link>
    </header>

    <div class="gate-body">
      <form class="gate-card glass" @submit.prevent="join">
        <p class="gate-kicker">Você foi chamado para a sala</p>
        <div class="tiles" :aria-label="`Sala ${room}`">
          <span v-for="(ch, i) in room.split('')" :key="i" class="tile">{{ ch }}</span>
        </div>

        <div class="gate-sep"></div>

        <h2 class="display">Como te chamam?</h2>
        <p class="gate-desc">É assim que a galera vai te ver no chat e na lista de quem está assistindo.</p>

        <label class="nick-field">
          <Avatar v-if="nick.trim()" :name="nick.trim()" :size="38" />
          <span v-else class="nick-empty"><Icon name="users" :size="18" /></span>
          <input v-model="nick" type="text" placeholder="Seu nick" maxlength="24" autofocus />
          <span class="mono counter">{{ nick.length }}/24</span>
        </label>
        <p class="gate-hint">A bolinha usa a inicial e a cor do seu nick.</p>

        <button class="primary gate-go" :disabled="!nick.trim()">Entrar na sala <Icon name="arrowRight" :size="18" /></button>
        <button type="button" class="ghost gate-cancel" @click="leave">Cancelar</button>
      </form>
      <p class="gate-foot">Sem cadastro. O nick fica salvo só neste navegador.</p>
    </div>
  </div>

  <div v-else class="room">
    <Fundo class="room-bg" :escurecer="status === 'live' ? 0.5 : 0.35" />

    <header class="topbar">
      <router-link to="/" class="brand"><img src="/logo.svg" alt="" /><span>PartyScreen</span></router-link>
      <span class="sep"></span>
      <span class="room-label">Sala</span>
      <button class="code-pill" title="Copiar código" @click="copy(room, 'Código copiado')">
        {{ room }} <Icon name="copy" :size="14" />
      </button>
      <div class="grow"></div>
      <div v-if="members.length" class="people">
        <div class="stack">
          <Avatar v-for="m in members.slice(0, 4)" :key="m.id" :name="m.name" :size="24" />
        </div>
        <span>{{ members.length }} na sala</span>
      </div>
      <button class="primary invite" title="Copiar convite" @click="copy(link, 'Convite copiado')">
        <Icon name="userPlus" :size="18" /> <span>Convidar</span>
      </button>
    </header>

    <div class="room-body">
      <div class="main">
        <!-- Tela grande -->
        <div ref="stage" class="stage-big" :class="{ live: status === 'live' }" @dblclick="toggleFullscreen">
          <video ref="video" autoplay playsinline></video>

          <div v-if="status !== 'live'" class="overlay-center">
            <template v-if="status === 'error'">
              <span class="err-icon"><Icon name="wifiOff" :size="24" /></span>
              <h2 class="display">A conexão falhou</h2>
              <p class="overlay-text">{{ message || 'A rede de um dos dois pode estar bloqueando.' }}</p>
              <div class="row overlay-actions">
                <button class="secondary" @click="leave">Voltar ao início</button>
                <button class="primary" @click="retry">Tentar de novo</button>
              </div>
            </template>
            <template v-else>
              <div class="spinner"></div>
              <h2 class="display">
                {{ status === 'connecting' ? 'Conectando…' : host ? `Esperando ${host.name} transmitir` : 'Esperando a transmissão começar' }}
              </h2>
              <p class="overlay-text">Você já está na sala. A tela aparece aqui assim que a transmissão começar.</p>
            </template>
          </div>

          <button v-if="status === 'live' && silent" class="unmute" @click="toggleMute">
            <Icon name="volumeX" :size="16" /> {{ isTouch ? 'Toque para ativar o som' : 'Clique para ativar o som' }}
          </button>
        </div>

        <!-- Quem transmite + controles -->
        <div class="dock">
          <div class="host">
            <Avatar v-if="host" :name="host.name" :size="40" />
            <span v-else class="host-empty"><Icon name="monitorPlay" :size="18" /></span>
            <div class="host-text">
              <div class="host-line">
                <span class="host-name">{{ host ? host.name : 'Ninguém transmitindo' }}</span>
                <span v-if="status === 'live'" class="live-pill"><i></i>AO VIVO</span>
              </div>
              <div class="host-sub">
                <template v-if="status === 'error'"><span class="dot-s bad"></span><span class="bad-text">Sem conexão</span></template>
                <template v-else-if="status === 'live'">
                  <template v-if="stats.route"><span class="dot-s" :class="stats.route === 'relay' ? 'warn' : 'ok'"></span>{{ ROUTES[stats.route] }}</template>
                  <template v-else>Conectado</template>
                </template>
                <template v-else-if="status === 'waiting'"><span class="dot-s warn"></span>Conectado ao servidor</template>
                <template v-else>Conectando…</template>
              </div>
            </div>
          </div>

          <div class="controls">
            <div class="volume" :class="{ off: status !== 'live' }">
              <button class="icon-btn" :class="{ on: silent && isTouch }" :disabled="status !== 'live'" :title="silent ? 'Ativar som' : 'Silenciar'" @click="toggleMute">
                <Icon :name="silent ? 'volumeX' : 'volume'" />
              </button>
              <input
                v-model.number="volume" type="range" min="0" max="1" step="0.01" class="slider"
                :disabled="status !== 'live'"
                :style="{ '--fill': (silent ? 0 : volume) * 100 + '%' }" title="Volume da transmissão"
                @input="muted = false"
              />
            </div>
            <button class="icon-btn" :disabled="status !== 'live'" :title="fullscreen ? 'Sair da tela cheia' : 'Tela cheia'" @click="toggleFullscreen">
              <Icon :name="fullscreen ? 'minimize' : 'maximize'" />
            </button>
            <button class="icon-btn chat-toggle" :class="{ on: showPanel }" :title="showPanel ? 'Esconder chat' : 'Mostrar chat'" @click="showPanel = !showPanel">
              <Icon name="messages" />
            </button>
          </div>

          <div class="leave-wrap">
            <button class="danger leave" title="Parar de assistir" @click="leave">
              <Icon name="phoneOff" :size="18" /> <span>Sair da sala</span>
            </button>
          </div>
        </div>
      </div>

      <!-- Chat + pessoas -->
      <aside v-if="showPanel" class="panel glass">
        <div class="tabs">
          <button :class="{ active: tab === 'chat' }" @click="tab = 'chat'">Chat</button>
          <button :class="{ active: tab === 'pessoas' }" @click="tab = 'pessoas'">Pessoas <span class="mono">{{ members.length }}</span></button>
        </div>

        <div v-if="tab === 'pessoas'" class="people-list">
          <div class="label">Na sala — {{ members.length }}</div>
          <div v-for="m in members" :key="m.id" class="member" :class="{ me: m.id === myId }">
            <div class="avatar-wrap">
              <Avatar :name="m.name" :size="32" />
              <span class="status" :class="m.role === 'host' ? 'red' : 'green'"></span>
            </div>
            <div class="member-text">
              <div class="member-name">{{ m.name }}<span v-if="m.id === myId"> (você)</span></div>
              <div class="member-sub">{{ m.role === 'host' ? 'Transmitindo' : 'Assistindo' }}</div>
            </div>
          </div>
        </div>

        <template v-else>
          <div ref="chatList" class="chat">
            <div v-if="!chat.length" class="chat-empty">
              <Icon name="messages" :size="32" />
              Ninguém falou nada ainda. Manda um oi!
            </div>
            <div class="chat-items">
              <template v-for="(m, i) in grouped" :key="i">
                <div v-if="m.system" class="sys">
                  <Icon name="arrowRight" :size="14" class="sys-icon" />
                  <span class="grow">{{ m.text }}</span>
                  <span class="ts">{{ time(m.ts) }}</span>
                </div>
                <div v-else-if="m.cont" class="msg-cont">{{ m.text }}</div>
                <div v-else class="msg">
                  <Avatar :name="m.name" :size="32" />
                  <div class="msg-body">
                    <div class="msg-head"><span class="msg-name">{{ m.name }}</span><span class="ts">{{ time(m.ts) }}</span></div>
                    <div class="msg-text">{{ m.text }}</div>
                  </div>
                </div>
              </template>
            </div>
          </div>

          <form class="composer" @submit.prevent="send">
            <input v-model="draft" type="text" placeholder="Mensagem para a sala" maxlength="500" />
            <button class="send" title="Enviar" :disabled="!draft.trim()"><Icon name="send" /></button>
          </form>
        </template>
      </aside>
    </div>

    <transition name="fade">
      <div v-if="toast" class="toast">✓ {{ toast }}</div>
    </transition>
  </div>
</template>

<style scoped>
/* ---------- Pedido do nick ---------- */
.gate { position: relative; isolation: isolate; height: 100%; display: flex; flex-direction: column; overflow-y: auto; }
.gate-body { flex: 1; display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 20px; padding: 24px 16px 60px; }
.gate-card { width: 100%; max-width: 460px; display: flex; flex-direction: column; align-items: stretch; padding: 32px 32px 24px; border-radius: 20px; background: rgb(255 255 255 / 7%); border-color: rgb(255 255 255 / 16%); backdrop-filter: blur(40px); -webkit-backdrop-filter: blur(40px); }
.gate-kicker { text-align: center; font-size: 13px; font-weight: 500; color: rgb(255 255 255 / 70%); }
.tiles { display: flex; justify-content: center; gap: 8px; margin-top: 14px; }
.tile { width: 52px; height: 62px; display: grid; place-items: center; border-radius: 12px; background: rgb(255 255 255 / 6%); border: 1px solid rgb(255 255 255 / 18%); font-family: var(--font-display); font-weight: 600; font-size: 24px; color: var(--text-strong); }
.gate-sep { height: 1px; margin: 28px 0; background: rgb(255 255 255 / 12%); }
.gate-card h2 { font-size: 20px; }
.gate-desc { margin-top: 6px; font-size: 14px; line-height: 1.5; color: rgb(255 255 255 / 66%); }
.nick-field { display: flex; align-items: center; gap: 12px; height: 54px; margin-top: 18px; padding: 0 14px 0 8px; border-radius: 12px; background: rgb(5 5 6 / 55%); border: 1.5px solid rgb(255 255 255 / 16%); cursor: text; }
.nick-field:focus-within { border-color: var(--primary); }
.nick-field input { flex: 1; height: 100%; padding: 0; border: 0; background: transparent; font-size: 16px; font-weight: 600; box-shadow: none !important; }
.nick-empty { width: 38px; height: 38px; border-radius: 50%; display: grid; place-items: center; background: rgb(255 255 255 / 8%); color: var(--on-glass-muted); flex-shrink: 0; }
.counter { font-size: 12px; color: rgb(255 255 255 / 50%); }
.gate-hint { margin-top: 8px; font-size: 12px; color: rgb(255 255 255 / 50%); }
.gate-go { height: 50px; margin-top: 24px; border-radius: 12px; font-size: 15px; }
.gate-cancel { margin-top: 10px; }
.gate-foot { font-size: 13px; color: rgb(255 255 255 / 55%); text-align: center; }

/* ---------- Sala ---------- */
.room { position: relative; isolation: isolate; height: 100%; display: flex; flex-direction: column; background: #050506; }
.sep { width: 1px; height: 20px; background: rgb(255 255 255 / 16%); }
.room-label { font-size: 14px; color: var(--on-glass-muted); }
.code-pill { height: 30px; padding: 0 8px 0 10px; gap: 8px; border-radius: 8px; background: rgb(255 255 255 / 10%); font-family: var(--font-mono); font-weight: 600; font-size: 14px; letter-spacing: .12em; }
.code-pill :deep(svg) { color: var(--on-glass-muted); }
.people { display: flex; align-items: center; gap: 10px; font-size: 13px; color: var(--on-glass-muted); }
.stack { display: flex; flex-direction: row-reverse; justify-content: flex-end; }
.stack > * { box-shadow: inset 0 0 0 1.5px rgb(13 8 5 / 85%); }
.stack > * + * { margin-right: -6px; }

.room-body { flex: 1; min-height: 0; display: flex; gap: 16px; padding: 16px; }
.main { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 12px; }

.stage-big { position: relative; flex: 1; min-height: 0; display: flex; border-radius: 14px; overflow: hidden; background: rgb(255 255 255 / 4%); border: 1px solid rgb(255 255 255 / 14%); backdrop-filter: blur(40px); -webkit-backdrop-filter: blur(40px); }
.stage-big.live { background: #000; border-color: rgb(255 255 255 / 10%); }
.stage-big video { flex: 1; width: 100%; min-height: 0; object-fit: contain; }
.stage-big:not(.live) video { visibility: hidden; }
.overlay-center { position: absolute; inset: 0; display: flex; flex-direction: column; gap: 12px; align-items: center; justify-content: center; padding: 24px; text-align: center; }
.overlay-center h2 { font-size: 18px; }
.overlay-text { max-width: 380px; font-size: 14px; line-height: 1.5; color: var(--on-glass-muted); }
.overlay-actions { margin-top: 8px; }
.err-icon { width: 56px; height: 56px; margin-bottom: 4px; border-radius: 50%; display: grid; place-items: center; background: var(--raised); color: var(--danger); }
.unmute { position: absolute; left: 50%; top: 16px; transform: translateX(-50%); height: 34px; padding: 0 14px 0 10px; gap: 6px; border-radius: 999px; background: rgb(0 0 0 / 70%); font-size: 13px; font-weight: 500; }
.unmute:hover { background: rgb(0 0 0 / 85%) !important; }

.dock { display: flex; align-items: center; gap: 16px; padding: 12px 12px 12px 14px; border-radius: 14px; background: rgb(255 255 255 / 6%); border: 1px solid rgb(255 255 255 / 14%); backdrop-filter: blur(40px); -webkit-backdrop-filter: blur(40px); }
.host { flex: 1; min-width: 0; display: flex; align-items: center; gap: 12px; }
.host-empty { width: 40px; height: 40px; border-radius: 50%; display: grid; place-items: center; background: rgb(255 255 255 / 8%); color: var(--on-glass-muted); flex-shrink: 0; }
.host-text { min-width: 0; }
.host-line { display: flex; align-items: center; gap: 8px; }
.host-name { font-size: 15px; font-weight: 600; color: var(--text-strong); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.live-pill { display: inline-flex; align-items: center; gap: 6px; padding: 2px 8px; border-radius: 6px; background: var(--danger); color: #fff; font-size: 11px; font-weight: 700; letter-spacing: .06em; flex-shrink: 0; }
.live-pill i { width: 6px; height: 6px; border-radius: 50%; background: #fff; }
.host-sub { display: flex; align-items: center; gap: 8px; margin-top: 2px; font-size: 12px; color: var(--on-glass-muted); white-space: nowrap; }
.dot-s { width: 6px; height: 6px; border-radius: 50%; flex-shrink: 0; }
.dot-s.ok { background: var(--success); }
.dot-s.warn { background: var(--warning); }
.dot-s.bad { background: var(--danger); }
.bad-text { color: var(--danger); }

.controls { display: flex; align-items: center; gap: 8px; }
.volume { display: flex; align-items: center; gap: 10px; padding-right: 14px; border-radius: 10px; background: rgb(255 255 255 / 8%); }
.volume .icon-btn { background: transparent; }
.volume.off { opacity: .45; }
.slider { -webkit-appearance: none; appearance: none; width: 112px; height: 4px; border-radius: 2px; cursor: pointer; background: linear-gradient(to right, var(--text-strong) var(--fill), rgb(255 255 255 / 20%) var(--fill)); }
.slider:disabled { cursor: default; }
.slider::-webkit-slider-thumb { -webkit-appearance: none; width: 14px; height: 14px; border-radius: 50%; background: var(--text-strong); }
.slider::-moz-range-thumb { width: 14px; height: 14px; border: 0; border-radius: 50%; background: var(--text-strong); }
.leave-wrap { flex: 1; display: flex; justify-content: flex-end; }

/* Painel da direita */
.panel { width: 340px; flex-shrink: 0; display: flex; flex-direction: column; border-radius: 14px; overflow: hidden; background: rgb(255 255 255 / 5%); backdrop-filter: blur(40px); -webkit-backdrop-filter: blur(40px); }
.tabs { display: flex; gap: 4px; margin: 12px 12px 8px; padding: 4px; border-radius: 10px; background: rgb(5 5 6 / 40%); }
.tabs button { flex: 1; height: 32px; padding: 0; border-radius: 7px; background: transparent; color: var(--on-glass-muted); font-size: 13px; font-weight: 500; }
.tabs button .mono { font-size: 12px; }
.tabs button.active { background: rgb(255 255 255 / 12%); color: var(--text-strong); }

.people-list { flex: 1; min-height: 0; overflow-y: auto; padding: 4px 6px 12px; }
.people-list .label { padding: 8px 10px 6px; }
.member { display: flex; align-items: center; gap: 12px; padding: 8px 10px; border-radius: 8px; }
.member.me { background: rgb(255 255 255 / 8%); }
.avatar-wrap { position: relative; flex-shrink: 0; }
.status { position: absolute; right: -2px; bottom: -2px; width: 12px; height: 12px; border-radius: 50%; box-shadow: 0 0 0 2.5px rgb(10 10 11 / 90%); }
.status.red { background: var(--danger); }
.status.green { background: var(--success); }
.member-text { min-width: 0; }
.member-name { font-size: 14px; font-weight: 500; color: var(--text-strong); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.member-sub { font-size: 12px; color: var(--on-glass-muted); }

.chat { flex: 1; min-height: 0; overflow-y: auto; display: flex; flex-direction: column; }
.chat-items { margin-top: auto; padding: 8px 0 14px; }
.chat-empty { display: flex; flex-direction: column; align-items: center; gap: 8px; padding: 40px 24px 0; text-align: center; color: var(--on-glass-muted); font-size: 14px; }
.sys { display: flex; align-items: center; gap: 8px; padding: 6px 16px; font-size: 12px; color: var(--on-glass-muted); }
.sys-icon { color: var(--success); flex-shrink: 0; }
.ts { font-family: var(--font-mono); font-size: 11px; color: var(--on-glass-muted); }
.msg { display: flex; gap: 12px; padding: 12px 16px 2px; }
.msg-body { min-width: 0; }
.msg-head { display: flex; align-items: baseline; gap: 8px; }
.msg-name { font-size: 14px; font-weight: 600; color: var(--text-strong); }
.msg-text, .msg-cont { margin-top: 2px; font-size: 14px; line-height: 20px; color: var(--on-glass); overflow-wrap: anywhere; }
.msg-cont { padding: 2px 16px 2px 60px; }

.composer { display: flex; align-items: center; gap: 4px; margin: 0 12px 12px; padding: 0 4px 0 14px; border-radius: 10px; background: rgb(5 5 6 / 45%); border: 1px solid rgb(255 255 255 / 14%); }
.composer:focus-within { border-color: var(--primary); }
.composer input { flex: 1; height: 44px; padding: 0; border: 0; background: transparent; box-shadow: none !important; font-size: 14px; }
.send { width: 40px; height: 40px; padding: 0; background: transparent; color: var(--on-glass-muted); }
.send:hover:not(:disabled) { background: transparent; color: var(--primary); }

.toast { position: fixed; left: 50%; bottom: 24px; transform: translateX(-50%); padding: 10px 16px; border-radius: 10px; background: #1b1b1e; border: 1px solid var(--border-strong); color: var(--text-strong); font-size: 14px; font-weight: 500; z-index: 50; }
.fade-enter-active, .fade-leave-active { transition: opacity .2s; }
.fade-enter-from, .fade-leave-to { opacity: 0; }

.stage-big:fullscreen { border-radius: 0; border: 0; }

/* ---------- Celular: sem degradê, vídeo em cima, chat embaixo ---------- */
@media (max-width: 900px) {
  .room { background: var(--bg); }
  .room-bg { display: none; }
  .room .topbar { height: 56px; padding: 0 12px 0 16px; gap: 10px; }
  .room .brand span, .sep, .room-label, .people { display: none; }
  .invite { width: 40px; padding: 0; background: rgb(255 255 255 / 10%); }
  .invite span { display: none; }

  .room-body { flex-direction: column; gap: 0; padding: 0; }
  .main { flex: none; gap: 0; }
  .stage-big { flex: none; aspect-ratio: 16 / 9; border-radius: 0; border: 0; background: #000; backdrop-filter: none; -webkit-backdrop-filter: none; }
  .overlay-center { gap: 8px; }
  .overlay-center h2 { font-size: 15px; }
  .overlay-text { font-size: 13px; }
  .spinner { width: 28px; height: 28px; }
  .unmute { top: 12px; }

  .dock { gap: 8px; padding: 12px 12px 12px 16px; border-radius: 0; border-width: 0 0 1px; background: transparent; backdrop-filter: none; -webkit-backdrop-filter: none; }
  .host :deep(.avatar) { width: 32px !important; height: 32px !important; font-size: 13px !important; }
  .host-empty { width: 32px; height: 32px; }
  .host-name { font-size: 14px; }
  .host-sub { display: none; }
  .volume { padding: 0; background: transparent; }
  .volume .icon-btn { background: rgb(255 255 255 / 8%); }
  .volume .icon-btn.on { background: var(--text-strong); }
  .slider, .chat-toggle { display: none; }
  .leave-wrap { flex: none; }
  .leave { width: 40px; padding: 0; }
  .leave span { display: none; }

  .panel { width: auto; flex: 1; min-height: 0; border: 0; border-radius: 0; background: transparent; backdrop-filter: none; -webkit-backdrop-filter: none; }
  .tabs { margin: 12px 12px 4px; background: var(--surface); }
  .composer { margin-bottom: max(12px, env(safe-area-inset-bottom)); }
}
</style>
