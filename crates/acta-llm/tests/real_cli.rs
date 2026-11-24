//! Pruebas contra el `claude` REAL del usuario (ignoradas por defecto).
//!
//! ```bash
//! ACTA_REAL_CLAUDE=/opt/homebrew/bin/claude CLAUDE_CONFIG_DIR=$HOME/.claude-personal \
//!   cargo test -p acta-llm --test real_cli -- --ignored --nocapture
//! ```
//! Reproducen el bug del 16-nov-2026: `--json-schema` con `$schema` 2020-12
//! hacía fallar el paso «Resumen» con código 1.

use acta_core::ids::{CaptureId, MeetingId, SpeakerId};
use acta_core::minutes::{UserIdentity, CaptureRef, GroupNamer, MinutesGenerator, MinutesInput};
use acta_core::detection::Platform;
use acta_core::recording::Track;
use acta_core::screen::CaptureReason;

use acta_core::time::{MediaMs, SystemClock, UnixMs};
use acta_core::transcription::{NameSource, Speaker, Utterance};
use acta_llm::minutes::LlmMinutesGenerator;
use acta_llm::naming::LlmGroupNamer;
use acta_llm::providers::claude::ClaudeCliProvider;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

fn provider() -> Option<Arc<ClaudeCliProvider>> {
    let bin = std::env::var("ACTA_REAL_CLAUDE").ok()?;
    let mut env = BTreeMap::new();
    if let Ok(dir) = std::env::var("CLAUDE_CONFIG_DIR") {
        env.insert("CLAUDE_CONFIG_DIR".to_string(), dir);
    }
    Some(Arc::new(ClaudeCliProvider::new("claude-cli".into(), bin.into(), None, env, Duration::from_secs(300))))
}

fn speaker(n: u128, is_me: bool) -> Speaker {
    Speaker {
        id: SpeakerId::from_u128(n),
        meeting_id: MeetingId::from_u128(1),
        label: if is_me { "Yo".into() } else { format!("Speaker {}", n - 1) },
        display_name: None,
        is_me,
        track: if is_me { Track::Mic } else { Track::System },
        diarization_label: None,
        name_source: NameSource::Generic,
        confirmed: false,
        voiceprint_id: None,
    }
}

fn utt(idx: u32, sp: u128, start_ms: u64, text: &str) -> Utterance {
    Utterance {
        idx,
        speaker_id: SpeakerId::from_u128(sp),
        track: if sp == 1 { Track::Mic } else { Track::System },
        start: MediaMs(start_ms),
        end: MediaMs(start_ms + 3_000),
        text: text.into(),
        confidence: None,
    }
}

#[tokio::test]
#[ignore = "usa el claude real; ACTA_REAL_CLAUDE=<ruta>"]
async fn real_claude_generates_minutes_with_shipped_schema() {
    let Some(p) = provider() else { return };
    let input = MinutesInput {
        meeting_id: MeetingId::from_u128(1),
        title: "Reunión".into(),
        platform: Platform::Other,
        started_at: UnixMs(1_794_821_400_000),
        duration: MediaMs(20_000),
        me: UserIdentity { display_name: "Edu".into(), aliases: vec![] },
        speakers: vec![speaker(1, true), speaker(2, false)],
        utterances: vec![
            utt(0, 1, 1_000, "Vale, decidimos lanzar la beta el viernes."),
            utt(1, 2, 5_000, "Yo preparo las notas de versión para el jueves."),
            utt(2, 1, 10_000, "Y yo envío el presupuesto a finanzas mañana."),
        ],
        captures: vec![CaptureRef { ref_id: "c1".into(), capture_id: CaptureId::from_u128(9), at: MediaMs(6_000), reason: CaptureReason::SlideChange }],
        language: "es".into(),
        user_note: None,
    };
    let out = LlmMinutesGenerator::new(p, Arc::new(SystemClock)).generate(input).await.expect("acta real");
    eprintln!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
    assert!(!out.minutes.summary_md.trim().is_empty());
    assert!(!out.minutes.action_items.is_empty());
    assert!(out.minutes.action_items.iter().any(|a| a.is_mine));
    assert!(!out.escaleta.temas.is_empty());
}

