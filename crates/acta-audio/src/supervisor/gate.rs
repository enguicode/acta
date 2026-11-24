//! Filtro de capturas automáticas. El sidecar manda una `slide_change` por
//! cada cambio estable de la ventana y una candidata cada N s, aunque nadie
//! comparta pantalla (vídeo de Teams, pantalla entera en grabación manual):
//! decenas de imágenes sin valor. Aquí solo pasa lo que aporta algo.
//!
//! - Las automáticas (cambio de diapositiva, candidata) exigen que alguien
//!   esté compartiendo.
//! - Y que el hash perceptual o la firma 64×36 (ver `thumb`) difieran lo bastante
//!   de la última conservada
//!   (descarta casi duplicadas).
//! - Y, para el cambio de diapositiva, un intervalo mínimo.
//! - Las que pide una persona o una regla (manual, palabra clave, LLM) pasan
//!   siempre.

use acta_core::screen::CaptureReason;
use acta_core::time::UnixMs;

/// Separación mínima entre dos cambios de diapositiva conservados.
pub const MIN_SLIDE_GAP_MS: i64 = 8_000;

pub use thumb::Thumb;

/// Firma de contenido 64×36 (gris) que manda el sidecar: el dHash de 64 bits
/// no distingue dos diapositivas con la misma plantilla y poco texto (1-3
/// bits, igual que el ruido). Se compara DÓNDE cambia la imagen: un cambio de
/// diapositiva toca muchas celdas repartidas; el ruido (cursor, reloj,
/// miniatura de vídeo) solo unas pocas, juntas y rellenas. Mismo algoritmo y
/// constantes que `SlideThumb` en `ScreenShareCapture.swift`.
mod thumb {
    pub const COLS: usize = 64;
    pub const ROWS: usize = 36;
    /// Salto de gris de una celda que cuenta como "cambió".
    const CELL_DELTA: i32 = 10;
    /// Un grupo de celdas contiguas que cabe en una esquina (≤ 25 % del ancho
    /// y del alto) y está casi relleno es una superposición (puntero, vídeo).
    const OVERLAY_MAX_COLS: usize = 16;
    const OVERLAY_MAX_ROWS: usize = 9;
    const OVERLAY_FILL: f64 = 0.5;
    /// Celdas cambiadas (sin superposiciones) y columnas distintas mínimas.
    const MIN_CELLS: usize = 6;
    const MIN_COLUMNS: u32 = 5;
    /// Cambio global de luminosidad media que basta solo (fondo, plantilla).
    const MEAN_DELTA: f64 = 12.0;

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Thumb(Vec<u8>);

    impl Thumb {
        /// Hex de 4608 caracteres; cualquier otra cosa es `None`.
        pub fn parse(hex: &str) -> Option<Self> {
            if hex.len() != COLS * ROWS * 2 || !hex.is_ascii() {
                return None;
            }
            hex.as_bytes()
                .chunks(2)
                .map(|p| u8::from_str_radix(std::str::from_utf8(p).ok()?, 16).ok())
                .collect::<Option<Vec<_>>>()
                .map(Self)
        }

