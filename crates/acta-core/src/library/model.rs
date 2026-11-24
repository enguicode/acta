use crate::detection::Platform;
use crate::ids::{GroupId, MeetingId};
use crate::minutes::{Escaleta, Minutes, TimelineEvent};
use crate::screen::Capture;
use crate::time::{MediaMs, UnixMs};
use crate::transcription::Transcript;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MeetingStatus {
    Recording,
    Processing,
    Ready,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Meeting {
    pub id: MeetingId,
    pub title: String,
    pub platform: Platform,
    pub app_bundle_id: Option<String>,
    pub started_at: UnixMs,
    pub ended_at: Option<UnixMs>,
    /// Duración del audio grabado (sin pausas).
    pub duration: MediaMs,
    pub status: MeetingStatus,
    /// Se recuperó tras un cierre inesperado.
    pub recovered: bool,
    pub error: Option<String>,
    pub language: String,
    pub created_at: UnixMs,
    pub updated_at: UnixMs,
    /// Marcada con la estrella. Las favoritas nunca se sugieren para borrar.
    #[serde(default)]
    pub favourite: bool,
    /// Si falló el resumen: qué proveedor y modelo fallaron (también van en `error`).
    #[serde(default)]
    pub error_provider: Option<String>,
    #[serde(default)]
    pub error_model: Option<String>,
    /// Nota personal del usuario (nota para mí). Privada: es contexto para
    /// el LLM y sale en el acta en su propia sección, sin atribuirla a nadie.
    #[serde(default)]
    pub user_note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewMeeting {
    pub id: MeetingId,
    pub title: String,
    pub platform: Platform,
    pub app_bundle_id: Option<String>,
    pub started_at: UnixMs,
    pub language: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingPatch {
    pub title: Option<String>,
    pub ended_at: Option<UnixMs>,
    pub duration: Option<MediaMs>,
    pub status: Option<MeetingStatus>,
    pub recovered: Option<bool>,
    /// `Some(None)` borra el error.
    pub error: Option<Option<String>>,
    /// `Some(None)` borra la nota.
    pub user_note: Option<Option<String>>,
}

/// Fila de la lista de reuniones.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingSummary {
    pub id: MeetingId,
    pub title: String,
    pub platform: Platform,
    pub started_at: UnixMs,
    pub duration: MediaMs,
    pub status: MeetingStatus,
    pub speaker_names: Vec<String>,
    /// Tareas mías sin hacer.
    pub my_open_tasks: u32,
    pub group_ids: Vec<GroupId>,
    pub capture_count: u32,
    /// Fragmento con la coincidencia si se listó con texto de búsqueda.
    pub snippet: Option<String>,
    /// Estrella de la reunión (no cuenta la de sus grupos).
    #[serde(default)]
    pub favourite: bool,
}

/// Rutas de audio (absolutas) para reproducir con `convertFileSrc`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingAudio {
    pub mic: Option<String>,
    pub system: Option<String>,
    pub mix: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingDetail {
    pub meeting: Meeting,
    pub transcript: Transcript,
    pub minutes: Option<Minutes>,
    pub escaleta: Option<Escaleta>,
    pub captures: Vec<Capture>,
    pub timeline: Vec<TimelineEvent>,
    pub groups: Vec<Group>,
    pub audio: MeetingAudio,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingQuery {
    /// Búsqueda FTS5 en títulos y transcript.
    pub text: Option<String>,
    pub group_id: Option<GroupId>,
    pub platform: Option<Platform>,
    pub from: Option<UnixMs>,
    pub to: Option<UnixMs>,
    /// Solo favoritas: reuniones con estrella y reuniones de grupos con estrella.
    #[serde(default)]
    pub favourites_only: bool,
    /// Por defecto 50.
    pub limit: Option<u32>,
    /// Cursor opaco devuelto por la página anterior.
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GroupKind {
    Manual,
    /// Venía de una sugerencia aceptada.
    Suggested,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Group {
    pub id: GroupId,
    pub name: String,
    pub description: Option<String>,
    pub kind: GroupKind,
    pub meeting_count: u32,
    pub created_at: UnixMs,
    pub updated_at: UnixMs,
    /// Grupo con estrella: sus reuniones cuentan como favoritas.
    #[serde(default)]
    pub favourite: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupSuggestion {
    /// Id provisional (se reutiliza como id del grupo si se acepta).
    pub id: GroupId,
    pub name: Option<String>,
    pub description: Option<String>,
    pub meeting_ids: Vec<MeetingId>,
    /// Cohesión media del cluster (similitud coseno 0..1).
    pub cohesion: f32,
    pub created_at: UnixMs,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum SearchScope {
    All,
    /// Reuniones con estrella y reuniones de grupos con estrella.
    Favourites,
    #[serde(rename_all = "camelCase")]
    Group { group_id: GroupId },
    #[serde(rename_all = "camelCase")]
    Meeting { meeting_id: MeetingId },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchQuery {
    pub text: String,
    pub scope: SearchScope,
    pub limit: u32,
}

/// Resultado de búsqueda híbrida (BM25 + vectores, fusión RRF).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub meeting_id: MeetingId,
    pub meeting_title: String,
    pub started_at: UnixMs,
    pub at: MediaMs,
    pub end: MediaMs,
    pub speaker: Option<String>,
    pub snippet: String,
    pub score: f32,
}

/// Unidad de indexado para RAG: ventana de utterances (≈ 45-60 s / ≤ 300 tokens).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Chunk {
    pub id: i64,
    pub meeting_id: MeetingId,
    pub start: MediaMs,
    pub end: MediaMs,
    /// "Nombre: texto" por línea.
    pub text: String,
}

/// Para qué se calcula un embedding (e5 usa prefijos "query: "/"passage: ").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EmbedKind {
    Query,
    Passage,
}