#[tokio::test]
#[ignore = "usa el claude real; ACTA_REAL_CLAUDE=<ruta>"]
async fn real_claude_names_a_group() {
    let Some(p) = provider() else { return };
    let meetings = vec![
        ("Presupuesto 2027".to_string(), "Revisión de partidas de marketing".to_string()),
        ("Cierre trimestral".to_string(), "Facturas pendientes y previsión".to_string()),
    ];
    let name = LlmGroupNamer::new(p, "Edu").name_group(&meetings).await.expect("nombre real");
    eprintln!("{name:?}");
    assert!(!name.name.trim().is_empty());
}

/// Red de empresa con inspección TLS (raíz corporativa en el Llavero):
/// openrouter.ai llega firmado por la CA del proxy. Con rustls + webpki-roots
/// fallaba con `UnknownIssuer`; debe llegar al servidor y dar 401 sin clave.
#[tokio::test]
#[ignore = "red real; ACTA_REAL_NET=1"]
async fn real_https_trusts_the_system_keychain() {
    use acta_core::llm::{LlmMessage, LlmProvider, LlmRequest, Role};
    use acta_llm::providers::openai::OpenAiCompatibleProvider;
    if std::env::var("ACTA_REAL_NET").is_err() {
        return;
    }
    let p = OpenAiCompatibleProvider::new("openrouter".into(), "https://openrouter.ai/api/v1", "x".into(), Some("sk-invalida".into()), Duration::from_secs(30));
    let req = LlmRequest { system: None, messages: vec![LlmMessage { role: Role::User, content: "hola".into() }], json_schema: None, max_tokens: Some(5), temperature: None };
    let err = p.complete(req).await.expect_err("sin clave válida");
    let msg = err.to_string();
    eprintln!("{msg}");
    // Llegar a HTTP (401, o el 503 de la página de bloqueo de la empresa)
    // demuestra que el TLS con la CA del proxy funciona.
    assert!(!msg.contains("no se pudo conectar"), "{msg}");
}

/// NaN Builders con un modelo de razonamiento SIN coste (qwen3.6 / gemma4).
/// Reproduce «El proveedor respondió vacío» (Probar conexión, max_tokens 16)
/// y el nombre de grupo «sin JSON» (max_tokens 300): el razonamiento se comía
/// el presupuesto. La clave se lee del Llavero y NUNCA se imprime.
/// `ACTA_REAL_NAN=qwen3.6 cargo test -p acta-llm --test real_cli real_nan -- --ignored --nocapture`
#[tokio::test]
#[ignore = "red real + clave de NaN en el Llavero; ACTA_REAL_NAN=<modelo>"]
async fn real_nan_reasoning_model_answers_the_test_and_names_a_group() {
    use acta_llm::providers::{openai::OpenAiCompatibleProvider, test_provider};
    let Ok(model) = std::env::var("ACTA_REAL_NAN") else { return };
    let mut cmd = tokio::process::Command::new("/usr/bin/security");
    cmd.args(["find-generic-password", "-s", "acta", "-a", "provider:nan", "-w"]);
    let out = acta_llm::process::run_supervised(cmd, None, Duration::from_secs(10)).await.expect("security");
    let key = String::from_utf8(out.stdout).expect("utf8").trim().to_owned();
    assert!(!key.is_empty(), "no hay clave de NaN en el Llavero");
    let p = Arc::new(OpenAiCompatibleProvider::new("nan".into(), "https://api.nan.builders/v1", model, Some(key), Duration::from_secs(120)));
    let t = test_provider(p.as_ref()).await;
    eprintln!("probar: {} ({:?} ms)", t.message, t.latency_ms);
    assert!(t.ok, "{}", t.message);
    let name = LlmGroupNamer::new(p, "Edu")
        .name_group(&[
            ("Daily del equipo".into(), "- Subcuentas pendientes de visto bueno.\n- Borrado LOPD corregido.".into()),
            ("Daily del equipo".into(), "- Vacantes internas publicadas.\n- Permisos de la shell ampliados.".into()),
        ])
        .await
        .expect("nombre");
    eprintln!("grupo: {} — {}", name.name, name.description);
}
