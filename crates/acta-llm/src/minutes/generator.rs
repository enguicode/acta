//! `MinutesGenerator`: una pasada si el transcript cabe; si no, map-reduce
//! (bloques de ~20 min → acta parcial → fusión con el mismo esquema).

use super::MinutesV1;
use super::meta::{MIN_TRANSCRIPT_WORDS, empty_minutes, strip_meta};
use super::convert::{ConvertCtx, convert, validate};
use crate::structured::complete_structured;
use crate::transcript::{
    Line, SAME_AS_MEETING, estimate_tokens, format_date, join_lines, me_label, mmss, of_user, output_language, render_lines, render_template,
    split_lines, user_ref,
};
use crate::{prompts, schemas};
use acta_core::PortResult;
use acta_core::detection::Platform;
use acta_core::llm::{LlmMessage, LlmProvider, LlmRequest, Role};
use acta_core::minutes::{MinutesGenerator, MinutesInput, MinutesOutput};
use acta_core::time::{Clock, MediaMs};
use async_trait::async_trait;
use std::sync::Arc;

/// Tokens que se reservan para la respuesta.
const OUTPUT_RESERVE: usize = 6_000;
/// Duración máxima de un bloque del map (ms).
const BLOCK_SPAN_MS: u64 = 20 * 60_000;

pub struct LlmMinutesGenerator {
    provider: Arc<dyn LlmProvider>,
    clock: Arc<dyn Clock>,
    model: Option<String>,
}

impl LlmMinutesGenerator {
    pub fn new(provider: Arc<dyn LlmProvider>, clock: Arc<dyn Clock>) -> Self {
        Self { provider, clock, model: None }
    }

    /// Modelo configurado (solo informativo, se guarda en `Minutes.model`).
    pub fn with_model(mut self, model: Option<String>) -> Self {
        self.model = model;
        self
    }

    fn usable_tokens(&self) -> usize {
        ((self.provider.capabilities().context_tokens as usize) * 7 / 10)
            .saturating_sub(OUTPUT_RESERVE)
            .max(1_500)
    }

    fn schema() -> serde_json::Value {
        serde_json::from_str(schemas::MINUTES_V1).unwrap_or(serde_json::Value::Null)
    }

    fn request(system: String, user: String) -> LlmRequest {
        LlmRequest {
            system: Some(system),
            messages: vec![LlmMessage { role: Role::User, content: user }],
            json_schema: Some(Self::schema()),
            max_tokens: Some(8_000),
            temperature: Some(0.2),
        }
    }

    async fn call(&self, system: String, user: String) -> PortResult<MinutesV1> {
        // El acta habla de la reunión, no de la grabación: ver `meta`.
        complete_structured::<MinutesV1, _>(self.provider.as_ref(), Self::request(system, user), validate).await.map(strip_meta)
    }
}

fn platform_name(p: Platform) -> &'static str {
    match p {
        Platform::Teams => "Microsoft Teams",
        Platform::Meet => "Google Meet",
        Platform::Zoom => "Zoom",
        Platform::Slack => "Slack",
        Platform::Other => "Otra / manual",
    }
}

/// Quién es el usuario para el modelo. Sin nombre en Ajustes, solo «Yo».
fn user_intro(me: &acta_core::minutes::UserIdentity) -> String {
    let name = me.display_name.trim();
    let aliases: Vec<&str> = me.aliases.iter().map(|a| a.trim()).filter(|a| !a.is_empty()).collect();
    if name.is_empty() {
        let also = if aliases.is_empty() { String::new() } else { format!(" (le llaman {})", aliases.join(", ")) };
        return format!("Las intervenciones del usuario{also} aparecen como \"Yo\".");
    }
    let also = if aliases.is_empty() { String::new() } else { format!(" (también llamado {})", aliases.join(", ")) };
    format!("\"{name}\"{also} es el usuario: sus intervenciones aparecen como \"{}\".", me_label(name))
}

/// «…o si otra persona se lo pide/asigna ___»: por su nombre si se sabe.
fn asked_by(me: &acta_core::minutes::UserIdentity) -> String {
    let names: Vec<&str> =
        std::iter::once(me.display_name.as_str()).chain(me.aliases.iter().map(String::as_str)).map(str::trim).filter(|n| !n.is_empty()).collect();
    if names.is_empty() { "hablándole directamente".to_owned() } else { format!("por su nombre o alias ({})", names.join(", ")) }
}

