// PartyScreen (desktop): interface em Leptos/Rust UI numa janela Tauri.
// Este lado cuida de compartilhar: captura, encoder H.264/Opus e WebRTC.
// Assistir roda direto no WebRTC da janela (webview), como no navegador.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(any(windows, target_os = "macos"))]
mod audio;
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
/// primeiro). Só no Mac; no Windows a lista vem vazia.
#[tauri::command]
async fn list_audio_apps() -> Result<Vec<session::AudioApp>, String> {
    #[cfg(target_os = "macos")]
    return tauri::async_runtime::spawn_blocking(|| capture::audio_apps().map_err(|e| format!("{e:#}")))
        .await
        .map_err(|e| e.to_string())?;
    #[cfg(not(target_os = "macos"))]
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

fn main() {
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
