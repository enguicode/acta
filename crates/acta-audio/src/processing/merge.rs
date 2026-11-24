//! Fusión del transcript (docs/01-arquitectura.md §6.4). Lógica PURA.
//!
//! - Pista `mic` → "Yo", salvo el bleed: lo que el micro oye de los altavoces
//!   (el remoto sonando en la sala) NO es "Yo". Un segmento `mic` solo es
//!   "Yo" si no lo explica el audio `system` de la misma ventana (ver
//!   `resolve_bleed`): la misma frase en ambas pistas se queda con la del
//!   sistema salvo que el micro domine con claridad (+6 dB sobre el sistema).
//! - Pista `system` → cada segmento ASR va al turno de diarización con mayor
//!   solape (o al más cercano si no solapa ninguno).
//! - Nombre de cada hablante diarizado, por orden: nombre confirmado antes
//!   por el usuario (reprocesado) > huella de voz > hablante activo de Teams
//!   con mayor solape > "Speaker N" (luego `DefaultPipeline` pide al
//!   `SpeakerNamer` el nombre que se dice en la reunión para los genéricos).
//! - Duplicados: mismo tramo + texto parecido en `mic` y `system` → se queda
//!   uno. Si el micro domina, es la voz del usuario saliendo por los altavoces
//!   (se descarta el `system`); si no, es bleed (se descarta el `mic`).

use super::energy::TrackLevels;
use acta_core::ids::{MeetingId, SpeakerId, VoiceprintId};
use acta_core::recording::Track;
use acta_core::time::MediaMs;
use acta_core::transcription::{AsrSegment, DiarizationResult, NameSource, Speaker, Transcript, Utterance};
use std::collections::{HashMap, HashSet};

/// Tramo en el que Teams marcó a `names` como hablando (ya en tiempo media).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveSpan {
    pub start: MediaMs,
    pub end: MediaMs,
    pub names: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct MergeInput {
    pub mic: Vec<AsrSegment>,
    pub system: Vec<AsrSegment>,
    pub diarization: Option<DiarizationResult>,
    pub active: Vec<ActiveSpan>,
    /// Nombres que son "yo" en la UI de la app (roster `is_self` + alias).
    pub self_names: Vec<String>,
    /// Coincidencias de huella por etiqueta del diarizador.
    pub voiceprints: HashMap<String, (VoiceprintId, String)>,
    /// Transcript anterior (reprocesado): conserva ids y nombres confirmados.
    pub previous: Option<Transcript>,
    /// Energía de ambas pistas: sin ella no se sabe si el micro domina y los
    /// duplicados se resuelven a favor del sistema.
    pub levels: Option<TrackLevels>,
}

/// Solape mínimo con el hablante activo de Teams para ponerle su nombre.
const MIN_UI_OVERLAP_MS: u64 = 1_000;
/// Solape (sobre el segmento más corto) y parecido de texto para llamar
/// "gemelos" a un segmento `mic` y otro `system`. El bleed llega distorsionado,
/// así que el parecido es más laxo que para un eco limpio.
const TWIN_OVERLAP: f64 = 0.5;
const TWIN_TEXT_SIMILARITY: f64 = 0.6;
/// El micro domina (voz propia) si supera al sistema en al menos esto (dB).
const MIC_DOMINANT_DB: f32 = 6.0;
/// Sin gemelo: el segmento `mic` es bleed si el sistema suena y el micro va
/// al menos esto por debajo (dB).
const BLEED_ONLY_DB: f32 = -12.0;
/// Por debajo de este nivel el sistema cuenta como callado.
const SYSTEM_ACTIVE_DB: f32 = -50.0;

fn overlap(a0: u64, a1: u64, b0: u64, b1: u64) -> u64 {
    a1.min(b1).saturating_sub(a0.max(b0))
}

fn norm_words(s: &str) -> Vec<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Jaccard de palabras.
fn similarity(a: &str, b: &str) -> f64 {
    let a: HashSet<String> = norm_words(a).into_iter().collect();
    let b: HashSet<String> = norm_words(b).into_iter().collect();
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    let inter = a.intersection(&b).count() as f64;
    let union = a.union(&b).count() as f64;
    inter / union
}

