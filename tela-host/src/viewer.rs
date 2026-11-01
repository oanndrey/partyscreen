// Lado de quem assiste: entra na sala pelo código, recebe o vídeo H.264 via WebRTC,
// decodifica com OpenH264 e entrega quadros RGBA para a interface desenhar.
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;
use webrtc::api::APIBuilder;
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::MediaEngine;
use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::interceptor::registry::Registry;
use webrtc::media::io::sample_builder::SampleBuilder;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::rtcp::payload_feedbacks::picture_loss_indication::PictureLossIndication;
use webrtc::rtp::codecs::h264::H264Packet;
use webrtc::rtp_transceiver::rtp_codec::RTPCodecType;
use webrtc::track::track_remote::TrackRemote;

use crate::session::{Handle, Repaint, SharedStatus, State, Status, endpoints};

pub struct VideoFrame {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
    /// Muda a cada quadro novo, para a interface saber quando atualizar a textura
    pub serial: u64,
}

pub type SharedFrame = Arc<Mutex<Option<VideoFrame>>>;

pub fn start(
    rt: &tokio::runtime::Runtime,
    site: String,
    code: String,
    status: SharedStatus,
    frame: SharedFrame,
    repaint: Repaint,
) -> Handle {
    let (stop_tx, stop_rx) = oneshot::channel();
    status.lock().unwrap().state = State::Connecting;
    repaint();

    let st = status.clone();
    rt.spawn(async move {
        let result = tokio::select! {
            r = run(site, code, st.clone(), frame.clone(), repaint.clone()) => r,
            _ = stop_rx => Ok(()),
        };
        *frame.lock().unwrap() = None;
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

async fn run(site: String, code: String, status: SharedStatus, frame: SharedFrame, repaint: Repaint) -> Result<()> {
    let (_, ws_url) = endpoints(&site)?;
    let (ws, _) = tokio_tungstenite::connect_async(&ws_url)
        .await
        .with_context(|| format!("não conectou em {ws_url}"))?;
    let (mut ws_write, mut ws_read) = ws.split();

    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Value>();
    let writer = tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            if ws_write.send(Message::text(msg.to_string())).await.is_err() {
                break;
            }
        }
    });
    let _writer = scopeguard(writer);

    let code = code.trim().to_uppercase();
    out_tx.send(json!({ "type": "join", "room": code, "role": "viewer" }))?;

    let mut pc: Option<Arc<RTCPeerConnection>> = None;
    let set_state = |state: State| {
        status.lock().unwrap().state = state;
        repaint();
    };

    let result = async {
        while let Some(msg) = ws_read.next().await {
            let Ok(Message::Text(text)) = msg else { continue };
            let Ok(msg) = serde_json::from_str::<Value>(&text) else { continue };

            match msg["type"].as_str() {
                Some("joined") => set_state(State::Waiting),
                Some("error") => anyhow::bail!("{}", msg["message"].as_str().unwrap_or("erro no servidor")),
                Some("host-left") => {
                    if let Some(old) = pc.take() {
                        let _ = old.close().await;
                    }
                    *frame.lock().unwrap() = None;
                    set_state(State::Waiting);
                }
                Some("signal") => {
                    let from = msg["from"].as_str().unwrap_or_default().to_string();
                    let data = &msg["data"];
                    if !data["sdp"].is_null() {
                        // Nova offer = nova sessão de quem compartilha
                        if let Some(old) = pc.take() {
                            let _ = old.close().await;
                        }
                        let offer = serde_json::from_value::<RTCSessionDescription>(data["sdp"].clone())?;
                        let conn = create_peer(&from, out_tx.clone(), status.clone(), frame.clone(), repaint.clone()).await?;
                        conn.set_remote_description(offer).await?;
                        let answer = conn.create_answer(None).await?;
                        conn.set_local_description(answer).await?;
                        let local = conn.local_description().await.context("sem descrição local")?;
                        out_tx.send(json!({ "type": "signal", "to": from, "data": { "sdp": local } }))?;
                        pc = Some(conn);
                    } else if !data["candidate"].is_null() {
                        if let Some(conn) = &pc {
                            let c = serde_json::from_value::<RTCIceCandidateInit>(data["candidate"].clone())?;
                            conn.add_ice_candidate(c).await?;
                        }
                    }
                }
                _ => {}
            }
        }
        anyhow::bail!("a conexão com o servidor caiu")
    }
    .await;

    if let Some(conn) = pc {
        let _ = conn.close().await;
    }
    result
}

