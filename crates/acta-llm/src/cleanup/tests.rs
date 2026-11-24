use super::*;
use crate::structured::testing::FakeProvider;
use acta_core::PortError;
use acta_core::ids::ProviderId;
use acta_core::llm::*;
use async_trait::async_trait;
use std::sync::Arc;
use std::time::Duration;

fn input(texts: &[&str]) -> CleanupInput {
    CleanupInput {
        texts: texts.iter().map(|t| (*t).to_owned()).collect(),
        glossary: vec!["Jira".into(), "Kubernetes".into()],
        language: "es".into(),
    }
}

fn reply(items: &[(u32, &str)]) -> String {
    serde_json::json!({ "segmentos": items.iter().map(|(id, t)| serde_json::json!({"id": id, "texto": t})).collect::<Vec<_>>() }).to_string()
}

fn cleaner(p: &Arc<FakeProvider>) -> LlmTranscriptCleaner {
    LlmTranscriptCleaner::new(p.clone())
}

#[tokio::test]
async fn corrected_texts_come_back_one_per_input_in_order() {
    let p = Arc::new(FakeProvider::new(vec![&reply(&[(0, "Las tareas en Jira."), (1, "Vale, perfecto.")])]));
    let out = cleaner(&p).clean(input(&["las tareas en gira", "vale perfecto"])).await;
    assert_eq!(out, vec![Some("Las tareas en Jira.".into()), Some("Vale, perfecto.".into())]);
    assert_eq!(p.calls(), 1);
}

