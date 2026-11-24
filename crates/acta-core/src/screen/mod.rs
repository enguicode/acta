//! Capturas de pantalla compartida (opt-in; requiere permiso de Grabación
//! de pantalla). Ver docs/01-arquitectura.md §8.

use crate::detection::MeetingApp;
use crate::ids::{ActionItemId, CaptureId, MeetingId};
use crate::time::{MediaMs, UnixMs};
use crate::{EventStream, PortResult};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CaptureReason {
    /// Cambio de diapositiva (diferencia de hash perceptual).
    SlideChange,
    /// Palabra clave en el transcript en vivo ("importante", "deadline"...).
    Keyword,
    /// El LLM eligió este instante tras la reunión.
    LlmPick,
    /// Botón de la pastilla.
    Manual,
    /// Candidata periódica (se poda si nadie la promueve).
    Candidate,
}

/// Captura guardada y vinculada a la reunión.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Capture {
    pub id: CaptureId,
    pub meeting_id: MeetingId,
    pub at: MediaMs,
    pub at_wall: UnixMs,
    /// Ruta relativa a la carpeta de la reunión (`captures/000123456-slide_change.jpg`).
    pub path: String,
    pub reason: CaptureReason,
    /// dHash 64 bits en hex (16 chars).
    pub phash: String,
    pub width: u32,
    pub height: u32,
    pub keyword: Option<String>,
    /// Ruta del tema en la escaleta ("2" o "2.1"), si está vinculada.
    pub topic_path: Option<String>,
    pub action_item_ids: Vec<ActionItemId>,
}

/// Tramo en el que alguien compartió pantalla (se guarda para el timeline).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenShare {
    pub start: MediaMs,
    /// `None` = seguía compartiendo al parar.
    pub end: Option<MediaMs>,
    pub presenter: Option<String>,
}

/// Fichero que reporta el capturador (antes de vincularse a la reunión).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapturedFrame {
    pub path: PathBuf,
    pub at: UnixMs,
    pub reason: CaptureReason,
    pub phash: String,
    pub width: u32,
    pub height: u32,
    pub keyword: Option<String>,
    /// Miniatura gris 64×36 en hex (4608 chars) para el filtro de capturas;
    /// no se guarda. `None` con sidecars viejos.
    #[serde(default)]
    pub thumb: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ScreenEvent {
    ShareStarted { presenter: Option<String>, at: UnixMs },
    ShareEnded { at: UnixMs },
    FrameSaved { frame: CapturedFrame },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenCaptureRequest {
    pub meeting_id: MeetingId,
    /// Ventana de la app de la reunión. `None` (grabación manual sin app
    /// detectada) = la pantalla principal, sin las ventanas de acta.
    pub app: Option<MeetingApp>,
    /// `captures/` de la reunión.
    pub out_dir: PathBuf,
    /// `captures/candidates/` de la reunión.
    pub candidates_dir: PathBuf,
    /// Muestreo (≈1.0 fps).
    pub fps: f32,
    /// Distancia de Hamming mínima (sobre 64) para considerar cambio de diapositiva.
    pub phash_threshold: u32,
    /// Frames consecutivos estables antes de guardar (evita transiciones).
    pub stable_frames: u32,
    /// Guardar una candidata cada N segundos durante la compartición.
    pub candidate_every_s: u32,
    /// Segundos de frames en memoria para `snapshot` retroactivo.
    pub ring_seconds: u32,
    /// Ancho máximo de la imagen guardada (px).
    pub max_width: u32,
}

/// Capturador de pantalla compartida. macOS: ScreenCaptureKit en el sidecar.
#[async_trait]
pub trait ScreenCapturer: Send + Sync {
    async fn start(&self, request: ScreenCaptureRequest) -> PortResult<EventStream<ScreenEvent>>;
    /// Guarda ya el frame más cercano a `at` (del anillo en memoria).
    async fn snapshot(
        &self,
        reason: CaptureReason,
        at: UnixMs,
        keyword: Option<String>,
    ) -> PortResult<Option<CapturedFrame>>;
    async fn stop(&self) -> PortResult<()>;
}

/// Imágenes que el usuario copia al portapapeles durante la grabación (su
/// propia herramienta de recortes, p. ej. Cmd+Ctrl+Mayús+4). Cada imagen
/// nueva se guarda como JPEG en `out_dir` y se anuncia como `Manual`.
/// No necesita el permiso de Grabación de pantalla.
#[async_trait]
pub trait ClipboardImporter: Send + Sync {
    /// Empieza a vigilar. Lo que ya estaba en el portapapeles NO se importa.
    async fn start(&self, out_dir: PathBuf, max_width: u32) -> PortResult<EventStream<CapturedFrame>>;
    async fn stop(&self) -> PortResult<()>;
}