fn are_twins(a: &AsrSegment, b: &AsrSegment) -> bool {
    let shorter = a.end_ms.saturating_sub(a.start_ms).min(b.end_ms.saturating_sub(b.start_ms)).max(1);
    let ov = overlap(a.start_ms, a.end_ms, b.start_ms, b.end_ms);
    ov as f64 / shorter as f64 >= TWIN_OVERLAP && similarity(&a.text, &b.text) >= TWIN_TEXT_SIMILARITY
}

/// ¿El micro suena claramente por encima del sistema en el tramo del segmento?
/// Sin niveles no se puede afirmar.
fn mic_dominates(seg: &AsrSegment, levels: Option<&TrackLevels>) -> bool {
    levels
        .and_then(|l| l.window(seg.start_ms, seg.end_ms))
        .is_some_and(|w| w.mic_over_system() >= MIC_DOMINANT_DB)
}

/// Segmento `mic` sin gemelo en el sistema que aun así es el remoto colándose:
/// el sistema suena y el micro va muy por debajo.
fn is_bleed_only(seg: &AsrSegment, levels: Option<&TrackLevels>) -> bool {
    levels
        .and_then(|l| l.window(seg.start_ms, seg.end_ms))
        .is_some_and(|w| w.system_db > SYSTEM_ACTIVE_DB && w.mic_over_system() <= BLEED_ONLY_DB)
}

/// Resuelve el bleed y los duplicados: devuelve los segmentos `mic` que
/// siguen siendo "Yo" y los `system` que se mantienen.
fn resolve_bleed<'a>(
    mic: &'a [AsrSegment],
    system: Vec<&'a AsrSegment>,
    levels: Option<&TrackLevels>,
) -> (Vec<&'a AsrSegment>, Vec<&'a AsrSegment>) {
    let mut drop_system: HashSet<usize> = HashSet::new();
    let mut keep_mic = Vec::new();
    for m in mic {
        let twins: Vec<usize> = system.iter().enumerate().filter(|(_, s)| are_twins(m, s)).map(|(i, _)| i).collect();
        if twins.is_empty() {
            if !is_bleed_only(m, levels) {
                keep_mic.push(m);
            }
        } else if mic_dominates(m, levels) {
            drop_system.extend(twins);
            keep_mic.push(m);
        }
    }
    let keep_system = system
        .into_iter()
        .enumerate()
        .filter(|(i, _)| !drop_system.contains(i))
        .map(|(_, s)| s)
        .collect();
    (keep_mic, keep_system)
}

fn names_eq(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}

/// Etiqueta del turno con mayor solape; si no solapa ninguno, el más cercano.
fn label_for(seg: &AsrSegment, d: &DiarizationResult) -> Option<String> {
    let best = d
        .turns
        .iter()
        .map(|t| (overlap(seg.start_ms, seg.end_ms, t.start_ms, t.end_ms), t))
        .filter(|(ov, _)| *ov > 0)
        .max_by_key(|(ov, _)| *ov)
        .map(|(_, t)| t.label.clone());
    best.or_else(|| {
        d.turns
            .iter()
            .min_by_key(|t| {
                if seg.end_ms <= t.start_ms {
                    t.start_ms - seg.end_ms
                } else {
                    seg.start_ms.saturating_sub(t.end_ms)
                }
            })
            .map(|t| t.label.clone())
    })
}

/// Para cada etiqueta, el nombre de Teams con más solape (cada nombre a una
/// sola etiqueta, de mayor a menor solape).
fn ui_names(d: &DiarizationResult, active: &[ActiveSpan], self_names: &[String]) -> HashMap<String, String> {
    let mut scores: HashMap<(String, String), u64> = HashMap::new();
    for t in &d.turns {
        for a in active {
            let ov = overlap(t.start_ms, t.end_ms, a.start.0, a.end.0);
            if ov == 0 {
                continue;
            }
            for n in &a.names {
                if self_names.iter().any(|s| names_eq(s, n)) || n.trim().is_empty() {
                    continue;
                }
                *scores.entry((t.label.clone(), n.trim().to_owned())).or_default() += ov;
            }
        }
    }
    let mut ranked: Vec<((String, String), u64)> = scores.into_iter().filter(|(_, v)| *v >= MIN_UI_OVERLAP_MS).collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let mut out = HashMap::new();
    let mut used = HashSet::new();
    for ((label, name), _) in ranked {
        if out.contains_key(&label) || used.contains(&name) {
            continue;
        }
        used.insert(name.clone());
        out.insert(label, name);
    }
    out
}

