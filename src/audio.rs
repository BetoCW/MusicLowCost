//! Motor de audio: rodio + symphonia (AAC/M4A) y efectos propios
//! (ecualizador de 10 bandas y saltar silencio inicial), port de los plugins
//! equalizer y skip-silences de Pear pero en Rust puro.

use rodio::{ChannelCount, Decoder, Player, SampleRate, Source};
use crate::stream::Growing;
use std::num::NonZero;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

pub const EQ_FREQS: [f32; 10] = [60.0, 170.0, 310.0, 600.0, 1000.0, 3000.0, 6000.0, 12000.0, 14000.0, 16000.0];

/// Preajustes del ecualizador (dB por banda): plano, mas graves, voces, mas agudos.
pub const EQ_PRESETS: [[f32; 10]; 4] = [
    [0.0; 10],
    [6.0, 5.0, 4.0, 2.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
    [-2.0, -1.0, 0.0, 2.0, 4.0, 4.0, 3.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 0.0, 0.0, 0.0, 2.0, 4.0, 5.0, 6.0, 6.0],
];

/// Parametros compartidos entre la interfaz y el hilo de audio.
pub struct Effects {
    pub eq_enabled: AtomicBool,
    pub eq_gains: Mutex<[f32; 10]>,
    /// Se incrementa cada vez que cambian las ganancias para recalcular filtros.
    pub eq_version: AtomicU32,
    pub skip_silence: AtomicBool,
}

impl Effects {
    pub fn new(enabled: bool, gains: [f32; 10], skip_silence: bool) -> Arc<Self> {
        Arc::new(Self {
            eq_enabled: AtomicBool::new(enabled),
            eq_gains: Mutex::new(gains),
            eq_version: AtomicU32::new(1),
            skip_silence: AtomicBool::new(skip_silence),
        })
    }

    pub fn set_eq(&self, enabled: bool, gains: [f32; 10]) {
        self.eq_enabled.store(enabled, Ordering::Relaxed);
        *self.eq_gains.lock().unwrap() = gains;
        self.eq_version.fetch_add(1, Ordering::Relaxed);
    }
}

pub struct Engine {
    mixer: rodio::mixer::Mixer,
    player: Mutex<Option<Player>>,
    /// Audio cargado y si se abrio con seek (solo cuando ya estaba completo).
    source: Mutex<Option<(Arc<Growing>, bool)>>,
    pub fx: Arc<Effects>,
    volume: Mutex<f32>,
}

impl Engine {
    /// Abre la salida de audio en un hilo propio (el stream de WASAPI no es Send).
    pub fn start(fx: Arc<Effects>) -> Result<Arc<Self>, String> {
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("audio-out".into())
            .stack_size(256 * 1024)
            .spawn(move || match rodio::DeviceSinkBuilder::open_default_sink() {
                Ok(mut sink) => {
                    sink.log_on_drop(false);
                    let _ = tx.send(Ok(sink.mixer().clone()));
                    // El sink tiene que seguir vivo mientras la app corra.
                    loop {
                        std::thread::park();
                    }
                }
                Err(e) => {
                    let _ = tx.send(Err(e.to_string()));
                }
            })
            .map_err(|e| e.to_string())?;
        let mixer = rx.recv().map_err(|e| e.to_string())??;
        Ok(Arc::new(Self { mixer, player: Mutex::new(None), source: Mutex::new(None), fx, volume: Mutex::new(0.8) }))
    }

    /// Reemplaza lo que suena por `src` (M4A en memoria, completo o todavia bajando).
    ///
    /// Si todavia se esta descargando se abre sin seek: con seek, symphonia recorre todos
    /// los fragmentos del archivo al abrirlo y habria que esperar la descarga entera. El
    /// primer seek reabre el audio (ya completo) con seek; ver `seek`.
    pub fn load(&self, src: &Arc<Growing>, start_paused: bool) -> Result<(), String> {
        let seekable = src.is_done();
        let decoder = Decoder::builder()
            .with_data(src.reader())
            .with_byte_len(src.total)
            .with_hint("m4a")
            .with_seekable(seekable)
            .build()
            .map_err(|e| format!("no se pudo decodificar el audio: {e}"))?;
        let player = Player::connect_new(&self.mixer);
        player.set_volume(*self.volume.lock().unwrap());
        if start_paused {
            player.pause();
        }
        player.append(Fx::new(decoder, self.fx.clone()));
        // Al soltar el Player anterior deja de sonar.
        *self.player.lock().unwrap() = Some(player);
        *self.source.lock().unwrap() = Some((src.clone(), seekable));
        Ok(())
    }

    pub fn stop(&self) {
        *self.player.lock().unwrap() = None;
        *self.source.lock().unwrap() = None;
    }

    /// true si un seek se puede hacer ya (el audio se abrio con seek o ya termino de bajar).
    pub fn can_seek(&self) -> bool {
        self.source.lock().unwrap().as_ref().is_some_and(|(s, seekable)| *seekable || s.is_done())
    }

    fn with<T>(&self, f: impl FnOnce(&Player) -> T) -> Option<T> {
        self.player.lock().unwrap().as_ref().map(f)
    }

    pub fn play(&self) {
        self.with(|p| p.play());
    }
    pub fn pause(&self) {
        self.with(|p| p.pause());
    }
    pub fn is_paused(&self) -> bool {
        self.with(|p| p.is_paused()).unwrap_or(true)
    }
    pub fn has_track(&self) -> bool {
        self.player.lock().unwrap().is_some()
    }
    /// true cuando la cancion cargada ya termino.
    pub fn finished(&self) -> bool {
        self.with(|p| p.empty()).unwrap_or(false)
    }
    pub fn position(&self) -> f64 {
        self.with(|p| p.get_pos().as_secs_f64()).unwrap_or(0.0)
    }
    /// Devuelve false si todavia no se puede (el audio sigue bajando): ver `can_seek`.
    pub fn seek(&self, secs: f64) -> bool {
        let src = self.source.lock().unwrap().clone();
        let Some((src, seekable)) = src else { return false };
        if !seekable && !src.is_done() {
            // Volver al principio no necesita seek: se reabre desde el inicio.
            if secs < 0.5 {
                let paused = self.is_paused();
                return self.load(&src, paused).is_ok();
            }
            return false;
        }
        if !seekable {
            // Ya bajo completo: se reabre con seek, conservando la pausa.
            let paused = self.is_paused();
            if let Err(e) = self.load(&src, paused) {
                log::warn!("seek: {e}");
                return false;
            }
        }
        self.with(|p| {
            if let Err(e) = p.try_seek(Duration::from_secs_f64(secs.max(0.0))) {
                log::warn!("seek: {e}");
            }
        });
        true
    }
    /// `v` en 0..=1 ya con la curva aplicada.
    pub fn set_volume(&self, v: f32) {
        *self.volume.lock().unwrap() = v;
        self.with(|p| p.set_volume(v));
    }
}

/// Filtro biquad (formulas del "Audio EQ Cookbook" de R. Bristow-Johnson).
#[derive(Clone, Copy, Default)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
}

