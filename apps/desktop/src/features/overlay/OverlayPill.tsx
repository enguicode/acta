import { Captions, Check, Pause, Play, Square, TriangleAlert } from "lucide-react";
import type { HealthWarning, Levels, ProcessingStep, Prompt, SessionState } from "../../api";
import { dbToLevel, formatClock, platformLabel } from "../../shared/format";
import { PlatformIcon, Spinner } from "../../shared/ui/ui";
import { STEP_LABEL } from "../meetings/ProcessingStatus";

export interface PillProps {
  state: SessionState;
  elapsedMs: number;
  levels: Levels | null;
  warning: HealthWarning | null;
  /** Segundos hasta la parada automática o fin de la ventana de descarte. */
  countdownS: number | null;
  /** Segundos totales de esa cuenta atrás (para la barra que se agota). */
  countdownTotalS?: number | null;
  onPause(): void;
  onResume(): void;
  onStop(): void;
  onAnswer(prompt: Prompt, answer: "accept" | "decline"): void;
  onDismiss(): void;
  onOpenMeeting(id: string): void;
  /** Abre la vista «En directo» de la grabación en curso en la ventana principal. */
  onLive?(meetingId: string): void;
}

function Meter({ label, db }: { label: string; db: number | null }) {
  const level = db == null ? 0 : dbToLevel(db);
  return (
    <div className="meter" role="meter" aria-label={`Nivel de ${label}`} aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(level * 100)}>
      <span className="meter-label">{label}</span>
      <span className="meter-track">
        <span className="meter-fill" style={{ transform: `scaleX(${level})` }} />
      </span>
    </div>
  );
}

interface QuestionProps {
  title: string;
  label: string;
  prompt: Prompt;
  yes: string;
  no: string;
  note?: string;
  danger?: boolean;
  /** 0..1: lo que queda de la cuenta atrás. */
  remaining?: number | null;
  onAnswer: PillProps["onAnswer"];
}

function Question({ title, label, prompt, yes, no, note, danger, remaining, onAnswer }: QuestionProps) {
  return (
    <div role="group" aria-label={label} className="ask">
      <p className="ask-text">{title}</p>
      {note ? <p className="ask-note">{note}</p> : null}
      {remaining != null ? (
        <span className="ask-countdown" aria-hidden>
          <span style={{ transform: `scaleX(${Math.min(1, Math.max(0, remaining))})` }} />
        </span>
      ) : null}
      <div className="ask-actions">
        <button type="button" className={`pbtn ${danger ? "pbtn-rec" : "pbtn-primary"}`} onClick={() => onAnswer(prompt, "accept")}>
          {yes}
        </button>
        <button type="button" className="pbtn" onClick={() => onAnswer(prompt, "decline")}>
          {no}
        </button>
      </div>
    </div>
  );
}

