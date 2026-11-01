// Codificação H.264 na placa de vídeo, usando o FFmpeg como ponte:
// NVIDIA (NVENC), AMD (AMF), Intel (Quick Sync), Media Foundation (qualquer placa no
// Windows). No Mac a codificação é no processador. O app manda quadros YUV pela entrada do FFmpeg e lê
// o H.264 pela saída, separando os quadros pelo marcador AUD que o FFmpeg insere.
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::OnceLock;
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use openh264::formats::{YUVBuffer, YUVSource};

#[derive(Clone, Copy, Debug)]
pub struct HwKind {
    codec: &'static str,
    /// Nome mostrado na interface
    pub label: &'static str,
    /// Opções de baixa latência específicas do codificador
    options: &'static [&'static str],
}

const CANDIDATES: &[HwKind] = &[
    #[cfg(windows)]
    HwKind {
        codec: "h264_nvenc",
        label: "Placa de vídeo · NVIDIA",
        options: &["-preset", "p1", "-tune", "ull", "-zerolatency", "1", "-rc", "cbr", "-profile:v", "baseline", "-pix_fmt", "nv12"],
    },
    #[cfg(windows)]
    HwKind {
        codec: "h264_amf",
        label: "Placa de vídeo · AMD",
        options: &["-usage", "ultralowlatency", "-quality", "speed", "-rc", "cbr", "-profile:v", "constrained_baseline", "-pix_fmt", "nv12"],
    },
    #[cfg(windows)]
    HwKind {
        codec: "h264_qsv",
        label: "Placa de vídeo · Intel",
        options: &["-preset", "veryfast", "-look_ahead", "0", "-async_depth", "1", "-profile:v", "baseline", "-pix_fmt", "nv12"],
    },
    #[cfg(windows)]
    HwKind {
        codec: "h264_mf",
        label: "Placa de vídeo · Windows",
        options: &["-hw_encoding", "1", "-scenario", "display_remoting", "-rate_control", "cbr", "-pix_fmt", "nv12"],
    },
    // Mac fica no processador: pelo FFmpeg o VideoToolbox entregava poucos quadros (5 fps)
];

/// FFmpeg que vai junto com o app (ao lado do executável); em desenvolvimento, o do sistema
fn ffmpeg_path() -> Option<PathBuf> {
    let name = if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" };
    let bundled = std::env::current_exe().ok()?.parent()?.join(name);
    if bundled.exists() {
        return Some(bundled);
    }
    ["/opt/homebrew/bin/ffmpeg", "/usr/local/bin/ffmpeg"].iter().map(PathBuf::from).find(|p| p.exists())
}

fn command(ffmpeg: &Path) -> Command {
    let mut cmd = Command::new(ffmpeg);
    cmd.args(["-hide_banner", "-loglevel", "error", "-nostdin"]);
    // Sem janela de terminal piscando no Windows
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    cmd
}

/// Testa os codificadores em ordem e guarda o primeiro que funciona (uma vez por execução)
pub fn detect() -> Option<(PathBuf, HwKind)> {
    static FOUND: OnceLock<Option<(PathBuf, HwKind)>> = OnceLock::new();
    FOUND
        .get_or_init(|| {
            let ffmpeg = ffmpeg_path()?;
            CANDIDATES.iter().copied().find(|kind| probe(&ffmpeg, kind))
                .map(|kind| (ffmpeg, kind))
        })
        .clone()
}

fn probe(ffmpeg: &Path, kind: &HwKind) -> bool {
    let child = command(ffmpeg)
        .args(["-f", "lavfi", "-i", "color=black:s=640x360:r=30", "-frames:v", "5", "-c:v", kind.codec])
        .args(kind.options)
        .args(["-b:v", "2M", "-bf", "0", "-f", "null", "-"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = child else { return false };
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                return false;
            }
        }
    }
}

pub struct HwEncoder {
    child: Child,
    stdin: ChildStdin,
    frames: Receiver<Vec<u8>>,
    pub dimensions: (usize, usize),
}

