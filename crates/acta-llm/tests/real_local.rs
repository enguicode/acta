#![allow(clippy::expect_used, clippy::unwrap_used)]
//! Prueba de humo REAL del modelo local integrado (ignorada por defecto):
//! descarga llama.cpp y el modelo pequeño en `ACTA_DATA_DIR`, lo arranca con
//! Metal y resume una reunión corta en castellano.
//!
//! ```bash
//! ACTA_LOCAL_LLM_E2E=1 ACTA_DATA_DIR=$(mktemp -d) \
//!   cargo test -p acta-llm --test real_local -- --ignored --nocapture
//! ```
//! Baja 1,3 GB. `ACTA_LOCAL_LLM_MODEL` elige otro nivel del catálogo.

use acta_core::detection::Platform;
use acta_core::ids::{MeetingId, ProviderId, SpeakerId};
use acta_core::layout::DataLayout;
use acta_core::llm::local::{LocalDownloadStage, RuntimeSource, catalog};
use acta_core::llm::*;
use acta_core::minutes::{MinutesGenerator, MinutesInput, UserIdentity};
use acta_core::recording::Track;
use acta_core::time::{MediaMs, SystemClock, UnixMs};
use acta_core::transcription::{NameSource, Speaker, Utterance};
use acta_core::{EventStream, PortResult};
use acta_llm::local::{BuiltInProvider, LocalLlm, LocalLlmConfig};
use acta_llm::minutes::LlmMinutesGenerator;
use async_trait::async_trait;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Mide cada llamada: tokens de salida y segundos.
struct Timed {
    inner: Arc<dyn LlmProvider>,
    calls: Mutex<Vec<(u64, f64)>>,
}

#[async_trait]
impl LlmProvider for Timed {
    fn id(&self) -> &ProviderId {
        self.inner.id()
    }
    fn capabilities(&self) -> ProviderCapabilities {
        self.inner.capabilities()
    }
    async fn complete(&self, request: LlmRequest) -> PortResult<LlmResponse> {
        let started = Instant::now();
        let r = self.inner.complete(request).await?;
        self.calls.lock().unwrap().push((r.usage.output_tokens.unwrap_or(0), started.elapsed().as_secs_f64()));
        Ok(r)
    }
    async fn stream(&self, request: LlmRequest) -> PortResult<EventStream<PortResult<String>>> {
        self.inner.stream(request).await
    }
}

fn speaker(n: u128, is_me: bool, name: Option<&str>) -> Speaker {
    Speaker {
        id: SpeakerId::from_u128(n),
        meeting_id: MeetingId::from_u128(1),
        label: if is_me { "Yo".into() } else { format!("Speaker {}", n - 1) },
        display_name: name.map(str::to_owned),
        is_me,
        track: if is_me { Track::Mic } else { Track::System },
        diarization_label: None,
        name_source: NameSource::Generic,
        confirmed: false,
        voiceprint_id: None,
    }
}

fn meeting() -> MinutesInput {
    let lines: [(u128, &str); 10] = [
        (2, "Buenos días a todos. Hoy tenemos que cerrar la fecha de lanzamiento de la versión dos de la aplicación."),
        (1, "Por mi parte el desarrollo está terminado. Solo falta la revisión de seguridad, que termina el jueves."),
        (3, "Marketing necesita una semana para preparar la campaña y las capturas de la tienda."),
        (2, "Entonces propongo lanzar el lunes veinticuatro de noviembre. ¿Estamos de acuerdo?"),
        (1, "De acuerdo. Yo me encargo de enviar el informe de seguridad a dirección el viernes."),
        (3, "Perfecto. Yo preparo la nota de prensa y la tengo lista el miércoles."),
        (2, "Otro tema: el presupuesto de servidores sube un quince por ciento el año que viene."),
        (1, "Puedo revisar si pasando a instancias reservadas ahorramos algo. Lo miro la semana que viene."),
        (2, "Gracias, Edu. Ana, ¿puedes hablar con el proveedor para pedir un descuento?"),
        (3, "Sí, les escribo mañana y os cuento. Decidido entonces: lanzamos el lunes veinticuatro."),
    ];
    MinutesInput {
        meeting_id: MeetingId::from_u128(1),
        title: "Reunión de Teams".into(),
        platform: Platform::Teams,
        started_at: UnixMs(1_794_821_400_000),
        duration: MediaMs(lines.len() as u64 * 9_000),
        me: UserIdentity { display_name: "Edu".into(), aliases: vec!["Eduardo".into(), "Edu Navajas".into()] },
        speakers: vec![speaker(1, true, None), speaker(2, false, Some("Marta Ruiz")), speaker(3, false, Some("Ana García"))],
        utterances: lines
            .iter()
            .enumerate()
            .map(|(i, (sp, text))| Utterance {
                idx: i as u32,
                speaker_id: SpeakerId::from_u128(*sp),
                track: if *sp == 1 { Track::Mic } else { Track::System },
                start: MediaMs(i as u64 * 9_000),
                end: MediaMs(i as u64 * 9_000 + 8_000),
                text: (*text).into(),
                confidence: None,
            })
            .collect(),
        captures: vec![],
        language: "es".into(),
        user_note: None,
    }
}

