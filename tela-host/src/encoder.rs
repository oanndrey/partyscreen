// Codificação H.264 (OpenH264), comum a Windows e macOS.
//
// Captura e codificação rodam em threads separadas: a captura só guarda o quadro
// mais recente e o encoder pega sempre o último. Se o encoder atrasar, quadros
// velhos são descartados em vez de acumular fila (mantém a latência baixa).
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use openh264::OpenH264API;
use openh264::encoder::{
    BitRate, Encoder, EncoderConfig, FrameRate, IntraFramePeriod, RateControlMode, UsageType,
};
use openh264::formats::{BgraSliceU8, YUVBuffer, YUVSource};
use tokio::sync::mpsc::UnboundedSender;

use crate::EncodedFrame;
use crate::session::{Config, SharedStatus};

pub struct RawFrame {
    pub width: usize,
    pub height: usize,
    pub bgra: Vec<u8>,
}

/// Último quadro capturado, já em YUV, compartilhado entre a captura e o encoder
#[derive(Default)]
pub struct Latest {
    frame: Mutex<Option<YUVBuffer>>,
    ready: Condvar,
}

impl Latest {
    /// Converte para YUV aqui, na thread da captura, para o encoder só codificar
    pub fn put(&self, frame: RawFrame) {
        let yuv = YUVBuffer::from_bgra8_source(BgraSliceU8::new(&frame.bgra, (frame.width, frame.height)));
        *self.frame.lock().unwrap() = Some(yuv);
        self.ready.notify_one();
    }
}

/// Copia `src` (BGRA com `stride` bytes por linha) reduzindo para no máximo `max_width`
/// (vizinho mais próximo) e garantindo dimensões pares, exigidas pelo YUV 4:2:0.
pub fn scale_bgra(src: &[u8], w: usize, h: usize, stride: usize, max_width: usize) -> RawFrame {
    let (dw, dh) = if w > max_width {
        (max_width, h * max_width / w)
    } else {
        (w, h)
    };
    let (dw, dh) = (dw & !1, dh & !1);

    let mut out = vec![0u8; dw * dh * 4];
    for y in 0..dh {
        let row = &src[(y * h / dh) * stride..];
        let dst = &mut out[y * dw * 4..(y + 1) * dw * 4];
        if dw == w {
            dst.copy_from_slice(&row[..dw * 4]);
            continue;
        }
        for x in 0..dw {
            let s = (x * w / dw) * 4;
            dst[x * 4..x * 4 + 4].copy_from_slice(&row[s..s + 4]);
        }
    }
    RawFrame { width: dw, height: dh, bgra: out }
}

/// Inicia captura + encoder. Cada quadro codificado vai para `tx`; soltar o
/// `Capture` devolvido para a captura, e o encoder para quando `tx` fecha.
/// `keyframe` = true força um quadro completo (novo espectador ou perda de pacote).
pub fn start(
    cfg: &Config,
    tx: UnboundedSender<EncodedFrame>,
    audio: Option<crate::audio::Packer>,
    keyframe: Arc<AtomicBool>,
    status: SharedStatus,
    repaint: Arc<dyn Fn() + Send + Sync>,
) -> anyhow::Result<crate::capture::Capture> {
    let latest = Arc::new(Latest::default());
    let capture = crate::capture::start(&cfg.source, cfg, latest.clone(), audio)?;

    let fps = cfg.fps;
    let bitrate = cfg.mbps * 1_000_000;
    std::thread::spawn(move || {
        if let Err(e) = encode_loop(&latest, tx, keyframe, fps, bitrate, status, repaint) {
            eprintln!("Erro no encoder: {e}");
        }
    });
    Ok(capture)
}

fn encode_loop(
    latest: &Latest,
    tx: UnboundedSender<EncodedFrame>,
    keyframe: Arc<AtomicBool>,
    fps: u32,
    bitrate: u32,
    status: SharedStatus,
    repaint: Arc<dyn Fn() + Send + Sync>,
) -> anyhow::Result<()> {
    let config = EncoderConfig::new()
        .bitrate(BitRate::from_bps(bitrate))
        .max_frame_rate(FrameRate::from_hz(fps as f32))
        .usage_type(UsageType::ScreenContentRealTime)
        .rate_control_mode(RateControlMode::Bitrate)
        .skip_frames(true)
        // Quadro completo a cada 2 s: quem perdeu pacotes se recupera sozinho
        .intra_frame_period(IntraFramePeriod::from_num_frames(fps * 2))
        // Fatias do tamanho de um pacote RTP; com várias fatias o OpenH264 usa várias threads
        .max_slice_len(1200)
        .num_threads(4);
    let mut encoder = Encoder::with_api_config(OpenH264API::from_source(), config)?;

    let interval = Duration::from_secs_f64(1.0 / fps as f64);
    let mut yuv: Option<YUVBuffer> = None;
    let mut last_sent = Instant::now();
    let mut stats = (Instant::now(), 0u32, 0usize, Duration::ZERO);

    loop {
        // Espera um quadro novo; com a tela parada, reenvia o último a cada 1 s
        // para quem entrar depois receber imagem.
        let new_frame = {
            let guard = latest.frame.lock().unwrap();
            let (mut guard, _) = latest
                .ready
                .wait_timeout_while(guard, Duration::from_secs(1), |f| f.is_none())
                .unwrap();
            guard.take()
        };

        if let Some(frame) = new_frame {
            if yuv.as_ref().map(|b| b.dimensions()) != Some(frame.dimensions()) {
                let (w, h) = frame.dimensions();
                eprintln!("Resolução enviada: {w}x{h}");
                keyframe.store(true, Ordering::Relaxed);
            }
            yuv = Some(frame);
        } else if tx.is_closed() {
            return Ok(());
        } else if yuv.is_none() || last_sent.elapsed() < Duration::from_secs(1) {
            continue;
        }

        let Some(buf) = &yuv else { continue };
        if keyframe.swap(false, Ordering::Relaxed) {
            encoder.force_intra_frame();
        }

        let started = Instant::now();
        let data = encoder.encode(buf)?.to_vec();
        let duration = last_sent.elapsed().min(Duration::from_secs(1));
        last_sent = Instant::now();
        stats.3 += started.elapsed();

        if !data.is_empty() {
            stats.1 += 1;
            stats.2 += data.len();
            if tx.send(EncodedFrame { data, duration }).is_err() {
                return Ok(());
            }
        }

        if stats.0.elapsed() >= Duration::from_secs(2) {
            let secs = stats.0.elapsed().as_secs_f64();
            let mut s = status.lock().unwrap();
            s.fps = stats.1 as f64 / secs;
            s.mbps = stats.2 as f64 * 8.0 / secs / 1e6;
            s.encode_ms = stats.3.as_secs_f64() * 1000.0 / stats.1.max(1) as f64;
            drop(s);
            repaint();
            stats = (Instant::now(), 0, 0, Duration::ZERO);
        }

        // Respeita o fps alvo mesmo se a captura mandar mais rápido
        if let Some(rest) = interval.checked_sub(started.elapsed()) {
            std::thread::sleep(rest);
        }
    }
}
