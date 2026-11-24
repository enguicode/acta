//! Ficheros que el supervisor escribe durante la reunión: señales en vivo
//! (JSON por líneas) y colocación de capturas con su nombre definitivo.

use acta_core::ids::MeetingId;
use acta_core::layout::DataLayout;
use acta_core::screen::{CaptureReason, CapturedFrame};
use acta_core::time::MediaMs;
use serde::Serialize;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Nombre de la razón en los ficheros (igual que en el protocolo del sidecar).
pub fn reason_wire(r: CaptureReason) -> &'static str {
    match r {
        CaptureReason::SlideChange => "slide_change",
        CaptureReason::Keyword => "keyword",
        CaptureReason::LlmPick => "llm_pick",
        CaptureReason::Manual => "manual",
        CaptureReason::Candidate => "candidate",
    }
}

/// Añade una línea JSON (crea carpeta y fichero si hace falta).
pub fn append_jsonl(path: &Path, value: &impl Serialize) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut line = serde_json::to_vec(value).map_err(std::io::Error::other)?;
    line.push(b'\n');
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    f.write_all(&line)
}

/// Lee un JSON por líneas ignorando líneas corruptas (p. ej. la última tras un crash).
pub fn read_jsonl<T: serde::de::DeserializeOwned>(path: &Path) -> Vec<T> {
    std::fs::read_to_string(path)
        .map(|s| s.lines().filter_map(|l| serde_json::from_str(l).ok()).collect())
        .unwrap_or_default()
}

fn move_file(from: &Path, to: &Path) -> std::io::Result<()> {
    if let Some(dir) = to.parent() {
        std::fs::create_dir_all(dir)?;
    }
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(_) => {
            std::fs::copy(from, to)?;
            std::fs::remove_file(from)
        }
    }
}

fn rel_to(meeting_dir: &Path, p: &Path) -> Option<String> {
    p.strip_prefix(meeting_dir)
        .ok()
        .map(|r| r.to_string_lossy().replace('\\', "/"))
}

/// Primera ruta libre: `name.jpg`, `name-2.jpg`, `name-3.jpg`...
fn free_path(meeting_dir: &Path, rel: &str) -> (PathBuf, String) {
    let first = meeting_dir.join(rel);
    if !first.exists() {
        return (first, rel.to_owned());
    }
    let stem = rel.trim_end_matches(".jpg");
    (2..)
        .map(|n| format!("{stem}-{n}.jpg"))
        .map(|r| (meeting_dir.join(&r), r))
        .find(|(p, _)| !p.exists())
        .unwrap_or((first, rel.to_owned()))
}

/// Mueve la captura que reportó el sidecar (`<unix_ms>-<reason>.jpg`) a su
/// nombre definitivo en la línea media (`captures/<media_ms 9>-<reason>.jpg`)
/// y devuelve la ruta relativa a la reunión. Las candidatas se quedan en
/// `captures/candidates/` (se podan al procesar).
pub fn place_frame(
    layout: &DataLayout,
    meeting_id: MeetingId,
    frame: &CapturedFrame,
    at: MediaMs,
) -> std::io::Result<String> {
    let dir = layout.meeting_dir(meeting_id);
    if frame.reason == CaptureReason::Candidate {
        if let Some(rel) = rel_to(&dir, &frame.path) {
            return Ok(rel);
        }
        let name = frame
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| format!("{}.jpg", frame.at.0));
        let dest = layout.capture_candidates_dir(meeting_id).join(&name);
        move_file(&frame.path, &dest)?;
        return Ok(rel_to(&dir, &dest).unwrap_or(name));
    }
    let wanted = DataLayout::capture_rel_path(at.0, reason_wire(frame.reason));
    if dir.join(&wanted) == frame.path {
        return Ok(wanted);
    }
    let (dest, rel) = free_path(&dir, &wanted);
    move_file(&frame.path, &dest)?;
    Ok(rel)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use acta_core::time::UnixMs;

    fn frame(path: PathBuf, reason: CaptureReason) -> CapturedFrame {
        CapturedFrame {
            path,
            at: UnixMs(1_700_000_000_123),
            reason,
            phash: "00ff00ff00ff00ff".into(),
            width: 1280,
            height: 720,
            keyword: None,
            thumb: None,
        }
    }

    #[test]
    fn slide_is_renamed_to_media_time_and_never_overwrites() {
        let root = tempfile::tempdir().unwrap();
        let layout = DataLayout::new(root.path());
        let id = MeetingId::from_u128(1);
        let caps = layout.captures_dir(id);
        std::fs::create_dir_all(&caps).unwrap();
        let src = caps.join("1700000000123-slide_change.jpg");
        std::fs::write(&src, b"a").unwrap();
        let rel = place_frame(&layout, id, &frame(src.clone(), CaptureReason::SlideChange), MediaMs(65_432)).unwrap();
        assert_eq!(rel, "captures/000065432-slide_change.jpg");
        assert!(!src.exists());
        assert_eq!(std::fs::read(layout.meeting_dir(id).join(&rel)).unwrap(), b"a");

        let src2 = caps.join("1700000000999-slide_change.jpg");
        std::fs::write(&src2, b"b").unwrap();
        let rel2 = place_frame(&layout, id, &frame(src2, CaptureReason::SlideChange), MediaMs(65_432)).unwrap();
        assert_eq!(rel2, "captures/000065432-slide_change-2.jpg");
        assert_eq!(std::fs::read(layout.meeting_dir(id).join(&rel)).unwrap(), b"a");
    }

    #[test]
    fn candidate_stays_in_candidates_dir() {
        let root = tempfile::tempdir().unwrap();
        let layout = DataLayout::new(root.path());
        let id = MeetingId::from_u128(2);
        let cand = layout.capture_candidates_dir(id);
        std::fs::create_dir_all(&cand).unwrap();
        let src = cand.join("1700000000123.jpg");
        std::fs::write(&src, b"c").unwrap();
        let rel = place_frame(&layout, id, &frame(src.clone(), CaptureReason::Candidate), MediaMs(1)).unwrap();
        assert_eq!(rel, "captures/candidates/1700000000123.jpg");
        assert!(src.exists());
    }

    #[test]
    fn jsonl_roundtrip_skips_corrupt_tail() {
        let root = tempfile::tempdir().unwrap();
        let p = root.path().join("live/x.jsonl");
        append_jsonl(&p, &serde_json::json!({"a": 1})).unwrap();
        append_jsonl(&p, &serde_json::json!({"a": 2})).unwrap();
        let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
        f.write_all(b"{\"a\":").unwrap();
        let v: Vec<serde_json::Value> = read_jsonl(&p);
        assert_eq!(v.len(), 2);
    }
}
