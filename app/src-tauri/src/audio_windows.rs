// Som do Windows sem alguns apps (Discord, TeamSpeak...).
//
// O loopback normal pega a mistura inteira da placa. Para tirar apps, usa a captura de
// áudio por processo do Windows (10 2004+): um stream "inclui este processo e filhos"
// para cada app que está tocando e não foi silenciado, e mistura todos aqui.
// A cada 2 s olha de novo quem tem sessão de áudio (app que abriu depois entra sozinho).
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::StructuredStorage::{PROPVARIANT, PROPVARIANT_0, PROPVARIANT_0_0, PROPVARIANT_0_0_0};
use windows::Win32::System::Com::{BLOB, CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
use windows::Win32::System::Variant::VT_BLOB;
use windows::core::{Interface, Ref, implement};

use crate::audio::{Packer, RATE};

/// Argumentos que fazem o partyscreen.exe rodar como auxiliar de som (sem janela)
pub const HELPER_ARG: &str = "--som-por-app";
pub const LIST_ARG: &str = "--listar-apps-de-som";
use crate::session::AudioApp;

/// Amostras (intercaladas, estéreo) que o mixer espera juntar antes de usar um app: 40 ms
const PRIME: usize = RATE as usize / 25 * 2;
/// Acima disso o app está adiantado e o excesso é descartado: 120 ms
const MAX_QUEUE: usize = RATE as usize * 12 / 100 * 2;

struct Proc {
    parent: u32,
    exe: String,
}

/// Processos em execução: pid → (pai, executável em minúsculas)
fn processes() -> HashMap<u32, Proc> {
    let mut map = HashMap::new();
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return map;
        };
        let mut e = PROCESSENTRY32W {
            dwSize: size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut ok = Process32FirstW(snap, &mut e).is_ok();
        while ok {
            let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
            let exe = String::from_utf16_lossy(&e.szExeFile[..len]).to_lowercase();
            map.insert(
                e.th32ProcessID,
                Proc {
                    parent: e.th32ParentProcessID,
                    exe,
                },
            );
            ok = Process32NextW(snap, &mut e).is_ok();
        }
        let _ = CloseHandle(snap);
    }
    map
}

/// `pid` e seus ancestrais (o próprio primeiro); para em ciclos e no pid 0
fn ancestry(pid: u32, procs: &HashMap<u32, Proc>) -> Vec<u32> {
    let mut chain = vec![pid];
    let mut cur = pid;
    while let Some(p) = procs.get(&cur) {
        if p.parent == 0 || chain.contains(&p.parent) || chain.len() > 32 {
            break;
        }
        chain.push(p.parent);
        cur = p.parent;
    }
    chain
}

fn com_init() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
}

/// Sessões de áudio em todas as saídas ativas: (pid, nome que o Windows mostra no mixer)
fn sessions() -> windows::core::Result<Vec<(u32, String)>> {
    com_init();
    let mut out = Vec::new();
    unsafe {
        let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let devices = enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)?;
        for d in 0..devices.GetCount()? {
            let Ok(device) = devices.Item(d) else { continue };
            let Ok(manager) = device.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) else {
                continue;
            };
            let Ok(list) = manager.GetSessionEnumerator() else {
                continue;
            };
            for i in 0..list.GetCount().unwrap_or(0) {
                let Ok(session) = list.GetSession(i) else { continue };
                let Ok(session) = session.cast::<IAudioSessionControl2>() else {
                    continue;
                };
                if session.IsSystemSoundsSession().0 == 0 || session.GetState() == Ok(AudioSessionStateExpired) {
                    continue;
                }
                let Ok(pid) = session.GetProcessId() else { continue };
                if pid == 0 {
                    continue;
                }
                let name = session
                    .GetDisplayName()
                    .ok()
                    .and_then(|p| {
                        let s = p.to_string().ok();
                        windows::Win32::System::Com::CoTaskMemFree(Some(p.0 as _));
                        s
                    })
                    .unwrap_or_default();
                out.push((pid, name));
            }
        }
    }
    Ok(out)
}

