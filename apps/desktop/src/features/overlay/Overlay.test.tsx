import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createMockApi, type MeetingApp, type Prompt, type SessionSnapshot } from "../../api";
import { ApiProvider } from "../../app/ApiContext";
import { OverlayContainer } from "./OverlayContainer";

const app: MeetingApp = { platform: "teams", bundleId: "com.microsoft.teams2", pid: 1, audioPids: [1], name: "Microsoft Teams", title: null, url: null };
const prompt = (kind: "start" | "stop", createdAt = Date.now()): Prompt =>
  kind === "start"
    ? { kind: "startRecording", app, id: "p1", createdAt }
    : { kind: "stopRecording", reason: "meetingEnded", id: "p2", createdAt };

const snap = (state: SessionSnapshot["state"], extra: Partial<SessionSnapshot> = {}): SessionSnapshot => ({
  state,
  elapsedMs: 0,
  warnings: [],
  screenSharing: false,
  captureCount: 0,
  at: Date.now(),
  ...extra,
});
const recording = (extra: Record<string, unknown> = {}) => ({ state: "recording" as const, meetingId: "m1", app, startedAt: Date.now(), stopPrompt: null, ...extra });

function setup(initial: SessionSnapshot, mount = true) {
  const mock = createMockApi();
  mock.controls.state.snapshot = initial;
  if (mount) render(
    <ApiProvider api={mock.api}>
      <OverlayContainer />
    </ApiProvider>,
  );
  return mock;
}

beforeEach(() => vi.useFakeTimers({ toFake: ["setInterval", "clearInterval", "Date"] }));
afterEach(() => vi.useRealTimers());

