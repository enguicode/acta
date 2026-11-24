#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Reuniones sintéticas con ASR REAL (Parakeet v3 vía el sidecar) y energía
//! real de las pistas. Las voces salen de `say` (Mónica = remoto, Eddy = yo);
//! el bleed/eco se mezcla con ffmpeg/python a niveles conocidos. Lo que se
//! versiona es la salida del ASR (`*.asr.json`) y las envolventes
//! (`*.levels.json`): sin audio, sin modelos, determinista.
//!
//! Regenerar las envolventes tras cambiar los WAV:
//! `SYNTH_DIR=<dir con a8.mic.wav ...> cargo test -p acta-audio --test synthetic_meetings -- --ignored regenerate`
//!
//! Escenarios (mic/system, dB relativos a la voz original):
//! - `aNN`: yo en mute, el remoto suena por los altavoces y entra en el mic a -NN dB.
//! - `bNN`: hablo yo; mi voz sale en el system a -NN dB (eco).
//! - `c`: turnos alternos, bleed -18 dB y eco -28 dB.
//! - `d`: hablo encima del remoto (doble habla).

use acta_audio::processing::energy::{Envelope, TrackLevels};
use acta_audio::processing::merge::{MergeInput, merge_transcript};
use acta_core::ids::MeetingId;
use acta_core::transcription::{AsrSegment, Transcript};
use serde_json::Value;
use std::path::PathBuf;

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/synthetic")
}

fn segments(name: &str, track: &str) -> Vec<AsrSegment> {
    let raw = std::fs::read_to_string(dir().join(format!("{name}.{track}.asr.json"))).unwrap();
    let v: Value = serde_json::from_str(&raw).unwrap();
    // Formato del fichero del sidecar (snake_case); las palabras no hacen falta.
    v["segments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| AsrSegment {
            start_ms: s["start_ms"].as_u64().unwrap(),
            end_ms: s["end_ms"].as_u64().unwrap(),
            text: s["text"].as_str().unwrap().to_owned(),
            confidence: s["confidence"].as_f64().map(|c| c as f32),
            words: vec![],
        })
        .collect()
}

fn levels(name: &str) -> TrackLevels {
    serde_json::from_str(&std::fs::read_to_string(dir().join(format!("{name}.levels.json"))).unwrap()).unwrap()
}

fn merged(name: &str) -> Transcript {
    merge_transcript(
        MeetingId::from_u128(1),
        MergeInput { mic: segments(name, "mic"), system: segments(name, "system"), levels: Some(levels(name)), ..MergeInput::default() },
    )
}

/// (quién, texto) por intervención, en orden.
fn who(t: &Transcript) -> Vec<(&'static str, String)> {
    t.utterances
        .iter()
        .map(|u| {
            let me = t.speakers.iter().find(|s| s.id == u.speaker_id).unwrap().is_me;
            (if me { "Yo" } else { "Otro" }, u.text.clone())
        })
        .collect()
}

fn speakers_seen(t: &Transcript) -> Vec<&'static str> {
    who(t).into_iter().map(|(w, _)| w).collect()
}

/// Edu en mute: todo lo que entra por el micro es el remoto sonando en la sala.
/// Da igual lo fuerte que se cuele (de -8 a -30 dB) o lo degradado que llegue
/// (paso banda + reverb + ruido de sala): ningún "Yo", ningún duplicado.
#[test]
fn muted_user_with_speakers_never_becomes_me() {
    for n in ["a8", "a12", "a15", "a20", "a25", "a30", "e", "e0x", "e3x", "e6", "e10"] {
        let t = merged(n);
        assert!(t.speakers.iter().all(|s| !s.is_me), "{n}: apareció un 'Yo'");
        assert_eq!(t.utterances.len(), segments(n, "system").len(), "{n}: duplicados o pérdidas");
        assert!(who(&t).iter().all(|(w, _)| *w == "Otro"), "{n}");
    }
}

/// Hablo yo y mi voz se oye débil en el system (eco, -10..-30 dB): sigo siendo
/// "Yo" y el eco no sale como segundo hablante.
#[test]
fn my_voice_with_echo_in_the_system_track_stays_me_once() {
    for n in ["b10", "b20", "b30", "f"] {
        let t = merged(n);
        assert_eq!(speakers_seen(&t), vec!["Yo", "Yo"], "{n}: {:?}", who(&t));
    }
}

#[test]
fn alternating_turns_keep_each_voice_in_its_place() {
    let t = merged("c");
    // Turnos, no frases: Parakeet puede partir un turno en dos segmentos
    // ("Buenas tardes a todos." / "Hoy vamos…") y ambos son del mismo hablante.
    let mut turns = speakers_seen(&t);
    turns.dedup();
    assert_eq!(turns, vec!["Otro", "Yo", "Otro", "Yo"], "{:?}", who(&t));
}

/// Hablo encima del remoto: mi frase se queda como "Yo" y el bleed del remoto
/// en el micro no genera un "Yo" falso.
#[test]
fn talking_over_the_remote_keeps_my_line_and_drops_the_bleed() {
    let t = merged("d");
    assert_eq!(speakers_seen(&t), vec!["Otro", "Yo"], "{:?}", who(&t));
    assert!(who(&t)[1].1.starts_with("Perfecto, yo me encargo"));
}

/// Límite conocido: si hablo flojito (-12 dB) justo encima del remoto, el
/// micro queda a -11,9 dB del sistema, pegado al umbral de bleed (-12 dB).
/// Hoy se conserva como "Yo"; si este test falla, alguien movió el umbral.
#[test]
fn soft_speech_over_the_remote_sits_right_at_the_bleed_threshold() {
    let t = merged("g");
    assert_eq!(speakers_seen(&t)[..2], ["Otro", "Yo"], "{:?}", who(&t));
}

/// El glosario sobre texto real de Parakeet: corrige lo que está a distancia
/// de edición razonable y deja intacto lo demás.
#[test]
fn glossary_fixes_real_asr_output() {
    use acta_audio::processing::glossary;
    let glossary: Vec<String> = ["Jira", "Terraform", "Elasticsearch", "Inditex"].map(String::from).to_vec();
    let mut j3 = segments("j3", "mic");
    glossary::apply(&mut j3, &glossary);
    assert!(j3[0].text.contains("configuración de Terraform en Azure"), "{}", j3[0].text);
    let mut j2 = segments("j2", "mic");
    glossary::apply(&mut j2, &glossary);
    assert!(j2[0].text.contains("llegar a Elasticsearch."), "{}", j2[0].text);
    // Parakeet escribe «gira» por «Jira»: 4 letras, por debajo del mínimo
    // seguro (gira es palabra española). Límite documentado, no bug.
    assert!(j3[0].text.contains("ticket en gira"), "{}", j3[0].text);
}

#[test]
#[ignore = "regenera las envolventes desde SYNTH_DIR"]
fn regenerate() {
    let src = PathBuf::from(std::env::var("SYNTH_DIR").expect("SYNTH_DIR"));
    for entry in std::fs::read_dir(&src).unwrap() {
        let name = entry.unwrap().file_name().into_string().unwrap();
        let Some(base) = name.strip_suffix(".mic.wav") else { continue };
        let l = TrackLevels {
            mic: Envelope::from_wav(&src.join(&name)).unwrap(),
            system: Envelope::from_wav(&src.join(format!("{base}.system.wav"))).unwrap(),
        };
        std::fs::write(dir().join(format!("{base}.levels.json")), serde_json::to_vec(&l).unwrap()).unwrap();
    }
}