fn system_prompt(input: &MinutesInput) -> String {
    let name = &input.me.display_name;
    render_template(
        prompts::MINUTES_SYSTEM,
        &[
            ("of_user", &of_user(name)),
            ("user_intro", &user_intro(&input.me)),
            ("user_ref", user_ref(name)),
            ("me_label", &me_label(name)),
            ("asked_by", &asked_by(&input.me)),
            ("language", &output_language(&input.language, SAME_AS_MEETING)),
        ],
    )
}

fn captures_text(input: &MinutesInput, from: u64, to: u64) -> String {
    let lines: Vec<String> = input
        .captures
        .iter()
        .filter(|c| c.at.0 >= from && c.at.0 < to)
        .map(|c| format!("{} [{}] {}", c.ref_id, mmss(c.at), reason_name(c.reason)))
        .collect();
    if lines.is_empty() { "(ninguna)".into() } else { lines.join("\n") }
}

fn reason_name(r: acta_core::screen::CaptureReason) -> &'static str {
    use acta_core::screen::CaptureReason::*;
    match r {
        SlideChange => "slide_change",
        Keyword => "keyword",
        LlmPick => "llm_pick",
        Manual => "manual",
        Candidate => "candidate",
    }
}

fn user_prompt(input: &MinutesInput, transcript: &str, captures: &str, note: Option<&str>) -> String {
    let participants = input
        .speakers
        .iter()
        .map(|s| crate::transcript::speaker_label(s, &input.me.display_name))
        .collect::<Vec<_>>()
        .join(", ");
    let mut p = render_template(
        prompts::MINUTES_USER,
        &[
            ("title", &input.title),
            ("platform", platform_name(input.platform)),
            ("date", &format_date(input.started_at)),
            ("duration", &mmss(input.duration)),
            ("participants", &participants),
            ("captures", captures),
            ("transcript", transcript),
        ],
    );
    if let Some(n) = note {
        p = format!("{n}\n\n{p}");
    }
    match user_note(input) {
        Some(n) => format!("{}\n\n{p}", private_note_block(input, n)),
        None => p,
    }
}

/// Nota personal del usuario, si tiene contenido.
fn user_note(input: &MinutesInput) -> Option<&str> {
    input.user_note.as_deref().map(str::trim).filter(|n| !n.is_empty())
}

fn private_note_block(input: &MinutesInput, note: &str) -> String {
    let owner = if input.me.display_name.trim().is_empty() { "el usuario".to_owned() } else { input.me.display_name.trim().to_owned() };
    format!(
        "NOTA PERSONAL de {owner} (privada: la escribió {owner} para sí; NO la dijo ningún participante ni sale en la transcripción). \
         Úsala solo como contexto de lo que le importa. No se la atribuyas a nadie ni la repitas en el resumen: se añade aparte.\n«{note}»"
    )
}

/// Añade la nota al final del resumen, en su propia sección y tal cual.
fn with_note_section(mut out: MinutesOutput, input: &MinutesInput) -> MinutesOutput {
    if let Some(n) = user_note(input) {
        out.minutes.summary_md = format!("{}\n\n## Nota personal\n\n{n}", out.minutes.summary_md.trim_end());
    }
    out
}

const REDUCE_INSTRUCTIONS: &str = "\
TAREA DE FUSIÓN. Te paso actas parciales (JSON `acta.minutes.v1`) de fragmentos CONSECUTIVOS de una misma reunión larga. \
Fusiónalas en UN único objeto `acta.minutes.v1`: resumen coherente de toda la reunión (5-12 viñetas), decisiones y tareas \
sin duplicados (une las repetidas, conserva `es_mia`, responsable, fecha y `ts_ms`), escaleta unificada y ordenada con \
3-10 temas (une temas contiguos del mismo asunto; los tiempos son los originales), `momentos_clave` y refs de capturas \
(\"c1\"...) tal cual. No inventes nada que no esté en los parciales.";

