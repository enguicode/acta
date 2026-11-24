//! Máquina de estados de la sesión. PURA: no hace I/O ni lee el reloj; el
//! tiempo llega dentro de los eventos y los ids con [`IdSource`]. El
//! supervisor (WP1, en acta-audio) ejecuta los [`Effect`] devueltos.
//!
//! ```text
//!            MeetingDetected                 Accept / StartRequested
//!   Idle ───────────────────▶ Detected ─────────────────────────────┐
//!    ▲ ▲   Decline/Expired/Ended │                                    ▼
//!    │ └─────────────────────────┘                 ┌──────────── Recording ◀─┐
//!    │                                    Pause    │   ▲  MeetingEnded/Silence │ Resume
//!    │ RecoveryRequired                            ▼   │  → stop_prompt       │
//!    ├──────────▶ Recovering                     Paused ┘                      │
//!    │               │ RecoveryFinished            │ Stop/AutoStopDue          │
//!    │               ▼                             ▼                           │
//!    │           Processing ◀── CaptureFinalized ─ Stopping ◀──────────────────┘
//!    │               │ ProcessingFinished                     Stop/AutoStopDue/Fatal
//!    │ Dismiss       ▼
//!    ├────────── Ready
//!    └────────── Failed ◀── ProcessingFailed / RecoveryFailed
//! ```
//!
//! Processing/Ready/Failed NO bloquean: desde ellos se puede detectar o
//! empezar otra reunión (el procesado sigue en la cola de fondo; los eventos
//! de procesado de una reunión que ya no es la actual se ignoran sin error).

use crate::detection::MeetingApp;
use crate::ids::{MeetingId, PromptId};
use crate::processing::ProcessingStep;
use crate::recording::StopReason;
use crate::time::UnixMs;
use serde::{Deserialize, Serialize};

/// Por qué preguntamos "¿Parar?".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StopPromptReason {
    /// "La llamada ha terminado — ¿Parar?"
    MeetingEnded,
    /// Silencio prolongado en ambas pistas sin señal de fin.
    Silence,
    /// Grabación automática recién iniciada: si se para en los primeros N s se descarta.
    AutoStarted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum PromptKind {
    /// "Reunión de Teams detectada — ¿Grabar?"
    StartRecording { app: MeetingApp },
    /// "La llamada ha terminado — ¿Parar?"
    StopRecording { reason: StopPromptReason },
}

/// Pregunta que muestra la pastilla expandida.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Prompt {
    pub id: PromptId,
    #[serde(flatten)]
    pub kind: PromptKind,
    pub created_at: UnixMs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PromptAnswer {
    Accept,
    Decline,
}

/// Datos de la grabación en curso (compartidos por Recording y Paused).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveRecording {
    pub meeting_id: MeetingId,
    pub app: Option<MeetingApp>,
    pub started_at: UnixMs,
    /// Pregunta "¿Parar?" abierta, si la hay.
    pub stop_prompt: Option<Prompt>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum SessionState {
    Idle,
    Detected {
        prompt: Prompt,
        app: MeetingApp,
    },
    Recording(ActiveRecording),
    Paused {
        #[serde(flatten)]
        recording: ActiveRecording,
        paused_at: UnixMs,
    },
    Stopping {
        meeting_id: MeetingId,
        reason: StopReason,
    },
    Processing {
        meeting_id: MeetingId,
        step: ProcessingStep,
    },
    Ready {
        meeting_id: MeetingId,
    },
    Recovering {
        meeting_id: MeetingId,
    },
    Failed {
        meeting_id: Option<MeetingId>,
        error: String,
    },
}

