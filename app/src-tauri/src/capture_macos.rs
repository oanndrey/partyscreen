// Captura no macOS (ScreenCaptureKit): tela ou janela, com áudio opcional.
// O próprio sistema já entrega o quadro reduzido para a largura alvo, então a
// cópia aqui é só para tirar o padding.
use std::sync::{Arc, Mutex};

use screencapturekit::cv::CVPixelBufferLockFlags;
use screencapturekit::prelude::*;

use crate::audio::{self, Packer};
use crate::encoder::{Latest, scale_bgra};
use crate::session::{AudioApp, Config, Source, SourceInfo};

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

/// Filtro do ScreenCaptureKit para a tela/janela escolhida, com o tamanho em pontos.
/// `muted` = bundle IDs cujo som não pode ir para a transmissão. Na tela inteira esses
/// apps saem do filtro (o áudio é filtrado por app), mas as janelas deles entram como
/// exceção, então continuam aparecendo no vídeo. Janelas que o app abrir depois do
/// início não estão na lista de exceções e ficam de fora do vídeo até reiniciar.
fn filter_for(
    content: &SCShareableContent,
    source: &Source,
    muted: &[String],
) -> anyhow::Result<(SCContentFilter, (f64, f64))> {
    Ok(match source {
        Source::Display(i) => {
            let displays = content.displays();
            let d = displays.get(*i).ok_or_else(|| anyhow::anyhow!("tela não encontrada"))?;
            let is_muted = |a: &SCRunningApplication| muted.iter().any(|m| *m == a.bundle_identifier());
            let apps: Vec<SCRunningApplication> = content.applications().into_iter().filter(|a| is_muted(a)).collect();
            let builder = SCContentFilter::create().with_display(d);
            let filter = if apps.is_empty() {
                builder.with_excluding_windows(&[]).build()?
            } else {
                let windows: Vec<SCWindow> = content
                    .windows()
                    .into_iter()
                    .filter(|w| w.owning_application().is_some_and(|a| is_muted(&a)))
                    .collect();
                let app_refs: Vec<&SCRunningApplication> = apps.iter().collect();
                let win_refs: Vec<&SCWindow> = windows.iter().collect();
                builder.with_excluding_applications(&app_refs, &win_refs).build()?
            };
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
    })
}

/// Na captura de janela o ScreenCaptureKit só entrega o som do app dono da janela;
/// se esse app estiver silenciado, a transmissão vai sem áudio.
fn window_app_muted(content: &SCShareableContent, source: &Source, muted: &[String]) -> bool {
    let Source::Window(id) = source else { return false };
    content
        .windows()
        .iter()
        .find(|w| w.window_id() as u64 == *id)
        .and_then(|w| w.owning_application())
        .is_some_and(|a| muted.iter().any(|m| *m == a.bundle_identifier()))
}

/// Apps em execução com janela, para escolher quais não mandam som
pub fn audio_apps() -> anyhow::Result<Vec<AudioApp>> {
    let content = content()?;
    let me = std::process::id() as i32;
    // pids que têm alguma janela normal (camada 0), mesmo minimizada
    let with_window: std::collections::HashSet<i32> = content
        .windows()
        .iter()
        .filter(|w| w.window_layer() == 0)
        .filter_map(|w| w.owning_application().map(|a| a.process_id()))
        .collect();
    let mut list: Vec<AudioApp> = Vec::new();
    for a in content.applications() {
        let bundle_id = a.bundle_identifier();
        if bundle_id.is_empty() || a.process_id() == me || list.iter().any(|x| x.bundle_id == bundle_id) {
            continue;
        }
        let name = a.application_name();
        let suggested = crate::session::is_call_app(&bundle_id, &name);
        // Apps de chamada entram mesmo sem janela (ex.: Discord só na barra de menus)
        if !suggested && !with_window.contains(&a.process_id()) {
            continue;
        }
        let name = if name.trim().is_empty() { bundle_id.clone() } else { name };
        list.push(AudioApp { bundle_id, name, suggested });
    }
    list.sort_by(|a, b| b.suggested.cmp(&a.suggested).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    Ok(list)
}

/// Foto pequena da tela/janela para a lista de escolha
pub fn thumbnail(source: &Source) -> anyhow::Result<String> {
    let content = content()?;
    let (filter, (pw, ph)) = filter_for(&content, source, &[])?;
    let width = crate::thumbnail::WIDTH;
    let height = ((ph * width as f64 / pw.max(1.0)) as usize).max(2) & !1;
    let config = SCStreamConfiguration::new()
        .with_width(width as u32)
        .with_height(height as u32)
        .with_pixel_format(PixelFormat::BGRA)
        .with_shows_cursor(false);
    let image = screencapturekit::screenshot_manager::SCScreenshotManager::capture_image(&filter, &config)?;
    use screencapturekit::screenshot_manager::CGImageExt;
    let (w, h) = (image.width(), image.height());
    let frame = scale_bgra(&image.bgra_data()?, w, h, w * 4, crate::thumbnail::WIDTH);
    crate::thumbnail::to_data_url(&frame)
}

pub fn start(source: &Source, cfg: &Config, latest: Arc<Latest>, audio: Option<Packer>) -> anyhow::Result<Capture> {
    let content = content()?;

    // Tamanhos em pontos; em telas Retina o nativo é o dobro. Envia no máximo max_width.
    let (filter, (pw, ph)) = filter_for(&content, source, &cfg.muted_apps)?;
    let audio = if window_app_muted(&content, source, &cfg.muted_apps) { None } else { audio };
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