#[async_trait]
impl MinutesGenerator for LlmMinutesGenerator {
    async fn generate(&self, input: MinutesInput) -> PortResult<MinutesOutput> {
        let lines = render_lines(&input.speakers, &input.utterances, &input.me.display_name);
        let system = system_prompt(&input);
        let usable = self.usable_tokens();

        let full_user = user_prompt(&input, &join_lines(&lines), &captures_text(&input, 0, u64::MAX), None);
        let words: usize = input.utterances.iter().map(|u| u.text.split_whitespace().count()).sum();
        let v1 = if words < MIN_TRANSCRIPT_WORDS {
            tracing::info!(words, "transcript sin contenido: acta neutra sin llamar al LLM");
            empty_minutes()
        } else if estimate_tokens(&system) + estimate_tokens(&full_user) <= usable {
            self.call(system, full_user).await?
        } else {
            tracing::info!(lines = lines.len(), usable, "transcript largo: map-reduce");
            self.map_reduce(&input, &lines, &system, usable).await?
        };

        let ctx = ConvertCtx { input: &input, provider: self.provider.id().clone(), model: self.model.clone(), now: self.clock.now() };
        Ok(with_note_section(convert(&ctx, v1), &input))
    }
}

impl LlmMinutesGenerator {
    async fn map_reduce(&self, input: &MinutesInput, lines: &[Line], system: &str, usable: usize) -> PortResult<MinutesV1> {
        // Presupuesto de transcript por bloque = lo utilizable menos el resto del prompt.
        let overhead = estimate_tokens(system) + estimate_tokens(&user_prompt(input, "", &captures_text(input, 0, u64::MAX), Some(NOTE_PLACEHOLDER)));
        let block_budget = (usable.saturating_sub(overhead) * 9 / 10).max(300);
        let blocks = split_lines(lines, block_budget, BLOCK_SPAN_MS);
        let n = blocks.len();

        let mut partials: Vec<MinutesV1> = Vec::with_capacity(n);
        for (i, block) in blocks.iter().enumerate() {
            let from = if i == 0 { 0 } else { block[0].at_ms };
            let to = blocks.get(i + 1).map_or(u64::MAX, |b| b[0].at_ms);
            let last_ms = block.last().map_or(from, |l| l.at_ms);
            let note = format!(
                "FRAGMENTO {} de {n} de una reunión larga (de {} a {}). Extrae SOLO lo de este tramo: \
                 `escaleta` cubre solo este tramo y los tiempos son los absolutos de las marcas.",
                i + 1,
                mmss(MediaMs(from)),
                mmss(MediaMs(last_ms)),
            );
            let user = user_prompt(input, &join_lines(block), &captures_text(input, from, to), Some(&note));
            partials.push(self.call(system.to_owned(), user).await?);
        }
        self.reduce(partials, system, usable).await
    }

    /// Fusiona parciales; si el JSON conjunto no cabe, fusiona por grupos y repite.
    async fn reduce(&self, mut partials: Vec<MinutesV1>, system: &str, usable: usize) -> PortResult<MinutesV1> {
        let reduce_system = format!("{system}\n\n{REDUCE_INSTRUCTIONS}");
        let budget = usable.saturating_sub(estimate_tokens(&reduce_system) + 200).max(500);
        loop {
            if partials.len() == 1 {
                return Ok(partials.remove(0));
            }
            let sizes: Vec<usize> = partials.iter().map(|p| estimate_tokens(&json_of(p))).collect();
            if sizes.iter().sum::<usize>() <= budget || partials.len() <= 2 {
                return self.call(reduce_system.clone(), reduce_prompt(&partials)).await;
            }
            // Agrupa en tandas que quepan y fusiona cada una.
            let mut next: Vec<MinutesV1> = Vec::new();
            let mut group: Vec<MinutesV1> = Vec::new();
            let mut group_size = 0usize;
            for (p, sz) in partials.drain(..).zip(sizes) {
                if !group.is_empty() && group_size + sz > budget {
                    next.push(self.merge_group(std::mem::take(&mut group), &reduce_system).await?);
                    group_size = 0;
                }
                group_size += sz;
                group.push(p);
            }
            if !group.is_empty() {
                next.push(self.merge_group(group, &reduce_system).await?);
            }
            partials = next;
        }
    }

    async fn merge_group(&self, mut group: Vec<MinutesV1>, reduce_system: &str) -> PortResult<MinutesV1> {
        if group.len() == 1 {
            return Ok(group.remove(0));
        }
        self.call(reduce_system.to_owned(), reduce_prompt(&group)).await
    }
}