impl Biquad {
    fn new(kind: u8, freq: f32, gain_db: f32, sample_rate: f32) -> Self {
        let freq = freq.min(sample_rate * 0.45);
        let a = 10f32.powf(gain_db / 40.0);
        let w0 = 2.0 * std::f32::consts::PI * freq / sample_rate;
        let (sin, cos) = w0.sin_cos();
        let q = 1.0f32;
        let alpha = sin / (2.0 * q);
        let (b0, b1, b2, a0, a1, a2) = match kind {
            // low shelf
            0 => {
                let s = 2.0 * a.sqrt() * alpha;
                (
                    a * ((a + 1.0) - (a - 1.0) * cos + s),
                    2.0 * a * ((a - 1.0) - (a + 1.0) * cos),
                    a * ((a + 1.0) - (a - 1.0) * cos - s),
                    (a + 1.0) + (a - 1.0) * cos + s,
                    -2.0 * ((a - 1.0) + (a + 1.0) * cos),
                    (a + 1.0) + (a - 1.0) * cos - s,
                )
            }
            // high shelf
            2 => {
                let s = 2.0 * a.sqrt() * alpha;
                (
                    a * ((a + 1.0) + (a - 1.0) * cos + s),
                    -2.0 * a * ((a - 1.0) + (a + 1.0) * cos),
                    a * ((a + 1.0) + (a - 1.0) * cos - s),
                    (a + 1.0) - (a - 1.0) * cos + s,
                    2.0 * ((a - 1.0) - (a + 1.0) * cos),
                    (a + 1.0) - (a - 1.0) * cos - s,
                )
            }
            // peaking
            _ => (
                1.0 + alpha * a,
                -2.0 * cos,
                1.0 - alpha * a,
                1.0 + alpha / a,
                -2.0 * cos,
                1.0 - alpha / a,
            ),
        };
        Self { b0: b0 / a0, b1: b1 / a0, b2: b2 / a0, a1: a1 / a0, a2: a2 / a0 }
    }
}

