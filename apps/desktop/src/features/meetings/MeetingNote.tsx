import { useState } from "react";

interface Props {
  /** Nota guardada (null = sin nota). */
  saved: string | null | undefined;
  onSave: (note: string) => void;
}

/** «Nota para mí»: privada; se guarda al salir del campo y va al acta como contexto. */
export function MeetingNote({ saved, onSave }: Props) {
  const [draft, setDraft] = useState(saved ?? "");
  return (
    <div className="meeting-note">
      <label htmlFor="meeting-note">Nota para mí</label>
      <textarea
        id="meeting-note"
        rows={2}
        value={draft}
        placeholder="Algo que quieras recordar o que el acta tenga en cuenta. Solo la ves tú."
        onChange={(e) => setDraft(e.target.value)}
        onBlur={() => {
          if (draft.trim() !== (saved ?? "").trim()) onSave(draft);
        }}
      />
    </div>
  );
}