        /// ¿Es `other` otra diapositiva respecto a `self`?
        pub fn is_slide_change(&self, other: &Thumb) -> bool {
            let (a, b) = (&self.0, &other.0);
            let n = a.len();
            let mut changed = vec![false; n];
            let (mut sum_a, mut sum_b) = (0i64, 0i64);
            for i in 0..n {
                sum_a += i64::from(a[i]);
                sum_b += i64::from(b[i]);
                changed[i] = (i32::from(a[i]) - i32::from(b[i])).abs() >= CELL_DELTA;
            }
            if ((sum_a - sum_b).abs() as f64) / n as f64 >= MEAN_DELTA {
                return true;
            }
            let mut seen = vec![false; n];
            let (mut kept, mut col_set) = (0usize, 0u64);
            for start in 0..n {
                if !changed[start] || seen[start] {
                    continue;
                }
                seen[start] = true;
                let mut stack = vec![start];
                let mut members = Vec::new();
                let (mut min_x, mut max_x, mut min_y, mut max_y) = (COLS, 0, ROWS, 0);
                while let Some(i) = stack.pop() {
                    members.push(i);
                    let (x, y) = (i % COLS, i / COLS);
                    min_x = min_x.min(x);
                    max_x = max_x.max(x);
                    min_y = min_y.min(y);
                    max_y = max_y.max(y);
                    for ny in y.saturating_sub(1)..=(y + 1).min(ROWS - 1) {
                        for nx in x.saturating_sub(1)..=(x + 1).min(COLS - 1) {
                            let j = ny * COLS + nx;
                            if changed[j] && !seen[j] {
                                seen[j] = true;
                                stack.push(j);
                            }
                        }
                    }
                }
                let (w, h) = (max_x - min_x + 1, max_y - min_y + 1);
                let fill = members.len() as f64 / (w * h) as f64;
                if w <= OVERLAY_MAX_COLS && h <= OVERLAY_MAX_ROWS && fill >= OVERLAY_FILL {
                    continue;
                }
                kept += members.len();
                for i in members {
                    col_set |= 1 << (i % COLS);
                }
            }
            kept >= MIN_CELLS && col_set.count_ones() >= MIN_COLUMNS
        }
    }
}

#[derive(Debug, Clone)]
pub struct CaptureGate {
    threshold: u32,
    last: Option<Last>,
}

#[derive(Debug, Clone)]
struct Last {
    hash: Option<u64>,
    thumb: Option<Thumb>,
    at: UnixMs,
}

impl CaptureGate {
    pub fn new(phash_threshold: u32) -> Self {
        Self { threshold: phash_threshold.max(1), last: None }
    }

    /// Nueva compartición: lo anterior ya no sirve de referencia.
    pub fn reset(&mut self) {
        self.last = None;
    }

