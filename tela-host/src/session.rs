// Sessão de compartilhamento: conecta no servidor de sinalização, cria a sala e
// abre uma conexão WebRTC por espectador. Fala o mesmo protocolo do navegador.
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

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
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::interceptor::registry::Registry;
use webrtc::media::Sample;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::rtcp::payload_feedbacks::picture_loss_indication::PictureLossIndication;
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability;
use webrtc::track::track_local::TrackLocal;
use webrtc::track::track_local::track_local_static_sample::TrackLocalStaticSample;

use crate::EncodedFrame;

/// O que compartilhar
#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    /// Índice da tela (0 = primeira)
    Display(usize),
    /// Identificador da janela no sistema
    Window(u64),
}

pub struct SourceInfo {
    pub source: Source,
    pub label: String,
}

#[derive(Clone)]
pub struct Config {
    pub source: Source,
    pub audio: bool,
    /// Endereço do site, como abre no navegador (ex.: https://abc.trycloudflare.com)
    pub site: String,
    pub fps: u32,
    pub mbps: u32,
    pub max_width: usize,
}

#[derive(Clone, Default, PartialEq)]
pub enum State {
    #[default]
    Idle,
    Connecting,
    /// Compartilhando: sala criada
    Live { code: String, link: String },
    /// Assistindo: entrou na sala, esperando vídeo
    Waiting,
    /// Assistindo: vídeo chegando
    Watching,
    Error(String),
}

/// Estado que a interface mostra; atualizado pela sessão e pelo encoder
#[derive(Clone, Default)]
pub struct Status {
    pub state: State,
    pub viewers: usize,
    pub fps: f64,
    pub mbps: f64,
    pub encode_ms: f64,
}

pub type SharedStatus = Arc<Mutex<Status>>;
pub type Repaint = Arc<dyn Fn() + Send + Sync>;

/// Sessão em andamento; soltar o `stop` encerra tudo
pub struct Handle {
    pub(crate) stop: Option<oneshot::Sender<()>>,
}

impl Handle {
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

    let st = status.clone();
    rt.spawn(async move {
        let result = tokio::select! {
            r = run(cfg, st.clone(), repaint.clone()) => r,
            _ = stop_rx => Ok(()),
        };
        let mut s = st.lock().unwrap();
        *s = Status::default();
        if let Err(e) = result {
            s.state = State::Error(format!("{e:#}"));
        }
        drop(s);
        repaint();
    });

    Handle { stop: Some(stop_tx) }
}

async fn run(cfg: Config, status: SharedStatus, repaint: Repaint) -> Result<()> {
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
    let _capture = crate::encoder::start(&cfg, frame_tx, packer, keyframe.clone(), status.clone(), repaint.clone())?;
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
    let api = APIBuilder::new().with_media_engine(media).with_interceptor_registry(registry).build();

    // Tudo que vai para o servidor passa por este canal (os callbacks do WebRTC também enviam)
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Value>();
    let _writer = AbortOnDrop(tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            if ws_write.send(Message::text(msg.to_string())).await.is_err() {
                break;
            }
        }
    }));

    out_tx.send(json!({ "type": "join", "room": code, "role": "host" }))?;

    let mut peers = Peers::default();

    while let Some(msg) = ws_read.next().await {
        let Ok(Message::Text(text)) = msg else { continue };
        let Ok(msg) = serde_json::from_str::<Value>(&text) else { continue };

        match msg["type"].as_str() {
            Some("joined") => {
                status.lock().unwrap().state = State::Live { link: format!("{web}/#/sala/{code}"), code: code.clone() };
                repaint();
            }
            Some("error") => anyhow::bail!("{}", msg["message"].as_str().unwrap_or("erro no servidor")),
            Some("viewer-joined") => {
                let id = msg["id"].as_str().unwrap_or_default().to_string();
                peers.remove(&id).await;
                let audio = cfg.audio.then(|| audio_track.clone());
                match create_peer(&api, &id, track.clone(), audio, keyframe.clone(), out_tx.clone()).await {
                    Ok(pc) => {
                        peers.0.insert(id, pc);
                    }
                    Err(e) => eprintln!("Falha ao conectar espectador: {e}"),
                }
            }
            Some("viewer-left") => peers.remove(msg["id"].as_str().unwrap_or_default()).await,
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

async fn create_peer(
    api: &API,
    id: &str,
    track: Arc<TrackLocalStaticSample>,
    audio: Option<Arc<TrackLocalStaticSample>>,
    keyframe: Arc<AtomicBool>,
    out: mpsc::UnboundedSender<Value>,
) -> Result<Arc<RTCPeerConnection>> {
    let pc = Arc::new(
        api.new_peer_connection(RTCConfiguration {
            ice_servers: vec![RTCIceServer {
                urls: vec!["stun:stun.l.google.com:19302".to_owned()],
                ..Default::default()
            }],
            ..Default::default()
        })
        .await?,
    );

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
            if packets.iter().any(|p| p.as_any().is::<PictureLossIndication>()) {
                kf.store(true, Ordering::Relaxed);
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
