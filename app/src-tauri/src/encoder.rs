// Codificação H.264 (OpenH264), comum a Windows e macOS.
//
// Captura e codificação rodam em threads separadas: a captura só guarda o quadro
// mais recente e o encoder pega sempre o último. Se o encoder atrasar, quadros
// velhos são descartados em vez de acumular fila (mantém a latência baixa).
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
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
/// (interpolação bilinear, para texto e bordas não serrilharem) e garantindo dimensões
/// pares, exigidas pelo YUV 4:2:0.
pub fn scale_bgra(src: &[u8], w: usize, h: usize, stride: usize, max_width: usize) -> RawFrame {
    let (dw, dh) = if w > max_width {
        (max_width, h * max_width / w)
    } else {
        (w, h)
    };
    let (dw, dh) = (dw & !1, dh & !1);

    let mut out = vec![0u8; dw * dh * 4];
    if dw == w {
        for y in 0..dh {
            out[y * dw * 4..(y + 1) * dw * 4].copy_from_slice(&src[y * stride..y * stride + dw * 4]);
        }
        return RawFrame { width: dw, height: dh, bgra: out };
    }

    // Posições de origem em ponto fixo (8 bits de fração)
    let fx = |x: usize| ((x * 2 + 1) * w * 128 / dw).saturating_sub(128);
    let cols: Vec<(usize, u32)> = (0..dw)
        .map(|x| {
            let p = fx(x);
            ((p >> 8).min(w - 2), (p & 255) as u32)
        })
        .collect();
    for y in 0..dh {
        let p = ((y * 2 + 1) * h * 128 / dh).saturating_sub(128);
        let (sy, ty) = ((p >> 8).min(h - 2), (p & 255) as u32);
        let (r0, r1) = (&src[sy * stride..], &src[(sy + 1) * stride..]);
        let dst = &mut out[y * dw * 4..(y + 1) * dw * 4];
        for (x, &(sx, tx)) in cols.iter().enumerate() {
            let i = sx * 4;
            for c in 0..4 {
                let top = r0[i + c] as u32 * (256 - tx) + r0[i + 4 + c] as u32 * tx;
                let bot = r1[i + c] as u32 * (256 - tx) + r1[i + 4 + c] as u32 * tx;
                dst[x * 4 + c] = ((top * (256 - ty) + bot * ty) >> 16) as u8;
            }
        }
    }
    RawFrame { width: dw, height: dh, bgra: out }
}

#[cfg(test)]
mod tests {
    use super::scale_bgra;

    #[test]
    fn reduz_1080p_para_720p_sem_estourar() {
        let (w, h, stride) = (1920, 1080, 1920 * 4 + 64);
        let src: Vec<u8> = (0..stride * h).map(|i| (i % 251) as u8).collect();
        let f = scale_bgra(&src, w, h, stride, 1280);
        assert_eq!((f.width, f.height), (1280, 720));
        assert_eq!(f.bgra.len(), 1280 * 720 * 4);
        // Cor sólida continua sólida depois da interpolação
        let solid = vec![200u8; stride * h];
        assert!(scale_bgra(&solid, w, h, stride, 1280).bgra.iter().all(|&v| v == 199 || v == 200));
    }
}

/// Inicia captura + encoder. Cada quadro codificado vai para `tx`; soltar o
/// `Capture` devolvido para a captura, e o encoder para quando `tx` fecha.
/// `keyframe` = true força um quadro completo (novo espectador ou perda de pacote).
/// `loss` = pior perda de pacotes informada pelos espectadores (por mil), zerada a cada leitura.
pub fn start(
    cfg: &Config,
    tx: UnboundedSender<EncodedFrame>,
    audio: Option<crate::audio::Packer>,
    keyframe: Arc<AtomicBool>,
    loss: Arc<AtomicU32>,
    status: SharedStatus,
    repaint: Arc<dyn Fn() + Send + Sync>,
) -> anyhow::Result<crate::capture::Capture> {
    let latest = Arc::new(Latest::default());
    let capture = crate::capture::start(&cfg.source, cfg, latest.clone(), audio)?;

    let fps = cfg.fps;
    let sharp = cfg.sharp;
    let bitrate = cfg.mbps * 1_000_000;
    std::thread::spawn(move || {
        if let Err(e) = encode_loop(&latest, tx, keyframe, loss, fps, bitrate, sharp, status, repaint) {
            eprintln!("Erro no encoder: {e}");
        }
    });
    Ok(capture)
}

