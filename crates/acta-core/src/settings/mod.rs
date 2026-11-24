//! Ajustes del usuario. Se guardan como JSON en la tabla `settings` (clave
//! "app"). Todos los campos tienen valor por defecto (`#[serde(default)]`)
//! para que añadir campos no rompa ajustes ya guardados.

use crate::detection::{AppRule, Platform};
use crate::ids::ProviderId;
use crate::llm::ProviderConfig;
use crate::minutes::UserIdentity;
use crate::recording::{AutoStopConfig, HealthConfig};
use serde::{Deserialize, Serialize};

pub mod language;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub user: UserIdentity,
    pub recording: RecordingSettings,
    pub auto_stop: AutoStopConfig,
    pub health: HealthConfig,
    pub detection: DetectionSettings,
    pub screen: ScreenSettings,
    pub transcription: TranscriptionSettings,
    pub llm: LlmSettings,
    pub embeddings: EmbeddingSettings,
    /// Bienvenida de primer arranque: la UI la marca al terminarla o saltarla.
    pub onboarding: OnboardingSettings,
    /// Sugerencias de borrado de reuniones antiguas (nunca borra sola).
    pub cleanup: CleanupSettings,
    /// De dónde sale el título de una reunión nueva.
    pub titles: TitleSettings,
}

/// Orden: título de la reunión en Teams → evento del Calendario de macOS a esa
/// hora (solo con `calendar`, pide permiso de Calendario) → título del LLM.
/// Un título que el usuario editó nunca se cambia.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct TitleSettings {
    pub calendar: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CleanupSettings {
    /// Enseñar el aviso «Tienes N reuniones antiguas…».
    pub enabled: bool,
    /// Antigüedad mínima (días) para sugerir una reunión.
    pub min_age_days: u32,
    /// «Recordármelo más tarde»: el aviso no vuelve hasta esta fecha.
    pub snoozed_until: Option<crate::time::UnixMs>,
}

