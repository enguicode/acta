//! Utilidades de test: binarios falsos (scripts `/bin/sh`) y servidor HTTP
//! de mentira. Sin red real ni LLM real.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Crea un ejecutable `name` en `dir` con el cuerpo dado (`#!/bin/sh` implícito).
pub fn fake_script(dir: &Path, name: &str, body: &str) -> PathBuf {
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    let mut f = std::fs::File::create(&path).expect("create script");
    f.write_all(format!("#!/bin/sh\n{body}").as_bytes()).expect("write script");
    f.set_permissions(std::fs::Permissions::from_mode(0o755)).expect("chmod");
    f.sync_all().expect("sync");
    drop(f);
    path
}

#[derive(Debug, Clone)]
pub struct Recorded {
    #[allow(dead_code)]
    pub method: String,
    pub path: String,
    /// Cabeceras en minúsculas.
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Recorded {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }
    pub fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.body).expect("body json")
    }
}

#[derive(Debug, Clone)]
pub struct Canned {
    pub status: u16,
    pub content_type: &'static str,
    pub body: String,
    /// Retraso antes de responder.
    pub delay_ms: u64,
}

impl Canned {
    pub fn json(body: serde_json::Value) -> Self {
        Self { status: 200, content_type: "application/json", body: body.to_string(), delay_ms: 0 }
    }
    pub fn sse(body: &str) -> Self {
        Self { status: 200, content_type: "text/event-stream", body: body.to_string(), delay_ms: 0 }
    }
    pub fn status(status: u16, body: &str) -> Self {
        Self { status, content_type: "application/json", body: body.to_string(), delay_ms: 0 }
    }
}

pub struct MockServer {
    pub url: String,
    recorded: Arc<Mutex<Vec<Recorded>>>,
}

impl MockServer {
    /// Responde a las peticiones sucesivas con `responses` (la última se repite).
    pub async fn start(responses: Vec<Canned>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let url = format!("http://{}", listener.local_addr().expect("addr"));
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let rec = recorded.clone();
        let queue = Arc::new(Mutex::new(std::collections::VecDeque::from(responses)));
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else { break };
                let rec = rec.clone();
                let queue = queue.clone();
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut tmp = [0u8; 4096];
                    let header_end = loop {
                        let Ok(n) = sock.read(&mut tmp).await else { return };
                        if n == 0 {
                            return;
                        }
                        buf.extend_from_slice(&tmp[..n]);
                        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                            break p + 4;
                        }
                    };
                    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
                    let mut lines = head.lines();
                    let first = lines.next().unwrap_or_default().to_string();
                    let mut parts = first.split_whitespace();
                    let method = parts.next().unwrap_or_default().to_string();
                    let path = parts.next().unwrap_or_default().to_string();
                    let headers: Vec<(String, String)> = lines
                        .filter_map(|l| l.split_once(':'))
                        .map(|(k, v)| (k.trim().to_lowercase(), v.trim().to_string()))
                        .collect();
                    let len: usize = headers
                        .iter()
                        .find(|(k, _)| k == "content-length")
                        .and_then(|(_, v)| v.parse().ok())
                        .unwrap_or(0);
                    while buf.len() < header_end + len {
                        let Ok(n) = sock.read(&mut tmp).await else { return };
                        if n == 0 {
                            break;
                        }
                        buf.extend_from_slice(&tmp[..n]);
                    }
                    let body = String::from_utf8_lossy(&buf[header_end..]).to_string();
                    rec.lock().expect("lock").push(Recorded { method, path, headers, body });
                    let canned = {
                        let mut q = queue.lock().expect("lock");
                        if q.len() > 1 { q.pop_front() } else { q.front().cloned() }
                    }
                    .unwrap_or(Canned::status(500, "sin respuesta"));
                    if canned.delay_ms > 0 {
                        tokio::time::sleep(std::time::Duration::from_millis(canned.delay_ms)).await;
                    }
                    let resp = format!(
                        "HTTP/1.1 {} X\r\ncontent-type: {}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                        canned.status,
                        canned.content_type,
                        canned.body.len(),
                        canned.body
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                    let _ = sock.shutdown().await;
                });
            }
        });
        Self { url, recorded }
    }

    pub fn requests(&self) -> Vec<Recorded> {
        self.recorded.lock().expect("lock").clone()
    }
}

// ---------- datos de ejemplo del dominio ----------
use acta_core::detection::Platform;
use acta_core::ids::{CaptureId, MeetingId, SpeakerId};
use acta_core::minutes::{CaptureRef, MinutesInput, UserIdentity};
use acta_core::recording::Track;
use acta_core::screen::CaptureReason;
use acta_core::time::{MediaMs, UnixMs};
use acta_core::transcription::{NameSource, Speaker, Utterance};

pub fn speaker(n: u128, is_me: bool, name: Option<&str>) -> Speaker {
    Speaker {
        id: SpeakerId::from_u128(n),
        meeting_id: MeetingId::from_u128(1),
        label: if is_me { "Yo".into() } else { format!("Speaker {n}") },
        display_name: name.map(str::to_owned),
        is_me,
        track: if is_me { Track::Mic } else { Track::System },
        diarization_label: None,
        name_source: NameSource::Generic,
        confirmed: false,
        voiceprint_id: None,
    }
}

pub fn utt(idx: u32, sp: u128, start_ms: u64, text: &str) -> Utterance {
    Utterance {
        idx,
        speaker_id: SpeakerId::from_u128(sp),
        track: if sp == 1 { Track::Mic } else { Track::System },
        start: MediaMs(start_ms),
        end: MediaMs(start_ms + 2_000),
        text: text.into(),
        confidence: None,
    }
}

/// Reunión de ejemplo: yo (Edu, speaker 1) y Ana García (speaker 2), 1 captura.
pub fn sample_input() -> MinutesInput {
    MinutesInput {
        meeting_id: MeetingId::from_u128(1),
        title: "Reunión de Teams".into(),
        platform: Platform::Teams,
        started_at: UnixMs(1_794_821_400_000),
        duration: MediaMs(0),
        me: UserIdentity { display_name: "Edu".into(), aliases: vec!["Eduardo".into(), "Edu Navajas".into()] },
        speakers: vec![speaker(1, true, None), speaker(2, false, Some("Ana García"))],
        utterances: vec![
            utt(0, 2, 1_000, "Edu, ¿puedes enviar el presupuesto el viernes?"),
            utt(1, 1, 6_000, "Sí, yo me encargo."),
            utt(2, 2, 12_000, "Yo preparo la demo. Esto es importante."),
        ],
        captures: vec![CaptureRef {
            ref_id: "c1".into(),
            capture_id: CaptureId::from_u128(77),
            at: MediaMs(8_000),
            reason: CaptureReason::SlideChange,
        }],
        language: "es".into(),
        user_note: None,
    }
}