#[tokio::test]
#[ignore = "real: descarga 1,3 GB y ejecuta el modelo (ACTA_LOCAL_LLM_E2E=1 ACTA_DATA_DIR=<temporal>)"]
async fn downloads_the_tiny_model_and_summarises_a_spanish_meeting_with_valid_json() {
    if std::env::var("ACTA_LOCAL_LLM_E2E").is_err() {
        eprintln!("ACTA_LOCAL_LLM_E2E no definido: se omite");
        return;
    }
    let data_dir = std::env::var("ACTA_DATA_DIR").expect("ACTA_DATA_DIR (usa un directorio temporal)");
    assert!(!data_dir.contains("Application Support/acta"), "nunca en los datos reales");
    let layout = DataLayout::new(&data_dir);
    let model_id = std::env::var("ACTA_LOCAL_LLM_MODEL").unwrap_or_else(|_| catalog()[0].id.clone());
    let mut cfg = LocalLlmConfig::for_layout(&layout);
    // Que mande la descarga de acta, no un llama.cpp de Homebrew o del entorno.
    cfg.search.env_override = None;
    cfg.search.exe_dir = None;
    let llm = Arc::new(LocalLlm::new(cfg));

    // 1) Descarga (llama.cpp + modelo) con SHA-256.
    let started = Instant::now();
    let last_stage = Arc::new(Mutex::new(None::<LocalDownloadStage>));
    let seen = last_stage.clone();
    llm.download(&model_id, &move |e| {
        let mut s = seen.lock().unwrap();
        if *s != Some(e.stage) {
            eprintln!("  descarga: {:?} ({} bytes)", e.stage, e.total_bytes);
            *s = Some(e.stage);
        }
        assert!(e.error.is_none(), "{:?}", e.error);
    })
    .await
    .expect("descarga");
    eprintln!("descarga + verificación: {:.1} s", started.elapsed().as_secs_f64());
    let model = catalog().into_iter().find(|m| m.id == model_id).expect("modelo");
    let on_disk = std::fs::metadata(layout.llm_models_dir().join(&model.file)).expect("gguf").len();
    assert_eq!(on_disk, model.size_bytes, "el tamaño del catálogo es el real");

    let hw = acta_core::llm::local::HardwareInfo { ram_bytes: 0, chip: String::new(), apple_silicon: true, free_disk_bytes: None };
    let status = llm.status(hw).await;
    assert_eq!(status.runtime.source, Some(RuntimeSource::Downloaded));

    // 2) Resumen con el esquema del acta como gramática.
    let provider: Arc<dyn LlmProvider> = Arc::new(BuiltInProvider::new("builtin".into(), model_id.clone(), llm.clone(), Duration::from_secs(600)));
    let timed = Arc::new(Timed { inner: provider, calls: Mutex::default() });
    let load = Instant::now();
    let generator = LlmMinutesGenerator::new(timed.clone(), Arc::new(SystemClock)).with_model(Some(model_id.clone()));
    let out = generator.generate(meeting()).await.expect("acta válida (JSON del esquema acta.minutes.v1)");
    let total = load.elapsed().as_secs_f64();

    let calls = timed.calls.lock().unwrap().clone();
    let (tokens, secs) = calls.iter().fold((0u64, 0f64), |a, c| (a.0 + c.0, a.1 + c.1));
    eprintln!("modelo: {model_id}");
    eprintln!("llamadas: {} · tokens de salida: {tokens} · {secs:.1} s (incluye cargar el modelo) · total {total:.1} s", calls.len());
    eprintln!("velocidad de extremo a extremo: {:.1} tokens/s", tokens as f64 / secs.max(0.001));
    eprintln!("--- resumen ---\n{}", out.minutes.summary_md);
    eprintln!("--- decisiones ---");
    for d in &out.minutes.decisions {
        eprintln!("- {}", d.text);
    }
    eprintln!("--- tareas ---");
    for t in &out.minutes.action_items {
        eprintln!("- [{}] {} ({:?}, {:?})", if t.is_mine { "mía" } else { "otro" }, t.text, t.owner_name, t.due);
    }
    eprintln!("--- escaleta ---");
    for t in &out.escaleta.temas {
        eprintln!("- {} [{}–{} ms]", t.titulo, t.inicio_ms, t.fin_ms);
    }

    assert_eq!(calls.len(), 1, "sin reintento de «repara este JSON»: la gramática lo deja válido a la primera");
    assert!(!out.minutes.summary_md.trim().is_empty());
    assert!(!out.escaleta.temas.is_empty(), "escaleta");
    assert!(!out.minutes.action_items.is_empty(), "tareas");
    for banned in ["(yo)", "transcripción"] {
        assert!(!out.minutes.summary_md.contains(banned), "metacomentario «{banned}»");
    }

    // 3) Segunda llamada con el modelo ya en memoria: velocidad de generación.
    let warm = Instant::now();
    let r = timed
        .complete(LlmRequest {
            system: Some("Eres un asistente que escribe en castellano de España.".into()),
            messages: vec![LlmMessage { role: Role::User, content: "Explica en unas 150 palabras para qué sirve un acta de reunión.".into() }],
            json_schema: None,
            max_tokens: Some(400),
            temperature: Some(0.2),
        })
        .await
        .expect("texto");
    let secs = warm.elapsed().as_secs_f64();
    let n = r.usage.output_tokens.unwrap_or(0);
    eprintln!("en caliente: {n} tokens en {secs:.2} s = {:.1} tokens/s", n as f64 / secs.max(0.001));
    assert!(n > 20);

    llm.shutdown().await;
    assert_eq!(llm.status(acta_core::llm::local::HardwareInfo { ram_bytes: 0, chip: String::new(), apple_silicon: true, free_disk_bytes: None }).await.running, None);
}
