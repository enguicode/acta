import { useEffect, useMemo, useState } from "react";
import type { HealthWarning, Levels, Prompt, SessionSnapshot } from "../../api";
import { useApi, useApiEvent } from "../../app/ApiContext";
import { remainingSeconds } from "../../shared/format";
import "./overlay.css";
import "../../styles/interactive.css";
import { OverlayPill } from "./OverlayPill";

const DEFAULT_UNANSWERED_MS = 120_000;
const DEFAULT_DISCARD_WINDOW_MS = 10_000;
const key = (w: HealthWarning) => JSON.stringify(w.issue);

export function OverlayContainer() {
  const api = useApi();
  const [snapshot, setSnapshot] = useState<SessionSnapshot | null>(null);
  const [levels, setLevels] = useState<Levels | null>(null);
  const [warnings, setWarnings] = useState<Record<string, HealthWarning>>({});
  const [unansweredMs, setUnansweredMs] = useState(DEFAULT_UNANSWERED_MS);
  const [discardWindowMs, setDiscardWindowMs] = useState(DEFAULT_DISCARD_WINDOW_MS);
  const [now, setNow] = useState(() => Date.now());

  const apply = (s: SessionSnapshot) => {
    setSnapshot(s);
    setWarnings(Object.fromEntries(s.warnings.filter((w) => !w.cleared).map((w) => [key(w), w])));
  };

  useEffect(() => {
    void api.sessionGetState().then(apply);
  }, [api]);

  const state = snapshot?.state ?? { state: "idle" as const };
  const live = state.state === "recording" || state.state === "paused";
  const stopPrompt = live ? state.stopPrompt : null;
  const hasStopPrompt = stopPrompt != null;

  // Vuelve a leer las duraciones al abrirse una pregunta para aplicar cambios de Ajustes sin reiniciar.
  useEffect(() => {
    void api.settingsGet().then((s) => {
      setUnansweredMs(s.autoStop.autoStopAfterUnansweredMs);
      setDiscardWindowMs(s.detection.autoRecordDelayS * 1000);
    });
  }, [api, hasStopPrompt]);

  useApiEvent("session:state", apply);
  useApiEvent("session:levels", setLevels);
  useApiEvent("session:health", (w) =>
    setWarnings((prev) => {
      const { [key(w)]: _old, ...rest } = prev;
      return w.cleared ? rest : { ...rest, [key(w)]: w };
    }),
  );

  useEffect(() => {
    if (!live) return;
    setNow(Date.now());
    const id = setInterval(() => setNow(Date.now()), 500);
    return () => clearInterval(id);
  }, [live]);

  const warning = live ? (Object.values(warnings)[0] ?? null) : null;
  const expanded = state.state === "detected" || state.state === "ready" || state.state === "failed" || hasStopPrompt || warning != null;
  useEffect(() => {
    void api.overlaySetExpanded(expanded);
  }, [api, expanded]);

  const elapsedMs = useMemo(() => {
    if (!snapshot) return 0;
    return snapshot.elapsedMs + (snapshot.state.state === "recording" ? Math.max(0, now - snapshot.at) : 0);
  }, [snapshot, now]);

  if (!snapshot) return null;

  const isAutoStarted = stopPrompt?.kind === "stopRecording" && stopPrompt.reason === "autoStarted";
  const promptTotalMs = isAutoStarted ? discardWindowMs : unansweredMs;

  return (
    <OverlayPill
      state={state}
      elapsedMs={elapsedMs}
      levels={levels}
      warning={warning}
      countdownS={stopPrompt ? remainingSeconds(stopPrompt.createdAt, promptTotalMs, now) : null}
      countdownTotalS={promptTotalMs / 1000}
      onPause={() => void api.sessionPause()}
      onResume={() => void api.sessionResume()}
      onStop={() => void api.sessionStop()}
      onAnswer={(p: Prompt, a) => void api.promptAnswer(p.id, a)}
      onDismiss={() => void api.sessionDismiss()}
      onOpenMeeting={(id) => void api.mainWindowShow(id)}
      onLive={(id) => void api.mainWindowShow(id)}
    />
  );
}
