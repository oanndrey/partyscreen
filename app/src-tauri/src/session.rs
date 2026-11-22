// Sessão de compartilhamento: conecta no servidor de sinalização, cria a sala e
// abre uma conexão WebRTC por espectador. Fala o mesmo protocolo do navegador.
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::{MIME_TYPE_H264, MIME_TYPE_OPUS, MediaEngine};
use webrtc::api::{API, APIBuilder};
use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;
use webrtc::api::setting_engine::SettingEngine;
use webrtc::ice_transport::ice_candidate::RTCIceCandidate;
use webrtc::ice_transport::ice_candidate_type::RTCIceCandidateType;
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::interceptor::registry::Registry;
use webrtc::media::Sample;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::rtcp::payload_feedbacks::picture_loss_indication::PictureLossIndication;
use webrtc::rtcp::receiver_report::ReceiverReport;
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability;
use webrtc::track::track_local::TrackLocal;
use webrtc::track::track_local::track_local_static_sample::TrackLocalStaticSample;

use crate::EncodedFrame;

/// O que compartilhar
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Source {
    /// Índice da tela (0 = primeira)
    Display(usize),
    /// Identificador da janela no sistema
    Window(u64),
}

#[derive(serde::Serialize)]
pub struct SourceInfo {
    pub source: Source,
    pub label: String,
}

/// App em execução que pode ser silenciado na transmissão (lista do comando `list_audio_apps`)
#[derive(serde::Serialize, serde::Deserialize)]
pub struct AudioApp {
    pub bundle_id: String,
    pub name: String,
    /// App de chamada conhecido (Discord, Zoom...), sugerido para silenciar
    pub suggested: bool,
}

#[derive(Clone, serde::Deserialize)]
pub struct Config {
    pub source: Source,
    pub audio: bool,
    /// Endereço do site, como abre no navegador (ex.: https://abc.trycloudflare.com)
    pub site: String,
    pub fps: u32,
    pub mbps: u32,
    /// true = prioriza nitidez (pode pular quadros); false = prioriza fluidez
    #[serde(default)]
    pub sharp: bool,
    #[serde(default = "default_max_width")]
    pub max_width: usize,
    /// Nick de quem transmite, mostrado para a sala
    #[serde(default)]
    pub name: String,
    /// Apps cujo som não vai para a transmissão: bundle IDs no Mac (ex.: "com.hnc.Discord"),
    /// nome do executável no Windows (ex.: "discord.exe").
    #[serde(default)]
    #[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
    pub muted_apps: Vec<String>,
}

/// Apps de chamada de voz/vídeo conhecidos (pedaços do bundle ID ou do nome, minúsculos)
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
const CALL_APPS: &[&str] = &[
    "com.hnc.discord",
    "discord",
    "teamspeak",
    "ts3client",
    "us.zoom.",
    "zoom.us",
    "com.tinyspeck.slackmacgap",
    "slack",
    "com.microsoft.teams",
    "microsoft teams",
    "whatsapp",
    "telegram",
    "ru.keepcoder.telegram",
    "skype",
    "com.apple.facetime",
    "facetime",
];

#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
pub fn is_call_app(bundle_id: &str, name: &str) -> bool {
    let (b, n) = (bundle_id.to_lowercase(), name.to_lowercase());
    CALL_APPS.iter().any(|k| b.contains(k) || n.contains(k))
}

fn default_max_width() -> usize {
    1920
}

#[derive(Clone, Default, PartialEq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum State {
    #[default]
    Idle,
    Connecting,
    /// Compartilhando: sala criada
    Live { code: String, link: String },
    Error { message: String },
}

/// Estado que a interface mostra; atualizado pela sessão e pelo encoder
#[derive(Clone, Default, serde::Serialize)]
pub struct Status {
    pub state: State,
    pub viewers: usize,
    pub fps: f64,
    pub mbps: f64,
    pub encode_ms: f64,
    /// Onde o vídeo é codificado ("Placa de vídeo · NVIDIA", "Processador"...)
    pub encoder: String,
    /// Caminho da conexão de cada espectador: "local", "direct", "direct-ipv6" ou "relay"
    pub routes: std::collections::BTreeMap<String, String>,
    /// Quem está na sala: [{ id, name, role }], como o servidor manda
    pub members: Vec<Value>,
    /// Chat da sala: [{ id?, name?, text, ts, system? }], os mais recentes no fim
    pub chat: Vec<Value>,
}

