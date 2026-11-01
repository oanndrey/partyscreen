// Áudio: empacota o som capturado em Opus (20 ms, 48 kHz, estéreo) e, do lado de
// quem assiste, decodifica e toca.
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use tokio::sync::mpsc::UnboundedSender;

pub const RATE: u32 = 48_000;
/// Amostras por canal em 20 ms
const FRAME: usize = 960;

/// Converte qualquer entrada (taxa/canais) para estéreo 48 kHz e envia pacotes Opus
pub struct Packer {
    encoder: opus::Encoder,
    pending: Vec<f32>,
    out: Vec<u8>,
    tx: UnboundedSender<Vec<u8>>,
    resample_pos: f64,
}

impl Packer {
    pub fn new(tx: UnboundedSender<Vec<u8>>) -> anyhow::Result<Self> {
        let mut encoder = opus::Encoder::new(RATE, opus::Channels::Stereo, opus::Application::Audio)?;
        encoder.set_bitrate(opus::Bitrate::Bits(128_000))?;
        Ok(Self { encoder, pending: Vec::with_capacity(FRAME * 4), out: vec![0; 4000], tx, resample_pos: 0.0 })
    }

    /// `data` intercalado com `channels` canais, na taxa `rate`
    pub fn push(&mut self, data: &[f32], channels: usize, rate: u32) {
        if channels == 0 {
            return;
        }
        let frames = data.len() / channels;
        let stereo = |i: usize| {
            let l = data[i * channels];
            let r = if channels > 1 { data[i * channels + 1] } else { l };
            (l, r)
        };

        if rate == RATE {
            for i in 0..frames {
                let (l, r) = stereo(i);
                self.pending.extend([l, r]);
            }
        } else {
            // Reamostragem linear simples (ex.: placa em 44,1 kHz)
            let step = rate as f64 / RATE as f64;
            while (self.resample_pos as usize) + 1 < frames {
                let i = self.resample_pos as usize;
                let t = (self.resample_pos - i as f64) as f32;
                let (l0, r0) = stereo(i);
                let (l1, r1) = stereo(i + 1);
                self.pending.extend([l0 + (l1 - l0) * t, r0 + (r1 - r0) * t]);
                self.resample_pos += step;
            }
            self.resample_pos -= (frames.saturating_sub(1)) as f64;
            self.resample_pos = self.resample_pos.max(0.0);
        }

        while self.pending.len() >= FRAME * 2 {
            let chunk: Vec<f32> = self.pending.drain(..FRAME * 2).collect();
            if let Ok(n) = self.encoder.encode_float(&chunk, &mut self.out) {
                let _ = self.tx.send(self.out[..n].to_vec());
            }
        }
    }
}

/// Toca os pacotes Opus recebidos. Mantém pouco áudio em fila para não acumular atraso.
pub struct Player {
    decoder: opus::Decoder,
    queue: Arc<Mutex<VecDeque<f32>>>,
    pcm: Vec<f32>,
    stop: Arc<AtomicBool>,
    /// Taxa da saída de áudio; se não for 48 kHz, reamostra antes de enfileirar
    out_rate: u32,
    resample_pos: f64,
}

/// Fila máxima: 150 ms. Acima disso descarta o excesso (rede soltou um bloco de uma vez).
fn max_queue(rate: u32) -> usize {
    rate as usize * 2 * 150 / 1000
}

