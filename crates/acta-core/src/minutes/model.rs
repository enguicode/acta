use crate::detection::Platform;
use crate::ids::{ActionItemId, CaptureId, DecisionId, MeetingId, ProviderId, SpeakerId};
use crate::recording::Track;
use crate::screen::CaptureReason;
use crate::time::{MediaMs, UnixMs};
use crate::transcription::{Speaker, Utterance};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Decision {
    pub id: DecisionId,
    pub text: String,
    pub at: Option<MediaMs>,
}

/// Tarea. `is_mine` = "en tu tejado" (responsable = el usuario).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionItem {
    pub id: ActionItemId,
    pub meeting_id: MeetingId,
    pub text: String,
    /// Nombre del responsable tal como lo dio el LLM.
    pub owner_name: Option<String>,
    pub owner_speaker_id: Option<SpeakerId>,
    pub is_mine: bool,
    /// Fecha límite en texto libre o ISO-8601 ("2026-11-20", "el viernes").
    pub due: Option<String>,
    pub done: bool,
    /// Momento en que se acordó.
    pub at: Option<MediaMs>,
    pub capture_ids: Vec<CaptureId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Minutes {
    pub meeting_id: MeetingId,
    /// Markdown en castellano.
    pub summary_md: String,
    pub decisions: Vec<Decision>,
    pub action_items: Vec<ActionItem>,
    pub provider: ProviderId,
    pub model: Option<String>,
    pub generated_at: UnixMs,
}

/// Escaleta. EXCEPCIÓN de nombres: se serializa con las claves en
/// castellano snake_case EXACTAMENTE como la devuelve el LLM y la pinta la UI:
/// `{ "temas": [ { "titulo", "resumen", "inicio_ms", "fin_ms", "subtemas", "capturas" } ] }`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Escaleta {
    pub temas: Vec<Tema>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tema {
    pub titulo: String,
    pub resumen: String,
    pub inicio_ms: u64,
    pub fin_ms: u64,
    #[serde(default)]
    pub subtemas: Vec<Tema>,
    /// Ids de captura vinculadas (en la respuesta del LLM son refs "c1".."cN"
    /// y acta-llm las traduce a `CaptureId` antes de guardar).
    #[serde(default)]
    pub capturas: Vec<String>,
}

/// Momento clave elegido por el LLM (para promover capturas candidatas).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyMoment {
    pub at: MediaMs,
    pub reason: String,
    /// Título del tema de la escaleta al que pertenece.
    pub topic: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TimelineKind {
    Topic,
    Decision,
    ActionItem,
    Capture,
    ScreenShare,
}

/// Evento de la línea de tiempo. Lo DERIVA el store a partir de escaleta,
/// decisiones, tareas y capturas (no lo genera el LLM).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimelineEvent {
    pub at: MediaMs,
    pub end: Option<MediaMs>,
    pub kind: TimelineKind,
    pub label: String,
    /// Id del objeto referido (tarea, captura...) como string.
    pub ref_id: Option<String>,
}

/// Identidad del usuario para "mis tareas".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserIdentity {
    /// Vacío hasta que el usuario lo pone (bienvenida o Ajustes): los
    /// prompts y «mis tareas» usan entonces solo «Yo».
    pub display_name: String,
    /// Otras formas de nombrarle ("Ana María", "Ana García").
    pub aliases: Vec<String>,
}

/// Captura que se ofrece al LLM, con ref corta.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureRef {
    /// "c1", "c2"...
    pub ref_id: String,
    pub capture_id: CaptureId,
    pub at: MediaMs,
    pub reason: CaptureReason,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MinutesInput {
    pub meeting_id: MeetingId,
    pub title: String,
    pub platform: Platform,
    pub started_at: UnixMs,
    pub duration: MediaMs,
    pub me: UserIdentity,
    pub speakers: Vec<Speaker>,
    pub utterances: Vec<Utterance>,
    pub captures: Vec<CaptureRef>,
    /// Idioma de salida: `"auto"` (el que se habla en la reunión) o ISO 639-1 ("es").
    pub language: String,
    /// Nota personal del usuario sobre la reunión: contexto privado, no la
    /// dijo nadie.
    #[serde(default)]
    pub user_note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MinutesOutput {
    pub minutes: Minutes,
    pub escaleta: Escaleta,
    pub key_moments: Vec<KeyMoment>,
    /// Título sugerido si la reunión no tenía uno bueno.
    pub suggested_title: Option<String>,
}

/// Coincidencia del detector de momentos importantes (en vivo).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeywordHit {
    pub keyword: String,
    pub track: Track,
    pub at: UnixMs,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupName {
    pub name: String,
    pub description: String,
}