pub type SharedStatus = Arc<Mutex<Status>>;
pub type Repaint = Arc<dyn Fn() + Send + Sync>;

/// Sessão em andamento; soltar o `stop` encerra tudo
pub struct Handle {
    pub(crate) stop: Option<oneshot::Sender<()>>,
    /// Canal para o servidor de sinalização (usado pelo chat)
    out: mpsc::UnboundedSender<Value>,
}

impl Handle {
    pub fn chat(&self, text: &str) {
        let _ = self.out.send(json!({ "type": "chat", "text": text }));
    }

    pub fn stop(mut self) {
        if let Some(tx) = self.stop.take() {
            let _ = tx.send(());
        }
    }
}

/// Aceita o endereço do site (http/https, com ou sem barra no fim) e devolve
/// (link base para quem assiste, URL do WebSocket).
pub fn endpoints(site: &str) -> Result<(String, String)> {
    let site = site.trim().trim_end_matches('/');
    let (scheme, host) = site
        .split_once("://")
        .map(|(s, h)| (s.to_lowercase(), h))
        .unwrap_or(("https".into(), site));
    let host = host.split(['/', '#']).next().unwrap_or_default();
    anyhow::ensure!(!host.is_empty(), "informe o endereço do servidor");
    let ws = match scheme.as_str() {
        "http" | "ws" => "ws",
        _ => "wss",
    };
    let web = if ws == "ws" { "http" } else { "https" };
    Ok((format!("{web}://{host}"), format!("{ws}://{host}/ws")))
}

fn room_code() -> String {
    const CHARS: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    (0..6).map(|_| CHARS[rand::random_range(0..CHARS.len())] as char).collect()
}

pub fn start(rt: &tokio::runtime::Runtime, cfg: Config, status: SharedStatus, repaint: Repaint) -> Handle {
    let (stop_tx, stop_rx) = oneshot::channel();
    status.lock().unwrap().state = State::Connecting;
    repaint();

    // Tudo que vai para o servidor passa por este canal (os callbacks do WebRTC e o chat também enviam)
    let (out_tx, out_rx) = mpsc::unbounded_channel::<Value>();
    let st = status.clone();
    let out = out_tx.clone();
    rt.spawn(async move {
        let result = tokio::select! {
            r = run(cfg, st.clone(), repaint.clone(), out_tx, out_rx) => r,
            _ = stop_rx => Ok(()),
        };
        let mut s = st.lock().unwrap();
        *s = Status::default();
        if let Err(e) = result {
            s.state = State::Error { message: format!("{e:#}") };
        }
        drop(s);
        repaint();
    });

    Handle { stop: Some(stop_tx), out }
}

