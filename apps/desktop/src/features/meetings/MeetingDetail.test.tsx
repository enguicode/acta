import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { createMockApi } from "../../api";
import { sampleMeeting } from "../../api/mock";
import { ApiProvider } from "../../app/ApiContext";
import { MeetingDetailContainer } from "./MeetingDetailContainer";

function setup(mutate: (d: ReturnType<typeof sampleMeeting>) => void = () => {}, props: Partial<React.ComponentProps<typeof MeetingDetailContainer>> = {}) {
  const { api, controls } = createMockApi();
  const d = controls.state.meetings[0]!;
  d.audio = { mic: "/m/mic.wav", system: "/m/system.wav", mix: "/m/mix.wav" };
  mutate(d);
  const onBack = vi.fn();
  const view = render(
    <ApiProvider api={api}>
      <MeetingDetailContainer meetingId={d.meeting.id} onBack={onBack} onOpenMeeting={() => {}} {...props} />
    </ApiProvider>,
  );
  return { api, controls, d, onBack, ...view };
}

describe("MeetingDetailContainer", () => {
  it("carga la reunión y muestra el resumen", async () => {
    setup();
    expect(await screen.findByRole("heading", { name: "Daily del equipo" })).toBeInTheDocument();
    expect(screen.getByText("Se revisó el plan del trimestre.")).toBeInTheDocument();
    expect(screen.getAllByRole("tab").map((t) => t.textContent)).toEqual(["Resumen", "Transcript", "Escaleta", "Timeline", "Capturas", "Chat"]);
  });

  it("marcar una tarea la persiste", async () => {
    const { controls } = setup();
    await userEvent.click(await screen.findByRole("checkbox", { name: "Enviar presupuesto" }));
    await waitFor(() => expect(controls.state.meetings[0]!.minutes!.actionItems[0]!.done).toBe(true));
    expect(screen.getByRole("checkbox", { name: "Enviar presupuesto" })).toBeChecked();
  });

  it("pulsar una línea del transcript salta el audio", async () => {
    const { container } = setup();
    await userEvent.click(await screen.findByRole("tab", { name: "Transcript" }));
    await userEvent.click(screen.getByRole("button", { name: /Yo envío el presupuesto/ }));
    expect(container.querySelector("audio")!.currentTime).toBeCloseTo(4.5);
  });

  it("renombrar un hablante lo refleja", async () => {
    const { controls } = setup();
    await userEvent.click(await screen.findByRole("tab", { name: "Transcript" }));
    await userEvent.click(screen.getByRole("button", { name: "Renombrar a Ana" }));
    const input = screen.getByRole("combobox", { name: "Nombre del hablante" });
    await userEvent.clear(input);
    await userEvent.type(input, "Ana García");
    await userEvent.click(screen.getByRole("button", { name: "Guardar nombre" }));
    await waitFor(() => expect(screen.getAllByText("Ana García").length).toBeGreaterThan(0));
    expect(controls.state.meetings[0]!.transcript.speakers[1]!.confirmed).toBe(true);
  });

  it("edita el título", async () => {
    const { controls } = setup();
    await userEvent.click(await screen.findByRole("button", { name: "Editar título" }));
    const input = screen.getByRole("textbox", { name: "Título" });
    await userEvent.clear(input);
    await userEvent.type(input, "Plan Q4{Enter}");
    await waitFor(() => expect(controls.state.meetings[0]!.meeting.title).toBe("Plan Q4"));
    expect(await screen.findByRole("heading", { name: "Plan Q4" })).toBeInTheDocument();
  });

  it("guarda la nota para mí al salir del campo y la muestra al volver", async () => {
    const { controls } = setup();
    const note = await screen.findByRole("textbox", { name: "Nota para mí" });
    expect(note).toHaveValue("");
    await userEvent.type(note, "Pedir descuento a Ana");
    expect(controls.state.meetings[0]!.meeting.userNote ?? null).toBeNull();
    await userEvent.tab();
    await waitFor(() => expect(controls.state.meetings[0]!.meeting.userNote).toBe("Pedir descuento a Ana"));
  });

  it("enseña la nota guardada y vaciarla la borra", async () => {
    const { controls } = setup((d) => {
      d.meeting.userNote = "Ya estaba";
    });
    const note = await screen.findByRole("textbox", { name: "Nota para mí" });
    expect(note).toHaveValue("Ya estaba");
    await userEvent.clear(note);
    await userEvent.tab();
    await waitFor(() => expect(controls.state.meetings[0]!.meeting.userNote ?? null).toBeNull());
  });

  it("si el procesado falló, permite reintentar", async () => {
    const { controls } = setup((d) => {
      d.meeting.status = "failed";
      d.meeting.error = "claude no responde";
    });
    await userEvent.click(await screen.findByRole("button", { name: "Reintentar" }));
    await waitFor(() => expect(controls.state.retries[0]?.steps).toEqual(expect.arrayContaining(["summarizing"])));
  });

  it("asigna la reunión a un grupo", async () => {
    const { api, controls } = setup();
    const g = await api.groupCreate("Producto");
    await userEvent.click(await screen.findByRole("button", { name: "Añadir a grupo" }));
    await userEvent.click(await screen.findByRole("option", { name: /Producto/ }));
    await userEvent.click(screen.getByRole("button", { name: "Añadir a 1 grupo" }));
    await waitFor(() => expect(controls.state.meetings[0]!.groups.map((x) => x.id)).toContain(g.id));
  });

  it("vuelve atrás", async () => {
    const { onBack } = setup();
    await userEvent.click(await screen.findByRole("button", { name: "Volver" }));
    expect(onBack).toHaveBeenCalled();
  });

  it("elimina la reunión tras confirmar", async () => {
    const { api, controls, d, onBack } = setup();
    const remove = vi.spyOn(api, "meetingDelete");
    await userEvent.click(await screen.findByRole("button", { name: "Eliminar reunión" }));
    const dialog = screen.getByRole("dialog", { name: "Eliminar reunión" });
    expect(within(dialog).getByText(/Daily del equipo/)).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: "Cancelar" })).toHaveFocus();
    expect(remove).not.toHaveBeenCalled();
    expect(onBack).not.toHaveBeenCalled();
    await userEvent.click(within(dialog).getByRole("button", { name: "Eliminar reunión" }));
    await waitFor(() => expect(controls.state.meetings).toHaveLength(0));
    expect(remove).toHaveBeenCalledExactlyOnceWith(d.meeting.id);
    expect(onBack).toHaveBeenCalledOnce();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it.each(["cancelar", "Escape", "fondo"])("%s cierra la confirmación sin borrar y devuelve el foco", async (action) => {
    const { api, controls, onBack } = setup();
    const remove = vi.spyOn(api, "meetingDelete");
    const trigger = await screen.findByRole("button", { name: "Eliminar reunión" });
    await userEvent.click(trigger);
    const dialog = screen.getByRole("dialog", { name: "Eliminar reunión" });
    if (action === "cancelar") await userEvent.click(within(dialog).getByRole("button", { name: "Cancelar" }));
    else if (action === "Escape") await userEvent.keyboard("{Escape}");
    else await userEvent.click(dialog.parentElement!);
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(remove).not.toHaveBeenCalled();
    expect(controls.state.meetings.length).toBeGreaterThan(0);
    expect(onBack).not.toHaveBeenCalled();
    expect(trigger).toHaveFocus();
  });

  it("mantiene el foco dentro de la confirmación", async () => {
    setup();
    await userEvent.click(await screen.findByRole("button", { name: "Eliminar reunión" }));
    const dialog = screen.getByRole("dialog");
    const cancel = within(dialog).getByRole("button", { name: "Cancelar" });
    const confirm = within(dialog).getByRole("button", { name: "Eliminar reunión" });
    await userEvent.tab({ shift: true });
    expect(confirm).toHaveFocus();
    await userEvent.tab();
    expect(cancel).toHaveFocus();
    await userEvent.tab();
    expect(confirm).toHaveFocus();
    await userEvent.tab();
    expect(cancel).toHaveFocus();
  });

  it("espera el borrado, evita envíos duplicados y no permite cerrar mientras está pendiente", async () => {
    const { api, onBack } = setup();
    let finish!: () => void;
    const remove = vi.spyOn(api, "meetingDelete").mockImplementation(() => new Promise<void>((resolve) => { finish = resolve; }));
    await userEvent.click(await screen.findByRole("button", { name: "Eliminar reunión" }));
    const dialog = screen.getByRole("dialog");
    await userEvent.dblClick(within(dialog).getByRole("button", { name: "Eliminar reunión" }));
    expect(remove).toHaveBeenCalledOnce();
    expect(onBack).not.toHaveBeenCalled();
    expect(within(dialog).getByRole("button", { name: "Eliminando…" })).toBeDisabled();
    expect(within(dialog).getByRole("button", { name: "Cancelar" })).toBeDisabled();
    await userEvent.keyboard("{Escape}");
    await userEvent.click(dialog.parentElement!);
    expect(dialog).toBeInTheDocument();
    finish();
    await waitFor(() => expect(onBack).toHaveBeenCalledOnce());
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("muestra el error de borrado y permite reintentar sin salir de la reunión", async () => {
    const { api, controls, onBack } = setup();
    const remove = vi.spyOn(api, "meetingDelete").mockRejectedValueOnce({ message: "No se pudo eliminar el audio." });
    await userEvent.click(await screen.findByRole("button", { name: "Eliminar reunión" }));
    const dialog = screen.getByRole("dialog");
    await userEvent.click(within(dialog).getByRole("button", { name: "Eliminar reunión" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("No se pudo eliminar el audio.");
    expect(onBack).not.toHaveBeenCalled();
    expect(controls.state.meetings.length).toBeGreaterThan(0);
    await userEvent.click(within(dialog).getByRole("button", { name: "Eliminar reunión" }));
    await waitFor(() => expect(controls.state.meetings).toHaveLength(0));
    expect(remove).toHaveBeenCalledTimes(2);
    expect(onBack).toHaveBeenCalledOnce();
  });
});
