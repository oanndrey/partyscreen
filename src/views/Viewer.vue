<script setup>
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useRouter } from 'vue-router'
import { connectSignaling, getRtcConfig } from '../lib/signaling'
import Icon from '../components/Icon.vue'
import Avatar from '../components/Avatar.vue'

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
const showPanel = ref(window.innerWidth > 900)
const fullscreen = ref(false)
const toast = ref('')

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
    <form class="gate-card" @submit.prevent="join">
      <div class="gate-body">
        <Avatar v-if="nick.trim()" :name="nick.trim()" :size="64" />
        <div v-else class="gate-empty"><Icon name="users" :size="28" /></div>
        <h2>Como te chamam?</h2>
        <p class="muted">Você vai entrar na sala <b class="mono">{{ room }}</b>. É assim que a galera vai te ver.</p>
        <input v-model="nick" type="text" placeholder="Seu nick" maxlength="24" autofocus />
      </div>
      <div class="gate-foot">
        <button type="button" class="ghost" @click="leave">Cancelar</button>
        <button class="primary" :disabled="!nick.trim()">Entrar na sala</button>
      </div>
    </form>
  </div>

  <div v-else class="room">
    <header class="room-head">
      <Icon name="volume" class="muted" />
      <h1>Sala</h1>
      <button class="code-pill" title="Copiar código" @click="copy(room, 'Código copiado')">
        {{ room }} <Icon name="copy" :size="14" />
      </button>
      <div class="grow"></div>
      <button class="primary small" @click="copy(link, 'Convite copiado')"><Icon name="userPlus" :size="16" /> Convidar</button>
    </header>

    <div class="room-body">
      <!-- Tela grande -->
      <div ref="stage" class="stage-big" :class="{ live: status === 'live' }" @dblclick="toggleFullscreen">
        <video ref="video" autoplay playsinline></video>

        <div v-if="status !== 'live'" class="overlay-center">
          <template v-if="status === 'error'">
            <p class="err">{{ message }}</p>
            <button @click="leave">Voltar</button>
          </template>
          <template v-else>
            <div class="spinner"></div>
            <div>
              {{ status === 'connecting' ? 'Conectando…' : host ? `Esperando ${host.name} transmitir…` : 'Esperando a transmissão começar…' }}
            </div>
          </template>
        </div>

        <div class="top-bar">
          <span class="live-pill">AO VIVO</span>
          <span v-if="host" class="host-name">{{ host.name }}</span>
          <span v-if="stats.route" class="chip"><span class="dot" :class="stats.route === 'relay' ? 'warn' : 'ok'"></span>{{ ROUTES[stats.route] }}</span>
          <div class="grow"></div>
          <span class="chip faint">{{ stats.fps.toFixed(0) }} fps · {{ stats.mbps.toFixed(1) }} Mbps</span>
        </div>

        <button v-if="status === 'live' && silent" class="unmute" @click="toggleMute">
          <Icon name="volumeX" :size="16" /> Clique para ativar o som
        </button>

        <div class="controls">
          <div class="volume">
            <button class="round" :class="{ on: silent }" :title="silent ? 'Ativar som' : 'Silenciar'" @click="toggleMute">
              <Icon :name="silent ? 'volumeX' : 'volume'" />
            </button>
            <input
              v-model.number="volume" type="range" min="0" max="1" step="0.01" class="slider"
              :style="{ '--fill': (silent ? 0 : volume) * 100 + '%' }" title="Volume da transmissão"
              @input="muted = false"
            />
          </div>
          <button class="round" :title="fullscreen ? 'Sair da tela cheia' : 'Tela cheia'" @click="toggleFullscreen">
            <Icon :name="fullscreen ? 'minimize' : 'maximize'" />
          </button>
          <button class="round" :class="{ on: showPanel }" :title="showPanel ? 'Esconder chat' : 'Mostrar chat'" @click="showPanel = !showPanel">
            <Icon name="message" />
          </button>
          <button class="leave" title="Parar de assistir" @click="leave">
            <Icon name="phoneOff" /> <span>Sair</span>
          </button>
        </div>
      </div>

      <!-- Membros + chat -->
      <aside v-if="showPanel" class="panel">
        <div class="members">
          <div class="label">Na sala — {{ members.length }}</div>
          <div v-for="m in members" :key="m.id" class="member">
            <div class="avatar-wrap">
              <Avatar :name="m.name" :size="32" />
              <span class="status" :class="m.role === 'host' ? 'red' : 'green'"></span>
            </div>
            <div class="member-text">
              <div class="member-name">{{ m.name }}<span v-if="m.id === myId" class="muted"> (você)</span></div>
              <div v-if="m.role === 'host'" class="member-sub">Transmitindo</div>
            </div>
          </div>
        </div>

        <div ref="chatList" class="chat">
          <div v-if="!chat.length" class="chat-empty">
            <Icon name="messages" :size="32" />
            Ninguém falou nada ainda. Manda um oi!
          </div>
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
                <div><span class="msg-name">{{ m.name }}</span> <span class="ts">{{ time(m.ts) }}</span></div>
                <div class="msg-text">{{ m.text }}</div>
              </div>
            </div>
          </template>
        </div>

        <form class="composer" @submit.prevent="send">
          <input v-model="draft" type="text" placeholder="Conversar na sala" maxlength="500" />
          <button class="send" title="Enviar" :disabled="!draft.trim()"><Icon name="send" /></button>
        </form>
      </aside>
    </div>

    <transition name="fade">
      <div v-if="toast" class="toast">✓ {{ toast }}</div>
    </transition>
  </div>