async fn run(
    cfg: Config,
    status: SharedStatus,
    repaint: Repaint,
    out_tx: mpsc::UnboundedSender<Value>,
    mut out_rx: mpsc::UnboundedReceiver<Value>,
) -> Result<()> {
    let (web, ws_url) = endpoints(&cfg.site)?;
    let code = room_code();

    let (ws, _) = tokio_tungstenite::connect_async(&ws_url)
        .await
        .with_context(|| format!("não conectou em {ws_url}"))?;
    let (mut ws_write, mut ws_read) = ws.split();

    // Trilha única de vídeo: o mesmo quadro codificado vai para todos os espectadores
    let track = Arc::new(TrackLocalStaticSample::new(
        RTCRtpCodecCapability {
            mime_type: MIME_TYPE_H264.to_owned(),
            clock_rate: 90000,
            sdp_fmtp_line: "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42e01f".to_owned(),
            ..Default::default()
        },
        "video".to_owned(),
        "tela-host".to_owned(),
    ));
    // Áudio na mesma stream do vídeo ("tela-host"): o navegador sincroniza os dois
    let audio_track = Arc::new(TrackLocalStaticSample::new(
        RTCRtpCodecCapability {
            mime_type: MIME_TYPE_OPUS.to_owned(),
            clock_rate: crate::audio::RATE,
            channels: 2,
            sdp_fmtp_line: "minptime=10;useinbandfec=1;stereo=1;sprop-stereo=1".to_owned(),
            ..Default::default()
        },
        "audio".to_owned(),
        "tela-host".to_owned(),
    ));
    let keyframe = Arc::new(AtomicBool::new(true));

    let (frame_tx, mut frame_rx) = mpsc::unbounded_channel::<EncodedFrame>();
    let (audio_tx, mut audio_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let packer = if cfg.audio { Some(crate::audio::Packer::new(audio_tx)?) } else { None };
    #[cfg(any(windows, target_os = "macos"))]
    let loss = Arc::new(AtomicU32::new(0));
    let _capture = crate::encoder::start(&cfg, frame_tx, packer, keyframe.clone(), loss.clone(), status.clone(), repaint.clone())?;
    #[cfg(not(any(windows, target_os = "macos")))]
    drop((frame_tx, packer));

    let audio_writer = audio_track.clone();
    let _audio_sender = AbortOnDrop(tokio::spawn(async move {
        while let Some(packet) = audio_rx.recv().await {
            let sample = Sample { data: Bytes::from(packet), duration: std::time::Duration::from_millis(20), ..Default::default() };
            let _ = audio_writer.write_sample(&sample).await;
        }
    }));

    let writer = track.clone();
    // Quando a sessão acaba, este laço termina e o encoder para (canal fechado)
    let _sender = tokio::spawn(async move {
        while let Some(frame) = frame_rx.recv().await {
            let sample = Sample { data: Bytes::from(frame.data), duration: frame.duration, ..Default::default() };
            let _ = writer.write_sample(&sample).await;
        }
    });
    let _sender = AbortOnDrop(_sender);

    let mut media = MediaEngine::default();
    media.register_default_codecs()?;
    let registry = register_default_interceptors(Registry::new(), &mut media)?;
    // Prioriza conexão direta: só aceita retransmissão (TURN) se nenhuma rota direta
    // responder em 6 s (o padrão é 2 s, e às vezes o TURN ganhava antes da direta)
    let mut settings = SettingEngine::default();
    settings.set_relay_acceptance_min_wait(Some(Duration::from_secs(6)));
    // Navegadores escondem o IP local atrás de nomes "xxxx.local" (mDNS); sem resolver,
    // dois computadores na mesma casa davam a volta pela VPS
    settings.set_ice_multicast_dns_mode(webrtc::ice::mdns::MulticastDnsMode::QueryOnly);
    let api = APIBuilder::new()
        .with_media_engine(media)
        .with_interceptor_registry(registry)
        .with_setting_engine(settings)
        .build();

    let _writer = AbortOnDrop(tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            if ws_write.send(Message::text(msg.to_string())).await.is_err() {
                break;
            }
        }
    }));

    out_tx.send(json!({ "type": "join", "room": code, "role": "host", "name": cfg.name }))?;

    let mut peers = Peers::default();
    let mut ice = default_ice_servers();

    while let Some(msg) = ws_read.next().await {
        let Ok(Message::Text(text)) = msg else { continue };
        let Ok(msg) = serde_json::from_str::<Value>(&text) else { continue };

        match msg["type"].as_str() {
            Some("joined") => {
                if let Some(list) = parse_ice_servers(&msg["iceServers"]) {
                    ice = list;
                }
                status.lock().unwrap().state = State::Live { link: format!("{web}/#/sala/{code}"), code: code.clone() };
                repaint();
            }
            Some("chat") => {
                let mut s = status.lock().unwrap();
                s.chat.push(msg.clone());
                let excess = s.chat.len().saturating_sub(200);
                s.chat.drain(..excess);
            }
            Some("members") => status.lock().unwrap().members = msg["members"].as_array().cloned().unwrap_or_default(),
            Some("error") => anyhow::bail!("{}", msg["message"].as_str().unwrap_or("erro no servidor")),
            Some("viewer-joined") => {
                let id = msg["id"].as_str().unwrap_or_default().to_string();
                peers.remove(&id).await;
                status.lock().unwrap().routes.remove(&id);
                let audio = cfg.audio.then(|| audio_track.clone());
                let route = RouteReporter { id: id.clone(), status: status.clone(), repaint: repaint.clone() };
                match create_peer(&api, &id, ice.clone(), track.clone(), audio, keyframe.clone(), loss.clone(), out_tx.clone(), route).await {
                    Ok(pc) => {
                        peers.0.insert(id, pc);
                    }
                    Err(e) => eprintln!("Falha ao conectar espectador: {e}"),
                }
            }
            Some("viewer-left") => {
                let id = msg["id"].as_str().unwrap_or_default();
                peers.remove(id).await;
                status.lock().unwrap().routes.remove(id);
            }
            Some("signal") => {
                let Some(pc) = peers.0.get(msg["from"].as_str().unwrap_or_default()) else { continue };
                if let Err(e) = apply_signal(pc, &msg["data"]).await {
                    eprintln!("Erro na sinalização: {e}");
                }
            }
            _ => {}
        }
        status.lock().unwrap().viewers = peers.0.len();
        repaint();
    }

    anyhow::bail!("a conexão com o servidor caiu")
}

