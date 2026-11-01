// Captura no Windows: tela ou janela (Windows Graphics Capture) e, opcionalmente,
// o som do computador (loopback do WASAPI).
use std::sync::Arc;
use std::time::Duration;

use windows_capture::capture::{CaptureControl, Context, GraphicsCaptureApiHandler};
use windows_capture::frame::Frame;
use windows_capture::graphics_capture_api::InternalCaptureControl;
use windows_capture::monitor::Monitor;
use windows_capture::settings::{
    ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
    MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
};
use windows_capture::window::Window;

use crate::audio::Packer;
use crate::encoder::{Latest, scale_bgra};
use crate::session::{Config, Source, SourceInfo};

type Error = Box<dyn std::error::Error + Send + Sync>;

struct Handler {
    latest: Arc<Latest>,
    max_width: usize,
}

impl GraphicsCaptureApiHandler for Handler {
    type Flags = (Arc<Latest>, usize);
    type Error = Error;

    fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
        let (latest, max_width) = ctx.flags;
        Ok(Self { latest, max_width })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame,
        _control: InternalCaptureControl,
    ) -> Result<(), Self::Error> {
        let mut buffer = frame.buffer()?;
        let (w, h, stride) = (buffer.width() as usize, buffer.height() as usize, buffer.row_pitch() as usize);
        if w < 2 || h < 2 {
            return Ok(());
        }
        let raw = scale_bgra(buffer.as_raw_buffer(), w, h, stride, self.max_width);
        self.latest.put(raw);
        Ok(())
    }
}

/// Mantém a captura viva; soltar para a captura
pub struct Capture {
    control: Option<CaptureControl<Handler, Error>>,
    _audio: Option<crate::audio::LoopbackGuard>,
}

impl Drop for Capture {
    fn drop(&mut self) {
        if let Some(c) = self.control.take() {
            let _ = c.stop();
        }
    }
}

pub fn sources() -> anyhow::Result<Vec<SourceInfo>> {
    let mut list = Vec::new();
    for (i, m) in Monitor::enumerate()?.iter().enumerate() {
        let size = match (m.width(), m.height()) {
            (Ok(w), Ok(h)) => format!(" ({w}x{h})"),
            _ => String::new(),
        };
        list.push(SourceInfo { source: Source::Display(i), label: format!("Tela {}{size}", i + 1) });
    }
    let me = std::process::id();
    for w in Window::enumerate()? {
        let Ok(title) = w.title() else { continue };
        if title.trim().is_empty() || w.process_id().ok() == Some(me) {
            continue;
        }
        let app = w.process_name().unwrap_or_default();
        list.push(SourceInfo { source: Source::Window(w.as_raw_hwnd() as u64), label: format!("{app} — {title}") });
    }
    Ok(list)
}

pub fn start(source: &Source, cfg: &Config, latest: Arc<Latest>, audio: Option<Packer>) -> anyhow::Result<Capture> {
    macro_rules! settings {
        ($item:expr) => {
            Settings::new(
                $item,
                CursorCaptureSettings::WithCursor,
                DrawBorderSettings::WithoutBorder,
                SecondaryWindowSettings::Default,
                MinimumUpdateIntervalSettings::Custom(Duration::from_secs_f64(1.0 / cfg.fps as f64)),
                DirtyRegionSettings::Default,
                ColorFormat::Bgra8,
                (latest, cfg.max_width),
            )
        };
    }

    let control = match source {
        Source::Display(i) => {
            let monitor = Monitor::from_index(i + 1).map_err(|_| anyhow::anyhow!("tela não encontrada"))?;
            Handler::start_free_threaded(settings!(monitor))
        }
        Source::Window(hwnd) => {
            let window = Window::from_raw_hwnd(*hwnd as *mut std::ffi::c_void);
            anyhow::ensure!(window.is_valid(), "a janela foi fechada");
            Handler::start_free_threaded(settings!(window))
        }
    }
    .map_err(|e| anyhow::anyhow!("falha ao iniciar a captura: {e}"))?;

    let audio = audio.map(crate::audio::start_loopback).transpose()?;
    Ok(Capture { control: Some(control), _audio: audio })
}

/// Captura de um quadro só, para a miniatura
struct ThumbHandler {
    slot: Arc<std::sync::Mutex<Option<crate::encoder::RawFrame>>>,
}

impl GraphicsCaptureApiHandler for ThumbHandler {
    type Flags = Arc<std::sync::Mutex<Option<crate::encoder::RawFrame>>>;
    type Error = Error;

    fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
        Ok(Self { slot: ctx.flags })
    }

    fn on_frame_arrived(&mut self, frame: &mut Frame, control: InternalCaptureControl) -> Result<(), Self::Error> {
        let mut buffer = frame.buffer()?;
        let (w, h, stride) = (buffer.width() as usize, buffer.height() as usize, buffer.row_pitch() as usize);
        if w >= 2 && h >= 2 {
            *self.slot.lock().unwrap() = Some(scale_bgra(buffer.as_raw_buffer(), w, h, stride, crate::thumbnail::WIDTH));
        }
        control.stop();
        Ok(())
    }
}

/// Foto pequena da tela/janela para a lista de escolha (janela minimizada não tem imagem)
pub fn thumbnail(source: &Source) -> anyhow::Result<String> {
    let slot = Arc::new(std::sync::Mutex::new(None));
    macro_rules! settings {
        ($item:expr) => {
            Settings::new(
                $item,
                CursorCaptureSettings::WithoutCursor,
                DrawBorderSettings::WithoutBorder,
                SecondaryWindowSettings::Default,
                MinimumUpdateIntervalSettings::Default,
                DirtyRegionSettings::Default,
                ColorFormat::Bgra8,
                slot.clone(),
            )
        };
    }
    let control = match source {
        Source::Display(i) => {
            let monitor = Monitor::from_index(i + 1).map_err(|_| anyhow::anyhow!("tela não encontrada"))?;
            ThumbHandler::start_free_threaded(settings!(monitor))
        }
        Source::Window(hwnd) => {
            let window = Window::from_raw_hwnd(*hwnd as *mut std::ffi::c_void);
            anyhow::ensure!(window.is_valid(), "a janela foi fechada");
            ThumbHandler::start_free_threaded(settings!(window))
        }
    }
    .map_err(|e| anyhow::anyhow!("falha na miniatura: {e}"))?;

    let deadline = std::time::Instant::now() + Duration::from_millis(1500);
    while slot.lock().unwrap().is_none() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(15));
    }
    let _ = control.stop();
    let frame = slot.lock().unwrap().take().ok_or_else(|| anyhow::anyhow!("sem imagem"))?;
    crate::thumbnail::to_data_url(&frame)
}
