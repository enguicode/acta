//! Envolventes de energía de las pistas `mic` y `system`, para decidir si un
//! segmento del micro es voz del usuario o solo el audio remoto que se cuela
//! por los altavoces (bleed). Lógica PURA salvo `from_wav`.

use acta_core::{PortError, PortResult};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Tamaño de frame de la envolvente.
pub const FRAME_MS: u64 = 100;
/// Nivel que se da al silencio digital.
pub const SILENCE_DB: f32 = -120.0;

/// Potencia media (muestras normalizadas al cuadrado) por frame de `frame_ms`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Envelope {
    pub frame_ms: u64,
    pub power: Vec<f32>,
}

fn to_db(power: f64) -> f32 {
    if power <= 0.0 {
        SILENCE_DB
    } else {
        (10.0 * power.log10()).max(f64::from(SILENCE_DB)) as f32
    }
}

impl Envelope {
    pub fn from_samples(samples: &[i16], rate: u32, frame_ms: u64) -> Self {
        let frame = ((u64::from(rate) * frame_ms / 1000).max(1)) as usize;
        let power = samples
            .chunks(frame)
            .map(|c| {
                let sum: f64 = c.iter().map(|&s| (f64::from(s) / 32_768.0).powi(2)).sum();
                (sum / c.len() as f64) as f32
            })
            .collect();
        Self { frame_ms, power }
    }

    /// Lee un WAV por bloques (sirve para reuniones de horas sin cargarlo entero).
    pub fn from_wav(path: &Path) -> PortResult<Self> {
        let mut reader = hound::WavReader::open(path).map_err(|e| PortError::Io(format!("{}: {e}", path.display())))?;
        let spec = reader.spec();
        let frame = ((u64::from(spec.sample_rate) * FRAME_MS / 1000).max(1)) as usize * usize::from(spec.channels.max(1));
        let mut power = Vec::new();
        let (mut sum, mut n) = (0f64, 0usize);
        for s in reader.samples::<i16>() {
            let s = s.map_err(|e| PortError::Io(format!("{}: {e}", path.display())))?;
            sum += (f64::from(s) / 32_768.0).powi(2);
            n += 1;
            if n == frame {
                power.push((sum / n as f64) as f32);
                (sum, n) = (0.0, 0);
            }
        }
        if n > 0 {
            power.push((sum / n as f64) as f32);
        }
        Ok(Self { frame_ms: FRAME_MS, power })
    }

    /// Nivel medio (dBFS, por potencia) en `[start_ms, end_ms)`. `None` si no hay frames.
    pub fn mean_db(&self, start_ms: u64, end_ms: u64) -> Option<f32> {
        if self.frame_ms == 0 || end_ms <= start_ms {
            return None;
        }
        let from = (start_ms / self.frame_ms) as usize;
        let to = (end_ms.div_ceil(self.frame_ms) as usize).min(self.power.len());
        if from >= to {
            return None;
        }
        let frames = &self.power[from..to];
        Some(to_db(frames.iter().map(|&p| f64::from(p)).sum::<f64>() / frames.len() as f64))
    }
}

/// Envolventes de ambas pistas.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TrackLevels {
    pub mic: Envelope,
    pub system: Envelope,
}

/// Nivel del micro y del sistema en una ventana.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WindowLevels {
    pub mic_db: f32,
    pub system_db: f32,
}

impl WindowLevels {
    /// Cuánto más alto suena el micro que el sistema (dB). Negativo = el micro
    /// va por debajo: lo que entra es el audio remoto que se cuela.
    pub fn mic_over_system(self) -> f32 {
        self.mic_db - self.system_db
    }
}

impl TrackLevels {
    pub fn window(&self, start_ms: u64, end_ms: u64) -> Option<WindowLevels> {
        Some(WindowLevels {
            mic_db: self.mic.mean_db(start_ms, end_ms)?,
            system_db: self.system.mean_db(start_ms, end_ms)?,
        })
    }

    /// Envolvente constante (solo tests).
    #[cfg(test)]
    pub fn constant(mic_db: f32, system_db: f32, len_ms: u64) -> Self {
        let frames = (len_ms / FRAME_MS) as usize;
        let p = |db: f32| 10f32.powf(db / 10.0);
        Self {
            mic: Envelope { frame_ms: FRAME_MS, power: vec![p(mic_db); frames] },
            system: Envelope { frame_ms: FRAME_MS, power: vec![p(system_db); frames] },
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn mean_db_averages_power_over_the_window() {
        let mut samples = vec![16_384i16; 1_600]; // 100 ms a 16 kHz, amplitud 0.5 = -6 dBFS
        samples.extend(vec![0i16; 1_600]);
        let env = Envelope::from_samples(&samples, 16_000, 100);
        assert_eq!(env.power.len(), 2);
        assert!((env.mean_db(0, 100).unwrap() - -6.02).abs() < 0.1);
        assert!((env.mean_db(0, 200).unwrap() - -9.03).abs() < 0.1, "la potencia se promedia, no los dB");
        assert_eq!(env.mean_db(100, 200), Some(SILENCE_DB));
        assert_eq!(env.mean_db(500, 600), None);
        assert_eq!(env.mean_db(50, 50), None);
    }

    #[test]
    fn from_wav_matches_from_samples() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.wav");
        let samples: Vec<i16> = (0..4_000).map(|i| ((i % 100) * 100) as i16).collect();
        crate::live::write_wav(&p, 16_000, &samples).unwrap();
        let a = Envelope::from_wav(&p).unwrap();
        let b = Envelope::from_samples(&samples, 16_000, FRAME_MS);
        assert_eq!(a, b);
    }

    #[test]
    fn window_gives_mic_over_system() {
        let l = TrackLevels::constant(-50.0, -20.0, 1_000);
        let w = l.window(0, 500).unwrap();
        assert!((w.mic_over_system() - -30.0).abs() < 0.1);
    }
}
