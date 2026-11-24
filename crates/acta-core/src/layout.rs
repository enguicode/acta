//! Disposición del directorio de datos. Lógica pura de rutas (sin I/O) para
//! que audio, store, sidecar y app coincidan SIEMPRE en dónde está cada cosa.
//!
//! ```text
//! ~/Library/Application Support/acta/          <- root
//!   acta.db  (+ -wal, -shm)
//!   logs/acta.log, logs/acta-mac.log
//!   models/                                   <- caché de modelos (fastembed, FluidAudio)
//!   models/llm/<modelo>.gguf (+ .part)        <- modelo local integrado (opcional)
//!   runtime/llama/<versión>/llama-server      <- llama.cpp descargado por acta
//!   cache/app-icons/<bundle id>.png           <- iconos de las apps instaladas (`apps_list`)
//!   meetings/<meeting_id>/
//!     manifest.json                           <- manifiesto de sesión (recuperable)
//!     audio/mic/seg-000000.wav                <- segmentos finalizados
//!     audio/mic/seg-000001.wav.tmp            <- segmento en escritura
//!     audio/system/seg-000000.wav
//!     audio/mic.wav  audio/system.wav         <- concatenación final (hound)
//!     audio/mix.wav                           <- mezcla mono para reproducir
//!     transcript/asr-mic.json  asr-system.json  diarization.json
//!     live/speakers.jsonl                     <- roster / hablante activo en vivo
//!     live/screen.jsonl                       <- inicio/fin de pantalla compartida
//!     transcript/segments-<track>.json        <- caché del pipeline (Vec<AsrSegment>)
//!     transcript/diarization-result.json      <- caché del pipeline (DiarizationResult)
//!     captures/<media_ms>-<reason>.jpg        <- capturas conservadas
//!     captures/candidates/<unix_ms>.jpg       <- candidatas (se podan al procesar)
//!     llm/minutes.json  llm/raw-<kind>-<n>.txt
//! ```

use crate::ids::MeetingId;
use crate::recording::Track;
use std::path::{Path, PathBuf};

/// Nombre de la carpeta bajo `~/Library/Application Support/`.
pub const APP_DIR_NAME: &str = "acta";
pub const DB_FILE: &str = "acta.db";
pub const MANIFEST_FILE: &str = "manifest.json";
pub const TMP_SUFFIX: &str = ".tmp";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataLayout {
    root: PathBuf,
}

