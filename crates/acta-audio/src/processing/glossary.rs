//! Glosario del usuario (`transcription.glossary`): corrige a posteriori los
//! términos de jerga que el ASR escribe mal. Parakeet TDT v3 no admite prompt
//! inicial ni vocabulario, así que se corrige el texto con coincidencia
//! aproximada (distancia de edición sobre palabra sin acentos ni mayúsculas).
//! Lógica PURA y conservadora: no toca palabras cortas ni las que ya están bien.

use acta_core::transcription::AsrSegment;

fn fold(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'á' | 'à' | 'ä' | 'â' => 'a',
            'é' | 'è' | 'ë' | 'ê' => 'e',
            'í' | 'ì' | 'ï' | 'î' => 'i',
            'ó' | 'ò' | 'ö' | 'ô' => 'o',
            'ú' | 'ù' | 'ü' | 'û' => 'u',
            c => c,
        })
        .collect::<String>()
        .to_lowercase()
}

fn edit_distance(a: &[char], b: &[char]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            cur.push((prev[j] + usize::from(ca != cb)).min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

/// Errores tolerados según la longitud del término (en caracteres).
fn tolerance(len: usize) -> usize {
    match len {
        0..=5 => 0,
        6..=8 => 1,
        _ => 2,
    }
}

fn best_term<'a>(word: &str, terms: &'a [(String, Vec<char>)], originals: &'a [String]) -> Option<&'a str> {
    let folded: Vec<char> = fold(word).chars().collect();
    let mut best: Option<(usize, usize)> = None;
    for (i, (_, t)) in terms.iter().enumerate() {
        let d = edit_distance(&folded, t);
        if d == 0 {
            return (word != originals[i]).then_some(originals[i].as_str()); // grafía canónica
        }
        if d <= tolerance(t.len()) && folded.len() >= 5 && best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, i));
        }
    }
    best.map(|(_, i)| originals[i].as_str())
}

/// Sustituye en `text` las palabras casi iguales a un término del glosario
/// (un término de una sola palabra; los compuestos se comparan por palabra suelta).
pub fn correct(text: &str, glossary: &[String]) -> String {
    let originals: Vec<String> = glossary.iter().map(|t| t.trim().to_owned()).filter(|t| !t.is_empty() && !t.contains(' ')).collect();
    if originals.is_empty() {
        return text.to_owned();
    }
    let terms: Vec<(String, Vec<char>)> = originals.iter().map(|t| (t.clone(), fold(t).chars().collect())).collect();
    let mut out = String::with_capacity(text.len());
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut String| {
        if !word.is_empty() {
            out.push_str(best_term(word, &terms, &originals).unwrap_or(word));
            word.clear();
        }
    };
    for c in text.chars() {
        if c.is_alphanumeric() {
            word.push(c);
        } else {
            flush(&mut word, &mut out);
            out.push(c);
        }
    }
    flush(&mut word, &mut out);
    out
}

/// Aplica el glosario al texto de cada segmento ASR.
pub fn apply(segments: &mut [AsrSegment], glossary: &[String]) {
    if glossary.iter().all(|t| t.trim().is_empty()) {
        return;
    }
    for s in segments {
        s.text = correct(&s.text, glossary);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(t: &[&str]) -> Vec<String> {
        t.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn near_miss_of_a_glossary_term_is_corrected() {
        assert_eq!(correct("vamos a hacer el deploy en kubernetes", &g(&["Kubernetes"])), "vamos a hacer el deploy en Kubernetes");
        assert_eq!(correct("el kubernetis falló", &g(&["Kubernetes"])), "el Kubernetes falló");
        assert_eq!(correct("usamos Terraform y Grafana.", &g(&["Terraform", "Grafana"])), "usamos Terraform y Grafana.");
        assert_eq!(correct("la grafana, no", &g(&["Grafana"])), "la Grafana, no", "mismas letras: se pone la grafía del glosario");
    }

    #[test]
    fn short_or_unrelated_words_are_left_alone() {
        assert_eq!(correct("hola que tal", &g(&["Jira"])), "hola que tal");
        assert_eq!(correct("pedros y pablo", &g(&["Pedro"])), "pedros y pablo");
        assert_eq!(correct("la reunión de ventas", &g(&["Kubernetes"])), "la reunión de ventas");
        assert_eq!(correct("nada que cambiar", &[]), "nada que cambiar");
        assert_eq!(correct("nada que cambiar", &g(&["", "  "])), "nada que cambiar");
    }

    #[test]
    fn accents_do_not_count_as_errors() {
        assert_eq!(correct("la integracion con Sección", &g(&["integración"])), "la integración con Sección");
    }

    #[test]
    fn apply_rewrites_every_segment() {
        let mut segs = vec![AsrSegment { start_ms: 0, end_ms: 1, text: "el kubernetis".into(), confidence: None, words: vec![] }];
        apply(&mut segs, &g(&["Kubernetes"]));
        assert_eq!(segs[0].text, "el Kubernetes");
    }
}
