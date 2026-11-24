import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { MeetingNote } from "./MeetingNote";

describe("MeetingNote", () => {
  it("muestra la nota guardada y no guarda si no cambió al salir del campo", async () => {
    const onSave = vi.fn();
    render(<MeetingNote saved="Pedir descuento" onSave={onSave} />);
    const box = screen.getByLabelText("Nota para mí");
    expect(box).toHaveValue("Pedir descuento");
    await userEvent.click(box);
    await userEvent.tab();
    expect(onSave).not.toHaveBeenCalled();
  });

  it("guarda al salir del campo, no mientras se escribe", async () => {
    const onSave = vi.fn();
    render(<MeetingNote saved={null} onSave={onSave} />);
    const box = screen.getByLabelText("Nota para mí");
    await userEvent.type(box, "Llamar a Ana");
    expect(onSave).not.toHaveBeenCalled();
    await userEvent.tab();
    expect(onSave).toHaveBeenCalledExactlyOnceWith("Llamar a Ana");
  });

  it("vaciar la nota la guarda como vacía (el backend la borra)", async () => {
    const onSave = vi.fn();
    render(<MeetingNote saved="Algo" onSave={onSave} />);
    await userEvent.clear(screen.getByLabelText("Nota para mí"));
    await userEvent.tab();
    expect(onSave).toHaveBeenCalledExactlyOnceWith("");
  });

  it("solo espacios sobre una nota vacía no dispara guardado", async () => {
    const onSave = vi.fn();
    render(<MeetingNote saved={undefined} onSave={onSave} />);
    await userEvent.type(screen.getByLabelText("Nota para mí"), "   ");
    await userEvent.tab();
    expect(onSave).not.toHaveBeenCalled();
  });
});
