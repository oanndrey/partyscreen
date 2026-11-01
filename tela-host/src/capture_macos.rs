// Captura no macOS (ScreenCaptureKit): tela ou janela, com áudio opcional.
// O próprio sistema já entrega o quadro reduzido para a largura alvo, então a
// cópia aqui é só para tirar o padding.
use std::sync::{Arc, Mutex};

use screencapturekit::cv::CVPixelBufferLockFlags;
use screencapturekit::prelude::*;

use crate::audio::{self, Packer};
use crate::encoder::{Latest, scale_bgra};
use crate::session::{Config, Source, SourceInfo};

struct VideoHandler {
    latest: Arc<Latest>,
}

impl SCStreamOutputTrait for VideoHandler {
    fn did_output_sample_buffer(&self, sample: CMSampleBuffer, kind: SCStreamOutputType) {
        if !matches!(kind, SCStreamOutputType::Screen) {
            return;
        }
        // Quadros sem mudança na tela chegam sem pixel buffer
        let Some(pixels) = sample.pixel_buffer() else { return };
        let Ok(guard) = pixels.lock(CVPixelBufferLockFlags::READ_ONLY) else { return };
        let Some(data) = (unsafe { guard.as_slice() }) else { return };
        let (w, h) = (guard.width(), guard.height());
        self.latest.put(scale_bgra(data, w, h, guard.bytes_per_row(), w));
    }
}

struct AudioHandler {
    packer: Mutex<Packer>,
}

impl SCStreamOutputTrait for AudioHandler {
    fn did_output_sample_buffer(&self, sample: CMSampleBuffer, kind: SCStreamOutputType) {
        if !matches!(kind, SCStreamOutputType::Audio) {
            return;
        }
        let Ok(list) = sample.audio_buffer_list() else { return };
        let as_f32 = |b: &[u8]| -> Vec<f32> {
            b.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
        };
        // Normalmente chega um buffer por canal (planar); intercala para o Opus
        let buffers: Vec<_> = (&list).into_iter().collect();
        let interleaved = match buffers.as_slice() {
            [l, r] if l.number_channels == 1 => {
                let (l, r) = (as_f32(l.data()), as_f32(r.data()));
                l.iter().zip(&r).flat_map(|(a, b)| [*a, *b]).collect::<Vec<_>>()
            }
            [single] => {
                let channels = single.number_channels.max(1) as usize;
                let data = as_f32(single.data());
                self.packer.lock().unwrap().push(&data, channels, audio::RATE);
                return;
            }
            _ => return,
        };
        self.packer.lock().unwrap().push(&interleaved, 2, audio::RATE);
    }
}

/// Mantém a captura viva; soltar para a captura
pub struct Capture(SCStream);

impl Drop for Capture {
    fn drop(&mut self) {
        let _ = self.0.stop_capture();
    }
}

fn content() -> anyhow::Result<SCShareableContent> {
    SCShareableContent::get().map_err(|e| {
        anyhow::anyhow!(
            "sem acesso à tela ({e}). Libere este app em Ajustes do Sistema → \
             Privacidade e Segurança → Gravação de Tela e abra de novo."
        )
    })
}

pub fn sources() -> anyhow::Result<Vec<SourceInfo>> {
    let content = content()?;
    let mut list = Vec::new();
    for (i, d) in content.displays().iter().enumerate() {
        list.push(SourceInfo { source: Source::Display(i), label: format!("Tela {} ({}x{})", i + 1, d.width(), d.height()) });
    }
    let me = std::process::id() as i32;
    for w in content.windows() {
        let Some(title) = w.title().filter(|t| !t.trim().is_empty()) else { continue };
        if !w.is_on_screen() || w.window_layer() != 0 {
            continue;
        }
        let app = w.owning_application();
        if app.as_ref().is_some_and(|a| a.process_id() == me) {
            continue;
        }
        let app = app.map(|a| a.application_name()).unwrap_or_default();
        list.push(SourceInfo { source: Source::Window(w.window_id() as u64), label: format!("{app} — {title}") });
    }
    Ok(list)
}

pub fn start(source: &Source, cfg: &Config, latest: Arc<Latest>, audio: Option<Packer>) -> anyhow::Result<Capture> {
    let content = content()?;

    // Tamanhos em pontos; em telas Retina o nativo é o dobro. Envia no máximo max_width.
    let (filter, (pw, ph)) = match source {
        Source::Display(i) => {
            let displays = content.displays();
            let d = displays.get(*i).ok_or_else(|| anyhow::anyhow!("tela não encontrada"))?;
            let filter = SCContentFilter::create().with_display(d).with_excluding_windows(&[]).build()?;
            (filter, (d.width() as f64, d.height() as f64))
        }
        Source::Window(id) => {
            let windows = content.windows();
            let w = windows
                .iter()
                .find(|w| w.window_id() as u64 == *id)
                .ok_or_else(|| anyhow::anyhow!("a janela foi fechada"))?;
            let size = w.frame().size;
            (SCContentFilter::create().with_window(w).build()?, (size.width, size.height))
        }
    };
    let (dw, dh) = ((pw * 2.0) as usize, (ph * 2.0) as usize);
    let width = dw.min(cfg.max_width).max(2) & !1;
    let height = (dh * width / dw.max(1)).max(2) & !1;

    let config = SCStreamConfiguration::new()
        .with_width(width as u32)
        .with_height(height as u32)
        .with_pixel_format(PixelFormat::BGRA)
        .with_shows_cursor(true)
        .with_queue_depth(3)
        .with_minimum_frame_interval(&CMTime::new(1, cfg.fps as i32))
        .with_captures_audio(audio.is_some())
        .with_sample_rate(audio::RATE as i32)
        .with_channel_count(2)
        // Não captura o som do próprio app (evita eco se também estiver assistindo)
        .with_excludes_current_process_audio(true);

    let mut stream = SCStream::new(&filter, &config)?;
    stream.add_output_handler(VideoHandler { latest }, SCStreamOutputType::Screen)?;
    if let Some(packer) = audio {
        stream.add_output_handler(AudioHandler { packer: Mutex::new(packer) }, SCStreamOutputType::Audio)?;
    }
    stream.start_capture()?;
    Ok(Capture(stream))
}
