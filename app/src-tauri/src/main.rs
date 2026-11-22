// PartyScreen (desktop): interface em Leptos/Rust UI numa janela Tauri.
// Este lado cuida de compartilhar: captura, encoder H.264/Opus e WebRTC.
// Assistir roda direto no WebRTC da janela (webview), como no navegador.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(any(windows, target_os = "macos"))]
mod audio;
#[cfg(windows)]
mod audio_windows;
#[cfg(windows)]
mod capture;
#[cfg(target_os = "macos")]
#[path = "capture_macos.rs"]
mod capture;
#[cfg(any(windows, target_os = "macos"))]
mod encoder;
#[cfg(any(windows, target_os = "macos"))]
mod hw_encoder;
#[cfg(any(windows, target_os = "macos"))]
mod thumbnail;
mod session;

use std::sync::{Arc, Mutex};

use tauri::{AppHandle, Emitter, State};

use session::{Config, Handle, SharedStatus, SourceInfo};

// A ponte Swift do ScreenCaptureKit referencia a biblioteca de compatibilidade do
// Swift 5.6, que só existe para macOS < 12 (e nem vem nas Command Line Tools).
// O app exige macOS 13, então o símbolo pode ficar vazio.
#[cfg(target_os = "macos")]
#[used]
#[unsafe(export_name = "_swift_FORCE_LOAD_$_swiftCompatibility56")]
static SWIFT_COMPATIBILITY_56: u8 = 0;

/// Quadro H.264 codificado pronto para enviar
pub struct EncodedFrame {
    pub data: Vec<u8>,
    pub duration: std::time::Duration,
}

struct Sharing {
    rt: tokio::runtime::Runtime,
    status: SharedStatus,
    handle: Mutex<Option<Handle>>,
}

#[tauri::command]
fn list_sources() -> Result<Vec<SourceInfo>, String> {
    #[cfg(any(windows, target_os = "macos"))]
    return capture::sources().map_err(|e| format!("{e:#}"));
    #[cfg(not(any(windows, target_os = "macos")))]
    Err("sistema não suportado".into())
}

/// Miniatura de uma tela/janela (JPEG em data URL). Roda fora da thread da interface.
#[tauri::command]
async fn thumbnail(source: session::Source) -> Result<String, String> {
    #[cfg(any(windows, target_os = "macos"))]
    return tauri::async_runtime::spawn_blocking(move || capture::thumbnail(&source).map_err(|e| format!("{e:#}")))
        .await
        .map_err(|e| e.to_string())?;
    #[cfg(not(any(windows, target_os = "macos")))]
    Err("sistema não suportado".into())
}

/// Apps em execução para escolher quais não mandam som (sugeridos = apps de chamada,
/// primeiro).
#[tauri::command]
async fn list_audio_apps() -> Result<Vec<session::AudioApp>, String> {
    #[cfg(target_os = "macos")]
    return tauri::async_runtime::spawn_blocking(|| capture::audio_apps().map_err(|e| format!("{e:#}")))
        .await
        .map_err(|e| e.to_string())?;
    #[cfg(windows)]
    return tauri::async_runtime::spawn_blocking(|| audio_windows::audio_apps().map_err(|e| format!("{e:#}")))
        .await
        .map_err(|e| e.to_string())?;
    #[cfg(not(any(windows, target_os = "macos")))]
    Ok(Vec::new())
}

#[tauri::command]
fn start_share(config: Config, app: AppHandle, state: State<'_, Sharing>) {
    if let Some(old) = state.handle.lock().unwrap().take() {
        old.stop();
    }
    // Cada mudança de estado (sala criada, espectador, estatísticas) vira um evento para a interface
    let status = state.status.clone();
    let emit = Arc::new(move || {
        let snapshot = status.lock().unwrap().clone();
        let _ = app.emit("share-status", snapshot);
    });
    let handle = session::start(&state.rt, config, state.status.clone(), emit);
    *state.handle.lock().unwrap() = Some(handle);
}

#[tauri::command]
fn send_chat(text: String, state: State<'_, Sharing>) {
    if let Some(h) = state.handle.lock().unwrap().as_ref() {
        h.chat(&text);
    }
}

#[tauri::command]
fn stop_share(state: State<'_, Sharing>) {
    if let Some(h) = state.handle.lock().unwrap().take() {
        h.stop();
    }
}

/// No Windows o app não tem terminal: tudo que iria para o stderr (avisos, pânicos, erros
/// do FFmpeg) vai para %LOCALAPPDATA%\\PartyScreen\\partyscreen.log, junto com o motivo
/// se o app fechar sozinho.
#[cfg(windows)]
fn setup_log() {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Console::{STD_ERROR_HANDLE, SetStdHandle};
    use windows::Win32::System::Diagnostics::Debug::{EXCEPTION_POINTERS, SetUnhandledExceptionFilter};

    unsafe extern "system" fn crash(info: *const EXCEPTION_POINTERS) -> i32 {
        unsafe {
            let rec = &*(*info).ExceptionRecord;
            let addr = rec.ExceptionAddress as usize;
            let base = windows::Win32::System::LibraryLoader::GetModuleHandleW(None).map(|h| h.0 as usize).unwrap_or(0);
            eprintln!(
                "FECHOU: exceção {:#010x} no endereço {addr:#x} (exe em {base:#x}, deslocamento {:#x})",
                rec.ExceptionCode.0 as u32,
                addr.wrapping_sub(base)
            );
        }
        0 // segue o fluxo normal do Windows (relatório de erro)
    }

    let dir = std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir).join("PartyScreen");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("partyscreen.log");
    // Log pequeno: recomeça quando passa de 1 MB
    let big = std::fs::metadata(&path).is_ok_and(|m| m.len() > 1 << 20);
    let Ok(file) = std::fs::OpenOptions::new().create(true).append(!big).write(true).truncate(big).open(&path) else { return };
    unsafe {
        let _ = SetStdHandle(STD_ERROR_HANDLE, HANDLE(file.as_raw_handle()));
        SetUnhandledExceptionFilter(Some(crash));
    }
    std::mem::forget(file);
    let since = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    eprintln!("\n=== PartyScreen {} aberto (unix {since}) ===", env!("CARGO_PKG_VERSION"));
}

fn main() {
    #[cfg(windows)]
    {
        let args: Vec<String> = std::env::args().skip(1).collect();
        if args.first().is_some_and(|a| a == audio_windows::HELPER_ARG || a == audio_windows::LIST_ARG) {
            audio_windows::helper_main(&args);
        }
        setup_log();
    }
    tauri::Builder::default()
        .manage(Sharing {
            rt: tokio::runtime::Runtime::new().expect("runtime tokio"),
            status: Arc::new(Mutex::new(Default::default())),
            handle: Mutex::new(None),
        })
        .invoke_handler(tauri::generate_handler![list_sources, thumbnail, list_audio_apps, start_share, stop_share, send_chat])
        .run(tauri::generate_context!())
        .expect("erro ao abrir o PartyScreen");
}