#[derive(Clone, Copy, Default)]
struct BiquadState {
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl BiquadState {
    #[inline]
    fn process(&mut self, f: &Biquad, x: f32) -> f32 {
        let y = f.b0 * x + f.b1 * self.x1 + f.b2 * self.x2 - f.a1 * self.y1 - f.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

const MAX_CH: usize = 8;
/// Umbral de "silencio" (~ -60 dBFS).
const SILENCE: f32 = 0.001;

struct Fx<S: Source> {
    inner: S,
    fx: Arc<Effects>,
    channels: usize,
    sample_rate: f32,
    ch: usize,
    counter: u32,
    version: u32,
    eq_on: bool,
    filters: [Biquad; 10],
    state: Vec<[BiquadState; 10]>,
    // saltar silencio inicial
    skipping: bool,
    skipped_frames: u64,
    frame: [f32; MAX_CH],
    pending: usize,
    pending_len: usize,
    // Ya salio la primera muestra con sonido (solo para las mediciones de bench).
    heard: bool,
}

impl<S: Source> Fx<S> {
    fn new(inner: S, fx: Arc<Effects>) -> Self {
        let channels = (inner.channels().get() as usize).clamp(1, MAX_CH);
        let sample_rate = inner.sample_rate().get() as f32;
        let skipping = fx.skip_silence.load(Ordering::Relaxed);
        let mut me = Self {
            inner,
            fx,
            channels,
            sample_rate,
            ch: 0,
            counter: 0,
            version: 0,
            eq_on: false,
            filters: [Biquad::default(); 10],
            state: vec![[BiquadState::default(); 10]; channels],
            skipping,
            skipped_frames: 0,
            frame: [0.0; MAX_CH],
            pending: 0,
            pending_len: 0,
            heard: false,
        };
        me.refresh();
        me
    }

    fn refresh(&mut self) {
        self.eq_on = self.fx.eq_enabled.load(Ordering::Relaxed);
        let v = self.fx.eq_version.load(Ordering::Relaxed);
        if v != self.version {
            self.version = v;
            let gains = *self.fx.eq_gains.lock().unwrap();
            for (i, f) in EQ_FREQS.iter().enumerate() {
                let kind = if i == 0 { 0 } else if i == 9 { 2 } else { 1 };
                self.filters[i] = Biquad::new(kind, *f, gains[i], self.sample_rate);
            }
        }
    }

    fn mark_heard(&mut self) {
        if !self.heard {
            self.heard = true;
            crate::bench::event("lat_first_loud_sample", 0);
        }
    }

    #[inline]
    fn eq(&mut self, s: f32) -> f32 {
        let ch = self.ch;
        self.ch = (self.ch + 1) % self.channels;
        if !self.eq_on {
            return s;
        }
        let mut y = s;
        let st = &mut self.state[ch];
        for (i, f) in self.filters.iter().enumerate() {
            y = st[i].process(f, y);
        }
        y.clamp(-1.0, 1.0)
    }
}

impl<S: Source> Iterator for Fx<S> {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        self.counter = self.counter.wrapping_add(1);
        if self.counter == 1 {
            crate::bench::event("lat_first_sample", 0);
        }
        if self.counter % 4096 == 0 {
            self.refresh();
        }
        // Devolver un frame que quedo pendiente al dejar de saltar.
        if self.pending < self.pending_len {
            let s = self.frame[self.pending];
            self.pending += 1;
            return Some(self.eq(s));
        }
        if self.skipping {
            // Se descartan frames completos (para no desalinear canales), maximo 15 s.
            let max_frames = (self.sample_rate * 15.0) as u64;
            loop {
                let mut loud = false;
                for c in 0..self.channels {
                    let s = self.inner.next()?;
                    self.frame[c] = s;
                    loud |= s.abs() > SILENCE;
                }
                self.skipped_frames += 1;
                if loud || self.skipped_frames >= max_frames {
                    self.skipping = false;
                    self.pending = 1;
                    self.pending_len = self.channels;
                    self.mark_heard();
                    let s = self.frame[0];
                    return Some(self.eq(s));
                }
            }
        }
        let s = self.inner.next()?;
        if !self.heard && s.abs() > SILENCE {
            self.mark_heard();
        }
        Some(self.eq(s))
    }
}

impl<S: Source> Source for Fx<S> {
    fn current_span_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> ChannelCount {
        NonZero::new(self.channels as u16).unwrap()
    }
    fn sample_rate(&self) -> SampleRate {
        self.inner.sample_rate()
    }
    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }
    fn try_seek(&mut self, pos: Duration) -> Result<(), rodio::source::SeekError> {
        self.skipping = false;
        self.pending_len = 0;
        self.ch = 0;
        for st in &mut self.state {
            *st = [BiquadState::default(); 10];
        }
        self.inner.try_seek(pos)
    }
}

/// Curva de volumen: lineal o exponencial (plugin exponential-volume de Pear, exponente 3).
pub fn volume_curve(percent: f32, exponential: bool) -> f32 {
    let v = (percent / 100.0).clamp(0.0, 1.0);
    if exponential { v.powi(3) } else { v }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curva_de_volumen() {
        assert_eq!(volume_curve(100.0, true), 1.0);
        assert!((volume_curve(50.0, true) - 0.125).abs() < 1e-6);
        assert_eq!(volume_curve(50.0, false), 0.5);
    }

    #[test]
    fn eq_plano_no_cambia_la_senal() {
        let f = Biquad::new(1, 1000.0, 0.0, 44100.0);
        let mut st = BiquadState::default();
        for x in [0.5f32, -0.3, 0.1, 0.9] {
            assert!((st.process(&f, x) - x).abs() < 1e-5);
        }
    }
}