/// Entradas de la máquina.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum SessionEvent {
    MeetingDetected { app: MeetingApp, at: UnixMs },
    /// Reunión detectada con grabación automática activa: empieza a grabar ya
    /// y, si `open_discard_prompt`, abre la ventana temporal para descartarla.
    AutoRecordStarted { app: MeetingApp, open_discard_prompt: bool, at: UnixMs },
    MeetingEnded { at: UnixMs },
    PromptAnswered { prompt_id: PromptId, answer: PromptAnswer, at: UnixMs },
    /// La pregunta de inicio caducó sin respuesta.
    PromptExpired { prompt_id: PromptId, at: UnixMs },
    /// Grabación manual (botón). `app` si hay una reunión detectada.
    StartRequested { app: Option<MeetingApp>, at: UnixMs },
    PauseRequested { at: UnixMs },
    ResumeRequested { at: UnixMs },
    StopRequested { reason: StopReason, at: UnixMs },
    /// La política de autoparada ve silencio largo en ambas pistas → preguntar.
    SilenceDetected { at: UnixMs },
    /// La política de autoparada decide parar (silencio sin respuesta o tope).
    AutoStopDue { reason: StopReason, at: UnixMs },
    /// El writer cerró todos los segmentos y el manifiesto (status Stopped).
    CaptureFinalized { meeting_id: MeetingId },
    ProcessingProgress { meeting_id: MeetingId, step: ProcessingStep },
    ProcessingFinished { meeting_id: MeetingId },
    ProcessingFailed { meeting_id: MeetingId, error: String },
    /// Al arrancar se encontró un manifiesto en Recording/Paused.
    RecoveryRequired { meeting_id: MeetingId },
    RecoveryFinished { meeting_id: MeetingId },
    RecoveryFailed { meeting_id: MeetingId, error: String },
    /// Error irrecuperable durante la captura.
    Fatal { error: String, at: UnixMs },
    /// El usuario cierra el resultado (Ready/Failed) → Idle.
    Dismiss,
}

