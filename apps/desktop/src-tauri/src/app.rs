//! `AppCore`: la lógica de todos los comandos, sin tipos de Tauri (se prueba
//! con el store real en un directorio temporal y adaptadores falsos).

use crate::llm::{KeychainResolver, build_provider};
use crate::native::NativePorts;
use crate::pipeline::{DynamicPipeline, LlmOverrides, SharedSettings, read};
use acta_audio::processing::voiceprints::rename_speaker;
use acta_audio::supervisor::{SupervisorConfig, SupervisorHandle, SupervisorPorts, spawn_with};
use acta_core::api::*;
use acta_core::chat::*;
use acta_core::detection::MeetingApp;
use acta_core::ids::*;
use acta_core::layout::DataLayout;
use acta_core::library::*;
use acta_core::llm::local::{HardwareInfo, LocalLlmDownload, LocalLlmStatus, ModelFit};
use acta_core::llm::{LlmProviderFactory, ProviderTestResult, SecretRef};
use acta_llm::local::{BuiltInProvider, LocalLlm, LocalLlmConfig};
use acta_core::minutes::GroupNamer;
use acta_core::permissions::{PermissionKind, PermissionState, PermissionStatus};
use acta_core::processing::{LlmOverride, ProcessingStep, ReprocessRequest};
use acta_core::recording::{HealthWarning, PromptAnswer, SessionEvent, SessionState, StopReason};
use acta_core::screen::Capture;
use acta_core::settings::Settings;
use acta_core::time::Clock;
use acta_core::transcription::VoiceprintSummary;
use acta_core::{PortError, PortResult};
use acta_llm::chat::LlmChatEngine;
use acta_llm::moments::KeywordMomentDetector;
use acta_llm::naming::LlmGroupNamer;
use acta_llm::providers::detect::detect_cli_any;
use acta_llm::providers::{DefaultProviderFactory, test_provider};
use acta_store::SqliteStore;
use acta_store::secrets::{SecretStore, secret_ref_for_provider};
use futures::StreamExt;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use tokio::sync::oneshot;

/// Nombre de un grupo creado sin nombre; se cambia solo por uno sacado de sus reuniones.
pub const PLACEHOLDER_GROUP: &str = "Nuevo grupo";

/// Respuestas en curso que «Detener» puede cortar (`chat_cancel`, `live_cancel`).
pub type Cancels = Arc<Mutex<HashMap<String, oneshot::Sender<()>>>>;

/// Publica eventos hacia la UI (Tauri `emit` en la app, un vector en tests).
pub trait Emit: Send + Sync {
    fn emit(&self, event: &str, payload: serde_json::Value);
}

pub struct AppCore {
    pub layout: DataLayout,
    pub clock: Arc<dyn Clock>,
    pub store: Arc<SqliteStore>,
    pub secrets: Arc<dyn SecretStore>,
    pub factory: Arc<dyn LlmProviderFactory>,
    pub settings: SharedSettings,
    pub supervisor: SupervisorHandle,
    pub native: NativePorts,
    pub emitter: Arc<dyn Emit>,
    /// Shell para detectar los CLI (`/bin/zsh`) y `$HOME`.
    pub shell: PathBuf,
    pub home: PathBuf,
    /// Modelo local integrado (descargas + `llama-server` supervisado).
    pub local_llm: Arc<LocalLlm>,
    /// Hardware fijo en vez del de este Mac (pruebas).
    pub hardware_override: Option<HardwareInfo>,
    /// Proveedor/modelo de un «Reintentar con…» pendiente.
    pub llm_overrides: LlmOverrides,
    pub cancels: Cancels,
    /// Transcripción en vivo de cada grabación (de esta ejecución).
    pub live_sessions: acta_audio::supervisor::LiveSessions,
}

pub struct CoreDeps {
    pub layout: DataLayout,
    pub clock: Arc<dyn Clock>,
    pub store: Arc<SqliteStore>,
    pub secrets: Arc<dyn SecretStore>,
    /// `None` = `DefaultProviderFactory` con el Llavero.
    pub factory: Option<Arc<dyn LlmProviderFactory>>,
    /// `None` = el real, en la carpeta de datos.
    pub local_llm: Option<Arc<LocalLlm>>,
    pub native: NativePorts,
    pub emitter: Arc<dyn Emit>,
    pub supervisor_config: SupervisorConfig,
}

fn to_json<T: serde::Serialize>(v: &T) -> serde_json::Value {
    serde_json::to_value(v).unwrap_or(serde_json::Value::Null)
}

impl AppCore {
    /// Monta todo y arranca el supervisor (recuperación incluida).
    pub async fn start(deps: CoreDeps) -> PortResult<Self> {
        use acta_core::library::SettingsStore;
        let settings = deps.store.load_settings().await?;
        let shared: SharedSettings = Arc::new(RwLock::new(settings.clone()));
        let local_llm = deps.local_llm.unwrap_or_else(|| Arc::new(LocalLlm::new(LocalLlmConfig::for_layout(&deps.layout))));
        // Un cierre brusco pudo dejar un llama-server huérfano ocupando memoria.
        local_llm.kill_stale().await;
        let factory = deps.factory.unwrap_or_else(|| {
            Arc::new(
                DefaultProviderFactory::new(Arc::new(KeychainResolver(deps.secrets.clone()))).with_local(local_llm.clone()),
            )
        });
        let llm_overrides: LlmOverrides = Arc::default();
        let pipeline = DynamicPipeline {
            overrides: llm_overrides.clone(),
            errors: Some(deps.store.clone()),
            layout: deps.layout.clone(),
            settings: shared.clone(),
            store: deps.store.clone(),
            voiceprints: deps.store.clone(),
            search: deps.store.clone(),
            transcriber: deps.native.transcriber.clone(),
            diarizer: deps.native.diarizer.clone(),
            factory: factory.clone(),
            clock: deps.clock.clone(),
        };
        let layout_root = deps.layout.root().to_path_buf();
        let live_sessions: acta_audio::supervisor::LiveSessions = Arc::default();
        let ports = SupervisorPorts {
            layout: deps.layout.clone(),
            clock: deps.clock.clone(),
            audio: deps.native.audio.clone(),
            detector: deps.native.detector.clone(),
            speakers: deps.native.speakers.clone(),
            screen: deps.native.screen.clone(),
            clipboard: deps.native.clipboard.clone(),
            live: deps.native.live.clone(),
            live_asr: Some(deps.native.transcriber.clone()),
            live_sessions: live_sessions.clone(),
            moments: Arc::new(KeywordMomentDetector::with_defaults(&settings.screen.keywords)),
            store: deps.store.clone(),
            pipeline: Arc::new(pipeline),
            disk_free: Arc::new(move |p| crate::system::disk_free(if p.exists() { p } else { &layout_root })),
        };
        let supervisor = spawn_with(ports, settings, deps.supervisor_config).await?;
        Ok(Self {
            layout: deps.layout,
            clock: deps.clock,
            store: deps.store,
            secrets: deps.secrets,
            factory,
            settings: shared,
            supervisor,
            native: deps.native,
            emitter: deps.emitter,
            shell: PathBuf::from("/bin/zsh"),
            home: dirs::home_dir().unwrap_or_else(|| PathBuf::from("/")),
            local_llm,
            hardware_override: None,
            llm_overrides,
            cancels: Arc::default(),
            live_sessions,
        })
    }