impl DataLayout {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn db_path(&self) -> PathBuf {
        self.root.join(DB_FILE)
    }
    pub fn logs_dir(&self) -> PathBuf {
        self.root.join("logs")
    }
    pub fn models_dir(&self) -> PathBuf {
        self.root.join("models")
    }
    /// Modelos GGUF del modelo local integrado: `models/llm/<fichero>.gguf`.
    pub fn llm_models_dir(&self) -> PathBuf {
        self.models_dir().join("llm")
    }
    /// `llama-server` descargado por acta: `runtime/llama/<versión>/`.
    pub fn llama_runtime_dir(&self) -> PathBuf {
        self.root.join("runtime").join("llama")
    }
    /// Iconos PNG de las apps instaladas (`apps_list`); se pueden borrar: se regeneran.
    pub fn app_icons_dir(&self) -> PathBuf {
        self.root.join("cache").join("app-icons")
    }
    pub fn meetings_dir(&self) -> PathBuf {
        self.root.join("meetings")
    }
    pub fn meeting_dir(&self, id: MeetingId) -> PathBuf {
        self.meetings_dir().join(id.to_string())
    }
    pub fn manifest_path(&self, id: MeetingId) -> PathBuf {
        self.meeting_dir(id).join(MANIFEST_FILE)
    }
    pub fn track_dir(&self, id: MeetingId, track: Track) -> PathBuf {
        self.meeting_dir(id).join("audio").join(track.as_str())
    }
    /// Segmento finalizado: `audio/<track>/seg-000042.wav`.
    pub fn segment_path(&self, id: MeetingId, track: Track, index: u32) -> PathBuf {
        self.track_dir(id, track).join(format!("seg-{index:06}.wav"))
    }
    /// Segmento en escritura: `audio/<track>/seg-000042.wav.tmp`.
    pub fn segment_tmp_path(&self, id: MeetingId, track: Track, index: u32) -> PathBuf {
        self.track_dir(id, track)
            .join(format!("seg-{index:06}.wav{TMP_SUFFIX}"))
    }
    /// Pista concatenada: `audio/mic.wav` / `audio/system.wav`.
    pub fn track_wav(&self, id: MeetingId, track: Track) -> PathBuf {
        self.meeting_dir(id)
            .join("audio")
            .join(format!("{}.wav", track.as_str()))
    }
    pub fn mix_wav(&self, id: MeetingId) -> PathBuf {
        self.meeting_dir(id).join("audio").join("mix.wav")
    }
    pub fn transcript_dir(&self, id: MeetingId) -> PathBuf {
        self.meeting_dir(id).join("transcript")
    }
    pub fn asr_json(&self, id: MeetingId, track: Track) -> PathBuf {
        self.transcript_dir(id)
            .join(format!("asr-{}.json", track.as_str()))
    }
    pub fn diarization_json(&self, id: MeetingId) -> PathBuf {
        self.transcript_dir(id).join("diarization.json")
    }
    pub fn captures_dir(&self, id: MeetingId) -> PathBuf {
        self.meeting_dir(id).join("captures")
    }
    pub fn capture_candidates_dir(&self, id: MeetingId) -> PathBuf {
        self.captures_dir(id).join("candidates")
    }
    /// Señales de hablante en vivo (roster / activo) como JSON por líneas.
    pub fn live_speakers_path(&self, id: MeetingId) -> PathBuf {
        self.meeting_dir(id).join("live").join("speakers.jsonl")
    }
    pub fn llm_dir(&self, id: MeetingId) -> PathBuf {
        self.meeting_dir(id).join("llm")
    }
    /// `ScreenEvent` (inicio/fin de compartición) en vivo, JSON por líneas.
    pub fn live_screen_path(&self, id: MeetingId) -> PathBuf {
        self.meeting_dir(id).join("live").join("screen.jsonl")
    }
    /// Transcripción en vivo CONFIRMADA de una pista (`LiveAsrState`): al
    /// parar, el pipeline solo transcribe lo que falta desde `coveredMs`.
    pub fn live_asr_path(&self, id: MeetingId, track: Track) -> PathBuf {
        self.meeting_dir(id).join("live").join(format!("asr-{}.json", track.as_str()))
    }
    /// Ventana de audio temporal que se transcribe en vivo (o la cola al parar).
    pub fn live_window_wav(&self, id: MeetingId, track: Track) -> PathBuf {
        self.meeting_dir(id).join("live").join(format!("win-{}.wav", track.as_str()))
    }
    /// Caché del pipeline: segmentos ASR de una pista (`Vec<AsrSegment>`).
    pub fn asr_cache_json(&self, id: MeetingId, track: Track) -> PathBuf {
        self.transcript_dir(id)
            .join(format!("segments-{}.json", track.as_str()))
    }
    /// Caché del pipeline: limpieza LLM del transcript (texto crudo → limpio).
    pub fn cleanup_cache_json(&self, id: MeetingId) -> PathBuf {
        self.transcript_dir(id).join("cleanup.json")
    }
    /// Caché del pipeline: `DiarizationResult` de la pista system.
    pub fn diarization_cache_json(&self, id: MeetingId) -> PathBuf {
        self.transcript_dir(id).join("diarization-result.json")
    }
    /// Salida completa del LLM (`MinutesOutput`), para podar capturas y reprocesar.
    pub fn minutes_json(&self, id: MeetingId) -> PathBuf {
        self.llm_dir(id).join("minutes.json")
    }
    /// Ruta relativa (a la carpeta de la reunión) de una captura conservada:
    /// `captures/<media_ms 9 dígitos>-<reason>.jpg`.
    pub fn capture_rel_path(media_ms: u64, reason_wire: &str) -> String {
        format!("captures/{media_ms:09}-{reason_wire}.jpg")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segment_paths_are_zero_padded_and_tmp_suffixed() {
        let l = DataLayout::new("/data");
        let id = MeetingId::from_u128(1);
        let p = l.segment_path(id, Track::Mic, 7);
        assert!(p.ends_with("audio/mic/seg-000007.wav"));
        let t = l.segment_tmp_path(id, Track::System, 7);
        assert!(t.ends_with("audio/system/seg-000007.wav.tmp"));
    }

    #[test]
    fn app_icons_live_in_a_cache_folder_inside_the_data_dir() {
        assert_eq!(DataLayout::new("/data").app_icons_dir(), PathBuf::from("/data/cache/app-icons"));
    }

    #[test]
    fn capture_rel_path_is_media_ms_zero_padded() {
        assert_eq!(DataLayout::capture_rel_path(123_456, "slide_change"), "captures/000123456-slide_change.jpg");
    }
}