impl Default for CleanupSettings {
    fn default() -> Self {
        Self { enabled: true, min_age_days: 90, snoozed_until: None }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OnboardingSettings {
    pub completed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RecordingSettings {
    pub segment_seconds: u32,
    pub frame_ms: u32,
    /// UID del micro. `None` = el de por defecto del sistema.
    pub mic_device_uid: Option<String>,
    /// Borrar segmentos tras concatenar con éxito.
    pub delete_segments_after_concat: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DetectionSettings {
    pub enabled: bool,
    pub rules: Vec<AppRule>,
    pub meet_url_patterns: Vec<String>,
    /// Si la pregunta "¿Grabar?" no se contesta en este tiempo, se cierra.
    pub start_prompt_timeout_s: u32,
    pub poll_ms: u32,
    pub debounce_ms: u32,
    /// Bundle ids que NUNCA disparan la detección aunque usen el micro
    /// (apps de dictado como Typeless u Orca). Se aplica antes que `rules`.
    pub denylist: Vec<String>,
    /// Empezar a grabar automáticamente al detectar una llamada. Si el usuario
    /// la para dentro de `auto_record_delay_s`, se descarta por completo.
    /// Apagado por defecto (opt-in explícito).
    pub auto_record: bool,
    /// Segundos desde el inicio automático durante los cuales parar la grabación
    /// la descarta (borra audio y reunión). `0` guarda siempre.
    pub auto_record_delay_s: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ScreenSettings {
    /// Opt-in: requiere permiso de Grabación de pantalla.
    pub enabled: bool,
    pub fps: f32,
    pub phash_threshold: u32,
    pub stable_frames: u32,
    pub candidate_every_s: u32,
    pub ring_seconds: u32,
    pub max_width: u32,
    /// Palabras clave que disparan captura (minúsculas, sin acentos al comparar).
    pub keywords: Vec<String>,
    /// Borrar candidatas no promovidas al terminar el procesado.
    pub prune_candidates: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TranscriptionSettings {
    /// Idioma de las reuniones: `"auto"` o un código ISO 639-1 (`"es"`, `"en"`…).
    /// Dirige la transcripción (pista para el ASR; `"auto"` = que lo detecte) y
    /// el idioma en que se escriben el acta, el resumen y el chat (`"auto"` =
    /// el que se habla en la reunión). Ver [`language`].
    pub language: String,
    pub diarization: bool,
    /// Transcripción en vivo durante la llamada (`live:transcript`, `live_ask`);
    /// al parar solo se finaliza en vez de transcribir todo otra vez.
    pub live_preview: bool,
    /// Umbral coseno para reconocer una huella de voz.
    pub voiceprint_min_similarity_pct: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LlmSettings {
    pub providers: Vec<ProviderConfig>,
    /// Proveedor para resumen/tareas/escaleta.
    pub minutes_provider: Option<ProviderId>,
    /// Proveedor para el chat.
    pub chat_provider: Option<ProviderId>,
    /// Proveedor para nombrar grupos.
    pub naming_provider: Option<ProviderId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct EmbeddingSettings {
    /// Id fastembed. Por defecto multilingual-e5-small (384 dims).
    pub model: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            // Sin nombre de fábrica: lo propone la bienvenida (`system_profile`).
            user: UserIdentity { display_name: String::new(), aliases: Vec::new() },
            recording: RecordingSettings::default(),
            auto_stop: AutoStopConfig::default(),
            health: HealthConfig::default(),
            detection: DetectionSettings::default(),
            screen: ScreenSettings::default(),
            transcription: TranscriptionSettings::default(),
            llm: LlmSettings::default(),
            embeddings: EmbeddingSettings::default(),
            onboarding: OnboardingSettings::default(),
            cleanup: CleanupSettings::default(),
            titles: TitleSettings::default(),
        }
    }
}

impl Default for RecordingSettings {
    fn default() -> Self {
        Self {
            segment_seconds: 30,
            frame_ms: 100,
            mic_device_uid: None,
            delete_segments_after_concat: false,
        }
    }
}

impl Default for DetectionSettings {
    fn default() -> Self {
        let rule = |bundle_id: &str, platform, is_browser| AppRule {
            bundle_id: bundle_id.into(),
            platform,
            is_browser,
        };
        Self {
            enabled: true,
            rules: vec![
                rule("com.microsoft.teams2", Platform::Teams, false),
                rule("com.microsoft.teams", Platform::Teams, false),
                rule("com.google.Chrome", Platform::Meet, true),
                rule("company.thebrowser.Browser", Platform::Meet, true),
                rule("com.microsoft.edgemac", Platform::Meet, true),
                rule("com.apple.Safari", Platform::Meet, true),
            ],
            meet_url_patterns: vec!["meet.google.com/".into()],
            start_prompt_timeout_s: 90,
            poll_ms: 1_000,
            debounce_ms: 3_000,
            denylist: Vec::new(),
            auto_record: false,
            auto_record_delay_s: 10,
        }
    }
}

impl Default for ScreenSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            fps: 1.0,
            phash_threshold: 10,
            stable_frames: 2,
            candidate_every_s: 30,
            ring_seconds: 20,
            max_width: 1920,
            keywords: [
                "importante",
                "apuntad",
                "apunta",
                "esto es clave",
                "clave",
                "fecha",
                "plazo",
                "deadline",
                "important",
                "key point",
                "take note",
                "due date",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            prune_candidates: true,
        }
    }
}

impl Default for TranscriptionSettings {
    fn default() -> Self {
        Self {
            language: language::AUTO.into(),
            diarization: true,
            live_preview: true,
            voiceprint_min_similarity_pct: 75,
        }
    }
}

/// De fábrica no hay NINGÚN proveedor: la bienvenida detecta lo que hay en
/// este Mac (CLI instalados, Ollama, modelo integrado) y el usuario elige.
impl Default for LlmSettings {
    fn default() -> Self {
        Self { providers: Vec::new(), minutes_provider: None, chat_provider: None, naming_provider: None }
    }
}

impl Default for EmbeddingSettings {
    fn default() -> Self {
        Self {
            model: "intfloat/multilingual-e5-small".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::ProviderKind;
    use std::path::PathBuf;

    #[test]
    fn live_preview_is_on_and_calendar_titles_are_opt_in_by_default() {
        let s = Settings::default();
        assert!(s.transcription.live_preview);
        assert!(!s.titles.calendar);
        let old: Settings = serde_json::from_str(r#"{"transcription":{"language":"es"}}"#).unwrap_or_default();
        assert!(!old.titles.calendar);
        assert_eq!(old.transcription.language, "es");
    }

    #[test]
    fn fresh_defaults_are_neutral() {
        let s = Settings::default();
        assert_eq!(s.user.display_name, "");
        assert!(s.user.aliases.is_empty());
        assert_eq!(s.transcription.language, "auto");
        assert!(s.llm.providers.is_empty(), "nada activado de fábrica: lo elige la bienvenida");
        assert_eq!((&s.llm.minutes_provider, &s.llm.chat_provider, &s.llm.naming_provider), (&None, &None, &None));
        assert!(!s.onboarding.completed);
    }

    #[test]
    fn fresh_defaults_contain_no_personal_strings() {
        let json = serde_json::to_string(&Settings::default()).expect("json");
        for personal in ["Edu", "Navajas", ".claude-personal", "eduardo", "qwen", "deepseek", "nan.builders", "CLAUDE_CONFIG_DIR", "/Users/"] {
            assert!(!json.contains(personal), "los ajustes de fábrica llevan «{personal}»: {json}");
        }
        assert!(!json.to_lowercase().contains("navajas"));
    }

    /// Ajustes guardados por una instalación anterior a los valores neutros:
    /// se leen TAL CUAL (los valores de fábrica solo son para instalaciones nuevas).
    const STORED_BEFORE_NEUTRAL_DEFAULTS: &str = r#"{
      "user": {"displayName": "Edu", "aliases": ["Eduardo", "Edu Navajas"]},
      "recording": {"segmentSeconds": 30, "frameMs": 100, "micDeviceUid": null, "deleteSegmentsAfterConcat": false},
      "detection": {"enabled": true, "rules": [{"bundleId": "com.microsoft.teams2", "platform": "teams", "isBrowser": false}],
        "meetUrlPatterns": ["meet.google.com/"], "startPromptTimeoutS": 90, "pollMs": 1000, "debounceMs": 3000, "denylist": ["com.typeless.app"]},
      "transcription": {"language": "es", "diarization": true, "livePreview": true, "voiceprintMinSimilarityPct": 75},
      "llm": {
        "providers": [
          {"id": "claude-cli", "label": "Claude Code (CLI)", "enabled": true, "timeoutS": 600,
           "kind": {"type": "claudeCli", "binaryPath": "/opt/homebrew/bin/claude", "model": null, "effort": null,
                    "extraEnv": {"CLAUDE_CONFIG_DIR": "/Users/someone/.claude-personal"}}},
          {"id": "codex-cli", "label": "Codex (CLI)", "enabled": false, "timeoutS": 600,
           "kind": {"type": "codexCli", "binaryPath": "/opt/homebrew/bin/codex", "model": null, "extraEnv": {}}},
          {"id": "ollama", "label": "Ollama (local)", "enabled": false, "timeoutS": 600,
           "kind": {"type": "openAiCompatible", "baseUrl": "http://localhost:11434/v1", "model": "qwen2.5:14b", "apiKey": null}},
          {"id": "nan", "label": "NaN Builders", "enabled": true, "timeoutS": 600,
           "kind": {"type": "openAiCompatible", "baseUrl": "https://api.nan.builders/v1", "model": "deepseek-v4-pro",
                    "apiKey": {"service": "acta", "account": "provider:nan"}}}
        ],
        "minutesProvider": "claude-cli", "chatProvider": "nan", "namingProvider": "claude-cli"
      },
      "embeddings": {"model": "intfloat/multilingual-e5-small"},
      "onboarding": {"completed": true},
      "cleanup": {"enabled": true, "minAgeDays": 90, "snoozedUntil": null},
      "titles": {"calendar": true}
    }"#;

    #[test]
    fn settings_stored_before_the_neutral_defaults_load_unchanged() {
        let s: Settings = serde_json::from_str(STORED_BEFORE_NEUTRAL_DEFAULTS).expect("ajustes guardados");
        assert_eq!(s.user, UserIdentity { display_name: "Edu".into(), aliases: vec!["Eduardo".into(), "Edu Navajas".into()] });
        assert_eq!(s.transcription.language, "es");
        let ids: Vec<(&str, bool)> = s.llm.providers.iter().map(|p| (p.id.0.as_str(), p.enabled)).collect();
        assert_eq!(ids, [("claude-cli", true), ("codex-cli", false), ("ollama", false), ("nan", true)]);
        assert!(matches!(&s.llm.providers[0].kind, ProviderKind::ClaudeCli { binary_path, extra_env, .. }
            if binary_path == &PathBuf::from("/opt/homebrew/bin/claude")
                && extra_env.get("CLAUDE_CONFIG_DIR").map(String::as_str) == Some("/Users/someone/.claude-personal")));
        assert!(matches!(&s.llm.providers[2].kind, ProviderKind::OpenAiCompatible { model, .. } if model == "qwen2.5:14b"));
        assert!(matches!(&s.llm.providers[3].kind, ProviderKind::OpenAiCompatible { base_url, model, api_key: Some(k) }
            if base_url == "https://api.nan.builders/v1" && model == "deepseek-v4-pro" && k.account == "provider:nan"));
        assert_eq!(s.llm.minutes_provider, Some("claude-cli".into()));
        assert_eq!(s.llm.chat_provider, Some("nan".into()));
        assert_eq!(s.llm.naming_provider, Some("claude-cli".into()));
        assert!(s.onboarding.completed && s.titles.calendar);
        assert_eq!(s.detection.denylist, ["com.typeless.app"]);
        // Guardar y volver a leer no cambia nada.
        let again: Settings = serde_json::from_str(&serde_json::to_string(&s).expect("json")).expect("ida y vuelta");
        assert_eq!(again, s);
    }

    #[test]
    fn a_partial_legacy_json_missing_newer_keys_still_loads() {
        // Sin `onboarding`, `cleanup`, `titles`, `livePreview` ni `denylist` (claves posteriores).
        let old = r#"{
          "user": {"displayName": "Edu", "aliases": ["Eduardo"]},
          "detection": {"enabled": false},
          "transcription": {"language": "es", "diarization": false},
          "llm": {"providers": [{"id": "claude-cli", "label": "Claude Code (CLI)", "enabled": true, "timeoutS": 600,
                   "kind": {"type": "claudeCli", "binaryPath": "/opt/homebrew/bin/claude", "model": null, "extraEnv": {}}}],
                  "minutesProvider": "claude-cli"}
        }"#;
        let s: Settings = serde_json::from_str(old).expect("ajustes antiguos");
        assert_eq!(s.user.display_name, "Edu");
        assert_eq!((s.transcription.language.as_str(), s.transcription.diarization, s.transcription.live_preview), ("es", false, true));
        assert_eq!(s.llm.providers.len(), 1);
        assert_eq!((s.llm.minutes_provider.clone(), s.llm.chat_provider.clone()), (Some("claude-cli".into()), None));
        assert!(!s.detection.enabled && s.detection.denylist.is_empty() && !s.detection.rules.is_empty());
        assert_eq!(s.cleanup, CleanupSettings::default());
        assert!(!s.onboarding.completed);
    }
}