/// Órdenes para el supervisor. Se ejecutan EN ORDEN.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "effect", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Effect {
    ShowPrompt { prompt: Prompt },
    ClosePrompt { prompt_id: PromptId },
    /// Crear carpeta + manifiesto, arrancar AudioSource, writer, watchdog,
    /// SpeakerNameSource y (si está activado) ScreenCapturer.
    StartCapture { meeting_id: MeetingId, app: Option<MeetingApp>, at: UnixMs },
    PauseCapture { meeting_id: MeetingId, at: UnixMs },
    ResumeCapture { meeting_id: MeetingId, at: UnixMs },
    /// Parar fuentes y cerrar segmentos. Al terminar → `CaptureFinalized`.
    StopCapture { meeting_id: MeetingId, reason: StopReason, at: UnixMs },
    /// Encolar el pipeline de procesado.
    StartProcessing { meeting_id: MeetingId },
    /// Reparar segmentos `.tmp` y manifiesto. Al terminar → `RecoveryFinished`.
    RunRecovery { meeting_id: MeetingId },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transition {
    pub next: SessionState,
    pub effects: Vec<Effect>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid transition: {event} in state {state}")]
pub struct TransitionError {
    pub state: &'static str,
    pub event: &'static str,
}

/// Fuente de ids (inyectada para que la máquina sea determinista).
pub trait IdSource {
    fn meeting_id(&mut self) -> MeetingId;
    fn prompt_id(&mut self) -> PromptId;
}

/// Ids reales (UUID v7).
#[derive(Debug, Default)]
pub struct RandomIds;

impl IdSource for RandomIds {
    fn meeting_id(&mut self) -> MeetingId {
        MeetingId::new()
    }
    fn prompt_id(&mut self) -> PromptId {
        PromptId::new()
    }
}

impl SessionState {
    pub fn name(&self) -> &'static str {
        match self {
            SessionState::Idle => "idle",
            SessionState::Detected { .. } => "detected",
            SessionState::Recording(_) => "recording",
            SessionState::Paused { .. } => "paused",
            SessionState::Stopping { .. } => "stopping",
            SessionState::Processing { .. } => "processing",
            SessionState::Ready { .. } => "ready",
            SessionState::Recovering { .. } => "recovering",
            SessionState::Failed { .. } => "failed",
        }
    }

    /// ¿Hay captura de audio activa (o pausada)?
    pub fn is_capturing(&self) -> bool {
        matches!(
            self,
            SessionState::Recording(_) | SessionState::Paused { .. } | SessionState::Stopping { .. }
        )
    }

    /// Aplica un evento. `Err` = evento no válido en este estado: el
    /// supervisor lo registra y lo ignora (el estado NO cambia).
    pub fn transition(
        &self,
        event: SessionEvent,
        ids: &mut dyn IdSource,
    ) -> Result<Transition, TransitionError> {
        use SessionEvent as E;
        use SessionState as S;

        let invalid = |event: &SessionEvent| TransitionError {
            state: self.name(),
            event: event.name(),
        };
        let stay = || Transition {
            next: self.clone(),
            effects: vec![],
        };

        match (self, event) {
            // --- Idle / Ready / Failed / Processing: se puede empezar otra ---
            (S::Idle | S::Ready { .. } | S::Failed { .. } | S::Processing { .. }, E::MeetingDetected { app, at }) => {
                let prompt = Prompt {
                    id: ids.prompt_id(),
                    kind: PromptKind::StartRecording { app: app.clone() },
                    created_at: at,
                };
                Ok(Transition {
                    next: S::Detected {
                        prompt: prompt.clone(),
                        app,
                    },
                    effects: vec![Effect::ShowPrompt { prompt }],
                })
            }
            (S::Idle | S::Ready { .. } | S::Failed { .. } | S::Processing { .. }, E::StartRequested { app, at }) => {
                Ok(start_recording(ids.meeting_id(), app, at, None))
            }
            (
                S::Idle | S::Ready { .. } | S::Failed { .. } | S::Processing { .. },
                E::AutoRecordStarted { app, open_discard_prompt, at },
            ) => {
                let meeting_id = ids.meeting_id();
                let mut effects = vec![Effect::StartCapture {
                    meeting_id,
                    app: Some(app.clone()),
                    at,
                }];
                let stop_prompt = open_discard_prompt.then(|| {
                    let prompt = Prompt {
                        id: ids.prompt_id(),
                        kind: PromptKind::StopRecording { reason: StopPromptReason::AutoStarted },
                        created_at: at,
                    };
                    effects.push(Effect::ShowPrompt { prompt: prompt.clone() });
                    prompt
                });
                Ok(Transition {
                    next: S::Recording(ActiveRecording {
                        meeting_id,
                        app: Some(app),
                        started_at: at,
                        stop_prompt,
                    }),
                    effects,
                })
            }
            (S::Idle, E::RecoveryRequired { meeting_id }) => Ok(Transition {
                next: S::Recovering { meeting_id },
                effects: vec![Effect::RunRecovery { meeting_id }],
            }),
            (S::Ready { .. } | S::Failed { .. }, E::Dismiss) => Ok(Transition {
                next: S::Idle,
                effects: vec![],
            }),

            // --- Detected ---
            (S::Detected { prompt, app }, E::PromptAnswered { prompt_id, answer, at }) => {
                if prompt_id != prompt.id {
                    return Err(invalid(&E::PromptAnswered { prompt_id, answer, at }));
                }
                match answer {
                    PromptAnswer::Accept => Ok(start_recording(
                        ids.meeting_id(),
                        Some(app.clone()),
                        at,
                        Some(prompt.id),
                    )),
                    PromptAnswer::Decline => Ok(close_to_idle(prompt.id)),
                }
            }
            (S::Detected { prompt, .. }, E::PromptExpired { prompt_id, at }) => {
                if prompt_id != prompt.id {
                    return Err(invalid(&E::PromptExpired { prompt_id, at }));
                }
                Ok(close_to_idle(prompt.id))
            }
            (S::Detected { prompt, .. }, E::MeetingEnded { .. }) => Ok(close_to_idle(prompt.id)),
            (S::Detected { prompt, app }, E::StartRequested { app: requested, at }) => Ok(start_recording(
                ids.meeting_id(),
                requested.or_else(|| Some(app.clone())),
                at,
                Some(prompt.id),
            )),
            (S::Detected { .. }, E::MeetingDetected { .. } | E::AutoRecordStarted { .. }) => Ok(stay()),

            // --- Recording / Paused ---
            (S::Recording(_) | S::Paused { .. }, E::MeetingDetected { .. } | E::AutoRecordStarted { .. }) => Ok(stay()),
            (S::Recording(rec), E::PauseRequested { at }) => Ok(Transition {
                next: S::Paused {
                    recording: rec.clone(),
                    paused_at: at,
                },
                effects: vec![Effect::PauseCapture {
                    meeting_id: rec.meeting_id,
                    at,
                }],
            }),
            (S::Paused { recording, .. }, E::ResumeRequested { at }) => Ok(Transition {
                next: S::Recording(recording.clone()),
                effects: vec![Effect::ResumeCapture {
                    meeting_id: recording.meeting_id,
                    at,
                }],
            }),
            (S::Recording(rec), E::MeetingEnded { at }) => {
                Ok(open_stop_prompt(self, rec, StopPromptReason::MeetingEnded, at, ids))
            }
            (S::Paused { recording, .. }, E::MeetingEnded { at }) => {
                Ok(open_stop_prompt(self, recording, StopPromptReason::MeetingEnded, at, ids))
            }
            (S::Recording(rec), E::SilenceDetected { at }) => {
                Ok(open_stop_prompt(self, rec, StopPromptReason::Silence, at, ids))
            }
            (S::Paused { .. }, E::SilenceDetected { .. }) => Ok(stay()),
            (S::Recording(rec), E::PromptAnswered { prompt_id, answer, at })
            | (S::Paused { recording: rec, .. }, E::PromptAnswered { prompt_id, answer, at }) => {
                let Some(open) = rec.stop_prompt.as_ref().filter(|p| p.id == prompt_id) else {
                    return Err(invalid(&E::PromptAnswered { prompt_id, answer, at }));
                };
                match answer {
                    PromptAnswer::Accept => {
                        let reason = match open.kind {
                            PromptKind::StopRecording { reason: StopPromptReason::AutoStarted } => StopReason::Discarded,
                            _ => StopReason::MeetingEndedConfirmed,
                        };
                        Ok(stop(rec, reason, at))
                    }
                    PromptAnswer::Decline => {
                        let cleared = ActiveRecording {
                            stop_prompt: None,
                            ..rec.clone()
                        };
                        let next = match self {
                            S::Paused { paused_at, .. } => S::Paused {
                                recording: cleared,
                                paused_at: *paused_at,
                            },
                            _ => S::Recording(cleared),
                        };
                        Ok(Transition {
                            next,
                            effects: vec![Effect::ClosePrompt { prompt_id: open.id }],
                        })
                    }
                }
            }
            (S::Recording(rec) | S::Paused { recording: rec, .. }, E::StopRequested { reason, at }) => {
                let effective = if reason == StopReason::User
                    && matches!(
                        rec.stop_prompt.as_ref().map(|p| &p.kind),
                        Some(PromptKind::StopRecording { reason: StopPromptReason::AutoStarted })
                    )
                {
                    StopReason::Discarded
                } else {
                    reason
                };
                Ok(stop(rec, effective, at))
            }
            (S::Recording(rec) | S::Paused { recording: rec, .. }, E::AutoStopDue { reason, at }) => {
                Ok(stop(rec, reason, at))
            }
            (S::Recording(rec) | S::Paused { recording: rec, .. }, E::Fatal { at, .. }) => {
                Ok(stop(rec, StopReason::Error, at))
            }

            // --- Stopping ---
            (S::Stopping { meeting_id, reason }, E::CaptureFinalized { meeting_id: done }) if *meeting_id == done => {
                if *reason == StopReason::Discarded {
                    Ok(Transition { next: S::Idle, effects: vec![] })
                } else {
                    Ok(start_processing(done))
                }
            }
            (
                S::Stopping { .. },
                E::MeetingEnded { .. } | E::MeetingDetected { .. } | E::AutoRecordStarted { .. } | E::SilenceDetected { .. },
            ) => {
                Ok(stay())
            }

            // --- Recovering ---
            (S::Recovering { meeting_id }, E::RecoveryFinished { meeting_id: done }) if *meeting_id == done => {
                Ok(start_processing(done))
            }
            (S::Recovering { meeting_id }, E::RecoveryFailed { meeting_id: failed, error }) if *meeting_id == failed => {
                Ok(Transition {
                    next: S::Failed {
                        meeting_id: Some(failed),
                        error,
                    },
                    effects: vec![],
                })
            }

            // --- Processing (solo si es la reunión actual) ---
            (S::Processing { meeting_id, .. }, E::ProcessingProgress { meeting_id: m, step }) if *meeting_id == m => {
                Ok(Transition {
                    next: S::Processing { meeting_id: m, step },
                    effects: vec![],
                })
            }
            (S::Processing { meeting_id, .. }, E::ProcessingFinished { meeting_id: m }) if *meeting_id == m => {
                Ok(Transition {
                    next: S::Ready { meeting_id: m },
                    effects: vec![],
                })
            }
            (S::Processing { meeting_id, .. }, E::ProcessingFailed { meeting_id: m, error }) if *meeting_id == m => {
                Ok(Transition {
                    next: S::Failed {
                        meeting_id: Some(m),
                        error,
                    },
                    effects: vec![],
                })
            }
            // Eventos de procesado de OTRA reunión (cola de fondo): se ignoran.
            (_, E::ProcessingProgress { .. } | E::ProcessingFinished { .. } | E::ProcessingFailed { .. }) => Ok(stay()),

            // Fin de llamada sin grabación en curso: nada que hacer.
            (S::Idle | S::Ready { .. } | S::Failed { .. } | S::Processing { .. } | S::Recovering { .. }, E::MeetingEnded { .. }) => {
                Ok(stay())
            }

            (_, event) => Err(invalid(&event)),
        }
    }
}

