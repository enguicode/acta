use crate::ids::{MeetingId, SpeakerId, VoiceprintId};
use crate::recording::Track;
use crate::time::{MediaMs, UnixMs};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Segmento ASR crudo (salida de Parakeet). Tiempos relativos al WAV de entrada.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AsrSegment {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
    pub confidence: Option<f32>,
    pub words: Vec<AsrWord>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AsrWord {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscribeRequest {
    pub wav_path: PathBuf,
    pub track: Track,
    /// BCP-47 ("es", "en"). `None` = autodetección.
    pub language: Option<String>,
}

/// Un turno de diarización de la pista `system`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiarizationTurn {
    /// Etiqueta local del diarizador ("S0", "S1"...).
    pub label: String,
    pub start_ms: u64,
    pub end_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiarizedSpeaker {
    pub label: String,
    /// Embedding medio del hablante (para casar con huellas de voz).
    pub embedding: Vec<f32>,
    pub total_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiarizationResult {
    pub turns: Vec<DiarizationTurn>,
    pub speakers: Vec<DiarizedSpeaker>,
    /// Id del modelo de embedding (las huellas solo se comparan con el mismo).
    pub embedding_model: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiarizeOptions {
    pub num_speakers: Option<u8>,
    pub min_speakers: Option<u8>,
    pub max_speakers: Option<u8>,
}

/// De dónde salió el nombre de un hablante.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NameSource {
    /// "Yo": la pista mic.
    Me,
    /// Lo escribió/confirmó el usuario.
    User,
    /// Hablante activo/roster de Teams vía Accessibility.
    AppUi,
    /// Huella de voz guardada.
    Voiceprint,
    /// Deducido de lo dicho en la reunión ("Hola, soy Marta", "gracias, Marta").
    Transcript,
    /// Sin nombre: "Speaker N".
    Generic,
}

/// Hablante dentro de UNA reunión.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Speaker {
    pub id: SpeakerId,
    pub meeting_id: MeetingId,
    /// "Yo", "Speaker 1", "Speaker 2"...
    pub label: String,
    /// Nombre real si se conoce ("Ana García").
    pub display_name: Option<String>,
    pub is_me: bool,
    pub track: Track,
    /// Etiqueta del diarizador ("S0") para la pista system.
    pub diarization_label: Option<String>,
    pub name_source: NameSource,
    /// El usuario confirmó el nombre (solo entonces se aprende la huella).
    pub confirmed: bool,
    pub voiceprint_id: Option<VoiceprintId>,
}

impl Speaker {
    pub fn shown_name(&self) -> &str {
        self.display_name.as_deref().unwrap_or(&self.label)
    }
}

/// Frase final del transcript, ya con hablante y en tiempo media.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Utterance {
    /// Índice estable dentro de la reunión (orden temporal).
    pub idx: u32,
    pub speaker_id: SpeakerId,
    pub track: Track,
    pub start: MediaMs,
    pub end: MediaMs,
    pub text: String,
    pub confidence: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Transcript {
    pub meeting_id: MeetingId,
    pub speakers: Vec<Speaker>,
    pub utterances: Vec<Utterance>,
}

/// Huella de voz: SOLO el embedding (dato biométrico; borrable).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Voiceprint {
    pub id: VoiceprintId,
    pub display_name: String,
    pub embedding: Vec<f32>,
    pub model: String,
    pub samples: u32,
    pub created_at: UnixMs,
    pub updated_at: UnixMs,
}

/// Vista para la UI (sin el vector).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceprintSummary {
    pub id: VoiceprintId,
    pub display_name: String,
    pub samples: u32,
    pub meetings: u32,
    pub updated_at: UnixMs,
}

/// Texto parcial de la previsualización en vivo (opcional).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LivePartial {
    pub track: Track,
    pub text: String,
    pub at: UnixMs,
    pub is_final: bool,
}

/// Lo que recibe un `SpeakerNamer`: el transcript ya fusionado y las pistas
/// de quién es quién. Solo se piden nombres para `targets` (los "Speaker N").
#[derive(Debug, Clone, PartialEq)]
pub struct SpeakerNamingInput {
    pub title: String,
    /// Nombre del usuario en Ajustes (sus líneas salen como "Ana (yo)").
    pub me_name: String,
    /// Nombres que son el usuario (Ajustes + alias + roster `is_self`): nunca
    /// se le ponen a otro hablante.
    pub self_names: Vec<String>,
    /// Nombres conocidos de la reunión (roster de la app), como pista.
    pub participants: Vec<String>,
    pub speakers: Vec<Speaker>,
    pub utterances: Vec<Utterance>,
    /// Hablantes sin nombre a los que buscarle uno.
    pub targets: Vec<SpeakerId>,
}

/// Nombre propuesto para un hablante (sin confirmar) y por qué.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpeakerNameGuess {
    pub speaker_id: SpeakerId,
    pub name: String,
    pub evidence: String,
}

/// Lo que recibe un `TranscriptCleaner`: los textos de las intervenciones, en
/// orden, y las pistas para corregirlos.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanupInput {
    /// Un texto por intervención; el resultado conserva el orden y el número.
    pub texts: Vec<String>,
    /// Jerga y nombres propios del usuario (`transcription.glossary`).
    pub glossary: Vec<String>,
    /// `"auto"` o código ISO 639-1: el idioma en que se habló (no se traduce).
    pub language: String,
}
