// Cliente do servidor de sinalização (WebSocket).
export function connectSignaling({ room, role, name, onMessage, onClose }) {
  const proto = location.protocol === 'https:' ? 'wss' : 'ws'
  const ws = new WebSocket(`${proto}://${location.host}/ws`)

  ws.onopen = () => ws.send(JSON.stringify({ type: 'join', room, role, name }))
  ws.onmessage = (e) => onMessage(JSON.parse(e.data))
  ws.onclose = () => onClose?.()

  return {
    signal(to, data) {
      if (ws.readyState === WebSocket.OPEN) ws.send(JSON.stringify({ type: 'signal', to, data }))
    },
    chat(text) {
      if (ws.readyState === WebSocket.OPEN) ws.send(JSON.stringify({ type: 'chat', text }))
    },
    close() {
      ws.onclose = null
      ws.close()
    },
  }
}

let configPromise
export function getRtcConfig() {
  configPromise ??= fetch('/api/config')
    .then((r) => r.json())
    .catch(() => ({ iceServers: [{ urls: 'stun:stun.l.google.com:19302' }] }))
  return configPromise
}

export function randomRoomCode() {
  const chars = 'ABCDEFGHJKLMNPQRSTUVWXYZ23456789'
  const bytes = crypto.getRandomValues(new Uint8Array(6))
  return Array.from(bytes, (b) => chars[b % chars.length]).join('')
}
