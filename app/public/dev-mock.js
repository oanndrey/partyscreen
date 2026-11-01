// Modo simulação: só ativa quando a interface roda num navegador comum (fora do app Tauri).
// Finge os comandos do backend Rust para dar para testar a interface com `trunk serve`.
(function () {
  if (window.__TAURI__) return

  const listeners = {}
  const emit = (event, payload) => (listeners[event] || []).forEach((cb) => cb({ payload }))
  let timers = []

  const sources = [
    { source: { Display: 0 }, label: 'Tela 1 (1920x1080)' },
    { source: { Display: 1 }, label: 'Tela 2 (2560x1440)' },
    { source: { Window: 101 }, label: 'Google Chrome — YouTube' },
    { source: { Window: 102 }, label: 'Visual Studio Code — main.rs — app' },
    { source: { Window: 103 }, label: 'Counter-Strike 2 — Counter-Strike 2' },
    { source: { Window: 104 }, label: 'Spotify — Spotify Premium' },
  ]

  // Miniaturas de exemplo desenhadas num canvas (no app de verdade é uma foto da tela/janela)
  const palettes = {
    'Display:0': ['#0f2027', '#2c5364', 'Área de trabalho'],
    'Display:1': ['#41295a', '#2f0743', 'Tela 2'],
    'Window:101': ['#ffffff', '#e8eaed', 'YouTube'],
    'Window:102': ['#1e1e1e', '#252526', 'main.rs'],
    'Window:103': ['#3a2a12', '#d08b2a', 'Counter-Strike 2'],
    'Window:104': ['#121212', '#1db954', 'Spotify'],
  }
  function fakeThumb(source) {
    const key = Object.entries(source)[0].join(':')
    const [a, b, label] = palettes[key] || ['#222', '#444', '?']
    const c = document.createElement('canvas')
    c.width = 480
    c.height = 270
    const g = c.getContext('2d')
    const grad = g.createLinearGradient(0, 0, 480, 270)
    grad.addColorStop(0, a)
    grad.addColorStop(1, b)
    g.fillStyle = grad
    g.fillRect(0, 0, 480, 270)
    g.fillStyle = 'rgba(0,0,0,.25)'
    g.fillRect(0, 0, 480, 24)
    g.fillStyle = a === '#ffffff' ? '#333' : 'rgba(255,255,255,.85)'
    g.font = '600 26px Inter, sans-serif'
    g.fillText(label, 24, 150)
    return c.toDataURL('image/jpeg', 0.8)
  }

  // Estado atual da transmissão simulada; cada mudança vira um evento 'share-status'
  let cur = {}
  const push = (patch) => {
    cur = { ...cur, ...patch }
    emit('share-status', cur)
  }
  const say = (who, text) => push({ chat: [...cur.chat, { id: who.id, name: who.name, text, ts: Date.now() }] })
  const notice = (text) => push({ chat: [...cur.chat, { system: true, text, ts: Date.now() }] })

  function startShare() {
    timers.forEach(clearTimeout)
    timers = []
    const me = { id: 'h', name: JSON.parse(localStorage.getItem('settings') || '{}').nick || 'Você', role: 'host' }
    const ana = { id: 'a', name: 'Ana', role: 'viewer' }
    const beto = { id: 'b', name: 'Beto_CS', role: 'viewer' }
    const live = { kind: 'live', code: 'K7QX2M', link: 'https://partyscreen.partyscreen.workers.dev/#/sala/K7QX2M' }
    cur = { members: [], chat: [], fps: 0, mbps: 0, encode_ms: 0, routes: {}, encoder: 'Processador', state: { kind: 'connecting' } }
    push({})
    const at = (ms, fn) => timers.push(setTimeout(fn, ms))
    at(800, () => { push({ state: live, members: [me], fps: 58, mbps: 4.2, encode_ms: 9 }); notice(`${me.name} começou a transmitir`) })
    at(3000, () => { push({ members: [me, ana], routes: { a: 'direct' }, fps: 60, mbps: 5.1 }); notice('Ana entrou na sala') })
    at(4500, () => say(ana, 'eae! tá chegando liso aqui'))
    at(5200, () => say(ana, 'que jogo é esse?'))
    at(7000, () => { push({ members: [me, ana, beto], routes: { a: 'direct', b: 'relay' }, mbps: 5.8 }); notice('Beto_CS entrou na sala') })
    at(8500, () => say(beto, 'cheguei 🔥'))
  }

  window.__TAURI__ = {
    core: {
      async invoke(cmd, args) {
        console.info('[simulação] invoke', cmd, args)
        if (cmd === 'list_sources') return new Promise((r) => setTimeout(() => r(sources), 400))
        if (cmd === 'thumbnail') return new Promise((r) => setTimeout(() => r(fakeThumb(args.source)), 300 + Math.random() * 700))
        if (cmd === 'list_audio_apps') return [
          { bundle_id: 'com.hnc.Discord', name: 'Discord', suggested: true },
          { bundle_id: 'com.teamspeak.TeamSpeak3', name: 'TeamSpeak 3', suggested: true },
          { bundle_id: 'com.google.Chrome', name: 'Google Chrome', suggested: false },
          { bundle_id: 'com.spotify.client', name: 'Spotify', suggested: false },
        ]
        if (cmd === 'start_share') return startShare()
        if (cmd === 'send_chat') return say({ id: 'h', name: cur.members?.[0]?.name || 'Você' }, args.text)
        if (cmd === 'stop_share') return timers.forEach(clearTimeout)
      },
    },
    event: {
      async listen(event, cb) {
        ;(listeners[event] ||= []).push(cb)
        return () => {}
      },
    },
  }
  console.info('[simulação] PartyScreen rodando fora do app: backend simulado')
})()