/// Nome amigável a partir do executável (ex.: "ts3client_win64.exe" → "TeamSpeak 3")
fn friendly(exe: &str) -> String {
    let stem = exe.strip_suffix(".exe").unwrap_or(exe);
    let known = [
        ("ts3client", "TeamSpeak 3"),
        ("teamspeak", "TeamSpeak"),
        ("discord", "Discord"),
        ("chrome", "Google Chrome"),
        ("msedge", "Microsoft Edge"),
        ("firefox", "Firefox"),
        ("spotify", "Spotify"),
        ("ms-teams", "Microsoft Teams"),
        ("whatsapp", "WhatsApp"),
        ("telegram", "Telegram"),
        ("zoom", "Zoom"),
        ("steam", "Steam"),
    ];
    if let Some((_, name)) = known.iter().find(|(k, _)| stem.starts_with(k)) {
        return name.to_string();
    }
    let mut c = stem.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

/// Apps para a lista "Sem som de": quem tem sessão de áudio + apps de chamada abertos.
/// O id é o executável ("discord.exe"). Roda no processo auxiliar.
fn audio_apps_local() -> anyhow::Result<Vec<AudioApp>> {
    let procs = processes();
    let me = std::process::id();
    let mut list: Vec<AudioApp> = Vec::new();
    let add = |exe: &str, display: &str, list: &mut Vec<AudioApp>| {
        if exe.is_empty() || list.iter().any(|a| a.bundle_id == exe) {
            return;
        }
        // Nome do mixer do Windows quando é texto de verdade (não "@%SystemRoot%\...")
        let name = if display.trim().is_empty() || display.starts_with('@') {
            friendly(exe)
        } else {
            display.to_string()
        };
        let suggested = crate::session::is_call_app(exe, &name);
        list.push(AudioApp {
            bundle_id: exe.to_string(),
            name,
            suggested,
        });
    };
    for (pid, display) in sessions()? {
        if ancestry(pid, &procs).contains(&me) {
            continue;
        }
        if let Some(p) = procs.get(&pid) {
            add(&p.exe, &display, &mut list);
        }
    }
    // Discord/TeamSpeak sem sessão ainda (antes de entrar na chamada)
    for p in procs.values() {
        if crate::session::is_call_app(&p.exe, "") {
            add(&p.exe, "", &mut list);
        }
    }
    list.sort_by(|a, b| {
        b.suggested
            .cmp(&a.suggested)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(list)
}

/// O processo da janela está silenciado (ele ou quem o abriu)
pub fn pid_muted(pid: u32, muted: &[String]) -> bool {
    let procs = processes();
    let muted: HashSet<String> = muted.iter().map(|m| m.to_lowercase()).collect();
    ancestry(pid, &procs)
        .iter()
        .any(|p| procs.get(p).is_some_and(|p| muted.contains(&p.exe)))
}

#[implement(IActivateAudioInterfaceCompletionHandler)]
struct Activated(Sender<windows::core::Result<windows::core::IUnknown>>);

impl IActivateAudioInterfaceCompletionHandler_Impl for Activated_Impl {
    fn ActivateCompleted(&self, op: Ref<IActivateAudioInterfaceAsyncOperation>) -> windows::core::Result<()> {
        // Como no cpal e no exemplo da Microsoft: o resultado é lido aqui dentro
        let result = op.ok().and_then(|op| unsafe {
            let mut hr = windows::core::HRESULT::default();
            let mut unknown: Option<windows::core::IUnknown> = None;
            op.GetActivateResult(&mut hr, &mut unknown)?;
            hr.ok()?;
            unknown.ok_or_else(|| windows::core::Error::new(AUDCLNT_E_DEVICE_INVALIDATED, "sem cliente de áudio"))
        });
        let _ = self.0.send(result);
        Ok(())
    }
}

/// Cliente de áudio que captura só `pid` e seus filhos
fn activate(pid: u32) -> anyhow::Result<IAudioClient> {
    let params = AUDIOCLIENT_ACTIVATION_PARAMS {
        ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
        Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
            ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                TargetProcessId: pid,
                ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
            },
        },
    };
    let prop = PROPVARIANT {
        Anonymous: PROPVARIANT_0 {
            Anonymous: std::mem::ManuallyDrop::new(PROPVARIANT_0_0 {
                vt: VT_BLOB,
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
                Anonymous: PROPVARIANT_0_0_0 {
                    blob: BLOB {
                        cbSize: size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32,
                        pBlobData: &params as *const _ as *mut u8,
                    },
                },
            }),
        },
    };
    let (tx, rx) = channel();
    let handler: IActivateAudioInterfaceCompletionHandler = Activated(tx).into();
    unsafe {
        let _op = ActivateAudioInterfaceAsync(
            VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
            &IAudioClient::IID,
            Some(&prop),
            &handler,
        )?;
        let unknown = rx
            .recv_timeout(Duration::from_secs(5))
            .map_err(|_| anyhow::anyhow!("o Windows não respondeu"))??;
        Ok(unknown.cast()?)
    }
}

/// Captura um processo até `stop`, jogando estéreo 48 kHz em `queue`
fn capture_process(
    pid: u32,
    stop: Arc<AtomicBool>,
    queue: Arc<Mutex<VecDeque<f32>>>,
) -> anyhow::Result<()> {
    com_init();
    let client = activate(pid).map_err(|e| e.context("não deu para ativar a captura"))?;
    let format = WAVEFORMATEX {
        wFormatTag: WAVE_FORMAT_PCM as u16,
        nChannels: 2,
        nSamplesPerSec: RATE,
        nAvgBytesPerSec: RATE * 4,
        nBlockAlign: 4,
        wBitsPerSample: 16,
        cbSize: 0,
    };
    unsafe {
        client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_EVENTCALLBACK | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
            2_000_000,
            0,
            &format,
            None,
        )?;
        let event: HANDLE = CreateEventW(None, false, false, None)?;
        let result = (|| -> anyhow::Result<()> {
            client.SetEventHandle(event)?;
            let capture: IAudioCaptureClient = client.GetService()?;
            client.Start()?;
            while !stop.load(Ordering::Relaxed) {
                WaitForSingleObject(event, 100);
                while capture.GetNextPacketSize()? > 0 {
                    let (mut data, mut frames, mut flags) = (std::ptr::null_mut(), 0u32, 0u32);
                    capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
                    let n = frames as usize * 2;
                    let mut q = queue.lock().unwrap();
                    if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null() {
                        q.extend(std::iter::repeat_n(0.0, n));
                    } else {
                        let samples = std::slice::from_raw_parts(data as *const i16, n);
                        q.extend(samples.iter().map(|&s| s as f32 / 32768.0));
                    }
                    drop(q);
                    capture.ReleaseBuffer(frames)?;
                }
            }
            let _ = client.Stop();
            Ok(())
        })();
        let _ = CloseHandle(event);
        result
    }
}