describe("Overlay", () => {
  it("en reposo no pinta nada", async () => {
    setup(snap({ state: "idle" }));
    await waitFor(() => expect(screen.queryByRole("group")).not.toBeInTheDocument());
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
  });

  it("pregunta si grabar al detectar una reunión", async () => {
    const { api } = setup(snap({ state: "detected", prompt: prompt("start"), app }));
    expect(await screen.findByText(/Reunión de Teams detectada/)).toBeInTheDocument();
    const spy = vi.spyOn(api, "promptAnswer");
    await userEvent.click(screen.getByRole("button", { name: "Grabar" }), { advanceTimers: vi.advanceTimersByTime });
    expect(spy).toHaveBeenCalledWith("p1", "accept");
  });

  it("Ignorar rechaza la detección", async () => {
    const { api } = setup(snap({ state: "detected", prompt: prompt("start"), app }));
    const spy = vi.spyOn(api, "promptAnswer");
    await userEvent.click(await screen.findByRole("button", { name: "Ignorar" }), { advanceTimers: vi.advanceTimersByTime });
    expect(spy).toHaveBeenCalledWith("p1", "decline");
  });

  it("grabación automática: muestra ventana de descarte y permite descartar", async () => {
    const created = Date.now();
    const mock = createMockApi();
    mock.controls.state.settings.detection.autoRecord = true;
    mock.controls.state.settings.detection.autoRecordDelayS = 10;
    const autoPrompt: Prompt = { kind: "stopRecording", reason: "autoStarted", id: "p-auto", createdAt: created };
    mock.controls.state.snapshot = snap(recording({ stopPrompt: autoPrompt }));
    render(
      <ApiProvider api={mock.api}>
        <OverlayContainer />
      </ApiProvider>,
    );
    const box = await screen.findByRole("group", { name: "Grabación automática" });
    expect(within(box).getByText(/Si paras en 00:10, se descartará/)).toBeInTheDocument();
    act(() => vi.advanceTimersByTime(4_000));
    expect(within(box).getByText(/Si paras en 00:06, se descartará/)).toBeInTheDocument();
    const spy = vi.spyOn(mock.api, "promptAnswer");
    await userEvent.click(within(box).getByRole("button", { name: "Descartar" }), { advanceTimers: vi.advanceTimersByTime });
    expect(spy).toHaveBeenCalledWith("p-auto", "accept");
  });

  it("grabando: temporizador, medidores y controles", async () => {
    const { controls, api } = setup(snap(recording(), { elapsedMs: 65_000 }));
    expect(await screen.findByText("01:05")).toBeInTheDocument();
    act(() =>
      controls.emit("session:levels", {
        mic: { rmsDbfs: -30, peakDbfs: -20, allZero: false },
        system: { rmsDbfs: -60, peakDbfs: -50, allZero: false },
        at: Date.now(),
      }),
    );
    expect(screen.getByRole("meter", { name: "Nivel de Yo" })).toHaveAttribute("aria-valuenow", "50");
    expect(screen.getByRole("meter", { name: "Nivel de Otros" })).toHaveAttribute("aria-valuenow", "0");
    const pause = vi.spyOn(api, "sessionPause");
    await userEvent.click(screen.getByRole("button", { name: "Pausar" }), { advanceTimers: vi.advanceTimersByTime });
    expect(pause).toHaveBeenCalled();
  });

  it("el temporizador avanza con el reloj", async () => {
    setup(snap(recording(), { elapsedMs: 0 }));
    await screen.findByText("00:00");
    act(() => vi.advanceTimersByTime(3000));
    expect(screen.getByText("00:03")).toBeInTheDocument();
  });

  it("en pausa se ofrece reanudar", async () => {
    const { api } = setup(snap({ ...recording(), state: "paused", pausedAt: Date.now() }));
    const spy = vi.spyOn(api, "sessionResume");
    await userEvent.click(await screen.findByRole("button", { name: "Reanudar" }), { advanceTimers: vi.advanceTimersByTime });
    expect(spy).toHaveBeenCalled();
  });

  it("Parar detiene la grabación", async () => {
    const { api } = setup(snap(recording()));
    const spy = vi.spyOn(api, "sessionStop");
    await userEvent.click(await screen.findByRole("button", { name: "Parar" }), { advanceTimers: vi.advanceTimersByTime });
    expect(spy).toHaveBeenCalled();
  });

  it("la llamada ha terminado: Parar / Seguir grabando con cuenta atrás", async () => {
    const created = Date.now();
    const { api } = setup(snap(recording({ stopPrompt: prompt("stop", created) })));
    const box = await screen.findByRole("group", { name: "La llamada ha terminado" });
    expect(within(box).getByText(/La llamada ha terminado — ¿Parar\?/)).toBeInTheDocument();
    expect(within(box).getByText(/parará solo en 02:00/)).toBeInTheDocument();
    act(() => vi.advanceTimersByTime(10_000));
    expect(within(box).getByText(/parará solo en 01:50/)).toBeInTheDocument();
    const spy = vi.spyOn(api, "promptAnswer");
    await userEvent.click(within(box).getByRole("button", { name: "Seguir grabando" }), { advanceTimers: vi.advanceTimersByTime });
    expect(spy).toHaveBeenCalledWith("p2", "decline");
  });

  it("confirmar la pregunta de parar", async () => {
    const { api } = setup(snap(recording({ stopPrompt: prompt("stop") })));
    const spy = vi.spyOn(api, "promptAnswer");
    const box = await screen.findByRole("group", { name: "La llamada ha terminado" });
    await userEvent.click(within(box).getByRole("button", { name: "Parar" }), { advanceTimers: vi.advanceTimersByTime });
    expect(spy).toHaveBeenCalledWith("p2", "accept");
  });

  it("avisos de salud visibles y se limpian", async () => {
    const { controls } = setup(snap(recording()));
    await screen.findByText("00:00");
    const warning = { issue: { kind: "trackAllZeros", track: "system" } as const, since: Date.now(), cleared: false, message: "No llega audio de Teams — revisa el permiso" };
    act(() => controls.emit("session:health", warning));
    expect(await screen.findByRole("alert")).toHaveTextContent("No llega audio de Teams — revisa el permiso");
    act(() => controls.emit("session:health", { ...warning, cleared: true }));
    await waitFor(() => expect(screen.queryByRole("alert")).not.toBeInTheDocument());
  });

  it("avisa a la ventana para expandirse cuando hay pregunta", async () => {
    const { api } = setup(snap({ state: "detected", prompt: prompt("start"), app }), false);
    const spy = vi.spyOn(api, "overlaySetExpanded");
    render(
      <ApiProvider api={api}>
        <OverlayContainer />
      </ApiProvider>,
    );
    await screen.findByText(/detectada/);
    expect(spy).toHaveBeenCalledWith(true);
  });

  it("procesando muestra el paso", async () => {
    setup(snap({ state: "processing", meetingId: "m1", step: "summarizing" }));
    expect(await screen.findByText(/Resumiendo/)).toBeInTheDocument();
  });

  it("lista: abre la reunión", async () => {
    const { api } = setup(snap({ state: "ready", meetingId: "m1" }));
    const spy = vi.spyOn(api, "mainWindowShow");
    await userEvent.click(await screen.findByRole("button", { name: "Abrir acta" }), { advanceTimers: vi.advanceTimersByTime });
    expect(spy).toHaveBeenCalledWith("m1");
  });

  it("fallo: muestra el error y permite cerrar", async () => {
    const { api } = setup(snap({ state: "failed", meetingId: null, error: "Sin espacio" }));
    expect(await screen.findByText(/Sin espacio/)).toBeInTheDocument();
    const spy = vi.spyOn(api, "sessionDismiss");
    await userEvent.click(screen.getByRole("button", { name: "Cerrar" }), { advanceTimers: vi.advanceTimersByTime });
    expect(spy).toHaveBeenCalled();
  });
});