fn scopeguard(handle: tokio::task::JoinHandle<()>) -> impl Drop {
    struct Abort(tokio::task::JoinHandle<()>);
    impl Drop for Abort {
        fn drop(&mut self) {
            self.0.abort();
        }
    }
    Abort(handle)
}

async fn create_peer(
    host_id: &str,
    out: mpsc::UnboundedSender<Value>,
    status: SharedStatus,
    frame: SharedFrame,
    repaint: Repaint,
) -> Result<Arc<RTCPeerConnection>> {
    let mut media = MediaEngine::default();
    media.register_default_codecs()?;
    let registry = register_default_interceptors(Registry::new(), &mut media)?;
    let api = APIBuilder::new().with_media_engine(media).with_interceptor_registry(registry).build();

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

    let to = host_id.to_string();
    pc.on_ice_candidate(Box::new(move |c| {
        let (to, out) = (to.clone(), out.clone());
        Box::pin(async move {
            let Some(c) = c else { return };
            if let Ok(init) = c.to_json() {
                let _ = out.send(json!({ "type": "signal", "to": to, "data": { "candidate": init } }));
            }
        })
    }));

    let weak = Arc::downgrade(&pc);
    pc.on_track(Box::new(move |track, _, _| {
        let (status, frame, repaint, weak) = (status.clone(), frame.clone(), repaint.clone(), weak.clone());
        Box::pin(async move {
            tokio::spawn(async move {
                let result = if track.kind() == RTPCodecType::Audio {
                    receive_audio(track).await
                } else {
                    receive_video(track, weak, status, frame, repaint).await
                };
                if let Err(e) = result {
                    eprintln!("Mídia encerrada: {e}");
                }
            });
        })
    }));

    Ok(pc)
}

async fn receive_video(
    track: Arc<TrackRemote>,
    pc: std::sync::Weak<RTCPeerConnection>,
    status: SharedStatus,
    frame: SharedFrame,
    repaint: Repaint,
) -> Result<()> {
    let mut builder = SampleBuilder::new(256, H264Packet::default(), 90000);
    let mut decoder = openh264::decoder::Decoder::new()?;
    let mut serial = 0u64;
    let mut last_pli = Instant::now() - Duration::from_secs(10);
    let mut stats = (Instant::now(), 0u32, 0usize);

    loop {
        let (packet, _) = track.read_rtp().await?;
        builder.push(packet);

        while let Some(sample) = builder.pop() {
            stats.1 += 1;
            stats.2 += sample.data.len();

            // Decodificar pesa: roda fora das threads de rede
            let decoded = tokio::task::block_in_place(|| match decoder.decode(&sample.data) {
                Ok(Some(yuv)) => {
                    use openh264::formats::YUVSource;
                    let (w, h) = yuv.dimensions();
                    let mut rgba = vec![0u8; w * h * 4];
                    yuv.write_rgba8(&mut rgba);
                    Ok(Some((w, h, rgba)))
                }
                Ok(None) => Ok(None),
                Err(e) => Err(e),
            });

            match decoded {
                Ok(Some((width, height, rgba))) => {
                    serial += 1;
                    *frame.lock().unwrap() = Some(VideoFrame { width, height, rgba, serial });
                    if serial == 1 {
                        status.lock().unwrap().state = State::Watching;
                    }
                    repaint();
                }
                Ok(None) => {}
                // Quadro corrompido (perda de pacote): pede um quadro completo, no máximo 1x/s
                Err(_) if last_pli.elapsed() > Duration::from_secs(1) => {
                    last_pli = Instant::now();
                    if let Some(pc) = pc.upgrade() {
                        let pli = PictureLossIndication { sender_ssrc: 0, media_ssrc: track.ssrc() };
                        let _ = pc.write_rtcp(&[Box::new(pli)]).await;
                    }
                }
                Err(_) => {}
            }
        }

        if stats.0.elapsed() >= Duration::from_secs(2) {
            let secs = stats.0.elapsed().as_secs_f64();
            let mut s = status.lock().unwrap();
            s.fps = stats.1 as f64 / secs;
            s.mbps = stats.2 as f64 * 8.0 / secs / 1e6;
            stats = (Instant::now(), 0, 0);
        }
    }
}

/// Cada pacote RTP de áudio é um quadro Opus inteiro: decodifica e toca direto
async fn receive_audio(track: Arc<TrackRemote>) -> Result<()> {
    let mut player = crate::audio::Player::new()?;
    loop {
        let (packet, _) = track.read_rtp().await?;
        if !packet.payload.is_empty() {
            player.play(&packet.payload);
        }
    }
}
