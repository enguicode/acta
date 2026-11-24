use crate::detection::MeetingApp;
use crate::ids::MeetingId;
use crate::time::{MediaMs, UnixMs};
use serde::{Deserialize, Serialize};

/// Las dos pistas SIEMPRE separadas: `Mic` = "Yo" (el usuario), `System` = los demás.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Track {
    Mic,
    System,
}

impl Track {
    pub const ALL: [Track; 2] = [Track::Mic, Track::System];

    pub fn as_str(self) -> &'static str {
        match self {
            Track::Mic => "mic",
            Track::System => "system",
        }
    }
}

/// Formato de audio de trabajo: 16 kHz, mono, PCM s16le. Es lo que entrega el
/// sidecar, lo que escribe el `SegmentWriter` y lo que consume el ASR.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioFormat {
    pub sample_rate: u32,
    pub channels: u16,
}

impl AudioFormat {
    pub const WORKING: AudioFormat = AudioFormat {
        sample_rate: 16_000,
        channels: 1,
    };

    pub fn samples_for_ms(&self, ms: u64) -> u64 {
        ms * self.sample_rate as u64 / 1000
    }
}

/// Un bloque de PCM de una pista (≈100 ms).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioFrame {
    pub track: Track,
    /// Secuencia por pista, empieza en 0. Un salto = frames perdidos.
    pub seq: u64,
    /// Instante del primer sample, en µs desde `capture.start` (reloj host
    /// común a ambas pistas). El writer usa esto para alinear y rellenar
    /// huecos con silencio.
    pub pts_us: u64,
    pub sample_rate: u32,
    /// Mono s16.
    pub samples: Vec<i16>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TrackState {
    Running,
    Stalled,
    Restarting,
    Stopped,
    Failed,
}

/// Lo que emite un `AudioSource` mientras captura.
#[derive(Debug, Clone, PartialEq)]
pub enum AudioEvent {
    Frame(AudioFrame),
    DeviceChanged {
        track: Track,
        device_name: Option<String>,
        reason: String,
    },
    Overrun {
        track: Track,
        dropped_frames: u64,
    },
    TrackState {
        track: Track,
        state: TrackState,
        message: Option<String>,
    },
    /// El adaptador reinició la captura (p. ej. el sidecar murió). Los `pts_us`
    /// posteriores son relativos al nuevo arranque `restarted_at`.
    Restarted { restarted_at: UnixMs },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MicTarget {
    pub enabled: bool,
    /// `None` = dispositivo de entrada por defecto (y seguirlo si cambia).
    pub device_uid: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemTarget {
    pub enabled: bool,
    /// PIDs para el process tap. Vacío + `bundle_ids` vacío = tap global
    /// de toda la salida (fallback, menos limpio).
    pub pids: Vec<i32>,
    pub bundle_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureRequest {
    pub meeting_id: MeetingId,
    pub format: AudioFormat,
    pub frame_ms: u32,
    pub mic: MicTarget,
    pub system: SystemTarget,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureStarted {
    /// Reloj de pared correspondiente a `pts_us = 0`.
    pub started_at: UnixMs,
    pub mic_device: Option<String>,
    pub system_pids: Vec<i32>,
}

/// Por qué se paró la grabación.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StopReason {
    /// Botón parar (pastilla o ventana principal).
    User,
    /// El usuario respondió "Sí" a "La llamada ha terminado — ¿Parar?".
    MeetingEndedConfirmed,
    /// Pregunta sin responder + silencio en ambas pistas N minutos.
    AutoSilence,
    /// Tope duro de duración.
    MaxDuration,
    /// La app se cierra.
    AppQuit,
    /// Error irrecuperable de captura: se para y se procesa lo que haya.
    Error,
    /// Parada dentro de la ventana de descarte de una grabación automática:
    /// se borra la reunión y sus archivos sin procesar.
    Discarded,
}

// ---------------------------------------------------------------------------
// Manifiesto de sesión (meetings/<id>/manifest.json)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ManifestStatus {
    Recording,
    Paused,
    /// La captura terminó y todos los segmentos están cerrados.
    Stopped,
    /// Concatenación hecha (`audio/mic.wav`, `audio/system.wav`).
    Finalized,
}

/// Un tramo continuo de grabación (entre arranque/reanudación y pausa/parada).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingSpan {
    pub wall_start: UnixMs,
    pub wall_end: Option<UnixMs>,
    /// Posición en la línea media donde empieza el tramo.
    pub media_start: MediaMs,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SegmentInfo {
    pub index: u32,
    /// Ruta relativa a la carpeta de la reunión, p. ej. `audio/mic/seg-000003.wav`.
    pub file: String,
    pub media_start: MediaMs,
    pub samples: u64,
    /// Samples de silencio insertados para tapar huecos en este segmento.
    pub padded_samples: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackManifest {
    pub track: Track,
    /// Solo segmentos FINALIZADOS (renombrados de `.tmp`).
    pub segments: Vec<SegmentInfo>,
}

/// Manifiesto recuperable. Se reescribe de forma atómica (tmp + fsync +
/// rename) cada vez que se finaliza un segmento o cambia el estado.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionManifest {
    pub version: u32,
    pub meeting_id: MeetingId,
    pub status: ManifestStatus,
    pub created_at: UnixMs,
    pub updated_at: UnixMs,
    pub app: Option<MeetingApp>,
    pub format: AudioFormat,
    pub segment_seconds: u32,
    pub spans: Vec<RecordingSpan>,
    pub tracks: Vec<TrackManifest>,
    pub stop_reason: Option<StopReason>,
}

impl SessionManifest {
    pub const VERSION: u32 = 1;

    /// Convierte un instante de pared a posición media (descontando pausas).
    /// `None` si cae fuera de cualquier tramo grabado (p. ej. en una pausa).
    pub fn wall_to_media(&self, t: UnixMs) -> Option<MediaMs> {
        for span in &self.spans {
            let end = span.wall_end.unwrap_or(UnixMs(i64::MAX));
            if t >= span.wall_start && t <= end {
                let delta = t.since(span.wall_start).max(0) as u64;
                return Some(MediaMs(span.media_start.0 + delta));
            }
        }
        None
    }
}

// ---------------------------------------------------------------------------
// Niveles (medidores de la pastilla + autoparada)
// ---------------------------------------------------------------------------

/// Nivel de una pista en una ventana (~100 ms). dBFS: 0 = máximo, -inf = silencio.
/// Silencio digital exacto (todo ceros) se marca con `all_zero` porque es la
/// firma de un permiso TCC denegado.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Level {
    pub rms_dbfs: f32,
    pub peak_dbfs: f32,
    pub all_zero: bool,
}

impl Level {
    pub const SILENT: Level = Level {
        rms_dbfs: -120.0,
        peak_dbfs: -120.0,
        all_zero: true,
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Levels {
    pub mic: Level,
    pub system: Level,
    pub at: UnixMs,
}
