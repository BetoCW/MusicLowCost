//! Estimacion del reloj del anfitrion desde un invitado (mismo calculo que NTP).
//!
//! Con un Ping (t0, reloj del invitado), el anfitrion anota cuando lo recibio (t1) y cuando
//! mando el Pong (t2), y el invitado cuando lo recibio (t3):
//!   desfase = ((t1 - t0) + (t2 - t3)) / 2
//!   demora  = (t3 - t0) - (t2 - t1)
//! Se usa la muestra con menor demora de las ultimas: es la que menos espero en colas.

use std::collections::VecDeque;
use std::time::Instant;

const WINDOW: usize = 8;

pub struct Clock {
    epoch: Instant,
    /// (demora, desfase) en microsegundos.
    samples: VecDeque<(i64, i64)>,
}

impl Clock {
    pub fn new() -> Self {
        Self { epoch: Instant::now(), samples: VecDeque::with_capacity(WINDOW) }
    }

    /// Reloj local en microsegundos.
    pub fn now_us(&self) -> i64 {
        micros_since(self.epoch)
    }

    pub fn add_sample(&mut self, t0: i64, t1: i64, t2: i64, t3: i64) {
        // En i128: los tiempos del anfitrion llegan por la red y podrian ser cualquier cosa.
        let (t0, t1, t2, t3) = (t0 as i128, t1 as i128, t2 as i128, t3 as i128);
        let delay = ((t3 - t0) - (t2 - t1)).clamp(0, i64::MAX as i128) as i64;
        let offset = (((t1 - t0) + (t2 - t3)) / 2).clamp(i64::MIN as i128 / 2, i64::MAX as i128 / 2) as i64;
        if self.samples.len() == WINDOW {
            self.samples.pop_front();
        }
        self.samples.push_back((delay, offset));
    }

    fn best(&self) -> Option<(i64, i64)> {
        self.samples.iter().copied().min_by_key(|s| s.0)
    }

    /// Reloj del anfitrion = reloj local + desfase.
    pub fn offset(&self) -> Option<i64> {
        self.best().map(|b| b.1)
    }

    /// Ida y vuelta de la mejor muestra, en milisegundos.
    pub fn rtt_ms(&self) -> Option<f64> {
        self.best().map(|b| b.0 as f64 / 1000.0)
    }

    /// Hora actual del anfitrion (si ya hay alguna muestra).
    pub fn host_now(&self) -> Option<i64> {
        self.offset().map(|o| self.now_us().saturating_add(o))
    }
}

pub fn micros_since(epoch: Instant) -> i64 {
    epoch.elapsed().as_micros() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calcula_el_desfase_con_la_muestra_de_menor_demora() {
        let mut c = Clock::new();
        assert!(c.offset().is_none());
        // Anfitrion 1 s adelantado, 10 ms de ida y 10 de vuelta, 1 ms de proceso.
        c.add_sample(0, 1_010_000, 1_011_000, 21_000);
        assert_eq!(c.offset(), Some(1_000_000));
        assert_eq!(c.rtt_ms(), Some(20.0));
        // Una muestra que espero en una cola (vuelta lenta) no la reemplaza.
        c.add_sample(100_000, 1_110_000, 1_111_000, 400_000);
        assert_eq!(c.offset(), Some(1_000_000));
        // Una muestra mas rapida si.
        c.add_sample(500_000, 1_502_000, 1_502_000, 504_000);
        assert_eq!(c.offset(), Some(1_000_000));
        assert_eq!(c.rtt_ms(), Some(4.0));
    }

    #[test]
    fn solo_recuerda_las_ultimas_muestras() {
        let mut c = Clock::new();
        c.add_sample(0, 5, 5, 0); // demora 0, desfase 5
        for i in 0..WINDOW as i64 {
            c.add_sample(i * 10, i * 10 + 7, i * 10 + 7, i * 10 + 2); // demora 2, desfase 6
        }
        assert_eq!(c.offset(), Some(6));
    }
}