struct Source {
    queue: Arc<Mutex<VecDeque<f32>>>,
    stop: Arc<AtomicBool>,
    primed: bool,
}

impl Drop for Source {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Processos para capturar: quem tem sessão de áudio, menos os silenciados, o próprio
/// PartyScreen e quem já entra pela árvore de outro. Um processo que abriu um app
/// silenciado (ex.: o explorer, pai de tudo) também fica de fora, senão o som vazaria.
fn wanted(muted: &HashSet<String>) -> HashSet<u32> {
    let procs = processes();
    let me = std::process::id();
    let is_muted = |pid: &u32| procs.get(pid).is_some_and(|p| muted.contains(&p.exe));
    // Ancestrais de algum processo silenciado
    let mut parents_of_muted = HashSet::new();
    for pid in procs.keys().filter(|p| is_muted(p)) {
        parents_of_muted.extend(ancestry(*pid, &procs).into_iter().skip(1));
    }
    let candidates: HashSet<u32> = sessions()
        .unwrap_or_default()
        .into_iter()
        .map(|(pid, _)| pid)
        .filter(|pid| {
            let chain = ancestry(*pid, &procs);
            !chain.contains(&me) && !chain.iter().any(is_muted) && !parents_of_muted.contains(pid)
        })
        .collect();
    candidates
        .iter()
        .copied()
        .filter(|pid| !ancestry(*pid, &procs).iter().skip(1).any(|a| candidates.contains(a)))
        .collect()
}

pub struct FilteredGuard(Arc<AtomicBool>);

impl Drop for FilteredGuard {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

fn helper(args: &[&str]) -> std::io::Result<std::process::Child> {
    let mut cmd = Command::new(std::env::current_exe()?);
    cmd.args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(0x0800_0000); // sem janela
    cmd.spawn()
}

/// Lista de apps para "Sem som de", calculada no processo auxiliar
pub fn audio_apps() -> anyhow::Result<Vec<AudioApp>> {
    let out = helper(&[LIST_ARG])?.wait_with_output()?;
    anyhow::ensure!(out.status.success(), "auxiliar de som fechou ({})", out.status);
    Ok(serde_json::from_slice(&out.stdout)?)
}

/// Som do computador sem os apps de `muted`. A captura por app roda num processo
/// auxiliar: se ele falhar ou fechar, o app segue com o som inteiro em vez de fechar junto.
pub fn start(mut packer: Packer, muted: &[String]) -> FilteredGuard {
    let stop = Arc::new(AtomicBool::new(false));
    let s = stop.clone();
    let mut args = vec![HELPER_ARG.to_string()];
    args.extend(muted.iter().cloned());
    std::thread::spawn(move || {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        match helper(&args) {
            Ok(mut child) => {
                let mut out = child.stdout.take().expect("stdout do auxiliar");
                let (mut buf, mut pending) = (vec![0u8; 1 << 14], Vec::<u8>::new());
                while !s.load(Ordering::Relaxed) {
                    match out.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            pending.extend_from_slice(&buf[..n]);
                            let whole = pending.len() / 8 * 8;
                            let samples: Vec<f32> = pending[..whole]
                                .chunks_exact(4)
                                .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                                .collect();
                            pending.drain(..whole);
                            packer.push(&samples, 2, RATE);
                        }
                    }
                }
                let _ = child.kill();
                let status = child.wait();
                if s.load(Ordering::Relaxed) {
                    return;
                }
                eprintln!("Som por app parou ({status:?}); usando o som inteiro");
            }
            Err(e) => eprintln!("Não abriu o auxiliar de som ({e}); usando o som inteiro"),
        }
        match crate::audio::start_loopback(packer) {
            Ok(guard) => {
                while !s.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(100));
                }
                drop(guard);
            }
            Err(e) => eprintln!("Sem som: {e:#}"),
        }
    });
    FilteredGuard(stop)
}