impl Player {
    pub fn new() -> anyhow::Result<Self> {
        let queue = Arc::new(Mutex::new(VecDeque::new()));
        let stop = Arc::new(AtomicBool::new(false));

        // O stream do cpal não pode trocar de thread em todo sistema: vive numa thread própria
        let (q, s) = (queue.clone(), stop.clone());
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let result = (|| -> anyhow::Result<(cpal::Stream, u32)> {
                let device = cpal::default_host()
                    .default_output_device()
                    .ok_or_else(|| anyhow::anyhow!("sem saída de áudio"))?;
                let default = device.default_output_config()?.config();
                let channels = default.channels as usize;
                let callback = move |out: &mut [f32], _: &cpal::OutputCallbackInfo| {
                        let mut q = q.lock().unwrap();
                        for frame in out.chunks_mut(channels) {
                            let l = q.pop_front().unwrap_or(0.0);
                            let r = q.pop_front().unwrap_or(l);
                            for (c, sample) in frame.iter_mut().enumerate() {
                                *sample = if c % 2 == 0 { l } else { r };
                            }
                        }
                };
                // Tenta 48 kHz; se a placa não aceitar, usa a taxa dela e reamostra
                let at_48k = cpal::StreamConfig { sample_rate: RATE, ..default.clone() };
                let (stream, rate) = match device.build_output_stream::<f32, _, _>(
                    at_48k,
                    callback.clone(),
                    |e| eprintln!("Erro no áudio: {e}"),
                    None,
                ) {
                    Ok(stream) => (stream, RATE),
                    Err(_) => {
                        let rate = default.sample_rate;
                        let stream =
                            device.build_output_stream::<f32, _, _>(default, callback, |e| eprintln!("Erro no áudio: {e}"), None)?;
                        (stream, rate)
                    }
                };
                stream.play()?;
                Ok((stream, rate))
            })();
            match result {
                Ok((stream, rate)) => {
                    let _ = ready_tx.send(Ok(rate));
                    while !s.load(Ordering::Relaxed) {
                        std::thread::sleep(std::time::Duration::from_millis(100));
                    }
                    drop(stream);
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                }
            }
        });
        let out_rate = ready_rx.recv()??;

        Ok(Self {
            decoder: opus::Decoder::new(RATE, opus::Channels::Stereo)?,
            queue,
            pcm: vec![0.0; FRAME * 2 * 6],
            stop,
            out_rate,
            resample_pos: 0.0,
        })
    }

    pub fn play(&mut self, packet: &[u8]) {
        let Ok(n) = self.decoder.decode_float(packet, &mut self.pcm, false) else { return };
        let pcm = &self.pcm[..n * 2];
        let mut q = self.queue.lock().unwrap();
        if self.out_rate == RATE {
            q.extend(pcm);
        } else {
            let step = RATE as f64 / self.out_rate as f64;
            while (self.resample_pos as usize) + 1 < n {
                let i = self.resample_pos as usize;
                let t = (self.resample_pos - i as f64) as f32;
                for c in 0..2 {
                    let (a, b) = (pcm[i * 2 + c], pcm[(i + 1) * 2 + c]);
                    q.push_back(a + (b - a) * t);
                }
                self.resample_pos += step;
            }
            self.resample_pos = (self.resample_pos - (n - 1) as f64).max(0.0);
        }
        let max = max_queue(self.out_rate);
        if q.len() > max {
            let excess = q.len() - max / 2;
            q.drain(..excess & !1);
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Captura o que está tocando no computador (loopback do WASAPI) — só Windows.
/// No macOS o áudio vem junto do ScreenCaptureKit.
#[cfg(windows)]
pub struct LoopbackGuard(Arc<AtomicBool>);

#[cfg(windows)]
impl Drop for LoopbackGuard {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

#[cfg(windows)]
pub fn start_loopback(mut packer: Packer) -> anyhow::Result<LoopbackGuard> {

    let stop = Arc::new(AtomicBool::new(false));
    let s = stop.clone();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = (|| -> anyhow::Result<cpal::Stream> {
            let device = cpal::default_host()
                .default_output_device()
                .ok_or_else(|| anyhow::anyhow!("sem saída de áudio"))?;
            let config = device.default_output_config()?.config();
            let (channels, rate) = (config.channels as usize, config.sample_rate);
            // Abrir um stream de entrada num dispositivo de saída = loopback no WASAPI
            let stream = device.build_input_stream::<f32, _, _>(
                config,
                move |data, _| packer.push(data, channels, rate),
                |e| eprintln!("Erro na captura de áudio: {e}"),
                None,
            )?;
            stream.play()?;
            Ok(stream)
        })();
        match result {
            Ok(stream) => {
                let _ = ready_tx.send(Ok(()));
                while !s.load(Ordering::Relaxed) {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                drop(stream);
            }
            Err(e) => {
                let _ = ready_tx.send(Err(e));
            }
        }
    });
    ready_rx.recv()??;
    Ok(LoopbackGuard(stop))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empacota_e_decodifica_opus() {
        for (rate, channels) in [(48_000u32, 2usize), (44_100, 2), (48_000, 1)] {
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            let mut packer = Packer::new(tx).unwrap();
            // 1 s de senoide 440 Hz, em blocos de 10 ms como chega da placa
            let block = rate as usize / 100;
            for b in 0..100 {
                let data: Vec<f32> = (0..block)
                    .flat_map(|i| {
                        let t = (b * block + i) as f32 / rate as f32;
                        std::iter::repeat_n((t * 440.0 * std::f32::consts::TAU).sin() * 0.5, channels)
                    })
                    .collect();
                packer.push(&data, channels, rate);
            }
            let mut decoder = opus::Decoder::new(RATE, opus::Channels::Stereo).unwrap();
            let mut pcm = vec![0.0f32; FRAME * 2];
            let mut packets = 0;
            let mut energy = 0.0f32;
            while let Ok(p) = rx.try_recv() {
                let n = decoder.decode_float(&p, &mut pcm, false).unwrap();
                energy += pcm[..n * 2].iter().map(|s| s * s).sum::<f32>();
                packets += 1;
            }
            // 1 s = 50 quadros de 20 ms (o último pode ficar pendente)
            assert!((49..=50).contains(&packets), "{rate}/{channels}: {packets} pacotes");
            assert!(energy > 100.0, "{rate}/{channels}: sem som decodificado");
        }
    }
}