</template>

<style scoped>
.mono { font-family: ui-monospace, monospace; letter-spacing: .1em; }

/* Pedido do nick */
.gate { flex: 1; display: grid; place-items: center; padding: 16px; background: rgb(0 0 0 / 35%); }
.gate-card { width: 100%; max-width: 400px; border-radius: 8px; overflow: hidden; background: var(--bg); box-shadow: 0 8px 32px rgb(0 0 0 / 50%); text-align: center; }
.gate-body { display: flex; flex-direction: column; align-items: center; padding: 24px 24px 16px; }
.gate-body h2 { margin-top: 16px; font-size: 1.25rem; }
.gate-body p { margin: 4px 0 20px; font-size: .875rem; }
.gate-empty { width: 64px; height: 64px; border-radius: 50%; display: grid; place-items: center; background: var(--elevated); color: var(--muted); }
.gate-foot { display: flex; justify-content: flex-end; gap: 8px; padding: 12px 16px; background: var(--card); }
button.ghost { background: transparent; }
button.ghost:hover { background: transparent; text-decoration: underline; }

/* Sala */
.room { flex: 1; min-height: 0; display: flex; flex-direction: column; }
.room-head { display: flex; align-items: center; gap: 8px; height: 48px; padding: 0 16px; flex-shrink: 0; border-bottom: 1px solid var(--border); box-shadow: 0 1px 2px rgb(0 0 0 / 20%); }
.room-head h1 { font-size: 15px; font-weight: 600; }
.code-pill { display: flex; align-items: center; gap: 6px; padding: 3px 8px; border-radius: 4px; background: var(--rail); color: var(--text-strong); font-family: ui-monospace, monospace; font-weight: 600; letter-spacing: .12em; font-size: .875rem; }
.code-pill:hover { background: var(--accent) !important; }
button.small { display: flex; align-items: center; gap: 6px; padding: 6px 12px; font-size: .875rem; }
.room-body { flex: 1; min-height: 0; display: flex; }

