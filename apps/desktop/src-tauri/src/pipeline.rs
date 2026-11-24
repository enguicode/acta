//! Pipeline que se monta en cada ejecución con los Ajustes del momento (el
//! usuario puede cambiar de proveedor LLM entre reuniones).

use crate::llm::{build_provider, model_of};
use acta_audio::processing::DefaultPipeline;
use acta_core::ids::MeetingId;
use acta_core::layout::DataLayout;
use acta_core::library::{MeetingStore, SearchIndex, VoiceprintStore};
use acta_core::llm::LlmProviderFactory;
use acta_core::minutes::{MinutesGenerator, MinutesInput, MinutesOutput};
use acta_core::processing::{LlmOverride, ProcessingPipeline, ProcessingStep, ProgressSink};
use acta_core::llm::ProviderKind;
use std::collections::HashMap;
use std::sync::Mutex;
use acta_core::settings::Settings;
use acta_core::time::Clock;
use acta_core::transcription::{Diarizer, Transcriber};
use acta_core::{PortError, PortResult};
use acta_core::transcription::{SpeakerNamer, TranscriptCleaner};
use acta_llm::minutes::LlmMinutesGenerator;
use acta_llm::cleanup::LlmTranscriptCleaner;
use acta_llm::naming::speakers::LlmSpeakerNamer;
use async_trait::async_trait;
use std::sync::{Arc, RwLock};

pub type SharedSettings = Arc<RwLock<Settings>>;

/// «Reintentar con…»: proveedor/modelo para la PRÓXIMA ejecución de esa
/// reunión (se consume al usarlo; los ajustes no cambian).
pub type LlmOverrides = Arc<Mutex<HashMap<MeetingId, LlmOverride>>>;

/// Proveedor y modelo de un resumen que falló.
type FailedLlm = Arc<Mutex<Option<(String, Option<String>)>>>;

/// Los ajustes con el proveedor/modelo del reintento como los del acta.
pub fn with_override(mut settings: Settings, o: &LlmOverride) -> Settings {
    if let Some(cfg) = settings.llm.providers.iter_mut().find(|p| p.id == o.provider_id) {
        cfg.enabled = true;
        if let Some(m) = o.model.clone().filter(|m| !m.trim().is_empty()) {
            match &mut cfg.kind {
                ProviderKind::ClaudeCli { model, .. }
                | ProviderKind::CodexCli { model, .. }
                | ProviderKind::OpenCodeCli { model, .. } => *model = Some(m),
                ProviderKind::OpenAiCompatible { model, .. } | ProviderKind::Anthropic { model, .. } => *model = m,
                ProviderKind::BuiltIn { model_id } => *model_id = m,
            }
        }
    }
    settings.llm.minutes_provider = Some(o.provider_id.clone());
    settings
}

/// El acta que falla dice QUIÉN falló (texto y campos de la reunión).
struct Named {
    inner: Arc<dyn MinutesGenerator>,
    provider: String,
    model: Option<String>,
    failed: FailedLlm,
}

#[async_trait]
impl MinutesGenerator for Named {
    async fn generate(&self, input: MinutesInput) -> PortResult<MinutesOutput> {
        self.inner.generate(input).await.map_err(|e| {
            *self.failed.lock().unwrap_or_else(|p| p.into_inner()) = Some((self.provider.clone(), self.model.clone()));
            let who = format!("proveedor: {}, modelo: {}", self.provider, self.model.as_deref().unwrap_or("el de por defecto"));
            match e {
                PortError::InvalidResponse(m) => PortError::InvalidResponse(format!("{m} ({who})")),
                PortError::Unavailable(m) => PortError::Unavailable(format!("{m} ({who})")),
                PortError::Timeout(m) => PortError::Timeout(format!("{m} ({who})")),
                PortError::Internal(m) => PortError::Internal(format!("{m} ({who})")),
                other => PortError::Internal(format!("{other} ({who})")),
            }
        })
    }
}