const NOTE_PLACEHOLDER: &str = "FRAGMENTO 00 de 00 de una reunión larga (de 00:00 a 00:00). Extrae SOLO lo de este tramo: `escaleta` cubre solo este tramo y los tiempos son los absolutos de las marcas.";

fn json_of(p: &MinutesV1) -> String {
    serde_json::to_string(p).unwrap_or_default()
}

fn reduce_prompt(parts: &[MinutesV1]) -> String {
    let body = parts
        .iter()
        .enumerate()
        .map(|(i, p)| format!("## Parcial {}\n{}", i + 1, json_of(p)))
        .collect::<Vec<_>>()
        .join("\n\n");
    format!("{REDUCE_INSTRUCTIONS}\n\n{body}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use acta_core::minutes::UserIdentity;
    use acta_core::PortError;
    use crate::structured::testing::FakeProvider;
    use crate::testutil::{sample_input, utt};
    use acta_core::time::{ManualClock, UnixMs};

    const GOOD: &str = r#"{"titulo_sugerido":"Plan","resumen":"- Resumen","decisiones":[{"texto":"Lanzar el lunes","ts_ms":6000}],
        "tareas":[{"texto":"Enviar presupuesto","responsable":"Edu","es_mia":true,"fecha_limite":"viernes","ts_ms":6000,"capturas":["c1"]},
                  {"texto":"Preparar demo","responsable":"Ana","es_mia":false,"fecha_limite":null,"ts_ms":12000,"capturas":[]}],
        "escaleta":{"temas":[{"titulo":"Presupuesto","resumen":"r","inicio_ms":0,"fin_ms":14000,"subtemas":[],"capturas":["c1"]}]},
        "momentos_clave":[{"ts_ms":12000,"motivo":"fechas","tema":"Presupuesto"}]}"#;

    fn make_gen(p: Arc<FakeProvider>) -> LlmMinutesGenerator {
        LlmMinutesGenerator::new(p, Arc::new(ManualClock::new(UnixMs(1234)))).with_model(Some("m1".into()))
    }

    #[tokio::test]
    async fn single_pass_builds_domain_output() {
        let p = Arc::new(FakeProvider::new(vec![GOOD]));
        let out = make_gen(p.clone()).generate(sample_input()).await.expect("generate");
        assert_eq!(p.calls(), 1);
        assert_eq!(out.minutes.action_items.len(), 2);
        assert!(out.minutes.action_items[0].is_mine && !out.minutes.action_items[1].is_mine);
        assert_eq!(out.minutes.model.as_deref(), Some("m1"));
        assert_eq!(out.minutes.generated_at, UnixMs(1234));
        assert_eq!(out.key_moments[0].at, MediaMs(12_000));
        let req = &p.requests.lock().unwrap()[0];
        assert!(req.json_schema.is_some());
        let user = &req.messages[0].content;
        assert!(user.contains("c1 [00:08] slide_change"));
        assert!(user.contains("[00:06|6000] Edu (yo): Sí, yo me encargo."));
        assert!(user.contains("Ana García: Edu, ¿puedes"));
        let sys = req.system.as_ref().expect("system");
        assert!(sys.contains("Edu") && sys.contains("Eduardo, Edu Navajas") && sys.contains("castellano"));
        assert!(!sys.contains("{{"));
    }

    /// Instalación nueva: sin nombre y con idioma automático. El prompt no
    /// deja huecos ni presupone castellano, y "Yo" sigue marcando mis tareas.
    #[tokio::test]
    async fn the_prompt_works_without_a_name_and_follows_the_meeting_language() {
        let p = Arc::new(FakeProvider::new(vec![GOOD]));
        let mut input = sample_input();
        input.me = UserIdentity { display_name: String::new(), aliases: vec![] };
        input.language = "auto".into();
        let out = make_gen(p.clone()).generate(input.clone()).await.expect("generate");
        let req = p.requests.lock().unwrap()[0].clone();
        let sys = req.system.expect("system");
        assert!(!sys.contains("{{") && !sys.contains("\"\"") && !sys.contains("()") && !sys.contains("de ."), "{sys}");
        assert!(!sys.contains("Edu") && !sys.contains("castellano"), "{sys}");
        assert!(sys.contains("mismo idioma") && sys.contains("\"Yo\""), "{sys}");
        assert!(req.messages[0].content.contains("[00:06|6000] Yo: Sí, yo me encargo."));
        assert!(!req.messages[0].content.contains(" (yo)"));
        assert!(out.minutes.action_items.iter().all(|t| t.owner_name.as_deref() != Some("")));

        for (code, name) in [("en", "inglés"), ("es", "castellano de España"), ("fr", "francés"), ("ja", "japonés")] {
            input.language = code.into();
            let sys = system_prompt(&input);
            assert!(sys.contains(&format!("Escribe en {name}")), "{code}: {sys}");
        }
    }

    /// La nota personal es contexto privado del usuario: va al LLM marcada como
    /// tal y sale en el acta en su propia sección (nunca dicha por alguien).
    #[tokio::test]
    async fn the_users_private_note_is_context_for_the_llm_and_its_own_section_in_the_summary() {
        let p = Arc::new(FakeProvider::new(vec![GOOD]));
        let mut input = sample_input();
        input.user_note = Some("Pedir descuento a Ana".into());
        let out = make_gen(p.clone()).generate(input).await.expect("generate");
        let user = p.requests.lock().unwrap()[0].messages[0].content.clone();
        assert!(user.contains("NOTA PERSONAL") && user.contains("Pedir descuento a Ana"), "{user}");
        assert!(user.contains("NO la dijo ningún participante"), "{user}");
        assert!(out.minutes.summary_md.ends_with("## Nota personal\n\nPedir descuento a Ana"), "{}", out.minutes.summary_md);
        assert!(out.minutes.summary_md.starts_with("- Resumen"));
    }

    #[tokio::test]
    async fn without_a_note_the_prompt_and_summary_have_no_note_section() {
        for note in [None, Some("   ".to_owned())] {
            let p = Arc::new(FakeProvider::new(vec![GOOD]));
            let mut input = sample_input();
            input.user_note = note;
            let out = make_gen(p.clone()).generate(input).await.expect("generate");
            assert!(!p.requests.lock().unwrap()[0].messages[0].content.contains("NOTA PERSONAL"));
            assert!(!out.minutes.summary_md.contains("Nota personal"));
        }
    }

    #[tokio::test]
    async fn the_summary_never_comments_on_the_recording_or_the_tooling() {
        let p = Arc::new(FakeProvider::new(vec![include_str!("../../tests/fixtures/minutes_meta_commentary.json")]));
        let out = make_gen(p.clone()).generate(sample_input()).await.expect("generate");
        let md = &out.minutes.summary_md;
        assert!(md.contains("publicación de la versión nueva"), "{md}");
        for banned in ["(yo)", "transcripción", "Grabación muy breve", "único hablante", "capturas"] {
            assert!(!md.contains(banned), "«{banned}» en {md}");
        }
        assert_eq!(out.minutes.decisions.len(), 1);
        let sys = p.requests.lock().unwrap()[0].system.clone().expect("system");
        assert!(sys.contains("Prohibido comentar la grabación"));
        assert!(sys.contains(super::super::meta::EMPTY_SUMMARY));
    }

    #[tokio::test]
    async fn a_transcript_with_nothing_in_it_gets_one_neutral_line_without_calling_the_llm() {
        for utterances in [vec![], vec![utt(0, 1, 0, "Hola, ¿se oye?")]] {
            let mut input = sample_input();
            input.utterances = utterances;
            let p = Arc::new(FakeProvider::new(vec![]));
            let out = make_gen(p.clone()).generate(input).await.expect("generate");
            assert_eq!(p.calls(), 0);
            assert_eq!(out.minutes.summary_md, super::super::meta::EMPTY_SUMMARY);
            assert!(out.minutes.action_items.is_empty() && out.escaleta.temas.is_empty());
        }
    }

    #[tokio::test]
    async fn invalid_json_is_repaired_with_one_retry() {
        let p = Arc::new(FakeProvider::new(vec!["esto no es json", GOOD]));
        make_gen(p.clone()).generate(sample_input()).await.expect("generate");
        assert_eq!(p.calls(), 2);
    }

    #[tokio::test]
    async fn still_invalid_after_repair_is_an_error() {
        let p = Arc::new(FakeProvider::new(vec!["x", "y"]));
        let e = make_gen(p.clone()).generate(sample_input()).await.expect_err("err");
        assert!(matches!(e, PortError::InvalidResponse(_)));
        assert_eq!(p.calls(), 2);
    }

    fn long_input(minutes: u64) -> MinutesInput {
        let mut input = sample_input();
        input.utterances = (0..minutes)
            .map(|m| utt(m as u32, if m % 2 == 0 { 2 } else { 1 }, m * 60_000, &format!("Minuto {m}: {}", "palabra ".repeat(60))))
            .collect();
        input.duration = MediaMs(minutes * 60_000);
        input
    }

    #[tokio::test]
    async fn long_transcript_uses_map_reduce() {
        // Contexto pequeño: 70 min de texto no caben -> bloques + 1 fusión.
        // context 10k -> usable = 7000-6000 -> 1500 (mínimo).
        let input = long_input(70);
        let lines = render_lines(&input.speakers, &input.utterances, "Edu");
        let usable = 1_500usize;
        let overhead = estimate_tokens(&system_prompt(&input))
            + estimate_tokens(&user_prompt(&input, "", &captures_text(&input, 0, u64::MAX), Some(NOTE_PLACEHOLDER)));
        let n_blocks = split_lines(&lines, (usable.saturating_sub(overhead) * 9 / 10).max(300), BLOCK_SPAN_MS).len();
        assert!(n_blocks >= 3, "esperaba varios bloques, hay {n_blocks}");
        // n parciales + (posibles fusiones intermedias) + final: damos respuestas de sobra.
        let mut fp = FakeProvider::new(std::iter::repeat_n(GOOD, n_blocks * 3).collect());
        fp.context_tokens = 10_000;
        let p = Arc::new(fp);
        let out = make_gen(p.clone()).generate(input).await.expect("generate");
        let calls = p.calls();
        assert!(calls >= n_blocks, "map ({n_blocks}) + reduce, hubo {calls}");
        assert!(!out.minutes.summary_md.is_empty());
        let reqs = p.requests.lock().unwrap();
        assert!(reqs[0].messages[0].content.contains("FRAGMENTO 1 de "));
        assert!(reqs.last().expect("last").messages[0].content.contains("TAREA DE FUSIÓN"));
    }

    /// Reunión larga + nota: la nota viaja en cada fragmento (el LLM sabe qué le
    /// importa al usuario) pero la sección «Nota personal» sale UNA sola vez.
    #[tokio::test]
    async fn the_note_survives_map_reduce_and_appears_once() {
        let mut input = long_input(70);
        input.user_note = Some("Vigilar el presupuesto".into());
        let mut fp = FakeProvider::new(std::iter::repeat_n(GOOD, 600).collect());
        fp.context_tokens = 10_000;
        let p = Arc::new(fp);
        let out = make_gen(p.clone()).generate(input).await.expect("generate");
        let reqs = p.requests.lock().unwrap();
        let fragments: Vec<_> = reqs.iter().filter(|r| r.messages[0].content.contains("FRAGMENTO ")).collect();
        assert!(fragments.len() >= 3);
        assert!(fragments.iter().all(|r| r.messages[0].content.contains("Vigilar el presupuesto")));
        assert_eq!(out.minutes.summary_md.matches("## Nota personal").count(), 1);
        assert!(out.minutes.summary_md.ends_with("Vigilar el presupuesto"));
    }

    /// Una reunión sin contenido (acta neutra, sin llamar al LLM) conserva la nota.
    #[tokio::test]
    async fn the_note_is_kept_when_the_transcript_is_too_short_for_the_llm() {
        let p = Arc::new(FakeProvider::new(vec![]));
        let mut input = sample_input();
        input.utterances.truncate(1);
        input.utterances[0].text = "Hola".into();
        input.user_note = Some("Llamar a Ana".into());
        let out = make_gen(p.clone()).generate(input).await.expect("generate");
        assert_eq!(p.calls(), 0);
        assert!(out.minutes.summary_md.ends_with("## Nota personal\n\nLlamar a Ana"), "{}", out.minutes.summary_md);
    }

    #[tokio::test]
    async fn short_transcript_never_map_reduces() {
        let p = Arc::new(FakeProvider::new(vec![GOOD]));
        make_gen(p.clone()).generate(sample_input()).await.expect("generate");
        assert!(!p.requests.lock().unwrap()[0].messages[0].content.contains("FRAGMENTO"));
    }
}
