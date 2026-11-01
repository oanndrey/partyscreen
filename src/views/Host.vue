<script setup>
import { computed, onBeforeUnmount, reactive, ref, shallowRef } from 'vue'
import { connectSignaling, getRtcConfig, randomRoomCode } from '../lib/signaling'

const status = ref('idle') // idle | live | error
const error = ref('')
const room = ref('')
const copied = ref(false)
const withAudio = ref(true)
const stream = shallowRef(null)
const video = ref(null)
// id do viewer -> estado da conexão (para mostrar na lista)
const viewers = reactive({})

const peers = new Map() // id -> { pc, queue }
let signaling = null

const shareLink = computed(() => `${location.origin}${location.pathname}#/sala/${room.value}`)
const viewerCount = computed(() => Object.values(viewers).filter((s) => s === 'connected').length)

async function start() {
  error.value = ''
  if (!navigator.mediaDevices?.getDisplayMedia) {
    error.value = window.isSecureContext
      ? 'Este navegador não suporta compartilhar a tela.'
      : 'Compartilhar a tela exige HTTPS. Abra o endereço com https://.'
    return
  }
  try {
    stream.value = await navigator.mediaDevices.getDisplayMedia({
      video: { frameRate: { ideal: 30 } },
      // Processamento de voz (eco/ruído/ganho) só atrapalha áudio do sistema e adiciona atraso
      audio: withAudio.value && {
        echoCancellation: false,
        noiseSuppression: false,
        autoGainControl: false,
      },
    })
  } catch (e) {
    if (e.name !== 'NotAllowedError') {
      error.value = 'Não foi possível capturar a tela: ' + e.message
    }
    return
  }

  video.value.srcObject = stream.value
  // Usuário clicou em "Parar de compartilhar" na barra do navegador
  stream.value.getVideoTracks()[0].addEventListener('ended', stop)

  room.value = randomRoomCode()
  status.value = 'live'

  const rtcConfig = await getRtcConfig()
  signaling = connectSignaling({
    room: room.value,
    role: 'host',
    onMessage: (msg) => handleMessage(msg, rtcConfig),
    onClose: () => {
      if (status.value === 'live') {
        error.value = 'Conexão com o servidor caiu. Quem já está assistindo continua conectado.'
      }
    },
  })
}

function handleMessage(msg, rtcConfig) {
  if (msg.type === 'error') {
    error.value = msg.message
    stop()
  } else if (msg.type === 'viewer-joined') {
    createPeer(msg.id, rtcConfig)
  } else if (msg.type === 'viewer-left') {
    closePeer(msg.id)
  } else if (msg.type === 'signal') {
    const peer = peers.get(msg.from)
    if (!peer) return
    // Processa em ordem para não aplicar ICE antes da answer
    peer.queue = peer.queue.then(() => applySignal(peer.pc, msg.data)).catch(console.error)
  }
}

async function applySignal(pc, data) {
  if (data.sdp) await pc.setRemoteDescription(data.sdp)
  else if (data.candidate) await pc.addIceCandidate(data.candidate)
}

async function createPeer(id, rtcConfig) {
  closePeer(id)
  const pc = new RTCPeerConnection(rtcConfig)
  const peer = { pc, queue: Promise.resolve() }
  peers.set(id, peer)
  viewers[id] = 'connecting'

  for (const track of stream.value.getTracks()) pc.addTrack(track, stream.value)
  preferQuality(pc)

  pc.onicecandidate = (e) => e.candidate && signaling.signal(id, { candidate: e.candidate })
  pc.onconnectionstatechange = () => {
    viewers[id] = pc.connectionState
    if (pc.connectionState === 'failed') closePeer(id)
  }

  peer.queue = peer.queue.then(async () => {
    await pc.setLocalDescription(await pc.createOffer())
    signaling.signal(id, { sdp: pc.localDescription })
  })
}

// Com áudio, o receptor sincroniza som e vídeo: se o vídeo engasga, o som espera junto.
// Por isso, com áudio priorizamos fluidez e mandamos o áudio na frente na rede.
function preferQuality(pc) {
  const hasAudio = stream.value.getAudioTracks().length > 0
  for (const sender of pc.getSenders()) {
    const params = sender.getParameters()
    if (sender.track?.kind === 'video') {
      sender.track.contentHint = hasAudio ? 'motion' : 'detail'
      params.degradationPreference = hasAudio ? 'balanced' : 'maintain-resolution'
    } else if (sender.track?.kind === 'audio') {
      for (const enc of params.encodings ?? []) {
        enc.maxBitrate = 128000
        enc.priority = 'high'
        enc.networkPriority = 'high'
      }
    }
    sender.setParameters(params).catch(() => {})
  }
}

function closePeer(id) {
  peers.get(id)?.pc.close()
  peers.delete(id)
  delete viewers[id]
}

function stop() {
  for (const id of [...peers.keys()]) closePeer(id)
  signaling?.close()
  signaling = null
  stream.value?.getTracks().forEach((t) => t.stop())
  stream.value = null
  if (video.value) video.value.srcObject = null
  status.value = 'idle'
  room.value = ''
}

async function copyLink() {
  await navigator.clipboard.writeText(shareLink.value)
  copied.value = true
  setTimeout(() => (copied.value = false), 1500)
}

onBeforeUnmount(stop)
</script>

<template>
  <section class="session">
    <div class="session-head">
      <div>
        <h1>{{ status === 'live' ? 'Você está compartilhando' : 'Compartilhar tela' }}</h1>
        <p v-if="status === 'live'" class="muted">
          <span class="dot live"></span>
          {{ viewerCount }} {{ viewerCount === 1 ? 'pessoa assistindo' : 'pessoas assistindo' }}
        </p>
      </div>
      <button v-if="status === 'live'" class="danger" @click="stop">Parar</button>
    </div>

    <p v-if="error" class="error">{{ error }}</p>

    <div v-if="status !== 'live'" class="card">
      <label class="check">
        <input type="checkbox" v-model="withAudio" />
        Incluir áudio (quando o navegador permitir)
      </label>
      <button class="primary" @click="start">Escolher tela e começar</button>
    </div>

    <div v-else class="card share">
      <div>
        <span class="label">Código</span>
        <strong class="code">{{ room }}</strong>
      </div>
      <div class="row grow">
        <input :value="shareLink" readonly @focus="$event.target.select()" />
        <button @click="copyLink">{{ copied ? 'Copiado!' : 'Copiar link' }}</button>
      </div>
    </div>

    <div class="stage" v-show="status === 'live'">
      <video ref="video" autoplay muted playsinline></video>
    </div>
  </section>
</template>