pub fn read(settings: &SharedSettings) -> Settings {
    settings.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Sin proveedor: el paso de resumen falla con un mensaje claro.
struct NoMinutes(String);

#[async_trait]
impl MinutesGenerator for NoMinutes {
    async fn generate(&self, _input: MinutesInput) -> PortResult<MinutesOutput> {
        Err(PortError::Unavailable(self.0.clone()))
    }
}

pub struct DynamicPipeline {
    pub layout: DataLayout,
    pub settings: SharedSettings,
    pub store: Arc<dyn MeetingStore>,
    pub voiceprints: Arc<dyn VoiceprintStore>,
    pub search: Arc<dyn SearchIndex>,
    pub transcriber: Arc<dyn Transcriber>,
    pub diarizer: Arc<dyn Diarizer>,
    pub factory: Arc<dyn LlmProviderFactory>,
    pub clock: Arc<dyn Clock>,
    pub overrides: LlmOverrides,
    /// Donde se apunta el proveedor/modelo que falló (`None` en pruebas sin store real).
    pub errors: Option<Arc<acta_store::SqliteStore>>,
}

impl DynamicPipeline {
    fn minutes(&self, settings: &Settings, failed: &FailedLlm) -> Arc<dyn MinutesGenerator> {
        match build_provider(settings, self.factory.as_ref(), settings.llm.minutes_provider.as_ref()) {
            Ok((cfg, provider)) => Arc::new(Named {
                inner: Arc::new(LlmMinutesGenerator::new(provider, self.clock.clone()).with_model(model_of(&cfg))),
                provider: cfg.id.0.clone(),
                model: model_of(&cfg),
                failed: failed.clone(),
            }),
            Err(e) => Arc::new(NoMinutes(e.to_string())),
        }
    }

    /// Mismo proveedor que el acta. Sin proveedor, los "Speaker N" se quedan
    /// así (el paso no falla).
    fn speaker_namer(&self, settings: &Settings) -> Option<Arc<dyn SpeakerNamer>> {
        let (_, provider) = build_provider(settings, self.factory.as_ref(), settings.llm.minutes_provider.as_ref()).ok()?;
        Some(Arc::new(LlmSpeakerNamer::new(provider)))
    }

    /// Mismo proveedor que el acta. Sin proveedor no hay limpieza (el texto se
    /// queda como lo dejó el ASR + glosario); con él, la casilla de Ajustes
    /// (`transcription.llmCleanup`, encendida de fábrica) decide.
    fn cleaner(&self, settings: &Settings) -> Option<Arc<dyn TranscriptCleaner>> {
        let (_, provider) = build_provider(settings, self.factory.as_ref(), settings.llm.minutes_provider.as_ref()).ok()?;
        Some(Arc::new(LlmTranscriptCleaner::new(provider)))
    }
}

#[async_trait]
impl ProcessingPipeline for DynamicPipeline {
    async fn run(&self, meeting_id: MeetingId, steps: &[ProcessingStep], progress: &dyn ProgressSink) -> PortResult<()> {
        let mut settings = read(&self.settings);
        let one_off = self.overrides.lock().unwrap_or_else(|p| p.into_inner()).remove(&meeting_id);
        if let Some(o) = &one_off {
            settings = with_override(settings, o);
        }
        let failed: FailedLlm = Arc::default();
        let pipeline = DefaultPipeline {
            layout: self.layout.clone(),
            minutes: self.minutes(&settings, &failed),
            speaker_namer: self.speaker_namer(&settings),
            cleaner: self.cleaner(&settings),
            settings,
            store: self.store.clone(),
            voiceprints: self.voiceprints.clone(),
            search: self.search.clone(),
            transcriber: self.transcriber.clone(),
            diarizer: self.diarizer.clone(),
        };
        let result = pipeline.run(meeting_id, steps, progress).await;
        let who = failed.lock().unwrap_or_else(|p| p.into_inner()).take();
        if let (Some((provider, model)), Some(store), Err(_)) = (who, &self.errors, &result)
            && let Err(e) = store.set_meeting_error_llm(meeting_id, Some(provider), model).await
        {
            tracing::warn!(%meeting_id, "no se pudo apuntar el proveedor que falló: {e}");
        }
        result
    }
}
