-- Nota personal del usuario sobre la reunión («nota para mí»). Va al LLM como
-- contexto privado y sale en el acta en su propia sección.
ALTER TABLE meetings ADD COLUMN user_note TEXT;