pub fn merge_transcript(meeting_id: MeetingId, input: MergeInput) -> Transcript {
    let prev_speakers = input.previous.as_ref().map(|t| t.speakers.clone()).unwrap_or_default();
    let prev_for = |track: Track, label: Option<&str>| {
        prev_speakers
            .iter()
            .find(|s| s.track == track && s.diarization_label.as_deref() == label)
            .cloned()
    };

    let mut speakers: Vec<Speaker> = Vec::new();
    let mut utterances: Vec<Utterance> = Vec::new();

    let system_all: Vec<&AsrSegment> = input.system.iter().filter(|s| !s.text.trim().is_empty()).collect();
    let (mic_kept, system) = resolve_bleed(&input.mic, system_all, input.levels.as_ref());

    // --- Yo ---
    if !mic_kept.is_empty() {
        let me = prev_for(Track::Mic, None);
        let id = me.as_ref().map_or_else(SpeakerId::new, |s| s.id);
        speakers.push(Speaker {
            id,
            meeting_id,
            label: "Yo".into(),
            display_name: me.as_ref().and_then(|s| s.display_name.clone()),
            is_me: true,
            track: Track::Mic,
            diarization_label: None,
            name_source: NameSource::Me,
            confirmed: me.as_ref().is_some_and(|s| s.confirmed),
            voiceprint_id: None,
        });
        for s in &mic_kept {
            utterances.push(utt(id, Track::Mic, s));
        }
    }

    // --- Otros ---
    let diar = input.diarization.as_ref().filter(|d| !d.turns.is_empty());
    let assigned: Vec<(Option<String>, &AsrSegment)> = system
        .iter()
        .map(|s| (diar.and_then(|d| label_for(s, d)), *s))
        .collect();
    // Etiquetas por orden de aparición.
    let mut order: Vec<Option<String>> = Vec::new();
    for (l, _) in &assigned {
        if !order.contains(l) {
            order.push(l.clone());
        }
    }
    let from_ui = diar.map(|d| ui_names(d, &input.active, &input.self_names)).unwrap_or_default();
    let mut ids: HashMap<Option<String>, SpeakerId> = HashMap::new();
    for (n, label) in order.iter().enumerate() {
        let prev = prev_for(Track::System, label.as_deref());
        let id = prev.as_ref().map_or_else(SpeakerId::new, |s| s.id);
        ids.insert(label.clone(), id);
        let generic = format!("Speaker {}", n + 1);
        let (display_name, name_source, confirmed, voiceprint_id) = match (&prev, label) {
            (Some(p), _) if p.confirmed => (p.display_name.clone(), p.name_source, true, p.voiceprint_id),
            (_, Some(l)) if input.voiceprints.contains_key(l) => {
                let (vp, name) = &input.voiceprints[l];
                (Some(name.clone()), NameSource::Voiceprint, false, Some(*vp))
            }
            (_, Some(l)) if from_ui.contains_key(l) => (Some(from_ui[l].clone()), NameSource::AppUi, false, None),
            _ => (None, NameSource::Generic, false, None),
        };
        speakers.push(Speaker {
            id,
            meeting_id,
            label: generic,
            display_name,
            is_me: false,
            track: Track::System,
            diarization_label: label.clone(),
            name_source,
            confirmed,
            voiceprint_id,
        });
    }
    for (label, s) in assigned {
        if let Some(id) = ids.get(&label) {
            utterances.push(utt(*id, Track::System, s));
        }
    }

    utterances.sort_by_key(|u| (u.start, u.track, u.end));
    for (i, u) in utterances.iter_mut().enumerate() {
        u.idx = i as u32;
    }
    Transcript {
        meeting_id,
        speakers,
        utterances,
    }
}