    /// ¿Se conserva esta captura? `thumb` es la firma 64×36 (hex) del sidecar;
    /// sin ella (sidecar viejo, ilegible) solo decide el dHash.
    pub fn admit(&mut self, reason: CaptureReason, at: UnixMs, phash: &str, thumb: Option<&str>, sharing: bool) -> bool {
        let hash = u64::from_str_radix(phash, 16).ok();
        let thumb = thumb.and_then(Thumb::parse);
        let automatic = matches!(reason, CaptureReason::SlideChange | CaptureReason::Candidate);
        if automatic {
            if !sharing {
                return false;
            }
            if let Some(last) = &self.last {
                let comparable = hash.zip(last.hash).is_some() || thumb.as_ref().zip(last.thumb.as_ref()).is_some();
                if comparable {
                    let hash_differs = hash.zip(last.hash).is_some_and(|(h, l)| (h ^ l).count_ones() >= self.threshold);
                    let thumb_differs = thumb.as_ref().zip(last.thumb.as_ref()).is_some_and(|(t, l)| l.is_slide_change(t));
                    if !hash_differs && !thumb_differs {
                        return false;
                    }
                    if reason == CaptureReason::SlideChange && at.0 - last.at.0 < MIN_SLIDE_GAP_MS {
                        return false;
                    }
                }
            }
        }
        if hash.is_some() || thumb.is_some() {
            self.last = Some(Last { hash, thumb, at });
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use CaptureReason::*;

    const A: &str = "0000000000000000";
    const FAR: &str = "ffffffffffffffff";
    const NEAR_A: &str = "0000000000000003";

    #[test]
    fn automatic_captures_need_someone_sharing() {
        let mut g = CaptureGate::new(10);
        assert!(!g.admit(SlideChange, UnixMs(1_000), A, None, false));
        assert!(!g.admit(Candidate, UnixMs(2_000), FAR, None, false));
        assert!(g.admit(SlideChange, UnixMs(3_000), A, None, true));
    }

    #[test]
    fn manual_keyword_and_llm_always_pass() {
        let mut g = CaptureGate::new(10);
        for r in [Manual, Keyword, LlmPick] {
            assert!(g.admit(r, UnixMs(1_000), A, None, false));
        }
    }

    #[test]
    fn near_duplicates_of_the_last_kept_are_dropped() {
        let mut g = CaptureGate::new(10);
        assert!(g.admit(SlideChange, UnixMs(0), A, None, true));
        assert!(!g.admit(SlideChange, UnixMs(60_000), NEAR_A, None, true));
        assert!(!g.admit(Candidate, UnixMs(61_000), A, None, true));
        assert!(g.admit(SlideChange, UnixMs(62_000), FAR, None, true));
    }

    #[test]
    fn slides_closer_than_the_minimum_gap_are_dropped() {
        let mut g = CaptureGate::new(10);
        assert!(g.admit(SlideChange, UnixMs(0), A, None, true));
        assert!(!g.admit(SlideChange, UnixMs(MIN_SLIDE_GAP_MS - 1), FAR, None, true));
        assert!(g.admit(SlideChange, UnixMs(MIN_SLIDE_GAP_MS), FAR, None, true));
    }

    #[test]
    fn a_manual_capture_counts_as_reference_and_reset_forgets_it() {
        let mut g = CaptureGate::new(10);
        assert!(g.admit(Manual, UnixMs(0), A, None, true));
        assert!(!g.admit(SlideChange, UnixMs(20_000), A, None, true));
        g.reset();
        assert!(g.admit(SlideChange, UnixMs(21_000), A, None, true));
    }

    fn thumb(name: &str) -> String {
        include_str!("../../tests/fixtures/slide_thumbs.txt")
            .lines()
            .find_map(|l| l.strip_prefix(name).and_then(|r| r.strip_prefix(' ')))
            .unwrap_or_else(|| panic!("fixture {name}"))
            .to_string()
    }

    /// dHash de la base: los casos dispersos distan 0-3 bits de ella (se simula el mismo).
    const BASE_H: &str = "1f1f3f1f1f3f1f1f";

    /// ¿Se guarda `next` como cambio de diapositiva tras haber guardado `prev`?
    fn kept_after(prev: &str, next: &str, next_hash: &str) -> bool {
        let mut g = CaptureGate::new(10);
        assert!(g.admit(SlideChange, UnixMs(0), BASE_H, Some(&thumb(prev)), true));
        g.admit(SlideChange, UnixMs(20_000), next_hash, Some(&thumb(next)), true)
    }

    #[test]
    fn sparse_title_and_template_changes_pass_although_dhash_barely_moves() {
        // 1-3 bits de dHash: antes se perdían.
        assert!(kept_after("base", "sparse_b", BASE_H));
        assert!(kept_after("base", "title_only", BASE_H));
        assert!(kept_after("base", "dark", BASE_H));
        assert!(kept_after("base", "template2", BASE_H));
        assert!(kept_after("dense_a", "dense_b", BASE_H));
    }

    #[test]
    fn noise_is_dropped_even_with_a_thumbnail() {
        assert!(!kept_after("base", "cursor_clock", BASE_H), "cursor + reloj");
        assert!(!kept_after("base", "jpeg", BASE_H), "recompresión JPEG");
        assert!(!kept_after("video1", "video2", BASE_H), "miniatura del presentador");
    }

    #[test]
    fn without_thumbnails_it_falls_back_to_dhash_only() {
        let mut g = CaptureGate::new(10);
        assert!(g.admit(SlideChange, UnixMs(0), A, None, true));
        assert!(!g.admit(SlideChange, UnixMs(20_000), NEAR_A, Some(&thumb("sparse_b")), true), "sin firma previa no hay con qué comparar");
        assert!(g.admit(SlideChange, UnixMs(40_000), FAR, None, true));
    }

    #[test]
    fn an_unreadable_thumbnail_is_ignored() {
        let mut g = CaptureGate::new(10);
        assert!(g.admit(SlideChange, UnixMs(0), A, Some("zz"), true));
        assert!(!g.admit(SlideChange, UnixMs(20_000), NEAR_A, Some("00"), true));
    }

    #[test]
    fn an_unreadable_hash_is_kept() {
        let mut g = CaptureGate::new(10);
        assert!(g.admit(SlideChange, UnixMs(0), "zz", None, true));
        assert!(g.admit(SlideChange, UnixMs(1), "zz", None, true));
    }
}
