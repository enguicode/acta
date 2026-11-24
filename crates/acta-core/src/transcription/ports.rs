use super::model::*;
use crate::recording::Track;
use crate::{EventStream, PortResult};
use async_trait::async_trait;
use std::path::Path;
use std::sync::Arc;

/// Progreso 0.0..=1.0 de un trabajo largo.
pub type ProgressFn = Arc<dyn Fn(f32) + Send + Sync>;

/// ASR offline. macOS: FluidAudio Parakeet TDT v3 en el sidecar.
/// Windows (después): whisper-rs o Parakeet ONNX.
#[async_trait]
pub trait Transcriber: Send + Sync {
    async fn transcribe(
        &self,
        request: TranscribeRequest,
        progress: Option<ProgressFn>,
    ) -> PortResult<Vec<AsrSegment>>;
}

/// Diarización offline (solo pista system). macOS: FluidAudio (pyannote community-1).
#[async_trait]
pub trait Diarizer: Send + Sync {
    async fn diarize(
        &self,
        wav_path: &Path,
        options: DiarizeOptions,
        progress: Option<ProgressFn>,
    ) -> PortResult<DiarizationResult>;
}

/// Previsualización en vivo (opcional; alimenta el detector de momentos
/// importantes). macOS: FluidAudio streaming en el sidecar.
#[async_trait]
pub trait LiveTranscriber: Send + Sync {
    async fn start(&self, tracks: &[Track], language: Option<String>) -> PortResult<EventStream<LivePartial>>;
    async fn stop(&self) -> PortResult<()>;
}

/// Propone nombres para los hablantes genéricos a partir de lo dicho en la
/// reunión (presentaciones, vocativos). Impl: acta-llm. Solo devuelve
/// nombres con evidencia; los dudosos se omiten.
#[async_trait]
pub trait SpeakerNamer: Send + Sync {
    async fn name_speakers(&self, input: SpeakerNamingInput) -> PortResult<Vec<SpeakerNameGuess>>;
}

/// Limpia el texto del ASR (jerga mal oída, puntuación, mayúsculas) sin tocar
/// quién habla, cuándo ni cuántas intervenciones hay. Impl: acta-llm.
///
/// Devuelve EXACTAMENTE un elemento por texto de entrada: `Some(texto limpio)`
/// o `None` si esa tanda falló (el que llama conserva el texto crudo y NO
/// guarda nada en caché, para reintentarlo en el siguiente procesado).
#[async_trait]
pub trait TranscriptCleaner: Send + Sync {
    async fn clean(&self, input: CleanupInput) -> Vec<Option<String>>;
    /// Versión del prompt/reglas: entra en la clave de caché, así que
    /// cambiarla invalida lo ya limpiado.
    fn version(&self) -> String {
        "1".into()
    }
}
