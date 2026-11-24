//! `TranscriptCleaner` (esquema `acta.transcript_cleanup.v1`): pasada del LLM
//! sobre el texto del ASR para arreglar jerga mal oída (con el glosario del
//! usuario), puntuación y mayúsculas. Reglas portadas de la limpieza de dictado
//! de Orca: no añadir contenido, no traducir, no responder al texto.
//!
//! Garantías (el transcript es lo que el usuario cree que dijo, así que ante
//! cualquier duda gana el texto crudo):
//! - se piden tandas de segmentos y la respuesta debe traer los MISMOS ids en
//!   el MISMO orden (mismo número); si no, la tanda entera se queda cruda;
//! - un segmento cuyo texto cambia de forma implausible (vacío, con saltos de
//!   línea, muy distinto en número de palabras) se queda crudo, sin arrastrar a
//!   sus vecinos;
//! - un fallo del proveedor o un tiempo agotado solo cuesta esa tanda.
//!
//! El resultado es `Option` por texto: `None` = no se limpió (crudo), y el que
//! llama NO lo cachea.

use crate::structured::complete_structured;
use crate::transcript::{output_language, render_template};
use crate::{prompts, schemas};
use acta_core::PortResult;
use acta_core::llm::{LlmMessage, LlmProvider, LlmRequest, Role};
use acta_core::transcription::{CleanupInput, TranscriptCleaner};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;

/// Caracteres de texto por tanda (≈ 1 500 tokens de entrada y otros tantos de salida).
pub const MAX_BATCH_CHARS: usize = 6_000;
/// Segmentos por tanda: con muchos cortos, el modelo empieza a perder ids.
pub const MAX_BATCH_ITEMS: usize = 40;
/// Términos del glosario que entran en el prompt.
const MAX_TERMS: usize = 200;
/// Tope por tanda (con el reintento de reparación incluido).
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(180);
/// Cambio máximo de palabras por segmento (fracción), con 2 palabras de holgura.
const MAX_WORD_DRIFT: f32 = 0.3;

/// Cambia con las reglas del prompt: invalida lo ya limpiado en caché.
const VERSION: &str = "1";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CleanupV1 {
    segmentos: Vec<CleanedSegment>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CleanedSegment {
    id: u32,
    texto: String,
}

#[derive(Serialize)]
struct Request<'a> {
    terminos: &'a [String],
    segmentos: Vec<RequestSegment<'a>>,
}

#[derive(Serialize)]
struct RequestSegment<'a> {
    id: u32,
    texto: &'a str,
}

pub struct LlmTranscriptCleaner {
    provider: Arc<dyn LlmProvider>,
    timeout: Duration,
}

impl LlmTranscriptCleaner {
    pub fn new(provider: Arc<dyn LlmProvider>) -> Self {
        Self { provider, timeout: DEFAULT_TIMEOUT }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Índices de `texts` agrupados en tandas contiguas.
    pub(crate) fn batches(&self, texts: &[String]) -> Vec<Vec<usize>> {
        let mut out: Vec<Vec<usize>> = Vec::new();
        let mut chars = 0usize;
        for (i, t) in texts.iter().enumerate() {
            let n = t.chars().count();
            match out.last_mut() {
                Some(b) if b.len() < MAX_BATCH_ITEMS && chars + n <= MAX_BATCH_CHARS => {
                    b.push(i);
                    chars += n;
                }
                _ => {
                    out.push(vec![i]);
                    chars = n;
                }
            }
        }
        out
    }

    async fn clean_batch(&self, texts: &[&str], terms: &[String], language: &str) -> PortResult<Vec<Option<String>>> {
        let body = Request {
            terminos: terms,
            segmentos: texts.iter().enumerate().map(|(i, t)| RequestSegment { id: i as u32, texto: t }).collect(),
        };
        let request = LlmRequest {
            system: Some(render_template(
                prompts::TRANSCRIPT_CLEANUP_SYSTEM,
                &[("language_line", &language_line(language))],
            )),
            messages: vec![LlmMessage { role: Role::User, content: serde_json::to_string(&body)? }],
            json_schema: serde_json::from_str(schemas::TRANSCRIPT_CLEANUP_V1).ok(),
            max_tokens: Some(u32::try_from(texts.iter().map(|t| t.len()).sum::<usize>() / 2 + 400).unwrap_or(8_000).min(16_000)),
            temperature: Some(0.0),
        };
        let n = texts.len();
        let v: CleanupV1 = complete_structured(self.provider.as_ref(), request, |v: &CleanupV1| {
            if v.segmentos.len() != n {
                return Err(format!("devolviste {} segmentos y había {n}: debe haber exactamente uno por cada segmento de entrada", v.segmentos.len()));
            }
            match v.segmentos.iter().enumerate().find(|(i, s)| s.id as usize != *i) {
                Some((i, s)) => Err(format!("el segmento en la posición {i} tiene id {}: deben ir los ids {}..{} en orden", s.id, 0, n - 1)),
                None => Ok(()),
            }
        })
        .await?;
        Ok(v.segmentos.iter().zip(texts).map(|(c, raw)| plausible(raw, &c.texto)).collect())
    }
}

fn language_line(language: &str) -> String {
    let named = output_language(language, "");
    if named.is_empty() { String::new() } else { format!("Las reuniones están en {named}.") }
}

/// El texto corregido, o `None` si se parece más a una reescritura que a una limpieza.
fn plausible(raw: &str, cleaned: &str) -> Option<String> {
    let cleaned = cleaned.trim();
    if cleaned.is_empty() || cleaned.contains(['\n', '\r']) {
        return None;
    }
    let (a, b) = (raw.split_whitespace().count() as f32, cleaned.split_whitespace().count() as f32);
    if (a - b).abs() > a * MAX_WORD_DRIFT + 2.0 {
        return None;
    }
    Some(cleaned.to_owned())
}

#[async_trait]
impl TranscriptCleaner for LlmTranscriptCleaner {
    async fn clean(&self, input: CleanupInput) -> Vec<Option<String>> {
        let mut out: Vec<Option<String>> = vec![None; input.texts.len()];
        let mut terms: Vec<String> = Vec::new();
        for t in input.glossary.iter().map(|t| t.trim()).filter(|t| !t.is_empty()) {
            if !terms.iter().any(|x| x == t) && terms.len() < MAX_TERMS {
                terms.push(t.to_owned());
            }
        }
        for batch in self.batches(&input.texts) {
            let texts: Vec<&str> = batch.iter().map(|&i| input.texts[i].as_str()).collect();
            match tokio::time::timeout(self.timeout, self.clean_batch(&texts, &terms, &input.language)).await {
                Ok(Ok(cleaned)) => {
                    for (&i, c) in batch.iter().zip(cleaned) {
                        out[i] = c;
                    }
                }
                Ok(Err(e)) => tracing::warn!(segments = batch.len(), "limpieza del transcript fallida; se queda el texto crudo: {e}"),
                Err(_) => tracing::warn!(segments = batch.len(), "limpieza del transcript: tiempo agotado; se queda el texto crudo"),
            }
        }
        out
    }

    fn version(&self) -> String {
        VERSION.into()
    }
}

#[cfg(test)]
mod tests;