#[tokio::test]
async fn the_request_carries_glossary_language_rules_and_a_strict_schema() {
    let p = Arc::new(FakeProvider::new(vec![&reply(&[(0, "Hola.")])]));
    cleaner(&p).clean(input(&["hola"])).await;
    let req = p.requests.lock().unwrap()[0].clone();
    let system = req.system.expect("system");
    assert!(system.contains("NO traduzcas") && system.contains("no fusiones"), "{system}");
    assert!(!system.contains("{{"), "{system}");
    let user = &req.messages[0].content;
    assert!(user.contains("Jira") && user.contains("Kubernetes"), "{user}");
    assert!(user.contains(r#""id":0"#) && user.contains("hola"), "{user}");
    assert!(req.json_schema.is_some());
}

#[tokio::test]
async fn the_language_is_named_when_known_and_left_open_on_auto() {
    let p = Arc::new(FakeProvider::new(vec![&reply(&[(0, "Hola.")]), &reply(&[(0, "Hola.")])]));
    cleaner(&p).clean(input(&["hola"])).await;
    let mut auto = input(&["hola"]);
    auto.language = "auto".into();
    cleaner(&p).clean(auto).await;
    let reqs = p.requests.lock().unwrap();
    assert!(reqs[0].system.as_ref().unwrap().contains("castellano"), "{:?}", reqs[0].system);
    assert!(!reqs[1].system.as_ref().unwrap().contains("castellano"));
}

#[tokio::test]
async fn a_changed_segment_count_keeps_the_raw_text_for_the_whole_batch() {
    // Fusionó dos segmentos en uno, también tras el reintento de reparación.
    let bad = reply(&[(0, "Las tareas en Jira. Vale, perfecto.")]);
    let p = Arc::new(FakeProvider::new(vec![&bad, &bad]));
    let out = cleaner(&p).clean(input(&["las tareas en gira", "vale perfecto"])).await;
    assert_eq!(out, vec![None, None]);
}

#[tokio::test]
async fn swapped_or_unknown_ids_keep_the_raw_text() {
    let swapped = reply(&[(1, "Vale, perfecto."), (0, "Las tareas en Jira.")]);
    let unknown = reply(&[(0, "Las tareas en Jira."), (7, "Vale, perfecto.")]);
    let p = Arc::new(FakeProvider::new(vec![&swapped, &swapped, &unknown, &unknown]));
    assert_eq!(cleaner(&p).clean(input(&["a b c d", "e f g h"])).await, vec![None, None]);
    assert_eq!(cleaner(&p).clean(input(&["a b c d", "e f g h"])).await, vec![None, None]);
}

#[tokio::test]
async fn a_first_invalid_reply_is_repaired_once() {
    let p = Arc::new(FakeProvider::new(vec!["esto no es json", &reply(&[(0, "Hola.")])]));
    let out = cleaner(&p).clean(input(&["hola"])).await;
    assert_eq!(out, vec![Some("Hola.".into())]);
    assert_eq!(p.calls(), 2);
}

#[tokio::test]
async fn a_provider_error_keeps_the_raw_text_and_never_panics() {
    let p = Arc::new(FakeProvider::with_results(vec![Err(PortError::Unavailable("claude caído".into()))]));
    assert_eq!(cleaner(&p).clean(input(&["hola", "adiós"])).await, vec![None, None]);
}

#[tokio::test]
async fn an_implausible_rewrite_of_one_segment_is_dropped_but_its_neighbours_stay() {
    // 0: respondió a la pregunta en vez de limpiarla; 1: bien; 2: lo vació; 3: metió un salto de línea.
    let r = reply(&[
        (0, "Claro que sí, aquí tienes una explicación muy larga de cómo funciona Kubernetes por dentro y por fuera."),
        (1, "Vale, perfecto."),
        (2, ""),
        (3, "uno\ndos tres cuatro"),
    ]);
    let p = Arc::new(FakeProvider::new(vec![&r]));
    let out = cleaner(&p).clean(input(&["qué es kubernetes", "vale perfecto", "algo largo aquí", "uno dos tres cuatro"])).await;
    assert_eq!(out, vec![None, Some("Vale, perfecto.".into()), None, None]);
}

#[tokio::test]
async fn casing_and_punctuation_changes_are_accepted() {
    let p = Arc::new(FakeProvider::new(vec![&reply(&[(0, "Necesitamos cerrar el presupuesto, ¿vale?")])]));
    let out = cleaner(&p).clean(input(&["necesitamos cerrar el presupuesto vale"])).await;
    assert_eq!(out, vec![Some("Necesitamos cerrar el presupuesto, ¿vale?".into())]);
}

#[test]
fn long_transcripts_split_into_contiguous_bounded_batches() {
    let long = "palabra ".repeat(300).trim().to_owned(); // 2 399 caracteres
    let texts: Vec<String> = (0..6).map(|_| long.clone()).collect(); // 14 394 caracteres
    let c = LlmTranscriptCleaner::new(Arc::new(FakeProvider::new(vec![])));
    let batches = c.batches(&texts);
    assert!(batches.len() >= 3, "{batches:?}");
    assert!(batches.iter().all(|b| b.iter().map(|&i| texts[i].chars().count()).sum::<usize>() <= MAX_BATCH_CHARS));
    assert_eq!(batches.concat(), (0..6).collect::<Vec<_>>()); // sin huecos ni repeticiones
    let many: Vec<String> = (0..(MAX_BATCH_ITEMS * 2 + 1)).map(|i| format!("t{i}")).collect();
    assert!(c.batches(&many).iter().all(|b| b.len() <= MAX_BATCH_ITEMS));
}

#[tokio::test]
async fn a_failed_batch_leaves_the_others_cleaned() {
    let texts: Vec<String> = (0..(MAX_BATCH_ITEMS + 3)).map(|i| format!("texto número {i}")).collect();
    let cleaned: Vec<String> = (0..MAX_BATCH_ITEMS).map(|i| format!("Texto número {i}.")).collect();
    let first = reply(&cleaned.iter().enumerate().map(|(i, t)| (i as u32, t.as_str())).collect::<Vec<_>>());
    // La segunda tanda falla dos veces (respuesta + reparación).
    let p = Arc::new(FakeProvider::new(vec![&first, "basura", "basura"]));
    let out = cleaner(&p)
        .clean(CleanupInput { texts: texts.clone(), glossary: vec![], language: "es".into() })
        .await;
    assert_eq!(out.len(), texts.len());
    assert!(out[..MAX_BATCH_ITEMS].iter().all(Option::is_some));
    assert!(out[MAX_BATCH_ITEMS..].iter().all(Option::is_none));
}

#[tokio::test]
async fn empty_input_makes_no_calls() {
    let p = Arc::new(FakeProvider::new(vec![]));
    assert!(cleaner(&p).clean(input(&[])).await.is_empty());
    assert_eq!(p.calls(), 0);
}

struct Slow;

#[async_trait]
impl LlmProvider for Slow {
    fn id(&self) -> &ProviderId {
        static ID: std::sync::OnceLock<ProviderId> = std::sync::OnceLock::new();
        ID.get_or_init(|| "slow".into())
    }
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities { streaming: false, native_json_schema: true, context_tokens: 100_000 }
    }
    async fn complete(&self, _r: LlmRequest) -> PortResult<LlmResponse> {
        tokio::time::sleep(Duration::from_secs(3600)).await;
        unreachable!()
    }
    async fn stream(&self, _r: LlmRequest) -> PortResult<acta_core::EventStream<PortResult<String>>> {
        unreachable!()
    }
}

#[tokio::test]
async fn a_provider_that_never_answers_times_out_and_keeps_the_raw_text() {
    let c = LlmTranscriptCleaner::new(Arc::new(Slow)).with_timeout(Duration::from_millis(50));
    let out = c.clean(input(&["hola", "adiós"])).await;
    assert_eq!(out, vec![None, None]);
}