    pub fn settings(&self) -> Settings {
        read(&self.settings)
    }

    fn emit<T: serde::Serialize>(&self, name: &str, payload: &T) {
        self.emitter.emit(name, to_json(payload));
    }

    // ------------------------------------------------------------------
    // Sesión
    // ------------------------------------------------------------------

    pub async fn session_get_state(&self) -> PortResult<SessionSnapshot> {
        self.supervisor.snapshot().await
    }

    async fn session(&self, event: SessionEvent) -> PortResult<SessionSnapshot> {
        self.supervisor.send(event).await
    }

    /// Micro y audio del sistema resueltos ANTES de empezar. Tras reinstalar
    /// (cdhash nuevo) macOS vuelve a preguntar; si la captura arranca con el
    /// diálogo abierto, la HAL se queda bloqueada, la pastilla dice «Grabando
    /// 00:00» y la reunión sale vacía. Aquí se pregunta primero y, si el
    /// usuario dice que no, no se crea ninguna reunión.
    async fn ensure_capture_permissions(&self) -> PortResult<()> {
        // Sin sidecar no hay a quién preguntar: que lo explique la captura.
        let Ok(states) = self.native.permissions.status().await else { return Ok(()) };
        for (kind, name) in [(PermissionKind::Microphone, "Micrófono"), (PermissionKind::SystemAudio, "Audio del sistema")] {
            let mut status = states.iter().find(|p| p.kind == kind).map_or(PermissionStatus::Unknown, |p| p.status);
            if status == PermissionStatus::NotDetermined {
                status = self.native.permissions.request(kind).await?;
            }
            if status == PermissionStatus::Denied {
                return Err(PortError::PermissionDenied(format!(
                    "acta no tiene permiso de «{name}». Actívalo en Ajustes del Sistema › Privacidad y seguridad › {name} y vuelve a grabar."
                )));
            }
        }
        Ok(())
    }

    pub async fn session_start(&self, app: Option<MeetingApp>) -> PortResult<SessionSnapshot> {
        self.ensure_capture_permissions().await?;
        self.session(SessionEvent::StartRequested { app, at: self.clock.now() }).await
    }
    pub async fn session_pause(&self) -> PortResult<SessionSnapshot> {
        self.session(SessionEvent::PauseRequested { at: self.clock.now() }).await
    }
    pub async fn session_resume(&self) -> PortResult<SessionSnapshot> {
        self.session(SessionEvent::ResumeRequested { at: self.clock.now() }).await
    }
    pub async fn session_stop(&self) -> PortResult<SessionSnapshot> {
        self.session(SessionEvent::StopRequested { reason: StopReason::User, at: self.clock.now() }).await
    }
    pub async fn session_dismiss(&self) -> PortResult<SessionSnapshot> {
        self.session(SessionEvent::Dismiss).await
    }
    pub async fn prompt_answer(&self, prompt_id: PromptId, answer: PromptAnswer) -> PortResult<SessionSnapshot> {
        // Aceptar «¿Grabar?» también empieza a capturar.
        if answer == PromptAnswer::Accept
            && matches!(self.session_get_state().await?.state, SessionState::Detected { .. })
        {
            self.ensure_capture_permissions().await?;
        }
        self.session(SessionEvent::PromptAnswered { prompt_id, answer, at: self.clock.now() }).await
    }
    pub async fn capture_manual(&self) -> PortResult<Option<Capture>> {
        self.supervisor.capture_manual().await
    }
    pub async fn health_get(&self) -> PortResult<Vec<HealthWarning>> {
        self.supervisor.health().await
    }

    // ------------------------------------------------------------------
    // Reuniones
    // ------------------------------------------------------------------

    pub async fn meetings_list(&self, query: MeetingQuery) -> PortResult<Page<MeetingSummary>> {
        self.store.list_meetings(query).await
    }

    /// Abrir el detalle cuenta como uso (protege la reunión de la limpieza).
    pub async fn meeting_get(&self, id: MeetingId) -> PortResult<MeetingDetail> {
        let detail = self
            .store
            .get_meeting_detail(id)
            .await?
            .ok_or_else(|| PortError::NotFound("No existe esa reunión.".into()))?;
        self.touch(&[id], MeetingTouch::Opened).await;
        Ok(detail)
    }

    /// Apunta el uso de unas reuniones. Mejor esfuerzo: nunca falla un comando por esto.
    async fn touch(&self, ids: &[MeetingId], touch: MeetingTouch) {
        let now = self.clock.now();
        let mut seen: Vec<MeetingId> = Vec::new();
        for id in ids {
            if seen.contains(id) {
                continue;
            }
            seen.push(*id);
            if let Err(e) = self.store.touch_meeting(*id, touch, now).await {
                tracing::debug!(%id, "no se pudo apuntar el uso: {e}");
            }
        }
    }

    pub async fn meeting_set_favourite(&self, id: MeetingId, favourite: bool) -> PortResult<Meeting> {
        let m = self.store.set_meeting_favourite(id, favourite).await.map_err(|e| match e {
            PortError::NotFound(_) => PortError::NotFound("No existe esa reunión.".into()),
            other => other,
        })?;
        self.emit(
            event_names::MEETING_UPDATED,
            &MeetingUpdated { meeting_id: id, status: m.status, title: m.title.clone() },
        );
        Ok(m)
    }

    // ------------------------------------------------------------------
    // Limpieza de reuniones antiguas (solo sugiere; borra lo que el usuario elige)
    // ------------------------------------------------------------------

    pub async fn cleanup_suggestions(&self) -> PortResult<CleanupSuggestions> {
        let cleanup = self.settings().cleanup;
        let min_age_days = cleanup.min_age_days.max(1);
        if !cleanup.enabled {
            return Ok(CleanupSuggestions { min_age_days, ..CleanupSuggestions::default() });
        }
        let now = self.clock.now();
        let mut candidates = Vec::new();
        for f in self.store.cleanup_facts().await? {
            let Some(reasons) = evaluate(&f, min_age_days, now) else { continue };
            let bytes = crate::system::dir_size(self.layout.meeting_dir(f.id)).await;
            candidates.push(CleanupCandidate {
                meeting_id: f.id,
                title: f.title,
                started_at: f.started_at,
                duration: f.duration,
                reasons,
                bytes,
            });
        }
        sort_candidates(&mut candidates);
        let total_bytes = candidates.iter().map(|c| c.bytes).sum();
        Ok(CleanupSuggestions { candidates, total_bytes, min_age_days })
    }