/// Fecha as conexões dos espectadores quando a sessão termina (inclusive ao parar)
#[derive(Default)]
struct Peers(HashMap<String, Arc<RTCPeerConnection>>);

impl Peers {
    async fn remove(&mut self, id: &str) {
        if let Some(pc) = self.0.remove(id) {
            let _ = pc.close().await;
        }
    }
}

impl Drop for Peers {
    fn drop(&mut self) {
        for (_, pc) in self.0.drain() {
            tokio::spawn(async move {
                let _ = pc.close().await;
            });
        }
    }
}

struct AbortOnDrop<T>(tokio::task::JoinHandle<T>);

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn apply_signal(pc: &RTCPeerConnection, data: &Value) -> Result<()> {
    if !data["sdp"].is_null() {
        let sdp = serde_json::from_value::<RTCSessionDescription>(data["sdp"].clone())?;
        pc.set_remote_description(sdp).await?;
    } else if !data["candidate"].is_null() {
        let candidate = serde_json::from_value::<RTCIceCandidateInit>(data["candidate"].clone())?;
        pc.add_ice_candidate(candidate).await?;
    }
    Ok(())
}

/// Atualiza na interface o caminho escolhido para um espectador
#[derive(Clone)]
struct RouteReporter {
    id: String,
    status: SharedStatus,
    repaint: Repaint,
}

/// Classifica o par de candidatos escolhido pelo ICE
fn route_kind(local: &RTCIceCandidate, remote: &RTCIceCandidate) -> &'static str {
    if local.typ == RTCIceCandidateType::Relay || remote.typ == RTCIceCandidateType::Relay {
        "relay"
    } else if local.typ == RTCIceCandidateType::Host && remote.typ == RTCIceCandidateType::Host {
        "local"
    } else if local.address.contains(':') {
        "direct-ipv6"
    } else {
        "direct"
    }
}

fn default_ice_servers() -> Vec<RTCIceServer> {
    vec![RTCIceServer { urls: vec!["stun:stun.l.google.com:19302".to_owned()], ..Default::default() }]
}

/// Lê a lista de servidores STUN/TURN enviada pelo servidor (formato do navegador).
/// Fica só com STUN e TURN via UDP, que é o que o webrtc-rs usa para retransmitir.
fn parse_ice_servers(value: &Value) -> Option<Vec<RTCIceServer>> {
    let list: Vec<RTCIceServer> = value
        .as_array()?
        .iter()
        .filter_map(|server| {
            let urls: Vec<String> = match &server["urls"] {
                Value::String(u) => vec![u.clone()],
                Value::Array(a) => a.iter().filter_map(|u| u.as_str().map(String::from)).collect(),
                _ => return None,
            };
            let urls: Vec<String> = urls
                .into_iter()
                .filter(|u| u.starts_with("stun:") || (u.starts_with("turn:") && !u.contains("transport=tcp")))
                .collect();
            (!urls.is_empty()).then(|| RTCIceServer {
                urls,
                username: server["username"].as_str().unwrap_or_default().to_string(),
                credential: server["credential"].as_str().unwrap_or_default().to_string(),
            })
        })
        .collect();
    (!list.is_empty()).then_some(list)
}