fn encode_loop(
    latest: &Latest,
    tx: UnboundedSender<EncodedFrame>,
    keyframe: Arc<AtomicBool>,
    loss: Arc<AtomicU32>,
    fps: u32,
    max_bitrate: u32,
    sharp: bool,
    status: SharedStatus,
    repaint: Arc<dyn Fn() + Send + Sync>,
) -> anyhow::Result<()> {
    // Prioridade escolhida na interface:
    // - fluidez (jogo/vídeo): modo de vídeo e nunca pula quadro; em movimento perde nitidez
    // - nitidez (texto/trabalho): modo de conteúdo de tela; pode pular quadro para manter a imagem nítida
    let usage = if sharp { UsageType::ScreenContentRealTime } else { UsageType::CameraVideoRealTime };
    let new_encoder = |bitrate: u32| {
        let config = EncoderConfig::new()
        .bitrate(BitRate::from_bps(bitrate))
        .max_frame_rate(FrameRate::from_hz(fps as f32))
        .usage_type(usage)
        .rate_control_mode(RateControlMode::Bitrate)
        .skip_frames(sharp)
        // Quadro completo a cada 6 s (é grande e pode dar tranco); perdas no meio
        // do caminho são recuperadas pelo pedido do navegador (PLI)
        .intra_frame_period(IntraFramePeriod::from_num_frames(fps * 6))
        // Fatias do tamanho de um pacote RTP; com várias fatias o OpenH264 usa várias threads
        .max_slice_len(1200)
        .num_threads(4);
        Encoder::with_api_config(OpenH264API::from_source(), config)
    };
    // Placa de vídeo (via FFmpeg) se houver; senão, OpenH264 no processador
    let hw = crate::hw_encoder::detect();
    {
        let label = hw.as_ref().map(|(_, k)| k.label).unwrap_or("Processador");
        status.lock().unwrap().encoder = label.to_string();
        eprintln!("Codificador: {label}");
    }
    let mut bitrate = max_bitrate;
    let mut encoder = new_encoder(bitrate)?;
    // Na placa de vídeo o encoder depende do tamanho do quadro: criado no primeiro quadro
    let mut gpu: Option<crate::hw_encoder::HwEncoder> = None;
    // Adaptação à rede: a cada 2 s olha a perda informada pelos espectadores
    let mut adapt = (Instant::now(), Instant::now());
    let mut last_idr = Instant::now() - Duration::from_secs(10);

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

        if adapt.0.elapsed() >= Duration::from_secs(2) {
            adapt.0 = Instant::now();
            let lost = loss.swap(0, Ordering::Relaxed) as f32 / 1000.0;
            // Perda alta: baixa 25% (mínimo 1,5 Mbps). Rede limpa: sobe 10% até o máximo.
            let target = if lost > 0.05 {
                (bitrate as f32 * 0.75) as u32
            } else if lost < 0.01 {
                (bitrate as f32 * 1.10) as u32
            } else {
                bitrate
            }
            .clamp(1_500_000.min(max_bitrate), max_bitrate);
            // Trocar a banda recria o encoder (gera imagem completa): no máximo a cada 4 s,
            // e só se a mudança for relevante
            let relevant = (target as f32 - bitrate as f32).abs() / bitrate as f32 > 0.08;
            if relevant && adapt.1.elapsed() >= Duration::from_secs(4) {
                adapt.1 = Instant::now();
                bitrate = target;
                encoder = new_encoder(bitrate)?;
                gpu = None;
                last_idr = Instant::now();
                eprintln!("Perda {:.1}%: banda ajustada para {:.1} Mbps", lost * 100.0, bitrate as f32 / 1e6);
            }
        }

        // Pedidos de imagem completa (espectador novo ou perda): no máximo 1 a cada 2 s.
        // Atender todos na hora gera um ciclo — imagem completa é grande, aumenta a perda,
        // que gera mais pedidos.
        // (Na placa de vídeo a imagem completa vem sozinha a cada 2 s.)
        if hw.is_none() && last_idr.elapsed() >= Duration::from_secs(2) && keyframe.swap(false, Ordering::Relaxed) {
            encoder.force_intra_frame();
            last_idr = Instant::now();
        }

        let started = Instant::now();
        let duration = last_sent.elapsed().min(Duration::from_secs(1));
        let frames = match &hw {
            Some((ffmpeg, kind)) => {
                if gpu.as_ref().map(|g| g.dimensions) != Some(buf.dimensions()) {
                    gpu = Some(crate::hw_encoder::HwEncoder::new(ffmpeg, kind, buf.dimensions(), fps, bitrate)?);
                }
                keyframe.store(false, Ordering::Relaxed);
                gpu.as_mut().unwrap().encode(buf)?
            }
            None => vec![encoder.encode(buf)?.to_vec()],
        };
        last_sent = Instant::now();
        stats.3 += started.elapsed();

        for data in frames.into_iter().filter(|d| !d.is_empty()) {
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