    /// Borra las reuniones elegidas en la hoja de revisión (filas, índice y
    /// carpeta con audio, capturas y transcript). Las favoritas, las de un
    /// grupo con estrella y la que se esté grabando se saltan siempre.
    pub async fn cleanup_delete(&self, ids: Vec<MeetingId>) -> PortResult<CleanupResult> {
        let facts = self.store.cleanup_facts().await?;
        let mut result = CleanupResult::default();
        for id in ids {
            let protected = facts.iter().find(|f| f.id == id).is_none_or(|f| f.favourite || f.in_favourite_group);
            if protected {
                result.skipped.push(id);
                continue;
            }
            let bytes = crate::system::dir_size(self.layout.meeting_dir(id)).await;
            match self.meeting_delete(id).await {
                Ok(()) => {
                    result.deleted.push(id);
                    result.freed_bytes += bytes;
                }
                Err(e) => {
                    tracing::warn!(%id, "limpieza: no se pudo borrar: {e}");
                    result.skipped.push(id);
                }
            }
        }
        Ok(result)
    }

    pub async fn cleanup_keep(&self, ids: Vec<MeetingId>) -> PortResult<()> {
        for id in ids {
            match self.store.set_cleanup_keep(id, true).await {
                Ok(()) | Err(PortError::NotFound(_)) => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    pub async fn meeting_update(&self, id: MeetingId, title: String) -> PortResult<Meeting> {
        let title = title.trim().to_owned();
        if title.is_empty() {
            return Err(PortError::InvalidInput("El título no puede estar vacío.".into()));
        }
        let m = self
            .store
            .patch_meeting(id, MeetingPatch { title: Some(title), ..MeetingPatch::default() })
            .await?;
        self.emit(
            event_names::MEETING_UPDATED,
            &MeetingUpdated { meeting_id: id, status: m.status, title: m.title.clone() },
        );
        Ok(m)
    }

    /// Nota personal del usuario ("nota para mí"). En blanco la borra.
    pub async fn meeting_set_note(&self, id: MeetingId, note: String) -> PortResult<Meeting> {
        let note = Some(note.trim().to_owned()).filter(|n| !n.is_empty());
        let m = self
            .store
            .patch_meeting(id, MeetingPatch { user_note: Some(note), ..MeetingPatch::default() })
            .await
            .map_err(|e| match e {
                PortError::NotFound(_) => PortError::NotFound("No existe esa reunión.".into()),
                other => other,
            })?;
        self.emit(event_names::MEETING_UPDATED, &MeetingUpdated { meeting_id: id, status: m.status, title: m.title.clone() });
        Ok(m)
    }

    fn recording_meeting(state: &SessionState) -> Option<MeetingId> {
        match state {
            SessionState::Recording(r) => Some(r.meeting_id),
            SessionState::Paused { recording, .. } => Some(recording.meeting_id),
            SessionState::Stopping { meeting_id, .. } => Some(*meeting_id),
            _ => None,
        }
    }

    /// Borra filas y la carpeta (audio, capturas...). No se puede borrar la
    /// reunión que se está grabando.
    pub async fn meeting_delete(&self, id: MeetingId) -> PortResult<()> {
        let snap = self.supervisor.snapshot().await?;
        if Self::recording_meeting(&snap.state) == Some(id) {
            return Err(PortError::InvalidInput("Para la grabación antes de borrarla.".into()));
        }
        self.store.delete_meeting(id).await?;
        match tokio::fs::remove_dir_all(self.layout.meeting_dir(id)).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    pub async fn meeting_reprocess(&self, request: ReprocessRequest) -> PortResult<()> {
        if self.store.get_meeting(request.meeting_id).await?.is_none() {
            return Err(PortError::NotFound("No existe esa reunión.".into()));
        }
        if let Some(o) = request.llm {
            self.check_provider(&o.provider_id)?;
            self.llm_overrides.lock().unwrap_or_else(|p| p.into_inner()).insert(request.meeting_id, o);
        }
        self.supervisor.reprocess(request.meeting_id, request.steps)
    }

    /// El proveedor elegido para UNA vez (reintento, pregunta en vivo o
    /// mensaje del chat) tiene que existir y estar activado.
    fn check_provider(&self, id: &ProviderId) -> PortResult<()> {
        match self.settings().llm.providers.iter().find(|p| &p.id == id) {
            None => Err(PortError::NotFound(format!("No existe el proveedor «{id}»."))),
            Some(p) if !p.enabled => Err(PortError::Unavailable(format!(
                "El proveedor «{}» está desactivado. Actívalo en Ajustes › Proveedores.",
                p.label
            ))),
            Some(_) => Ok(()),
        }
    }

    /// Ajustes y proveedor para una respuesta: el elegido para esta vez (con
    /// su modelo, si se indica) o el del chat. Los ajustes guardados no cambian.
    fn one_off_provider(&self, provider_id: Option<ProviderId>, model: Option<String>) -> PortResult<(Settings, Option<ProviderId>)> {
        let settings = self.settings();
        match provider_id {
            Some(p) => {
                self.check_provider(&p)?;
                Ok((crate::pipeline::with_override(settings, &LlmOverride { provider_id: p.clone(), model }), Some(p)))
            }
            None => {
                let wanted = settings.llm.chat_provider.clone();
                Ok((settings, wanted))
            }
        }
    }

    /// «Reintentar» (y «Reintentar con…»). Sin pasos: desde transcribir.
    /// El proveedor/modelo solo vale para esta vez.
    pub async fn meeting_retry(
        &self,
        meeting_id: MeetingId,
        steps: Option<Vec<ProcessingStep>>,
        provider_id: Option<ProviderId>,
        model: Option<String>,
    ) -> PortResult<()> {
        let llm = provider_id.map(|provider_id| LlmOverride { provider_id, model: model.filter(|m| !m.trim().is_empty()) });
        self.meeting_reprocess(ReprocessRequest { meeting_id, steps: steps.unwrap_or_default(), llm }).await
    }

    /// Coincidencias dentro del transcript (sin acentos ni mayúsculas).
    pub async fn meeting_search_transcript(&self, meeting_id: MeetingId, query: String) -> PortResult<Vec<TranscriptMatch>> {
        if self.store.get_meeting(meeting_id).await?.is_none() {
            return Err(PortError::NotFound("No existe esa reunión.".into()));
        }
        let utterances = self.store.get_transcript(meeting_id).await?.map(|t| t.utterances).unwrap_or_default();
        Ok(acta_store::text::search_utterances(&utterances, &query))
    }

    pub async fn speaker_rename(
        &self,
        meeting_id: MeetingId,
        speaker_id: SpeakerId,
        display_name: String,
        remember: bool,
    ) -> PortResult<()> {
        rename_speaker(
            &self.layout,
            self.store.as_ref(),
            self.store.as_ref(),
            meeting_id,
            speaker_id,
            display_name,
            remember,
            self.clock.now(),
        )
        .await?;
        // Los chunks llevan el nombre: se reindexa en segundo plano.
        let store = self.store.clone();
        tokio::spawn(async move {
            if let Err(e) = store.index_meeting(meeting_id).await {
                tracing::warn!("reindexado tras renombrar: {e}");
            }
        });
        Ok(())
    }

    pub async fn action_item_set_done(&self, id: ActionItemId, done: bool) -> PortResult<()> {
        self.store.set_action_item_done(id, done).await
    }

    pub async fn capture_delete(&self, id: CaptureId) -> PortResult<()> {
        let capture = self
            .store
            .get_capture(id)
            .await?
            .ok_or_else(|| PortError::NotFound("No existe esa captura.".into()))?;
        self.store.delete_capture(id).await?;
        let path = self.layout.meeting_dir(capture.meeting_id).join(&capture.path);
        match tokio::fs::remove_file(path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    // ------------------------------------------------------------------
    // Grupos
    // ------------------------------------------------------------------

    pub async fn groups_list(&self) -> PortResult<Vec<Group>> {
        self.store.list_groups().await
    }
    /// Sin nombre: «Nuevo grupo», y se nombra solo con lo hablado en cuanto
    /// tenga reuniones (ver `group_add_meeting`).
    pub async fn group_create(&self, name: String, description: Option<String>) -> PortResult<Group> {
        let name = name.trim();
        let name = if name.is_empty() { PLACEHOLDER_GROUP.to_owned() } else { name.to_owned() };
        self.store.create_group(name, description).await
    }
    pub async fn group_update(&self, id: GroupId, name: Option<String>, description: Option<String>) -> PortResult<Group> {
        self.store.update_group(id, name, description).await
    }
    pub async fn group_delete(&self, id: GroupId) -> PortResult<()> {
        self.store.delete_group(id).await
    }
    pub async fn group_set_favourite(&self, id: GroupId, favourite: bool) -> PortResult<Group> {
        self.store.set_group_favourite(id, favourite).await
    }
    pub async fn group_add_meeting(&self, group_id: GroupId, meeting_id: MeetingId) -> PortResult<()> {
        GroupStore::add_meeting(self.store.as_ref(), group_id, meeting_id).await?;
        let placeholder = self.store.list_groups().await?.into_iter().any(|g| g.id == group_id && g.name == PLACEHOLDER_GROUP);
        if placeholder {
            let (store, factory, settings) = (self.store.clone(), self.factory.clone(), self.settings());
            tokio::spawn(async move {
                match suggest_group_title(store.clone(), factory, settings, group_id).await {
                    Ok(t) => {
                        // Solo si sigue sin nombre (el usuario pudo ponerle uno mientras).
                        if store.list_groups().await.is_ok_and(|gs| gs.iter().any(|g| g.id == group_id && g.name == PLACEHOLDER_GROUP))
                            && let Err(e) = store.update_group(group_id, Some(t.name), t.description).await
                        {
                            tracing::warn!("no se pudo nombrar el grupo: {e}");
                        }
                    }
                    Err(e) => tracing::warn!("no se pudo nombrar el grupo: {e}"),
                }
            });
        }
        Ok(())
    }
    pub async fn group_remove_meeting(&self, group_id: GroupId, meeting_id: MeetingId) -> PortResult<()> {
        GroupStore::remove_meeting(self.store.as_ref(), group_id, meeting_id).await
    }
    pub async fn group_suggestions_list(&self) -> PortResult<Vec<GroupSuggestion>> {
        self.store.list_suggestions().await
    }
    pub async fn group_suggestion_accept(&self, id: GroupId, name: Option<String>) -> PortResult<Group> {
        self.store.accept_suggestion(id, name).await
    }
    pub async fn group_suggestion_dismiss(&self, id: GroupId) -> PortResult<()> {
        self.store.dismiss_suggestion(id).await
    }

    /// Reintenta indexar las reuniones que quedaron sin vectores (el modelo
    /// de búsqueda no estaba disponible). Devuelve cuántas siguen sin ellos.
    async fn reindex_stale(&self) -> usize {
        let Ok(stale) = self.store.stale_meetings().await else { return 0 };
        for id in &stale {
            if let Err(e) = self.store.index_meeting(*id).await {
                tracing::warn!(%id, "reindexado pendiente: {e}");
            }
            // Si el modelo sigue sin estar, no se insiste con las demás.
            if self.store.stale_meetings().await.is_ok_and(|s| s.contains(id)) {
                break;
            }
        }
        self.store.stale_meetings().await.map_or(0, |s| s.len())
    }

    /// Clustering + nombre con el LLM (si hay proveedor; si no, sin nombre).
    ///
    /// Regla (`acta_store::clustering::ClusterConfig`): hacen falta al menos 3
    /// reuniones parecidas (similitud media ≥ 0,80), indexadas y que no estén
    /// ya en ningún grupo. «No hay sugerencias» es una lista vacía; que no se
    /// hayan podido comparar las reuniones es un ERROR, no una lista vacía.
    pub async fn group_suggestions_refresh(&self) -> PortResult<Vec<GroupSuggestion>> {
        let unindexed = self.reindex_stale().await;
        let mut suggestions = self.store.suggest_groups().await?;
        if suggestions.is_empty() && unindexed > 0 {
            let meetings = if unindexed == 1 { "1 reunión no se ha podido".to_owned() } else { format!("{unindexed} reuniones no se han podido") };
            return Err(PortError::Unavailable(format!(
                "No se pueden sugerir grupos todavía: {meetings} preparar para compararlas porque el modelo de búsqueda no está disponible \
                 (la primera vez se descarga de internet). Comprueba la conexión y vuelve a intentarlo."
            )));
        }
        let settings = self.settings();
        // Sin proveedor (o con uno que no arranca) las sugerencias van sin nombre.
        let namer = match build_provider(&settings, self.factory.as_ref(), settings.llm.naming_provider.as_ref()) {
            Ok((_, p)) => Some(LlmGroupNamer::new(p, settings.user.display_name.clone()).with_language(settings.transcription.language.clone())),
            Err(e) => {
                if !suggestions.is_empty() {
                    tracing::warn!("grupos sugeridos sin nombre: {e}");
                }
                None
            }
        };
        if let Some(namer) = namer {
            for s in &mut suggestions {
                let mut metas = Vec::new();
                for id in &s.meeting_ids {
                    let title = self.store.get_meeting(*id).await?.map(|m| m.title).unwrap_or_default();
                    let summary = self.store.get_minutes(*id).await?.map(|m| m.summary_md).unwrap_or_default();
                    metas.push((title, summary));
                }
                match namer.name_group(&metas).await {
                    Ok(n) => {
                        s.name = Some(n.name);
                        s.description = Some(n.description);
                    }
                    Err(e) => tracing::warn!("no se pudo nombrar un grupo sugerido: {e}"),
                }
            }
        }
        self.store.save_suggestions(suggestions).await?;
        self.store.list_suggestions().await
    }

    /// Título para un grupo sacado de lo hablado en sus reuniones (no lo aplica).
    pub async fn groups_suggest_title(&self, group_id: GroupId) -> PortResult<GroupTitleSuggestion> {
        suggest_group_title(self.store.clone(), self.factory.clone(), self.settings(), group_id).await
    }

    /// Tras procesar una reunión: reindexa las que quedaron sin vectores y
    /// refresca las sugerencias de grupos. Mejor esfuerzo.
    pub async fn after_processing(&self) {
        if let Err(e) = self.group_suggestions_refresh().await {
            tracing::warn!("sugerencias de grupos: {e}");
        }
    }

    // ------------------------------------------------------------------
    // Búsqueda y chat
    // ------------------------------------------------------------------

    /// Las reuniones que salen en una búsqueda cuentan como usadas.
    pub async fn search(&self, query: SearchQuery) -> PortResult<Vec<SearchHit>> {
        let hits = self.store.search(query).await?;
        let ids: Vec<MeetingId> = hits.iter().map(|h| h.meeting_id).collect();
        self.touch(&ids, MeetingTouch::Used).await;
        Ok(hits)
    }
    pub async fn chat_threads_list(&self, scope: Option<ChatScope>) -> PortResult<Vec<ChatThread>> {
        self.store.list_threads(scope).await
    }

    pub async fn chat_thread_create(&self, scope: ChatScope) -> PortResult<ChatThread> {
        let title = match &scope {
            ChatScope::Meeting { meeting_id } => self
                .store
                .get_meeting(*meeting_id)
                .await?
                .map_or_else(|| "Reunión".to_owned(), |m| m.title),
            ChatScope::Group { group_id } => self
                .store
                .list_groups()
                .await?
                .into_iter()
                .find(|g| g.id == *group_id)
                .map_or_else(|| "Grupo".to_owned(), |g| g.name),
            ChatScope::All => "Todas las reuniones".to_owned(),
        };
        self.store.create_thread(scope, title).await
    }

    pub async fn chat_thread_delete(&self, thread_id: ChatThreadId) -> PortResult<()> {
        self.store.delete_thread(thread_id).await
    }
    pub async fn chat_messages_list(&self, thread_id: ChatThreadId) -> PortResult<Vec<ChatMessage>> {
        self.store.list_messages(thread_id).await
    }

    /// Guarda la pregunta, lanza la respuesta en streaming (`chat:delta`) y la
    /// guarda al terminar. `provider_id`/`model` valen solo para ESTA respuesta.
    pub async fn chat_send(
        &self,
        thread_id: ChatThreadId,
        text: String,
        provider_id: Option<ProviderId>,
        model: Option<String>,
    ) -> PortResult<ChatSendResult> {
        let text = text.trim().to_owned();
        if text.is_empty() {
            return Err(PortError::InvalidInput("Escribe una pregunta.".into()));
        }
        let thread = self
            .store
            .list_threads(None)
            .await?
            .into_iter()
            .find(|t| t.id == thread_id)
            .ok_or_else(|| PortError::NotFound("No existe esa conversación.".into()))?;
        let history = self.store.list_messages(thread_id).await?;
        let (settings, wanted) = self.one_off_provider(provider_id, model)?;
        let (cfg, provider) = build_provider(&settings, self.factory.as_ref(), wanted.as_ref())?;
        let user_message = ChatMessage {
            id: ChatMessageId::new(),
            thread_id,
            role: ChatRole::User,
            content: text.clone(),
            citations: vec![],
            provider: None,
            created_at: self.clock.now(),
        };
        self.store.append_message(user_message.clone()).await?;
        if let ChatScope::Meeting { meeting_id } = &thread.scope {
            self.touch(&[*meeting_id], MeetingTouch::Used).await;
        }
        let engine = LlmChatEngine::new(provider, self.store.clone(), self.store.clone(), settings.user.clone())
            .with_language(settings.transcription.language.clone());
        let assistant_message_id = ChatMessageId::new();
        let store = self.store.clone();
        let emitter = self.emitter.clone();
        let clock = self.clock.clone();
        let (cancel_tx, mut cancel_rx) = oneshot::channel::<()>();
        let key = assistant_message_id.to_string();
        self.cancels.lock().unwrap_or_else(|p| p.into_inner()).insert(key.clone(), cancel_tx);
        let cancels = self.cancels.clone();
        // La respuesta entera va en segundo plano: con un CLI (`claude -p`)
        // `ask` no vuelve hasta el final, y «Detener» tiene que poder cortarlo
        // (soltar el futuro mata el proceso: `kill_on_drop`).
        tokio::spawn(async move {
            let finish = |error: Option<String>| ChatDelta {
                thread_id,
                message_id: assistant_message_id,
                delta: String::new(),
                done: true,
                citations: vec![],
                error,
            };
            let mut content = String::new();
            let mut citations = Vec::new();
            // El delta final (`done`) NO sale hasta que la respuesta está guardada:
            // la interfaz, al recibirlo, suelta el texto en streaming y relee los
            // mensajes guardados. Si saliera antes, una respuesta larga (con muchas
            // citas que anotar) desaparecería hasta que terminara de guardarse.
            let stream = tokio::select! {
                r = engine.ask(&thread, &history, &text, assistant_message_id) => Some(r),
                _ = &mut cancel_rx => None,
            };
            let last: ChatDelta = match stream {
                None => finish(None),
                Some(Err(e)) => finish(Some(e.to_string())),
                Some(Ok(mut stream)) => loop {
                    let next = tokio::select! {
                        d = stream.next() => d,
                        _ = &mut cancel_rx => break finish(None),
                    };
                    let Some(delta) = next else { break finish(None) };
                    content.push_str(&delta.delta);
                    if delta.done {
                        citations = delta.citations.clone();
                        break delta;
                    }
                    emitter.emit(event_names::CHAT_DELTA, to_json(&delta));
                },
            };
            cancels.lock().unwrap_or_else(|p| p.into_inner()).remove(&key);
            if !content.trim().is_empty() {
                // Las reuniones citadas en la respuesta cuentan como usadas.
                let now = clock.now();
                for c in &citations {
                    let _ = store.touch_meeting(c.meeting_id, MeetingTouch::Used, now).await;
                }
                let msg = ChatMessage {
                    id: assistant_message_id,
                    thread_id,
                    role: ChatRole::Assistant,
                    content,
                    citations,
                    provider: Some(cfg.id),
                    created_at: clock.now(),
                };
                if let Err(e) = store.append_message(msg).await {
                    tracing::warn!("no se pudo guardar la respuesta del chat: {e}");
                }
            }
            emitter.emit(event_names::CHAT_DELTA, to_json(&last));
        });
        Ok(ChatSendResult { user_message, assistant_message_id })
    }

    /// Todo lo transcrito en vivo de una grabación. Sin sesión en memoria (la
    /// app se reinició), lo confirmado que quedó en disco.
    pub async fn live_transcript_get(&self, session_id: MeetingId) -> PortResult<LiveTranscript> {
        let session = self.live_sessions.lock().unwrap_or_else(|p| p.into_inner()).get(&session_id).cloned();
        if let Some(s) = session {
            return Ok(s.transcript());
        }
        if self.store.get_meeting(session_id).await?.is_none() {
            return Err(PortError::NotFound("No existe esa grabación.".into()));
        }
        let mut segments = Vec::new();
        for track in acta_core::recording::Track::ALL {
            if let Some(state) = acta_audio::live::read_state(&self.layout, session_id, track) {
                let mut t = acta_audio::live::TrackLive::new(track);
                t.state = state;
                segments.extend(t.all());
            }
        }
        segments.sort_by_key(|s| s.start_ms);
        Ok(LiveTranscript { session_id, segments })
    }

    /// Pregunta sobre lo dicho hasta ahora. La respuesta llega por
    /// `live:answer` (streaming, citas `[mm:ss]` al final); `live_cancel` la corta.
    pub async fn live_ask(
        &self,
        session_id: MeetingId,
        question: String,
        provider_id: Option<ProviderId>,
        model: Option<String>,
    ) -> PortResult<String> {
        let question = question.trim().to_owned();
        if question.is_empty() {
            return Err(PortError::InvalidInput("Escribe una pregunta.".into()));
        }
        let segments = self.live_transcript_get(session_id).await?.segments;
        let (settings, wanted) = self.one_off_provider(provider_id, model)?;
        let (_, provider) = build_provider(&settings, self.factory.as_ref(), wanted.as_ref())?;
        let request =
            acta_llm::live::live_ask_request(&segments, &question, &settings.user.display_name, &settings.transcription.language);
        let request_id = ChatMessageId::new().to_string();
        let (cancel_tx, mut cancel_rx) = oneshot::channel::<()>();
        self.cancels.lock().unwrap_or_else(|p| p.into_inner()).insert(request_id.clone(), cancel_tx);
        let (cancels, emitter, rid) = (self.cancels.clone(), self.emitter.clone(), request_id.clone());
        tokio::spawn(async move {
            let send = |a: LiveAnswer| emitter.emit(event_names::LIVE_ANSWER, to_json(&a));
            let done = |citations: Option<Vec<LiveCitation>>, error: Option<String>| LiveAnswer { request_id: rid.clone(), delta: None, done: true, citations, error };
            let stream = tokio::select! {
                s = provider.stream(request) => Some(s),
                _ = &mut cancel_rx => None,
            };
            let mut full = String::new();
            match stream {
                None => send(done(None, None)),
                Some(Err(e)) => send(done(None, Some(e.to_string()))),
                Some(Ok(mut stream)) => loop {
                    let next = tokio::select! {
                        c = stream.next() => c,
                        _ = &mut cancel_rx => { send(done(None, None)); break; }
                    };
                    match next {
                        Some(Ok(t)) if t.is_empty() => {}
                        Some(Ok(t)) => {
                            full.push_str(&t);
                            send(LiveAnswer { request_id: rid.clone(), delta: Some(t), done: false, citations: None, error: None });
                        }
                        Some(Err(e)) => {
                            send(done(None, Some(e.to_string())));
                            break;
                        }
                        None => {
                            send(done(Some(acta_llm::live::citations(&full, &segments)), None));
                            break;
                        }
                    }
                },
            }
            cancels.lock().unwrap_or_else(|p| p.into_inner()).remove(&rid);
        });
        Ok(request_id)
    }

    /// «Detener» de `chat_send` o de `live_ask`: corta la generación. Lo que
    /// ya no está en curso se ignora.
    pub async fn chat_cancel(&self, request_id: String) -> PortResult<()> {
        if let Some(tx) = self.cancels.lock().unwrap_or_else(|p| p.into_inner()).remove(&request_id) {
            let _ = tx.send(());
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // Huellas, ajustes, proveedores, secretos, modelos, permisos
    // ------------------------------------------------------------------

    pub async fn voiceprints_list(&self) -> PortResult<Vec<VoiceprintSummary>> {
        self.store.list_voiceprints().await
    }
    pub async fn voiceprint_delete(&self, id: VoiceprintId) -> PortResult<()> {
        self.store.delete_voiceprint(id).await
    }

    pub async fn settings_get(&self) -> PortResult<Settings> {
        Ok(self.settings())
    }

    pub async fn settings_update(&self, settings: Settings) -> PortResult<Settings> {
        use acta_core::library::SettingsStore;
        self.store.save_settings(settings.clone()).await?;
        *self.settings.write().unwrap_or_else(|e| e.into_inner()) = settings.clone();
        self.supervisor.update_settings(settings.clone())?;
        Ok(settings)
    }

    pub async fn llm_provider_test(&self, provider_id: ProviderId) -> PortResult<ProviderTestResult> {
        let settings = self.settings();
        let cfg = settings
            .llm
            .providers
            .iter()
            .find(|p| p.id == provider_id)
            .ok_or_else(|| PortError::NotFound(format!("No existe el proveedor «{provider_id}».")))?;
        match self.factory.build(cfg) {
            Ok(p) => Ok(test_provider(p.as_ref()).await),
            Err(e) => Ok(ProviderTestResult { ok: false, message: e.to_string(), latency_ms: None }),
        }
    }

    /// Proveedores activados y sus modelos elegibles («Reintentar con…»).
    pub async fn llm_providers_available(&self) -> PortResult<Vec<AvailableProvider>> {
        use acta_core::llm::ProviderKind;
        let settings = self.settings();
        let roles = |id: &ProviderId| {
            let l = &settings.llm;
            [("minutes", &l.minutes_provider), ("chat", &l.chat_provider), ("naming", &l.naming_provider)]
                .into_iter()
                .filter(|(_, p)| p.as_ref() == Some(id))
                .map(|(r, _)| r.to_owned())
                .collect::<Vec<_>>()
        };
        let mut out = Vec::new();
        for cfg in settings.llm.providers.iter().filter(|p| p.enabled) {
            let configured = crate::llm::model_of(cfg);
            let (kind, mut models): (&str, Vec<String>) = match &cfg.kind {
                ProviderKind::ClaudeCli { .. } => ("claudeCli", ["sonnet", "opus", "haiku"].map(String::from).to_vec()),
                ProviderKind::CodexCli { .. } => ("codexCli", vec![]),
                ProviderKind::OpenCodeCli { binary_path, extra_env, .. } => {
                    let listed = acta_llm::providers::opencode::list_models(
                        binary_path,
                        extra_env,
                        std::time::Duration::from_secs(5),
                    )
                    .await;
                    ("opencodeCli", listed.unwrap_or_default())
                }
                ProviderKind::Anthropic { .. } => ("anthropic", vec![]),
                ProviderKind::BuiltIn { .. } => (
                    "builtIn",
                    self.local_llm_status().await.map(|s| s.models.into_iter().filter(|m| m.downloaded).map(|m| m.model.id).collect()).unwrap_or_default(),
                ),
                ProviderKind::OpenAiCompatible { base_url, api_key, .. } => {
                    let key = api_key.as_ref().and_then(|r| self.secrets.get(r).ok().flatten());
                    let listed = acta_llm::providers::models::list_models(base_url, key.as_deref(), std::time::Duration::from_secs(5)).await;
                    ("openAiCompatible", listed.unwrap_or_default())
                }
            };
            if let Some(m) = &configured
                && !models.contains(m)
            {
                models.insert(0, m.clone());
            }
            let default_for = roles(&cfg.id);
            out.push(AvailableProvider {
                provider_id: cfg.id.clone(),
                label: cfg.label.clone(),
                kind: kind.to_owned(),
                models,
                default_model: configured,
                is_default: default_for.iter().any(|r| r == "minutes"),
                default_for,
            });
        }
        Ok(out)
    }

    pub async fn llm_cli_detect(&self) -> PortResult<CliDetection> {
        let claude = detect_cli_any(&self.shell, "claude", &self.home).await?;
        let codex = detect_cli_any(&self.shell, "codex", &self.home).await?;
        let mut opencode = detect_cli_any(&self.shell, "opencode", &self.home).await?;
        if opencode.is_none() {
            let candidate = self.home.join(".opencode/bin/opencode");
            if tokio::fs::metadata(&candidate).await.is_ok_and(|m| m.is_file()) {
                opencode = Some(acta_llm::providers::detect::DetectedCli {
                    path: candidate,
                    env: std::collections::BTreeMap::new(),
                });
            }
        }
        let opencode_model = opencode.as_ref().and_then(|_| acta_llm::providers::opencode::resolve_default_model(&self.home));
        Ok(CliDetection {
            claude: claude.as_ref().map(|d| d.path.to_string_lossy().into_owned()),
            codex: codex.as_ref().map(|d| d.path.to_string_lossy().into_owned()),
            opencode: opencode.as_ref().map(|d| d.path.to_string_lossy().into_owned()),
            claude_env: claude.map(|d| d.env).unwrap_or_default(),
            codex_env: codex.map(|d| d.env).unwrap_or_default(),
            opencode_env: opencode.map(|d| d.env).unwrap_or_default(),
            opencode_model,
        })
    }

    pub async fn secret_set(&self, provider_id: ProviderId, value: String) -> PortResult<SecretRef> {
        let value = value.trim().to_owned();
        if value.is_empty() {
            return Err(PortError::InvalidInput("La clave está vacía.".into()));
        }
        let r = secret_ref_for_provider(&provider_id.0);
        self.secrets.set(&r, &value)?;
        Ok(r)
    }

    pub async fn secret_delete(&self, provider_id: ProviderId) -> PortResult<()> {
        self.secrets.delete(&secret_ref_for_provider(&provider_id.0))
    }

    // ------------------------------------------------------------------
    // Modelo local integrado (opcional)
    // ------------------------------------------------------------------

    async fn hardware(&self) -> HardwareInfo {
        match &self.hardware_override {
            Some(h) => h.clone(),
            None => crate::system::hardware_info(self.layout.root()).await,
        }
    }

    pub async fn local_llm_status(&self) -> PortResult<LocalLlmStatus> {
        Ok(self.local_llm.status(self.hardware().await).await)
    }

    /// Lanza la descarga (llama.cpp + modelo); el progreso llega como
    /// `localmodel:download`. Se niega si el modelo no cabe en este Mac.
    pub async fn local_llm_download(&self, model_id: String) -> PortResult<()> {
        let status = self.local_llm_status().await?;
        let entry = status
            .models
            .iter()
            .find(|m| m.model.id == model_id)
            .ok_or_else(|| PortError::NotFound(format!("No existe el modelo local «{model_id}».")))?;
        match entry.fit {
            ModelFit::TooBig => {
                return Err(PortError::InvalidInput(format!(
                    "{} necesita al menos {} GB de memoria y este Mac tiene {}.",
                    entry.model.name,
                    entry.model.min_ram_gb,
                    status.hardware.ram_gb()
                )));
            }
            ModelFit::NoDisk => {
                return Err(PortError::InvalidInput(format!(
                    "No hay espacio en el disco para {}: ocupa {:.1} GB y hay que dejar 2 GB libres.",
                    entry.model.name,
                    entry.model.size_bytes as f64 / 1e9
                )));
            }
            ModelFit::Comfortable | ModelFit::Tight => {}
        }
        if entry.downloading {
            return Ok(());
        }
        let (local, emitter) = (self.local_llm.clone(), self.emitter.clone());
        tokio::spawn(async move {
            let on_event = move |e: LocalLlmDownload| emitter.emit(event_names::LOCAL_LLM_DOWNLOAD, to_json(&e));
            if let Err(e) = local.download(&model_id, &on_event).await {
                tracing::warn!(%model_id, "descarga del modelo local: {e}");
            }
        });
        Ok(())
    }

    pub async fn local_llm_cancel(&self, model_id: String) -> PortResult<()> {
        self.local_llm.cancel(&model_id);
        Ok(())
    }

    pub async fn local_llm_delete(&self, model_id: String) -> PortResult<()> {
        self.local_llm.delete(&model_id).await.map(|_| ())
    }

    /// «Probar»: carga el modelo y le pide una palabra.
    pub async fn local_llm_test(&self, model_id: String) -> PortResult<ProviderTestResult> {
        let provider = BuiltInProvider::new(
            acta_core::llm::local::BUILTIN_PROVIDER_ID.into(),
            model_id,
            self.local_llm.clone(),
            std::time::Duration::from_secs(300),
        );
        Ok(test_provider(&provider).await)
    }

    pub async fn models_status(&self) -> PortResult<Vec<ModelStatus>> {
        self.native.models.status().await
    }

    /// Lanza la descarga; el progreso llega como `model:download`.
    pub async fn models_download(&self, name: String) -> PortResult<()> {
        if !matches!(name.as_str(), "asr" | "diarizer" | "vad") {
            return Err(PortError::InvalidInput(format!("Modelo desconocido: {name}")));
        }
        let mut stream = self.native.models.download(&name).await?;
        let emitter = self.emitter.clone();
        tokio::spawn(async move {
            while let Some(p) = stream.next().await {
                emitter.emit(event_names::MODEL_DOWNLOAD, to_json(&p));
            }
        });
        Ok(())
    }

    /// Pone un título mejor que «Reunión de Teams» si se puede: el nombre de
    /// la reunión en la ventana de Teams; si no, el evento del Calendario a esa
    /// hora (solo con `titles.calendar`). El del LLM llega después, al resumir.
    /// Nunca toca un título editado. Devuelve el nuevo título, si lo hubo.
    pub async fn resolve_title(&self, id: MeetingId, app: Option<&MeetingApp>) -> PortResult<Option<String>> {
        use acta_core::library::titles::{best_window_title, is_generic_title, pick_calendar_title};
        let Some(meeting) = self.store.get_meeting(id).await? else { return Ok(None) };
        if !is_generic_title(&meeting.title) {
            return Ok(None);
        }
        let mut title = None;
        if let Some(app) = app {
            let mut titles: Vec<String> = app.title.iter().cloned().collect();
            match self.native.meeting_info.window_titles(app.pid, &app.bundle_id).await {
                Ok(t) => titles.extend(t),
                Err(e) => tracing::debug!("títulos de ventana: {e}"),
            }
            title = best_window_title(&titles);
        }
        if title.is_none() && self.settings().titles.calendar {
            let from = meeting.started_at.plus_ms(-30 * 60_000);
            let to = meeting.ended_at.unwrap_or(meeting.started_at).plus_ms(30 * 60_000);
            match self.native.meeting_info.calendar_events(from, to, true).await {
                Ok(events) => title = pick_calendar_title(&events, meeting.started_at.0, meeting.ended_at.map(|t| t.0)),
                Err(e) => tracing::info!("sin título del calendario: {e}"),
            }
        }
        let Some(title) = title else { return Ok(None) };
        let Some(m) = self.store.set_auto_title(id, title.clone()).await? else { return Ok(None) };
        self.emit(event_names::MEETING_UPDATED, &MeetingUpdated { meeting_id: id, status: m.status, title: m.title });
        Ok(Some(title))
    }

    /// Carga en memoria el ASR y el diarizador ya descargados (la primera
    /// carga en frío tarda ≈25 s). Se llama al empezar a grabar.
    pub async fn warm_models(&self) {
        let Ok(status) = self.native.models.status().await else { return };
        for m in status.iter().filter(|m| m.downloaded && matches!(m.name.as_str(), "asr" | "diarizer")) {
            match self.native.models.download(&m.name).await {
                Ok(mut s) => while s.next().await.is_some() {},
                Err(e) => tracing::warn!(model = %m.name, "no se pudo precargar: {e}"),
            }
        }
    }

    /// Nombre de la cuenta e idiomas de macOS para proponerlos en la
    /// bienvenida. Nunca falla: lo que no se sabe va vacío.
    pub async fn system_profile(&self) -> PortResult<SystemProfile> {
        let (full_name, languages) = match self.native.system.profile().await {
            Ok(p) => p,
            Err(e) => {
                tracing::info!("perfil del sistema no disponible: {e}");
                (None, Vec::new())
            }
        };
        Ok(SystemProfile::from_system(full_name, languages))
    }

    /// Apps instaladas y en marcha, sin repetir y por nombre. Los iconos
    /// quedan en `cache/app-icons` de la carpeta de datos.
    pub async fn apps_list(&self) -> PortResult<Vec<InstalledApp>> {
        let icons = self.layout.app_icons_dir();
        tokio::fs::create_dir_all(&icons).await?;
        Ok(tidy_apps(self.native.system.installed_apps(&icons).await?))
    }

    pub async fn audio_inputs_list(&self) -> PortResult<Vec<AudioInput>> {
        self.native.devices.inputs().await
    }

    pub async fn permissions_status(&self) -> PortResult<Vec<PermissionState>> {
        let app = crate::system::app_level_permissions();
        let states = match self.native.permissions.status().await {
            Ok(states) => crate::system::merge_app_level(states, app),
            Err(e) if self.native.error.is_some() => {
                tracing::debug!("permisos sin sidecar: {e}");
                crate::system::merge_app_level(vec![], app)
            }
            Err(e) => return Err(e),
        };
        // Si la captura de esta grabación acaba de chocar con TCC, eso manda.
        let denied: Vec<PermissionKind> = self
            .supervisor
            .health()
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|w| !w.cleared)
            .filter_map(|w| match w.issue {
                acta_core::recording::HealthIssue::PermissionMissing { permission } => Some(permission),
                _ => None,
            })
            .collect();
        Ok(crate::system::apply_runtime_denials(states, &denied))
    }
    pub async fn permissions_request(&self, kind: PermissionKind) -> PortResult<PermissionStatus> {
        self.native.permissions.request(kind).await
    }

    pub fn app_info(&self) -> AppInfo {
        AppInfo {
            version: env!("CARGO_PKG_VERSION").into(),
            data_dir: self.layout.root().to_string_lossy().into_owned(),
            sidecar_version: self.native.sidecar_version.clone(),
            macos_version: self.native.macos_version.clone(),
        }
    }

    /// Cierre ordenado: para la grabación (se procesará al volver) y el sidecar.
    pub async fn shutdown(&self) {
        let _ = self.supervisor.shutdown().await;
        self.local_llm.shutdown().await;
        #[cfg(target_os = "macos")]
        if let Some(c) = &self.native.client {
            let _ = c.shutdown().await;
        }
    }
}

/// Nombre de grupo con el LLM a partir de los títulos y resúmenes de sus reuniones.
async fn suggest_group_title(
    store: Arc<SqliteStore>,
    factory: Arc<dyn LlmProviderFactory>,
    settings: Settings,
    group_id: GroupId,
) -> PortResult<GroupTitleSuggestion> {
    if !store.list_groups().await?.iter().any(|g| g.id == group_id) {
        return Err(PortError::NotFound("No existe ese grupo.".into()));
    }
    let page = store.list_meetings(MeetingQuery { group_id: Some(group_id), limit: Some(30), ..Default::default() }).await?;
    if page.items.is_empty() {
        return Err(PortError::InvalidInput("El grupo aún no tiene reuniones de las que sacar un título.".into()));
    }
    let (_, provider) = build_provider(&settings, factory.as_ref(), settings.llm.naming_provider.as_ref())?;
    let mut metas = Vec::new();
    for m in &page.items {
        let summary = store.get_minutes(m.id).await?.map(|x| x.summary_md).unwrap_or_default();
        metas.push((m.title.clone(), summary));
    }
    let n = LlmGroupNamer::new(provider, settings.user.display_name.clone())
        .with_language(settings.transcription.language.clone())
        .name_group(&metas)
        .await?;
    Ok(GroupTitleSuggestion { name: n.name, description: Some(n.description).filter(|d| !d.is_empty()) })
}
