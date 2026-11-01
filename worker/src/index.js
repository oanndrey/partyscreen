// Servidor de sinalização do PartyScreen na Cloudflare (Worker + Durable Object).
// Mesmo protocolo de server/index.js: só troca SDP/ICE; o vídeo vai direto entre os peers.
import { DurableObject } from 'cloudflare:workers'

const STUN = [{ urls: 'stun:stun.cloudflare.com:3478' }, { urls: 'stun:stun.l.google.com:19302' }]

// TURN (retransmite quando a conexão direta falha). Duas opções, por secrets:
// - TURN_ICE_SERVERS: lista JSON fixa (credenciais estáticas do Metered)
// - METERED_TURN_URL: link de credenciais do Metered (plano grátis, sem cartão)
// - TURN_KEY_ID + TURN_KEY_API_TOKEN: TURN da Cloudflare
// Sem nenhum, só STUN.
// Senha temporária do coturn (modo use-auth-secret): usuário = "expira:partyscreen",
// senha = base64(HMAC-SHA1(segredo, usuário)). Vale 24 h.
async function coturnCredentials(env) {
  const username = `${Math.floor(Date.now() / 1000) + 86400}:partyscreen`
  const key = await crypto.subtle.importKey('raw', new TextEncoder().encode(env.TURN_SECRET), { name: 'HMAC', hash: 'SHA-1' }, false, ['sign'])
  const sig = await crypto.subtle.sign('HMAC', key, new TextEncoder().encode(username))
  const credential = btoa(String.fromCharCode(...new Uint8Array(sig)))
  const host = env.TURN_HOST
  return [
    { urls: `stun:${host}:3478` },
    { urls: [`turn:${host}:3478?transport=udp`, `turn:${host}:3478?transport=tcp`], username, credential },
  ]
}

async function iceServers(env) {
  // TURN próprio (coturn na VPS): secrets TURN_HOST + TURN_SECRET
  if (env.TURN_HOST && env.TURN_SECRET) {
    try {
      return [...STUN, ...(await coturnCredentials(env))]
    } catch {}
  }
  // Lista fixa de servidores (ex.: credenciais estáticas do Metered), em JSON
  if (env.TURN_ICE_SERVERS) {
    try {
      return [...STUN, ...JSON.parse(env.TURN_ICE_SERVERS)]
    } catch {}
  }
  if (env.METERED_TURN_URL) {
    try {
      const r = await fetch(env.METERED_TURN_URL)
      if (r.ok) return [...STUN, ...(await r.json())]
    } catch {}
    return STUN
  }
  if (!env.TURN_KEY_ID || !env.TURN_KEY_API_TOKEN) return STUN
  try {
    const r = await fetch(`https://rtc.live.cloudflare.com/v1/turn/keys/${env.TURN_KEY_ID}/credentials/generate-ice-servers`, {
      method: 'POST',
      headers: { Authorization: `Bearer ${env.TURN_KEY_API_TOKEN}`, 'Content-Type': 'application/json' },
      body: JSON.stringify({ ttl: 86400 }),
    })
    if (!r.ok) return STUN
    const data = await r.json()
    const turn = Array.isArray(data.iceServers) ? data.iceServers : [data.iceServers]
    return [...STUN, ...turn.filter(Boolean)]
  } catch {
    return STUN
  }
}

export default {
  async fetch(request, env) {
    const url = new URL(request.url)

    if (url.pathname === '/api/config') {
      return Response.json({ iceServers: await iceServers(env) }, { headers: { 'Access-Control-Allow-Origin': '*' } })
    }

    if (url.pathname === '/ws') {
      if (request.headers.get('Upgrade') !== 'websocket') {
        return new Response('Esperado WebSocket', { status: 426 })
      }
      // Uma instância única cuida de todas as salas (volume pequeno)
      const stub = env.SIGNALING.get(env.SIGNALING.idFromName('global'))
      return stub.fetch(request)
    }

    return new Response('Não encontrado', { status: 404 })
  },
}