export function OverlayPill(p: PillProps) {
  const s = p.state;
  let body: React.ReactNode = null;
  let expanded = false;

  if (s.state === "detected") {
    const name = s.app.platform === "other" ? s.app.name : platformLabel(s.app.platform);
    expanded = true;
    body = (
      <div className="pill-detected">
        <PlatformIcon platform={s.app.platform} />
        <Question title={`Reunión de ${name} detectada — ¿Grabar?`} label="Reunión detectada" prompt={s.prompt} yes="Grabar" no="Ignorar" note={s.app.title ?? "Se grabará tu voz y la de los demás, por separado."} danger onAnswer={p.onAnswer} />
      </div>
    );
  } else if (s.state === "recording" || s.state === "paused") {
    const paused = s.state === "paused";
    const stop = s.stopPrompt;
    expanded = Boolean(stop) || Boolean(p.warning);
    body = (
      <>
        <div className="pill-row">
          <span className={`rec-dot${paused ? " rec-dot-paused" : ""}`} aria-hidden />
          <span className="timer" aria-label="Tiempo grabado">
            {formatClock(p.elapsedMs)}
          </span>
          <div className="meters">
            <Meter label="Yo" db={paused ? null : (p.levels?.mic.rmsDbfs ?? null)} />
            <Meter label="Otros" db={paused ? null : (p.levels?.system.rmsDbfs ?? null)} />
          </div>
          {p.onLive ? (
            <button type="button" className="pbtn pbtn-icon pbtn-live" aria-label="Ver en directo" title="Ver la transcripción en directo y preguntar" onClick={() => p.onLive?.(s.meetingId)}>
              <Captions size={14} strokeWidth={2.1} aria-hidden />
            </button>
          ) : null}
          <button type="button" className="pbtn pbtn-icon" aria-label={paused ? "Reanudar" : "Pausar"} onClick={paused ? p.onResume : p.onPause}>
            {paused ? <Play size={13} fill="currentColor" strokeWidth={0} aria-hidden /> : <Pause size={13} fill="currentColor" strokeWidth={0} aria-hidden />}
          </button>
          <button type="button" className="pbtn pbtn-icon pbtn-stop" aria-label="Parar" onClick={p.onStop}>
            <Square size={11} fill="currentColor" strokeWidth={0} aria-hidden />
          </button>
        </div>
        {stop && stop.kind === "stopRecording" ? (
          stop.reason === "autoStarted" ? (
            <Question
              title="Grabación automática iniciada"
              label="Grabación automática"
              prompt={stop}
              yes="Descartar"
              no="Seguir grabando"
              note={p.countdownS != null ? `Si paras en ${formatClock(p.countdownS * 1000)}, se descartará` : "Si paras ahora, se descartará la grabación."}
              danger
              remaining={p.countdownS != null && p.countdownTotalS ? p.countdownS / p.countdownTotalS : null}
              onAnswer={p.onAnswer}
            />
          ) : (
            <Question
              title={stop.reason === "silence" ? "Parece que no habla nadie — ¿Parar?" : "La llamada ha terminado — ¿Parar?"}
              label="La llamada ha terminado"
              prompt={stop}
              yes="Parar"
              no="Seguir grabando"
              note={p.countdownS != null ? `Si nadie habla, parará solo en ${formatClock(p.countdownS * 1000)}` : undefined}
              remaining={p.countdownS != null && p.countdownTotalS ? p.countdownS / p.countdownTotalS : null}
              onAnswer={p.onAnswer}
            />
          )
        ) : null}
        {p.warning ? (
          <p role="alert" className="warn">
            <TriangleAlert size={13} strokeWidth={2.2} aria-hidden />
            <span>{p.warning.message}</span>
          </p>
        ) : null}
      </>
    );
  } else if (s.state === "stopping" || s.state === "recovering" || s.state === "processing") {
    const label = s.state === "stopping" ? "Guardando el audio" : s.state === "recovering" ? "Recuperando una grabación interrumpida" : STEP_LABEL[s.step as ProcessingStep];
    body = (
      <div className="pill-row pill-busy">
        <Spinner />
        <p className="status">{label}…</p>
      </div>
    );
  } else if (s.state === "ready") {
    expanded = true;
    body = (
      <div className="pill-done">
        <div className="pill-row">
          <span className="done-mark" aria-hidden>
            <Check size={14} strokeWidth={3} />
          </span>
          <div className="done-text">
            <p className="status">Acta lista</p>
            <p className="ask-note">Resumen, decisiones y tareas ya están preparados.</p>
          </div>
        </div>
        <div className="ask-actions">
          <button type="button" className="pbtn pbtn-primary" onClick={() => p.onOpenMeeting(s.meetingId)}>
            Abrir acta
          </button>
          <button type="button" className="pbtn" onClick={p.onDismiss}>
            Cerrar
          </button>
        </div>
      </div>
    );
  } else if (s.state === "failed") {
    expanded = true;
    body = (
      <div className="pill-done">
        <div className="pill-row">
          <span className="done-mark done-mark-warn" aria-hidden>
            <TriangleAlert size={14} strokeWidth={2.4} />
          </span>
          <div className="done-text">
            <p className="status">Algo ha ido mal</p>
            <p className="ask-note warn-text">{s.error}</p>
          </div>
        </div>
        <div className="ask-actions">
          <button type="button" className="pbtn" onClick={p.onDismiss}>
            Cerrar
          </button>
        </div>
      </div>
    );
  }

  if (!body) return null;
  return (
    <div className={`pill${expanded ? " pill-expanded" : ""}`} data-state={s.state} data-tauri-drag-region>
      {body}
    </div>
  );
}
