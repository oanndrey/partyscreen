// Servidor de sinalização: só troca mensagens SDP/ICE entre os navegadores.
// O vídeo da tela trafega direto entre os peers via WebRTC (P2P).
import http from 'node:http'
import fs from 'node:fs'
import path from 'node:path'
import crypto from 'node:crypto'
import { fileURLToPath } from 'node:url'
import { WebSocketServer } from 'ws'

const PORT = Number(process.env.PORT) || 3001
const DIST = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../dist')

const MIME = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript',
  '.css': 'text/css',
  '.svg': 'image/svg+xml',
  '.png': 'image/png',
  '.ico': 'image/x-icon',
  '.json': 'application/json',
}

function iceServers() {
  const servers = [{ urls: 'stun:stun.l.google.com:19302' }]
  if (process.env.TURN_URL) {
    servers.push({
      urls: process.env.TURN_URL.split(','),
      username: process.env.TURN_USER,
      credential: process.env.TURN_PASS,
    })
  }
  return servers
}

const server = http.createServer((req, res) => {
  const url = new URL(req.url, 'http://localhost')

  if (url.pathname === '/api/config') {
    // O app desktop (outra origem) também lê esta configuração
    res.writeHead(200, { 'Content-Type': 'application/json', 'Access-Control-Allow-Origin': '*' })
    return res.end(JSON.stringify({ iceServers: iceServers() }))
  }

  // Em produção serve o build do Vue; em dev quem serve é o Vite.
  let file = path.join(DIST, path.normalize(url.pathname).replace(/^(\.\.[/\\])+/, ''))
  if (!file.startsWith(DIST) || !fs.existsSync(file) || fs.statSync(file).isDirectory()) {
    file = path.join(DIST, 'index.html')
  }
  fs.readFile(file, (err, data) => {
    if (err) {
      res.writeHead(404)
      return res.end('Rode "npm run build" antes de "npm start".')
    }
    res.writeHead(200, { 'Content-Type': MIME[path.extname(file)] || 'application/octet-stream' })
    res.end(data)
  })
})

// rooms: código -> { host: ws | null, viewers: Map<id, ws> }
const rooms = new Map()
const wss = new WebSocketServer({ server, path: '/ws' })

function send(ws, msg) {
  if (ws && ws.readyState === ws.OPEN) ws.send(JSON.stringify(msg))
}

function findPeer(room, id) {
  if (room.host?.id === id) return room.host
  return room.viewers.get(id)
}

wss.on('connection', (ws) => {
  ws.id = crypto.randomUUID()
  ws.isAlive = true
  ws.on('pong', () => (ws.isAlive = true))

  ws.on('message', (raw) => {
    let msg
    try {
      msg = JSON.parse(raw)
    } catch {
      return
    }

    if (msg.type === 'join') {
      const code = String(msg.room || '').toUpperCase().slice(0, 16)
      if (!code) return send(ws, { type: 'error', message: 'Código de sala inválido.' })

      let room = rooms.get(code)
      if (!room) {
        room = { host: null, viewers: new Map() }
        rooms.set(code, room)
      }

      if (msg.role === 'host') {
        if (room.host) return send(ws, { type: 'error', message: 'Essa sala já tem alguém compartilhando.' })
        room.host = ws
        ws.room = code
        ws.role = 'host'
        send(ws, { type: 'joined', id: ws.id })
        // Viewers que chegaram antes do host
        for (const id of room.viewers.keys()) send(ws, { type: 'viewer-joined', id })
      } else {
        room.viewers.set(ws.id, ws)
        ws.room = code
        ws.role = 'viewer'
        send(ws, { type: 'joined', id: ws.id, hostPresent: !!room.host })
        send(room.host, { type: 'viewer-joined', id: ws.id })
      }
      return
    }

    if (msg.type === 'signal') {
      const room = rooms.get(ws.room)
      if (!room) return
      send(findPeer(room, msg.to), { type: 'signal', from: ws.id, data: msg.data })
    }
  })

  ws.on('close', () => {
    const room = rooms.get(ws.room)
    if (!room) return
    if (ws.role === 'host' && room.host === ws) {
      room.host = null
      for (const v of room.viewers.values()) send(v, { type: 'host-left' })
    } else if (ws.role === 'viewer') {
      room.viewers.delete(ws.id)
      send(room.host, { type: 'viewer-left', id: ws.id })
    }
    if (!room.host && room.viewers.size === 0) rooms.delete(ws.room)
  })
})

// Derruba conexões mortas
setInterval(() => {
  for (const ws of wss.clients) {
    if (!ws.isAlive) {
      ws.terminate()
      continue
    }
    ws.isAlive = false
    ws.ping()
  }
}, 30000)

server.listen(PORT, () => {
  console.log(`Sinalização em http://localhost:${PORT} (ws: /ws)`)
})