fn start_recording(
    meeting_id: MeetingId,
    app: Option<MeetingApp>,
    at: UnixMs,
    close_prompt: Option<PromptId>,
) -> Transition {
    let mut effects = Vec::with_capacity(2);
    if let Some(prompt_id) = close_prompt {
        effects.push(Effect::ClosePrompt { prompt_id });
    }
    effects.push(Effect::StartCapture {
        meeting_id,
        app: app.clone(),
        at,
    });
    Transition {
        next: SessionState::Recording(ActiveRecording {
            meeting_id,
            app,
            started_at: at,
            stop_prompt: None,
        }),
        effects,
    }
}

fn close_to_idle(prompt_id: PromptId) -> Transition {
    Transition {
        next: SessionState::Idle,
        effects: vec![Effect::ClosePrompt { prompt_id }],
    }
}

/// Abre "¿Parar?" si no hay ya una pregunta abierta.
fn open_stop_prompt(
    current: &SessionState,
    rec: &ActiveRecording,
    reason: StopPromptReason,
    at: UnixMs,
    ids: &mut dyn IdSource,
) -> Transition {
    if rec.stop_prompt.is_some() {
        return Transition {
            next: current.clone(),
            effects: vec![],
        };
    }
    let prompt = Prompt {
        id: ids.prompt_id(),
        kind: PromptKind::StopRecording { reason },
        created_at: at,
    };
    let with_prompt = ActiveRecording {
        stop_prompt: Some(prompt.clone()),
        ..rec.clone()
    };
    let next = match current {
        SessionState::Paused { paused_at, .. } => SessionState::Paused {
            recording: with_prompt,
            paused_at: *paused_at,
        },
        _ => SessionState::Recording(with_prompt),
    };
    Transition {
        next,
        effects: vec![Effect::ShowPrompt { prompt }],
    }
}