impl HwEncoder {
    pub fn new(ffmpeg: &Path, kind: &HwKind, (w, h): (usize, usize), fps: u32, bitrate: u32) -> anyhow::Result<Self> {
        let b = bitrate.to_string();
        // Buffer curto = taxa constante sem picos grandes (menos perda na rede)
        let buf = (bitrate / fps.max(1) * 4).to_string();
        let mut child = command(ffmpeg)
            .args(["-f", "rawvideo", "-pix_fmt", "yuv420p", "-s", &format!("{w}x{h}"), "-r", &fps.to_string(), "-i", "pipe:0"])
            .args(["-c:v", kind.codec])
            .args(kind.options)
            .args(["-b:v", &b, "-maxrate", &b, "-bufsize", &buf])
            // Imagem completa a cada 2 s (quem entra no meio ou perdeu pacote se recupera)
            .args(["-g", &(fps * 2).to_string(), "-bf", "0"])
            // AUD marca o início de cada quadro; dump_extra repete SPS/PPS em toda imagem completa
            .args(["-bsf:v", "h264_metadata=aud=insert,dump_extra=freq=keyframe"])
            .args(["-flush_packets", "1", "-f", "h264", "pipe:1"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;
        let stdin = child.stdin.take().expect("stdin do ffmpeg");
        let mut stdout = child.stdout.take().expect("stdout do ffmpeg");

        let (tx, frames) = channel();
        std::thread::spawn(move || {
            let mut pending: Vec<u8> = Vec::with_capacity(1 << 20);
            let mut chunk = vec![0u8; 1 << 16];
            while let Ok(n) = stdout.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                pending.extend_from_slice(&chunk[..n]);
                // Cada quadro começa com AUD (00 00 00 01 09): quando aparece o próximo,
                // o anterior está completo
                while let Some(next) = find_aud(&pending, 4) {
                    let frame: Vec<u8> = pending.drain(..next).collect();
                    if tx.send(frame).is_err() {
                        return;
                    }
                }
            }
        });

        Ok(Self { child, stdin, frames, dimensions: (w, h) })
    }

    /// Envia um quadro para codificar e devolve os quadros que já saíram
    pub fn encode(&mut self, yuv: &YUVBuffer) -> anyhow::Result<Vec<Vec<u8>>> {
        self.stdin.write_all(yuv.y())?;
        self.stdin.write_all(yuv.u())?;
        self.stdin.write_all(yuv.v())?;
        self.stdin.flush()?;
        Ok(self.frames.try_iter().collect())
    }
}

impl Drop for HwEncoder {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Posição do próximo marcador AUD a partir de `from`
fn find_aud(data: &[u8], from: usize) -> Option<usize> {
    data.get(from..)?.windows(5).position(|w| w == [0, 0, 0, 1, 9]).map(|p| p + from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn separa_quadros_pelo_aud() {
        let data = [[0, 0, 0, 1, 9, 0xf0].as_slice(), &[0, 0, 0, 1, 0x65, 1, 2], &[0, 0, 0, 1, 9, 0xf0], &[0, 0, 0, 1, 0x41]].concat();
        assert_eq!(find_aud(&data, 4), Some(13));
        assert_eq!(find_aud(&data, 14), None);
    }

    /// Codifica de verdade se houver FFmpeg e placa de vídeo nesta máquina
    #[test]
    fn codifica_na_placa_de_video_se_disponivel() {
        let Some((ffmpeg, kind)) = detect() else { return };
        let mut enc = HwEncoder::new(&ffmpeg, &kind, (1280, 720), 60, 6_000_000).unwrap();
        let yuv = YUVBuffer::new(1280, 720);
        let mut frames = Vec::new();
        for _ in 0..30 {
            frames.extend(enc.encode(&yuv).unwrap());
            std::thread::sleep(Duration::from_millis(16));
        }
        assert!(frames.len() >= 20, "{}: só {} quadros", kind.label, frames.len());
        // O primeiro quadro tem que trazer SPS (7) e imagem completa (5)
        let types: Vec<u8> = frames[0].windows(4).filter(|w| w[..3] == [0, 0, 1]).map(|w| w[3] & 0x1f).collect();
        assert!(types.contains(&7) && types.contains(&5), "tipos no 1º quadro: {types:?}");
    }
}
