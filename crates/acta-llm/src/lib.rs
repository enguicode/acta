//! # acta-llm (WP3)
//!
//! - `providers`: impl de `LlmProvider` — `claude -p`, `codex exec`,
//!   OpenAI-compatible (Ollama/LM Studio/OpenRouter), Anthropic API — y la
//!   `LlmProviderFactory`. Los CLI se lanzan con `process::run_supervised`.
//! - `local`: modelo local integrado (descarga de GGUF + `llama-server` supervisado).
//! - `transcript`: formato de transcript para prompts (`[mm:ss|ms] Nombre: texto`)
//!   y troceo map-reduce si no cabe en contexto.
//! - `minutes`: impl de `MinutesGenerator` (esquema `schemas/minutes.v1.json`).
//! - `chat`: impl de `ChatEngine` (reunión = transcript completo; grupo/todo = RAG con citas `[n]`).
//! - `naming`: impl de `GroupNamer` (esquema `schemas/group_name.v1.json`) y
//!   `naming::speakers`: impl de `SpeakerNamer` (esquema `schemas/speaker_names.v1.json`).
//! - `cleanup`: impl de `TranscriptCleaner` (esquema `schemas/transcript_cleanup.v1.json`).
//! - `live`: preguntas sobre la reunión EN CURSO (`live_ask`), con citas `[mm:ss]`.
//! - `moments`: impl de `ImportantMomentDetector` (palabras clave ES/EN).
#![deny(clippy::zombie_processes)]

pub mod chat;
pub mod cleanup;
pub mod live;
pub mod local;
pub mod minutes;
pub mod moments;
pub mod naming;
pub mod process;
pub mod providers;
pub mod structured;
#[cfg(test)]
pub(crate) mod testutil;
pub mod transcript;

/// Esquemas JSON embebidos (contrato con el LLM).
pub mod schemas {
    pub const MINUTES_V1: &str = include_str!("../schemas/minutes.v1.json");
    pub const GROUP_NAME_V1: &str = include_str!("../schemas/group_name.v1.json");
    pub const SPEAKER_NAMES_V1: &str = include_str!("../schemas/speaker_names.v1.json");
    pub const TRANSCRIPT_CLEANUP_V1: &str = include_str!("../schemas/transcript_cleanup.v1.json");
}

/// Plantillas de prompt embebidas (`{{variable}}`).
pub mod prompts {
    pub const MINUTES_SYSTEM: &str = include_str!("../prompts/minutes.system.md");
    pub const MINUTES_USER: &str = include_str!("../prompts/minutes.user.md");
    pub const CHAT_SYSTEM: &str = include_str!("../prompts/chat.system.md");
    pub const GROUP_NAME_SYSTEM: &str = include_str!("../prompts/group_name.system.md");
    pub const LIVE_ASK_SYSTEM: &str = include_str!("../prompts/live_ask.system.md");
    pub const SPEAKER_NAMES_SYSTEM: &str = include_str!("../prompts/speaker_names.system.md");
    pub const TRANSCRIPT_CLEANUP_SYSTEM: &str = include_str!("../prompts/transcript_cleanup.system.md");
}

#[cfg(test)]
mod tests {
    /// `claude --json-schema` (ajv) rechaza el meta-esquema draft 2020-12:
    /// los esquemas embebidos NO llevan `$schema` (bug real del 16-nov-2026).
    #[test]
    fn shipped_schemas_have_no_meta_schema_keyword() {
        for raw in [super::schemas::MINUTES_V1, super::schemas::GROUP_NAME_V1, super::schemas::SPEAKER_NAMES_V1, super::schemas::TRANSCRIPT_CLEANUP_V1] {
            let v: serde_json::Value = serde_json::from_str(raw).expect("json");
            assert!(!crate::providers::common::contains_key(&v, "$schema"), "{raw}");
        }
    }
}