fn stop(rec: &ActiveRecording, reason: StopReason, at: UnixMs) -> Transition {
    let mut effects = Vec::with_capacity(2);
    if let Some(p) = &rec.stop_prompt {
        effects.push(Effect::ClosePrompt { prompt_id: p.id });
    }
    effects.push(Effect::StopCapture {
        meeting_id: rec.meeting_id,
        reason,
        at,
    });
    Transition {
        next: SessionState::Stopping {
            meeting_id: rec.meeting_id,
            reason,
        },
        effects,
    }
}

fn start_processing(meeting_id: MeetingId) -> Transition {
    Transition {
        next: SessionState::Processing {
            meeting_id,
            step: ProcessingStep::Concatenating,
        },
        effects: vec![Effect::StartProcessing { meeting_id }],
    }
}

impl SessionEvent {
    pub fn name(&self) -> &'static str {
        match self {
            SessionEvent::MeetingDetected { .. } => "meetingDetected",
            SessionEvent::AutoRecordStarted { .. } => "autoRecordStarted",
            SessionEvent::MeetingEnded { .. } => "meetingEnded",
            SessionEvent::PromptAnswered { .. } => "promptAnswered",
            SessionEvent::PromptExpired { .. } => "promptExpired",
            SessionEvent::StartRequested { .. } => "startRequested",
            SessionEvent::PauseRequested { .. } => "pauseRequested",
            SessionEvent::ResumeRequested { .. } => "resumeRequested",
            SessionEvent::StopRequested { .. } => "stopRequested",
            SessionEvent::SilenceDetected { .. } => "silenceDetected",
            SessionEvent::AutoStopDue { .. } => "autoStopDue",
            SessionEvent::CaptureFinalized { .. } => "captureFinalized",
            SessionEvent::ProcessingProgress { .. } => "processingProgress",
            SessionEvent::ProcessingFinished { .. } => "processingFinished",
            SessionEvent::ProcessingFailed { .. } => "processingFailed",
            SessionEvent::RecoveryRequired { .. } => "recoveryRequired",
            SessionEvent::RecoveryFinished { .. } => "recoveryFinished",
            SessionEvent::RecoveryFailed { .. } => "recoveryFailed",
            SessionEvent::Fatal { .. } => "fatal",
            SessionEvent::Dismiss => "dismiss",
        }
    }
}