/// Processo auxiliar: `--listar-apps-de-som` imprime a lista em JSON;
/// `--som-por-app <exe...>` manda o som misturado (f32 estéreo 48 kHz) pela saída
/// até o app principal fechar a entrada.
pub fn helper_main(args: &[String]) -> ! {
    com_init();
    if args.first().map(String::as_str) == Some(LIST_ARG) {
        let list = audio_apps_local().unwrap_or_default();
        let _ = std::io::stdout().write_all(&serde_json::to_vec(&list).unwrap_or_default());
        std::process::exit(0);
    }
    // App principal fechou (ou parou a transmissão): a entrada acaba
    std::thread::spawn(|| {
        let mut sink = [0u8; 64];
        while matches!(std::io::stdin().read(&mut sink), Ok(n) if n > 0) {}
        std::process::exit(0);
    });
    let muted: HashSet<String> = args[1..].iter().map(|m| m.to_lowercase()).collect();
    eprintln!("Som por app: auxiliar iniciado (silenciados: {muted:?})");
    let mut stdout = std::io::stdout().lock();
    let mut bytes = Vec::new();
    let mut sources: HashMap<u32, Source> = HashMap::new();
    let mut failed: HashSet<u32> = HashSet::new();
    let mut last_scan = Instant::now() - Duration::from_secs(10);
    let started = Instant::now();
    let mut emitted: u64 = 0;
    loop {
        if last_scan.elapsed() >= Duration::from_secs(2) {
            last_scan = Instant::now();
            let want = wanted(&muted);
            sources.retain(|pid, _| want.contains(pid));
            failed.retain(|pid| want.contains(pid));
            for pid in want {
                if sources.contains_key(&pid) || failed.contains(&pid) {
                    continue;
                }
                eprintln!("Som: capturando o processo {pid}");
                let queue = Arc::new(Mutex::new(VecDeque::new()));
                let stop = Arc::new(AtomicBool::new(false));
                let (q, st) = (queue.clone(), stop.clone());
                std::thread::spawn(move || {
                    if let Err(e) = capture_process(pid, st.clone(), q) {
                        eprintln!("Som do processo {pid}: {e:#}");
                    }
                    st.store(true, Ordering::Relaxed);
                });
                sources.insert(
                    pid,
                    Source {
                        queue,
                        stop,
                        primed: false,
                    },
                );
            }
            // Captura que falhou não é tentada de novo até o processo sumir e voltar
            sources.retain(|pid, src| {
                let dead = src.stop.load(Ordering::Relaxed);
                if dead {
                    failed.insert(*pid);
                }
                !dead
            });
        }

        // Mistura no ritmo do relógio: quantos quadros de 48 kHz já deviam ter saído
        let due = (started.elapsed().as_secs_f64() * RATE as f64) as u64 - emitted;
        if due > 0 {
            let n = due as usize * 2;
            let mut mix = vec![0.0f32; n];
            for src in sources.values_mut() {
                let mut q = src.queue.lock().unwrap();
                if q.len() > MAX_QUEUE {
                    let extra = q.len() - PRIME;
                    q.drain(..extra);
                }
                if !src.primed {
                    if q.len() < PRIME {
                        continue;
                    }
                    src.primed = true;
                }
                let take = q.len().min(n);
                for (m, v) in mix.iter_mut().zip(q.drain(..take)) {
                    *m += v;
                }
                if take < n {
                    src.primed = false;
                }
            }
            for m in &mut mix {
                *m = m.clamp(-1.0, 1.0);
            }
            bytes.clear();
            bytes.extend(mix.iter().flat_map(|v| v.to_le_bytes()));
            if stdout.write_all(&bytes).and_then(|_| stdout.flush()).is_err() {
                std::process::exit(0);
            }
            emitted += due;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