async fn create_peer(
    api: &API,
    id: &str,
    ice_servers: Vec<RTCIceServer>,
    track: Arc<TrackLocalStaticSample>,
    audio: Option<Arc<TrackLocalStaticSample>>,
    keyframe: Arc<AtomicBool>,
    loss: Arc<AtomicU32>,
    out: mpsc::UnboundedSender<Value>,
    route: RouteReporter,
) -> Result<Arc<RTCPeerConnection>> {
    let pc = Arc::new(
        api.new_peer_connection(RTCConfiguration {
            ice_servers,
            ..Default::default()
        })
        .await?,
    );

    pc.sctp().transport().ice_transport().on_selected_candidate_pair_change(Box::new(move |pair| {
        let kind = route_kind(&pair.local, &pair.remote);
        route.status.lock().unwrap().routes.insert(route.id.clone(), kind.to_string());
        (route.repaint)();
        Box::pin(async {})
    }));

    let sender = pc.add_track(track as Arc<dyn TrackLocal + Send + Sync>).await?;
    if let Some(audio) = audio {
        let audio_sender = pc.add_track(audio as Arc<dyn TrackLocal + Send + Sync>).await?;
        // RTCP precisa ser lido para os interceptors funcionarem
        tokio::spawn(async move { while audio_sender.read_rtcp().await.is_ok() {} });
    }

    // O navegador pede um quadro completo (PLI) quando perde pacotes
    let kf = keyframe.clone();
    tokio::spawn(async move {
        while let Ok((packets, _)) = sender.read_rtcp().await {
            for p in &packets {
                if p.as_any().is::<PictureLossIndication>() {
                    kf.store(true, Ordering::Relaxed);
                }
                // Relatório de recepção: guarda a pior perda (fração de 256) para o encoder adaptar a banda
                if let Some(rr) = p.as_any().downcast_ref::<ReceiverReport>() {
                    for r in &rr.reports {
                        loss.fetch_max(r.fraction_lost as u32 * 1000 / 256, Ordering::Relaxed);
                    }
                }
            }
        }
    });

    let to = id.to_string();
    let ice_out = out.clone();
    pc.on_ice_candidate(Box::new(move |c| {
        let (to, out) = (to.clone(), ice_out.clone());
        Box::pin(async move {
            let Some(c) = c else { return };
            if let Ok(init) = c.to_json() {
                let _ = out.send(json!({ "type": "signal", "to": to, "data": { "candidate": init } }));
            }
        })
    }));

    pc.on_peer_connection_state_change(Box::new(move |state| {
        if state == RTCPeerConnectionState::Connected {
            keyframe.store(true, Ordering::Relaxed);
        }
        Box::pin(async {})
    }));

    let offer = pc.create_offer(None).await?;
    pc.set_local_description(offer).await?;
    let local = pc.local_description().await.context("sem descrição local")?;
    out.send(json!({ "type": "signal", "to": id, "data": { "sdp": local } }))?;

    Ok(pc)
}

#[cfg(test)]
mod tests {
    use super::endpoints;

    #[test]
    fn endpoints_aceitam_varios_formatos() {
        let t = |s| endpoints(s).unwrap();
        assert_eq!(t("https://abc.trycloudflare.com/"), ("https://abc.trycloudflare.com".into(), "wss://abc.trycloudflare.com/ws".into()));
        assert_eq!(t("abc.trycloudflare.com"), ("https://abc.trycloudflare.com".into(), "wss://abc.trycloudflare.com/ws".into()));
        assert_eq!(t("http://192.168.2.111:3001"), ("http://192.168.2.111:3001".into(), "ws://192.168.2.111:3001/ws".into()));
        assert_eq!(t("https://x.com/#/sala/ABC"), ("https://x.com".into(), "wss://x.com/ws".into()));
        assert!(endpoints("  ").is_err());
    }
}

#[cfg(test)]
mod ice_tests {
    use super::parse_ice_servers;

    #[test]
    fn le_turn_da_cloudflare() {
        let v = serde_json::json!([
            { "urls": "stun:stun.cloudflare.com:3478" },
            { "urls": ["turn:turn.cloudflare.com:3478?transport=udp", "turn:turn.cloudflare.com:3478?transport=tcp", "turns:turn.cloudflare.com:5349?transport=tcp"],
              "username": "u", "credential": "c" }
        ]);
        let list = parse_ice_servers(&v).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[1].urls, vec!["turn:turn.cloudflare.com:3478?transport=udp"]);
        assert_eq!((list[1].username.as_str(), list[1].credential.as_str()), ("u", "c"));
        assert!(parse_ice_servers(&serde_json::Value::Null).is_none());
    }
}
