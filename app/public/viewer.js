// Lado de quem assiste: entra na sala e recebe vídeo/áudio pelo WebRTC da janela.
// Mesmo protocolo da página web (src/views/Viewer.vue). Exposto como window.telaViewer.
(function () {
  function endpoints(site) {
    let s = site.trim().replace(/\/+$/, '')
    if (!/^[a-z]+:\/\//i.test(s)) s = 'https://' + s
    const url = new URL(s)
    const secure = url.protocol === 'https:' || url.protocol === 'wss:'
    return {
      api: `${secure ? 'https' : 'http'}://${url.host}/api/config`,
      ws: `${secure ? 'wss' : 'ws'}://${url.host}/ws`,
    }
  }

  async function iceServers(api) {
    try {
      const r = await fetch(api)
      return (await r.json()).iceServers
    } catch {
      return [{ urls: 'stun:stun.l.google.com:19302' }]
    }
  }

  // Caminho da conexão: mesma rede, direta pela internet (IPv4/IPv6) ou retransmitida (TURN)
  function routeKind(local, remote) {
    if (!local || !remote) return undefined
    if (local.candidateType === 'relay' || remote.candidateType === 'relay') return 'relay'
    if (local.candidateType === 'host' && remote.candidateType === 'host') return 'local'
    const addr = remote.address || remote.ip || ''
    return addr.includes(':') ? 'direct-ipv6' : 'direct'
  }

  // onStatus recebe { state: 'connecting'|'waiting'|'live'|'error', message?, fps?, mbps?, route?, members?, me?, chat? }
  function start(site, code, name, video, onStatus) {
    let ws, pc, statsTimer
    let stopped = false
    let queue = Promise.resolve()
    const ep = endpoints(site)

    const closePeer = () => {
      clearInterval(statsTimer)
      pc?.close()
      pc = null
      queue = Promise.resolve()
      video.srcObject = null
    }

    iceServers(ep.api).then((servers) => {
      if (stopped) return
      ws = new WebSocket(ep.ws)
      ws.onopen = () => ws.send(JSON.stringify({ type: 'join', room: code.toUpperCase(), role: 'viewer', name }))
      ws.onclose = () => !stopped && onStatus({ state: 'error', message: 'A conexão com o servidor caiu.' })
      ws.onerror = () => !stopped && onStatus({ state: 'error', message: 'Não conectou no servidor.' })
      ws.onmessage = (e) => {
        const msg = JSON.parse(e.data)
        if (msg.type === 'joined') onStatus({ state: 'waiting', me: msg.id })
        else if (msg.type === 'members') onStatus({ members: msg.members })
        else if (msg.type === 'chat') onStatus({ chat: msg })
        else if (msg.type === 'error') onStatus({ state: 'error', message: msg.message })
        else if (msg.type === 'host-left') {
          closePeer()
          onStatus({ state: 'waiting' })
        } else if (msg.type === 'signal') {
          if (msg.data.sdp?.type === 'offer') createPeer(msg.from, servers)
          queue = queue.then(() => apply(msg.from, msg.data)).catch(console.error)
        }
      }
    })

    function createPeer(hostId, servers) {
      closePeer()
      pc = new RTCPeerConnection({ iceServers: servers })
      pc.onicecandidate = (e) =>
        e.candidate && ws.send(JSON.stringify({ type: 'signal', to: hostId, data: { candidate: e.candidate } }))
      pc.ontrack = (e) => {
        if (video.srcObject !== e.streams[0]) video.srcObject = e.streams[0]
        video.play().catch(() => {})
        onStatus({ state: 'live' })
      }
      pc.onconnectionstatechange = () => {
        if (pc?.connectionState === 'failed') {
          onStatus({ state: 'error', message: 'A conexão direta falhou. A rede de um dos dois pode estar bloqueando.' })
        }
      }
      // Estatísticas de recepção a cada 2 s
      let lastBytes = 0
      let lastTime = performance.now()
      statsTimer = setInterval(async () => {
        if (!pc) return
        const stats = await pc.getStats()
        let video
        let pairId
        stats.forEach((s) => {
          if (s.type === 'inbound-rtp' && s.kind === 'video') video = s
          if (s.type === 'transport' && s.selectedCandidatePairId) pairId = s.selectedCandidatePairId
        })
        // Alguns navegadores não informam o par escolhido no "transport": usa o nomeado
        if (!pairId) stats.forEach((s) => {
          if (s.type === 'candidate-pair' && s.nominated && s.state === 'succeeded') pairId = s.id
        })
        const pair = pairId && stats.get(pairId)
        const route = pair && routeKind(stats.get(pair.localCandidateId), stats.get(pair.remoteCandidateId))
        if (video) {
          const now = performance.now()
          const mbps = ((video.bytesReceived - lastBytes) * 8) / ((now - lastTime) / 1000) / 1e6
          lastBytes = video.bytesReceived
          lastTime = now
          onStatus({ state: 'live', fps: video.framesPerSecond || 0, mbps, route })
        }
      }, 2000)
    }

    async function apply(hostId, data) {
      if (!pc) return
      if (data.sdp) {
        await pc.setRemoteDescription(data.sdp)
        await pc.setLocalDescription(await pc.createAnswer())
        ws.send(JSON.stringify({ type: 'signal', to: hostId, data: { sdp: pc.localDescription } }))
      } else if (data.candidate) {
        await pc.addIceCandidate(data.candidate)
      }
    }

    return {
      chat(text) {
        if (ws?.readyState === WebSocket.OPEN) ws.send(JSON.stringify({ type: 'chat', text }))
      },
      stop() {
        stopped = true
        closePeer()
        if (ws) {
          ws.onclose = null
          ws.close()
        }
      },
    }
  }

  window.telaViewer = { start }
})()