fn utt(speaker_id: SpeakerId, track: Track, s: &AsrSegment) -> Utterance {
    Utterance {
        idx: 0,
        speaker_id,
        track,
        start: MediaMs(s.start_ms),
        end: MediaMs(s.end_ms.max(s.start_ms)),
        text: s.text.trim().to_owned(),
        confidence: s.confidence,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use acta_core::transcription::{DiarizationTurn, DiarizedSpeaker};

    fn seg(a: u64, b: u64, t: &str) -> AsrSegment {
        AsrSegment { start_ms: a, end_ms: b, text: t.into(), confidence: None, words: vec![] }
    }
    fn turn(l: &str, a: u64, b: u64) -> DiarizationTurn {
        DiarizationTurn { label: l.into(), start_ms: a, end_ms: b }
    }
    fn diar(turns: Vec<DiarizationTurn>) -> DiarizationResult {
        let labels: HashSet<String> = turns.iter().map(|t| t.label.clone()).collect();
        DiarizationResult {
            speakers: labels.into_iter().map(|l| DiarizedSpeaker { label: l, embedding: vec![1.0], total_ms: 1 }).collect(),
            turns,
            embedding_model: "m".into(),
        }
    }
    fn name_of<'a>(t: &'a Transcript, u: &Utterance) -> &'a Speaker {
        t.speakers.iter().find(|s| s.id == u.speaker_id).unwrap()
    }

    #[test]
    fn mic_is_me_and_system_follows_diarization_with_generic_names() {
        let id = MeetingId::from_u128(1);
        let t = merge_transcript(
            id,
            MergeInput {
                mic: vec![seg(0, 1_000, "hola")],
                system: vec![seg(1_000, 3_000, "buenas"), seg(3_000, 5_000, "qué tal"), seg(5_100, 6_000, "bien")],
                diarization: Some(diar(vec![turn("S1", 900, 3_100), turn("S0", 3_100, 5_000), turn("S1", 5_000, 6_000)])),
                ..MergeInput::default()
            },
        );
        assert_eq!(t.speakers.len(), 3);
        let me = &t.speakers[0];
        assert!(me.is_me && me.label == "Yo" && me.name_source == NameSource::Me);
        let labels: Vec<_> = t.utterances.iter().map(|u| name_of(&t, u).label.clone()).collect();
        assert_eq!(labels, vec!["Yo", "Speaker 1", "Speaker 2", "Speaker 1"]);
        assert_eq!(t.utterances.iter().map(|u| u.idx).collect::<Vec<_>>(), vec![0, 1, 2, 3]);
    }

    #[test]
    fn names_come_from_voiceprint_then_teams_overlap_excluding_self() {
        let mut vps = HashMap::new();
        let vp = VoiceprintId::from_u128(9);
        vps.insert("S0".to_string(), (vp, "Ana García".to_string()));
        let t = merge_transcript(
            MeetingId::from_u128(1),
            MergeInput {
                system: vec![seg(0, 4_000, "uno"), seg(4_000, 8_000, "dos")],
                diarization: Some(diar(vec![turn("S0", 0, 4_000), turn("S1", 4_000, 8_000)])),
                active: vec![
                    ActiveSpan { start: MediaMs(0), end: MediaMs(4_000), names: vec!["Ana García".into()] },
                    ActiveSpan { start: MediaMs(4_000), end: MediaMs(8_000), names: vec!["Edu".into(), "Luis".into()] },
                ],
                self_names: vec!["Edu".into()],
                voiceprints: vps,
                ..MergeInput::default()
            },
        );
        let s0 = t.speakers.iter().find(|s| s.diarization_label.as_deref() == Some("S0")).unwrap();
        assert_eq!((s0.display_name.as_deref(), s0.name_source, s0.voiceprint_id), (Some("Ana García"), NameSource::Voiceprint, Some(vp)));
        assert!(!s0.confirmed);
        let s1 = t.speakers.iter().find(|s| s.diarization_label.as_deref() == Some("S1")).unwrap();
        assert_eq!((s1.display_name.as_deref(), s1.name_source), (Some("Luis"), NameSource::AppUi));
    }

    use super::super::energy::TrackLevels;

    #[test]
    fn echo_of_my_voice_on_system_is_dropped_when_the_mic_dominates() {
        let t = merge_transcript(
            MeetingId::from_u128(1),
            MergeInput {
                mic: vec![seg(0, 2_000, "vamos a revisar el plan")],
                system: vec![seg(100, 2_000, "Vamos a revisar el plan."), seg(2_500, 3_000, "vale")],
                levels: Some(TrackLevels::constant(-25.0, -50.0, 4_000)),
                ..MergeInput::default()
            },
        );
        let texts: Vec<_> = t.utterances.iter().map(|u| u.text.as_str()).collect();
        assert_eq!(texts, vec!["vamos a revisar el plan", "vale"]);
        assert_eq!(t.speakers.iter().filter(|s| !s.is_me).count(), 1);
    }

    #[test]
    fn speaker_bleed_on_mic_is_not_me_the_remote_attribution_wins() {
        // Edu muteado con altavoces: el micro solo oye, bajito, lo que dice el remoto.
        let t = merge_transcript(
            MeetingId::from_u128(1),
            MergeInput {
                mic: vec![seg(0, 2_000, "vamos a revisar el plan")],
                system: vec![seg(0, 2_000, "Vamos a revisar el plan.")],
                diarization: Some(diar(vec![turn("S0", 0, 2_000)])),
                levels: Some(TrackLevels::constant(-50.0, -20.0, 3_000)),
                ..MergeInput::default()
            },
        );
        assert_eq!(t.utterances.len(), 1);
        let who = name_of(&t, &t.utterances[0]);
        assert!(!who.is_me);
        assert_eq!(t.utterances[0].track, Track::System);
        assert!(t.speakers.iter().all(|s| !s.is_me), "sin voz propia no hay hablante «Yo»");
    }

    #[test]
    fn duplicate_phrase_without_levels_prefers_the_system_speaker() {
        let t = merge_transcript(
            MeetingId::from_u128(1),
            MergeInput {
                mic: vec![seg(0, 2_000, "vamos a revisar el plan")],
                system: vec![seg(50, 2_100, "vamos a revisar el plan")],
                ..MergeInput::default()
            },
        );
        assert_eq!(t.utterances.len(), 1);
        assert_eq!(t.utterances[0].track, Track::System);
    }

    #[test]
    fn bleed_transcribed_differently_is_dropped_when_far_below_the_system() {
        let t = merge_transcript(
            MeetingId::from_u128(1),
            MergeInput {
                mic: vec![seg(0, 2_000, "bamos arrevisar el plan")],
                system: vec![seg(0, 2_000, "ya vamos a revisar los números del plan de ventas")],
                levels: Some(TrackLevels::constant(-55.0, -25.0, 3_000)),
                ..MergeInput::default()
            },
        );
        assert!(t.utterances.iter().all(|u| u.track == Track::System));
        assert!(t.speakers.iter().all(|s| !s.is_me));
    }

    #[test]
    fn my_real_speech_over_silent_system_stays_me() {
        let t = merge_transcript(
            MeetingId::from_u128(1),
            MergeInput {
                mic: vec![seg(0, 2_000, "buenos días a todos")],
                system: vec![seg(5_000, 6_000, "hola")],
                levels: Some(TrackLevels::constant(-30.0, -90.0, 7_000)),
                ..MergeInput::default()
            },
        );
        assert!(t.utterances.iter().any(|u| name_of(&t, u).is_me && u.text == "buenos días a todos"));
    }

    #[test]
    fn my_speech_over_a_talking_remote_stays_me_when_the_mic_is_not_far_below() {
        let t = merge_transcript(
            MeetingId::from_u128(1),
            MergeInput {
                mic: vec![seg(0, 2_000, "perdona que te interrumpa")],
                system: vec![seg(0, 2_000, "entonces el presupuesto del trimestre")],
                levels: Some(TrackLevels::constant(-28.0, -26.0, 3_000)),
                ..MergeInput::default()
            },
        );
        assert_eq!(t.utterances.len(), 2);
        assert!(t.utterances.iter().any(|u| name_of(&t, u).is_me));
    }

    #[test]
    fn reprocessing_keeps_ids_and_confirmed_names() {
        let input = || MergeInput {
            system: vec![seg(0, 1_000, "x")],
            diarization: Some(diar(vec![turn("S0", 0, 1_000)])),
            ..MergeInput::default()
        };
        let mut first = merge_transcript(MeetingId::from_u128(1), input());
        first.speakers[0].display_name = Some("Marta".into());
        first.speakers[0].confirmed = true;
        first.speakers[0].name_source = NameSource::User;
        let again = merge_transcript(MeetingId::from_u128(1), MergeInput { previous: Some(first.clone()), ..input() });
        assert_eq!(again.speakers[0].id, first.speakers[0].id);
        assert_eq!(again.speakers[0].display_name.as_deref(), Some("Marta"));
        assert!(again.speakers[0].confirmed);
    }
}