export class Signaling extends DurableObject {
  async fetch() {
    const { 0: client, 1: server } = new WebSocketPair()
    // Hibernação: a Cloudflare pode descarregar o objeto entre mensagens sem derrubar as conexões
    this.ctx.acceptWebSocket(server)
    server.serializeAttachment({ id: crypto.randomUUID(), room: null, role: null })
    return new Response(null, { status: 101, webSocket: client })
  }

  info(ws) {
    return ws.deserializeAttachment() || {}
  }

  members(room) {
    return this.ctx.getWebSockets().filter((ws) => this.info(ws).room === room)
  }

  // Lista de quem está na sala (quem transmite primeiro), enviada para todos a cada entrada/saída
  broadcastMembers(room) {
    const list = this.members(room)
      .map((ws) => this.info(ws))
      .map(({ id, name, role }) => ({ id, name: name || 'Convidado', role }))
      .sort((a, b) => (a.role === 'host' ? -1 : b.role === 'host' ? 1 : 0))
    for (const ws of this.members(room)) this.send(ws, { type: 'members', members: list })
  }

  // Mensagem do chat para todos da sala (system = aviso de entrada/saída)
  broadcastChat(room, msg) {
    const full = { type: 'chat', ts: Date.now(), ...msg }
    for (const ws of this.members(room)) this.send(ws, full)
  }

  send(ws, msg) {
    try {
      ws?.send(JSON.stringify(msg))
    } catch {}
  }

  async webSocketMessage(ws, raw) {
    let msg
    try {
      msg = JSON.parse(raw)
    } catch {
      return
    }
    const me = this.info(ws)

    if (msg.type === 'join') {
      const room = String(msg.room || '').toUpperCase().slice(0, 16)
      if (!room) return this.send(ws, { type: 'error', message: 'Código de sala inválido.' })
      const others = this.members(room)
      const name = String(msg.name || '').trim().slice(0, 24) || 'Convidado'
      const host = others.find((o) => this.info(o).role === 'host')

      if (msg.role === 'host') {
        if (host) return this.send(ws, { type: 'error', message: 'Essa sala já tem alguém compartilhando.' })
        ws.serializeAttachment({ ...me, room, name, role: 'host' })
        // O app nativo recebe a configuração de rede junto com a sala
        this.send(ws, { type: 'joined', id: me.id, iceServers: await iceServers(this.env) })
        // Viewers que chegaram antes do host
        for (const v of others) this.send(ws, { type: 'viewer-joined', id: this.info(v).id })
      } else {
        ws.serializeAttachment({ ...me, room, name, role: 'viewer' })
        this.send(ws, { type: 'joined', id: me.id, hostPresent: !!host })
        this.send(host, { type: 'viewer-joined', id: me.id })
      }
      this.broadcastMembers(room)
      this.broadcastChat(room, { system: true, text: msg.role === 'host' ? `${name} começou a transmitir` : `${name} entrou na sala` })
      return
    }

    if (msg.type === 'chat' && me.room) {
      const text = String(msg.text || '').trim().slice(0, 500)
      if (text) this.broadcastChat(me.room, { id: me.id, name: me.name || 'Convidado', text })
      return
    }

    if (msg.type === 'signal' && me.room) {
      const target = this.members(me.room).find((o) => this.info(o).id === msg.to)
      this.send(target, { type: 'signal', from: me.id, data: msg.data })
    }
  }

  async webSocketClose(ws) {
    this.leave(ws)
  }

  async webSocketError(ws) {
    this.leave(ws)
  }

  leave(ws) {
    const me = this.info(ws)
    if (!me.room) return
    const others = this.members(me.room).filter((o) => o !== ws)
    if (me.role === 'host') {
      for (const v of others) this.send(v, { type: 'host-left' })
    } else {
      const host = others.find((o) => this.info(o).role === 'host')
      this.send(host, { type: 'viewer-left', id: me.id })
    }
    ws.serializeAttachment({ ...me, room: null })
    this.broadcastMembers(me.room)
    const name = me.name || 'Convidado'
    this.broadcastChat(me.room, { system: true, text: me.role === 'host' ? `${name} parou de transmitir` : `${name} saiu da sala` })
  }
}