.stage-big { position: relative; flex: 1; min-width: 0; background: #000; display: flex; }
.stage-big video { flex: 1; width: 100%; min-height: 0; object-fit: contain; }
.overlay-center { position: absolute; inset: 0; display: flex; flex-direction: column; gap: 12px; align-items: center; justify-content: center; color: var(--muted); font-size: .875rem; text-align: center; padding: 24px; }
.err { color: var(--text-strong); max-width: 420px; margin: 0; }

.top-bar, .controls { transition: opacity .3s; }
.stage-big.live .top-bar, .stage-big.live .controls { opacity: 0; }
.stage-big:not(.live) .top-bar { display: none; }
.stage-big.live:hover .top-bar, .stage-big.live:hover .controls { opacity: 1; }
.top-bar { position: absolute; inset: 0 0 auto; display: flex; align-items: center; gap: 8px; padding: 12px; background: linear-gradient(rgb(0 0 0 / 70%), transparent); }
.live-pill { padding: 1px 6px; border-radius: 4px; background: var(--danger); color: #fff; font-size: 11px; font-weight: 700; letter-spacing: .03em; }
.host-name { color: #fff; font-size: .875rem; font-weight: 500; }
.chip { display: flex; align-items: center; gap: 6px; padding: 2px 8px; border-radius: 4px; background: rgb(0 0 0 / 50%); color: #fff; font-size: .75rem; }
.chip.faint { background: none; color: rgb(255 255 255 / 70%); font-variant-numeric: tabular-nums; }
.dot { width: 6px; height: 6px; margin: 0; }
.dot.ok { background: var(--success); }
.dot.warn { background: var(--warning); }

.unmute { position: absolute; left: 50%; top: 56px; transform: translateX(-50%); display: flex; align-items: center; gap: 6px; background: rgb(0 0 0 / 75%); font-size: .875rem; }

.controls { position: absolute; left: 50%; bottom: 24px; transform: translateX(-50%); display: flex; align-items: center; gap: 8px; padding: 8px; border-radius: 999px; background: rgb(0 0 0 / 70%); backdrop-filter: blur(8px); }
.round { width: 44px; height: 44px; padding: 0; border-radius: 50%; display: grid; place-items: center; background: rgb(255 255 255 / 10%); }
.round:hover:not(:disabled) { background: rgb(255 255 255 / 20%); }
.round.on, .round.on:hover { background: #fff; color: #000; }
.volume { display: flex; align-items: center; gap: 8px; padding-right: 8px; }
.slider { -webkit-appearance: none; appearance: none; width: 96px; height: 4px; border-radius: 2px; cursor: pointer; background: linear-gradient(to right, #fff var(--fill), rgb(255 255 255 / 25%) var(--fill)); }
.slider::-webkit-slider-thumb { -webkit-appearance: none; width: 14px; height: 14px; border-radius: 50%; background: #fff; }
.slider::-moz-range-thumb { width: 14px; height: 14px; border: 0; border-radius: 50%; background: #fff; }
.leave { display: flex; align-items: center; gap: 8px; height: 44px; padding: 0 18px; border-radius: 999px; background: var(--danger); font-weight: 600; }
.leave:hover { background: #a12828 !important; }

/* Painel da direita */
.panel { width: 320px; flex-shrink: 0; display: flex; flex-direction: column; background: var(--card); border-left: 1px solid var(--border); }
.members { max-height: 40%; overflow-y: auto; padding: 12px; border-bottom: 1px solid var(--border); flex-shrink: 0; }
.members .label { padding: 0 8px 4px; }
.member { display: flex; align-items: center; gap: 12px; padding: 4px 8px; border-radius: 4px; }
.member:hover { background: rgb(53 55 60 / 50%); }
.avatar-wrap { position: relative; }
.status { position: absolute; right: -2px; bottom: -2px; width: 14px; height: 14px; border-radius: 50%; border: 3px solid var(--card); }
.status.red { background: var(--danger); }
.status.green { background: var(--success); }
.member-text { min-width: 0; }
.member-name { font-size: .875rem; font-weight: 500; color: var(--text-strong); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.member-name .muted { font-weight: 400; }
.member-sub { font-size: .75rem; color: var(--danger); }

.chat { flex: 1; min-height: 0; overflow-y: auto; padding: 12px 0; }
.chat-empty { display: flex; flex-direction: column; align-items: center; gap: 8px; padding: 40px 24px 0; text-align: center; color: var(--muted); font-size: .875rem; }
.sys { display: flex; align-items: center; gap: 8px; padding: 4px 16px; font-size: .75rem; color: var(--muted); }
.sys-icon { color: var(--success); flex-shrink: 0; }
.ts { font-size: 11px; color: var(--muted); }
.msg { display: flex; gap: 12px; padding: 12px 16px 2px; }
.msg:hover, .msg-cont:hover { background: rgb(0 0 0 / 10%); }
.msg-body { min-width: 0; }
.msg-name { font-size: .875rem; font-weight: 500; color: var(--text-strong); }
.msg-text, .msg-cont { font-size: .875rem; line-height: 1.45; overflow-wrap: anywhere; }
.msg-cont { padding: 2px 16px 2px 60px; }

.composer { display: flex; align-items: center; gap: 4px; margin: 0 12px 12px; padding: 0 4px 0 12px; border-radius: 8px; background: var(--input); }
.composer:focus-within { box-shadow: 0 0 0 2px var(--primary); }
.composer input { flex: 1; height: 44px; padding: 0; background: transparent; box-shadow: none !important; font-size: .875rem; }
.send { width: 36px; height: 36px; padding: 0; display: grid; place-items: center; background: transparent; color: var(--muted); }
.send:hover:not(:disabled) { background: transparent; color: var(--primary); }

.toast { position: fixed; left: 50%; bottom: 24px; transform: translateX(-50%); padding: 10px 16px; border-radius: 6px; background: #111214; color: var(--text-strong); font-size: .875rem; font-weight: 500; box-shadow: 0 8px 24px rgb(0 0 0 / 40%); z-index: 50; }
.fade-enter-active, .fade-leave-active { transition: opacity .2s; }
.fade-enter-from, .fade-leave-to { opacity: 0; }

/* Celular: vídeo em cima, chat embaixo, controles sempre visíveis */
@media (max-width: 900px) {
  .room-body { flex-direction: column; }
  .stage-big { flex: none; aspect-ratio: 16 / 9; }
  .panel { width: auto; flex: 1; min-height: 0; border-left: 0; }
  .stage-big.live .controls, .stage-big.live .top-bar { opacity: 1; }
  .controls { bottom: 8px; padding: 4px; }
  .round, .leave { height: 36px; }
  .round { width: 36px; }
  .slider { display: none; }
  .leave span { display: none; }
}
.stage-big:fullscreen .controls { bottom: 32px; }
</style>