describe("pastilla: detalles", () => {
  it("se expande solo cuando hay algo que decidir o avisar", async () => {
    const { controls } = setup(snap(recording()));
    await screen.findByText("00:00");
    const pill = document.querySelector(".pill")!;
    expect(pill).not.toHaveClass("pill-expanded");
    act(() => controls.emit("session:state", snap(recording({ stopPrompt: prompt("stop") }))));
    await screen.findByRole("group", { name: "La llamada ha terminado" });
    expect(document.querySelector(".pill")).toHaveClass("pill-expanded");
  });

  it("al detectar una reunión enseña de cuál se trata", async () => {
    setup(snap({ state: "detected", prompt: prompt("start"), app: { ...app, title: "Daily del equipo" } }));
    const box = await screen.findByRole("group", { name: "Reunión detectada" });
    expect(within(box).getByText("Daily del equipo")).toBeInTheDocument();
  });

  it("si para por silencio lo dice", async () => {
    setup(snap(recording({ stopPrompt: { kind: "stopRecording", reason: "silence", id: "p3", createdAt: Date.now() } })));
    expect(await screen.findByText(/Parece que no habla nadie — ¿Parar\?/)).toBeInTheDocument();
  });

  it("en pausa los medidores se quedan a cero", async () => {
    const { controls } = setup(snap({ ...recording(), state: "paused", pausedAt: Date.now() }));
    await screen.findByRole("button", { name: "Reanudar" });
    act(() => controls.emit("session:levels", { mic: { rmsDbfs: -10, peakDbfs: -5, allZero: false }, system: { rmsDbfs: -10, peakDbfs: -5, allZero: false }, at: Date.now() }));
    expect(screen.getByRole("meter", { name: "Nivel de Yo" })).toHaveAttribute("aria-valuenow", "0");
  });

  it("guardando y recuperando se explican", async () => {
    const { controls } = setup(snap({ state: "stopping", meetingId: "m1", reason: "user" }));
    expect(await screen.findByText(/Guardando el audio/)).toBeInTheDocument();
    act(() => controls.emit("session:state", snap({ state: "recovering", meetingId: "m1" })));
    expect(await screen.findByText(/Recuperando una grabación interrumpida/)).toBeInTheDocument();
  });

  it("grabando, un toque abre la vista en directo en la ventana principal", async () => {
    vi.useRealTimers();
    const { api } = setup(snap(recording()));
    const show = vi.spyOn(api, "mainWindowShow");
    await userEvent.click(await screen.findByRole("button", { name: "Ver en directo" }));
    expect(show).toHaveBeenCalledWith("m1");
  });
});
