import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { createMockApi } from "../../api";
import { freshSettings } from "../../api/mock";
import { ApiProvider } from "../../app/ApiContext";
import { SettingsContainer } from "./SettingsContainer";

function setup(seed: Parameters<typeof createMockApi>[0] = {}) {
  const mock = createMockApi(seed);
  render(
    <ApiProvider api={mock.api}>
      <SettingsContainer />
    </ApiProvider>,
  );
  return mock;
}

/** Abre una sección desde la lista de la izquierda (como en Ajustes del Sistema). */
async function goTo(name: string) {
  const nav = await screen.findByRole("navigation", { name: "Secciones de ajustes" });
  await userEvent.click(within(nav).getByRole("button", { name }));
}
const advanced = (name = "Avanzado") => userEvent.click(screen.getByRole("button", { name }));

describe("SettingsContainer", () => {
  it("empieza en General y lista las secciones como Ajustes del Sistema", async () => {
    setup();
    expect(await screen.findByRole("region", { name: "General" })).toBeInTheDocument();
    const nav = screen.getByRole("navigation", { name: "Secciones de ajustes" });
    expect(within(nav).getAllByRole("button").map((b) => b.textContent)).toEqual([
      "General",
      "Grabación y autoparada",
      "Detección",
      "Capturas",
      "Proveedores de IA",
      "Modelos",
      "Permisos",
      "Acerca de",
    ]);
    await goTo("Permisos");
    expect(screen.queryByRole("region", { name: "General" })).not.toBeInTheDocument();
    expect(within(nav).getByRole("button", { name: "Permisos" })).toHaveAttribute("aria-current", "page");
  });

  it("la barra de guardado solo aparece cuando hay cambios", async () => {
    setup();
    await goTo("Capturas");
    expect(screen.queryByRole("button", { name: "Guardar cambios" })).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("checkbox", { name: "Capturar la pantalla compartida" }));
    expect(screen.getByText("Tienes cambios sin guardar")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Descartar" }));
    expect(screen.queryByRole("button", { name: "Guardar cambios" })).not.toBeInTheDocument();
    expect(screen.getByRole("checkbox", { name: "Capturar la pantalla compartida" })).not.toBeChecked();
  });

  it("confirma que se ha guardado", async () => {
    setup();
    await goTo("Capturas");
    await userEvent.click(screen.getByRole("checkbox", { name: "Capturar la pantalla compartida" }));
    await userEvent.click(screen.getByRole("button", { name: "Guardar cambios" }));
    expect(await screen.findByRole("status")).toHaveTextContent("Cambios guardados");
  });

  it("guarda tu nombre y tus alias, que se editan como fichas", async () => {
    const { controls } = setup();
    const name = await screen.findByLabelText("Tu nombre");
    expect(name).toHaveValue("Álex Romero");
    await userEvent.clear(name);
    await userEvent.type(name, "Alejandra Romero");
    const aliases = screen.getByLabelText("Otros nombres");
    await userEvent.type(aliases, "Ale{Enter}Jandra,");
    await userEvent.click(screen.getByRole("button", { name: "Quitar «Álex»" }));
    await userEvent.click(screen.getByRole("button", { name: "Guardar cambios" }));
    await waitFor(() => expect(controls.state.settings.user).toEqual({ displayName: "Alejandra Romero", aliases: ["Ale", "Jandra"] }));
  });

  it("en una instalación nueva no hay nombre ni alias de nadie, y avisa de para qué sirve", async () => {
    setup({ settings: { ...freshSettings(), onboarding: { completed: true } } });
    const name = await screen.findByLabelText("Tu nombre");
    expect(name).toHaveValue("");
    expect(screen.getByText("Sin tu nombre, acta no puede saber qué tareas son tuyas.")).toBeInTheDocument();
    expect(screen.getByLabelText("Otros nombres")).toHaveAttribute("placeholder", "Álex, Alejandra");
    expect(screen.queryByRole("button", { name: /^Quitar «/ })).not.toBeInTheDocument();
  });

  it("idioma de las reuniones: automático por defecto y los mismos idiomas que la bienvenida", async () => {
    const { controls } = setup();
    const language = await screen.findByLabelText("Idioma de las reuniones");
    expect(language).toHaveValue("auto");
    expect(within(language).getAllByRole("option").map((o) => o.textContent)).toEqual(["Automático (detectar)", "Español", "English", "Català", "Français", "Deutsch", "Italiano", "Português"]);
    expect(screen.getByText(/La interfaz de acta está, por ahora, en español/)).toBeInTheDocument();
    await userEvent.selectOptions(language, "ca");
    await userEvent.click(screen.getByRole("button", { name: "Guardar cambios" }));
    await waitFor(() => expect(controls.state.settings.transcription.language).toBe("ca"));
  });

  it("si los ajustes no se pueden leer, lo dice y deja reintentar", async () => {
    const mock = createMockApi();
    const get = vi.spyOn(mock.api, "settingsGet").mockRejectedValueOnce({ code: "io", message: "disco lleno" });
    render(
      <ApiProvider api={mock.api}>
        <SettingsContainer />
      </ApiProvider>,
    );
    expect(await screen.findByRole("alert")).toHaveTextContent("No se han podido leer los ajustes: disco lleno");
    await userEvent.click(screen.getByRole("button", { name: "Reintentar" }));
    expect(await screen.findByRole("region", { name: "General" })).toBeInTheDocument();
    expect(get).toHaveBeenCalledTimes(2);
  });

  it("usar el calendario para titular reuniones es opcional y se guarda", async () => {
    const { controls } = setup();
    const toggle = await screen.findByRole("checkbox", { name: "Usar el calendario para titular reuniones" });
    expect(toggle).not.toBeChecked();
    await userEvent.click(toggle);
    await userEvent.click(screen.getByRole("button", { name: "Guardar cambios" }));
    await waitFor(() => expect(controls.state.settings.titles).toEqual({ calendar: true }));
  });

  it("limpiar el transcript con el LLM viene encendido y se puede apagar", async () => {
    const { controls } = setup();
    const toggle = await screen.findByRole("checkbox", { name: "Limpiar el transcript con IA" });
    expect(toggle).toBeChecked();
    expect(controls.state.settings.transcription.llmCleanup).toBe(true);
    await userEvent.click(toggle);
    await userEvent.click(screen.getByRole("button", { name: "Guardar cambios" }));
    await waitFor(() => expect(controls.state.settings.transcription.llmCleanup).toBe(false));
  });

  it("esconde lo técnico de un proveedor tras «Avanzado»", async () => {
    setup();
    await goTo("Proveedores de IA");
    const section = await screen.findByRole("region", { name: "Proveedores de IA" });
    expect(within(section).getByText("Instalado en tu Mac")).toBeInTheDocument();
    expect(within(section).queryByLabelText("Ruta del binario de Claude Code (CLI)")).not.toBeInTheDocument();
    expect(within(section).queryByText(/Variables de entorno/)).not.toBeInTheDocument();
    await advanced("Avanzado de Claude Code (CLI)");
    expect(within(section).getByLabelText("Ruta del binario de Claude Code (CLI)")).toHaveValue("/opt/homebrew/bin/claude");
    expect(within(section).getByText(/Variables de entorno/)).toBeInTheDocument();
  });

  it("avisa si falta localizar el CLI", async () => {
    const mock = createMockApi();
    const k = mock.controls.state.settings.llm.providers[0]!.kind;
    if (k.type === "claudeCli") k.binaryPath = "";
    render(
      <ApiProvider api={mock.api}>
        <SettingsContainer />
      </ApiProvider>,
    );
    await goTo("Proveedores de IA");
    expect(await screen.findByText(/Sin localizar/)).toBeInTheDocument();
  });

  describe("Detección: listas de apps", () => {
    const list = async (name: string) => within(await screen.findByRole("region", { name: "Detección" })).getByRole("group", { name });
    const pick = async (group: HTMLElement, app: string) => {
      await userEvent.click(within(group).getByRole("button", { name: "Añadir app" }));
      await userEvent.click(await screen.findByRole("option", { name: app }));
    };
    const saveChanges = () => userEvent.click(screen.getByRole("button", { name: "Guardar cambios" }));

    it("enseña cada app con su nombre, su icono y su identificador", async () => {
      const mock = createMockApi();
      mock.controls.state.apps = mock.controls.state.apps.map((a) => (a.bundleId === "com.microsoft.teams2" ? { ...a, iconPath: "/cache/teams.png" } : a));
      render(
        <ApiProvider api={mock.api}>
          <SettingsContainer />
        </ApiProvider>,
      );
      await goTo("Detección");
      const row = within(await list("Apps de reunión")).getByRole("listitem", { name: "Microsoft Teams" });
      expect(within(row).getByText("com.microsoft.teams2")).toBeInTheDocument();
      expect(within(row).getByText("Teams")).toBeInTheDocument();
      await waitFor(() => expect(row.querySelector("img")).toHaveAttribute("src", "file:///cache/teams.png"));
    });

    it("añade una app habitual instalada con un toque", async () => {
      const { controls } = setup();
      await goTo("Detección");
      const rules = await list("Apps de reunión");
      await userEvent.click(await within(rules).findByRole("button", { name: "Añadir Zoom" }));
      expect(within(rules).queryByRole("button", { name: "Añadir Zoom" })).not.toBeInTheDocument();
      // Chrome no está instalado en este Mac: no se ofrece.
      expect(within(rules).queryByRole("button", { name: "Añadir Google Chrome" })).not.toBeInTheDocument();
      await saveChanges();
      await waitFor(() => expect(controls.state.settings.detection.rules).toContainEqual({ bundleId: "us.zoom.xos", platform: "zoom", isBrowser: false }));
    });

    it("apps ignoradas: se eligen de la lista de apps de este Mac y se guardan", async () => {
      const { controls } = setup();
      await goTo("Detección");
      const ignored = await list("Apps ignoradas");
      expect(within(ignored).getByText("Ninguna")).toBeInTheDocument();
      await pick(ignored, "Slack");
      expect(within(ignored).getByRole("listitem", { name: "Slack" })).toBeInTheDocument();
      expect(screen.queryByRole("button", { name: "Guardar cambios" })).toBeInTheDocument();
      expect(controls.state.settings.detection.denylist).toEqual([]);
      await saveChanges();
      await waitFor(() => expect(controls.state.settings.detection.denylist).toEqual(["com.tinyspeck.slackmacgap"]));
      await userEvent.click(within(ignored).getByRole("button", { name: "Dejar de ignorar Slack" }));
      await saveChanges();
      await waitFor(() => expect(controls.state.settings.detection.denylist).toEqual([]));
    });

    it("sugiere ignorar las apps de dictado que están instaladas, y solo esas", async () => {
      const { controls } = setup();
      await goTo("Detección");
      const ignored = await list("Apps ignoradas");
      await userEvent.click(await within(ignored).findByRole("button", { name: "Ignorar Typeless" }));
      expect(within(ignored).getByRole("button", { name: "Ignorar superwhisper" })).toBeInTheDocument();
      expect(within(ignored).queryByRole("button", { name: "Ignorar Typeless" })).not.toBeInTheDocument();
      expect(within(ignored).queryByRole("button", { name: /Ignorar (Wispr Flow|MacWhisper|Orca|Aqua Voice)/ })).not.toBeInTheDocument();
      await saveChanges();
      await waitFor(() => expect(controls.state.settings.detection.denylist).toEqual(["com.typeless.app"]));
    });

    it("sin apps de dictado instaladas no sugiere nada", async () => {
      setup({ apps: [{ bundleId: "com.apple.Safari", name: "Safari", path: "/Applications/Safari.app", running: false, iconPath: null }] });
      await goTo("Detección");
      const ignored = await list("Apps ignoradas");
      await within(await list("Apps de reunión")).findByRole("button", { name: "Añadir Safari" });
      expect(within(ignored).queryByRole("button", { name: /^Ignorar / })).not.toBeInTheDocument();
    });

    it("a mano, por identificador: valida, avisa de los repetidos y añade", async () => {
      const { controls } = setup();
      await goTo("Detección");
      const ignored = await list("Apps ignoradas");
      await userEvent.click(within(ignored).getByRole("button", { name: "Avanzado de Apps ignoradas" }));
      const input = within(ignored).getByLabelText("Identificador de la app");
      await userEvent.type(input, "dictado{Enter}");
      expect(within(ignored).getByRole("alert")).toHaveTextContent(/no parece un identificador/);
      await userEvent.clear(input);
      await userEvent.type(input, " com.example.dictado {Enter}");
      expect(within(ignored).getByRole("listitem", { name: "Dictado" })).toBeInTheDocument();
      await userEvent.type(input, "com.example.dictado{Enter}");
      expect(within(ignored).getByRole("alert")).toHaveTextContent("Dictado ya está en la lista.");
      await saveChanges();
      await waitFor(() => expect(controls.state.settings.detection.denylist).toEqual(["com.example.dictado"]));
    });

    it("app de reunión: tras elegirla pregunta qué es, con lo más probable ya puesto", async () => {
      const { controls } = setup();
      await goTo("Detección");
      const rules = await list("Apps de reunión");
      await pick(rules, "Safari");
      const step = within(rules).getByRole("group", { name: "Tipo de reunión de Safari" });
      expect(within(step).getByRole("button", { name: "Google Meet" })).toHaveAttribute("aria-pressed", "true");
      expect(within(step).getByRole("checkbox", { name: "Es un navegador" })).toBeChecked();
      expect(controls.state.settings.detection.rules).toHaveLength(1);
      await userEvent.click(within(step).getByRole("button", { name: "Añadir a la lista" }));
      expect(within(rules).getByRole("listitem", { name: "Safari" })).toHaveTextContent("Google Meet · en el navegador");
      await pick(rules, "Slack");
      const slack = within(rules).getByRole("group", { name: "Tipo de reunión de Slack" });
      expect(within(slack).getByRole("button", { name: "Slack" })).toHaveAttribute("aria-pressed", "true");
      expect(within(slack).getByRole("checkbox", { name: "Es un navegador" })).not.toBeChecked();
      await userEvent.click(within(slack).getByRole("button", { name: "Otra" }));
      await userEvent.click(within(slack).getByRole("button", { name: "Añadir a la lista" }));
      await saveChanges();
      await waitFor(() =>
        expect(controls.state.settings.detection.rules).toEqual([
          { bundleId: "com.microsoft.teams2", platform: "teams", isBrowser: false },
          { bundleId: "com.apple.Safari", platform: "meet", isBrowser: true },
          { bundleId: "com.tinyspeck.slackmacgap", platform: "other", isBrowser: false },
        ]),
      );
    });

    it("app de reunión: se puede cancelar la elección, añadir por identificador y quitar", async () => {
      const { controls } = setup();
      await goTo("Detección");
      const rules = await list("Apps de reunión");
      await pick(rules, "Safari");
      await userEvent.click(within(rules).getByRole("button", { name: "Cancelar" }));
      expect(within(rules).queryByRole("listitem", { name: "Safari" })).not.toBeInTheDocument();
      await userEvent.click(within(rules).getByRole("button", { name: "Avanzado de Apps de reunión" }));
      await userEvent.type(within(rules).getByLabelText("Identificador de la app"), "com.hnc.Discord{Enter}");
      const step = within(rules).getByRole("group", { name: "Tipo de reunión de Discord" });
      expect(within(step).getByRole("button", { name: "Otra" })).toHaveAttribute("aria-pressed", "true");
      await userEvent.click(within(step).getByRole("button", { name: "Añadir a la lista" }));
      await userEvent.click(within(rules).getByRole("button", { name: "Quitar Microsoft Teams" }));
      await saveChanges();
      await waitFor(() => expect(controls.state.settings.detection.rules).toEqual([{ bundleId: "com.hnc.Discord", platform: "other", isBrowser: false }]));
    });

    it("si no se pueden leer las apps del Mac, las listas siguen ahí y el selector deja reintentar", async () => {
      const mock = createMockApi();
      const apps = vi.spyOn(mock.api, "appsList").mockRejectedValueOnce({ code: "io", message: "sin acceso" });
      render(
        <ApiProvider api={mock.api}>
          <SettingsContainer />
        </ApiProvider>,
      );
      await goTo("Detección");
      const ignored = await list("Apps ignoradas");
      expect(within(await list("Apps de reunión")).getByRole("listitem", { name: "Microsoft Teams" })).toBeInTheDocument();
      await userEvent.click(within(ignored).getByRole("button", { name: "Añadir app" }));
      expect(await screen.findByRole("alert")).toHaveTextContent("sin acceso");
      await userEvent.click(screen.getByRole("button", { name: "Reintentar" }));
      expect(await screen.findByRole("option", { name: "Typeless" })).toBeInTheDocument();
      expect(apps).toHaveBeenCalledTimes(2);
    });

    it("las apps del Mac solo se piden al entrar en Detección", async () => {
      const mock = createMockApi();
      const apps = vi.spyOn(mock.api, "appsList");
      render(
        <ApiProvider api={mock.api}>
          <SettingsContainer />
        </ApiProvider>,
      );
      await screen.findByRole("region", { name: "General" });
      expect(apps).not.toHaveBeenCalled();
      await goTo("Detección");
      await waitFor(() => expect(apps).toHaveBeenCalledTimes(1));
    });
  });

  it("edita las palabras clave como fichas", async () => {
    const { controls } = setup();
    await goTo("Capturas");
    await userEvent.type(await screen.findByLabelText("Palabras clave"), "urgente{Enter}");
    await userEvent.click(screen.getByRole("button", { name: "Quitar «importante»" }));
    await userEvent.click(screen.getByRole("button", { name: "Guardar cambios" }));
    await waitFor(() => expect(controls.state.settings.screen.keywords).toEqual(["apuntad", "esto es clave", "deadline", "fecha", "urgente"]));
  });

  it("los contadores suben y bajan con sus botones", async () => {
    const { controls } = setup();
    await goTo("Grabación y autoparada");
    await userEvent.click(await screen.findByRole("button", { name: "Aumentar: Minutos de silencio antes de preguntar" }));
    expect(screen.getByLabelText("Minutos de silencio antes de preguntar")).toHaveValue(4);
    await userEvent.click(screen.getByRole("button", { name: "Reducir: Duración máxima (horas)" }));
    expect(screen.getByLabelText("Duración máxima (horas)")).toHaveValue(3.5);
    await advanced();
    expect(screen.getByLabelText("Umbral de silencio (dBFS)")).toHaveValue(-50);
    await userEvent.click(screen.getByRole("button", { name: "Guardar cambios" }));
    await waitFor(() => expect(controls.state.settings.autoStop).toMatchObject({ silencePromptAfterMs: 240_000, maxDurationMs: 12_600_000 }));
  });

  it("acerca de: versión a la vista y la carpeta de datos en Avanzado", async () => {
    setup();
    await goTo("Acerca de");
    const section = await screen.findByRole("region", { name: "Acerca de" });
    expect(await within(section).findByText("0.1.0-mock")).toBeInTheDocument();
    expect(within(section).queryByText("/tmp/acta")).not.toBeInTheDocument();
    await advanced();
    expect(within(section).getByText("/tmp/acta")).toBeInTheDocument();
  });

  it("muestra el proveedor por defecto y prueba la conexión", async () => {
    const { api } = setup();
    await goTo("Proveedores de IA");
    const spy = vi.spyOn(api, "llmProviderTest");
    const section = await screen.findByRole("region", { name: "Proveedores de IA" });
    expect(within(section).getByLabelText("Nombre de Claude Code (CLI)")).toHaveValue("Claude Code (CLI)");
    await userEvent.click(within(section).getByRole("button", { name: "Probar conexión Claude Code (CLI)" }));
    expect(await within(section).findByText(/OK \(mock\)/)).toBeInTheDocument();
    expect(spy).toHaveBeenCalledWith("claude-cli");
  });

  it("guarda la ruta del binario editada", async () => {
    const { controls } = setup();
    await goTo("Proveedores de IA");
    await advanced("Avanzado de Claude Code (CLI)");
    const input = await screen.findByLabelText("Ruta del binario de Claude Code (CLI)");
    await userEvent.clear(input);
    await userEvent.type(input, "/usr/local/bin/claude");
    await userEvent.click(screen.getByRole("button", { name: "Guardar cambios" }));
    await waitFor(() => {
      const k = controls.state.settings.llm.providers[0]!.kind;
      expect(k.type === "claudeCli" && k.binaryPath).toBe("/usr/local/bin/claude");
    });
  });

  it("detecta los CLI y añade Codex", async () => {
    const { api } = setup();
    await goTo("Proveedores de IA");
    vi.spyOn(api, "llmCliDetect").mockResolvedValue({ claude: "/x/claude", codex: "/x/codex", opencode: "/x/opencode", claudeEnv: { CLAUDE_CONFIG_DIR: "/h/.config/claude" }, codexEnv: {}, opencodeEnv: {}, opencodeModel: "github-copilot/gemini-4-neon" });
    await userEvent.click(await screen.findByRole("button", { name: "Detectar CLI instalados" }));
    await userEvent.click(await screen.findByRole("button", { name: "Avanzado de Codex (CLI)" }));
    await userEvent.click(await screen.findByRole("button", { name: "Avanzado de OpenCode (CLI)" }));
    await advanced("Avanzado de Claude Code (CLI)");
    expect(await screen.findByDisplayValue("/x/codex")).toBeInTheDocument();
    expect(screen.getByDisplayValue("/x/opencode")).toBeInTheDocument();
    expect(screen.getByDisplayValue("github-copilot/gemini-4-neon")).toBeInTheDocument();
    expect(screen.getByDisplayValue("/x/claude")).toBeInTheDocument();
    expect(screen.getByDisplayValue("CLAUDE_CONFIG_DIR")).toBeInTheDocument();
    expect(screen.getByDisplayValue("/h/.config/claude")).toBeInTheDocument();
  });

  it("detectar los CLI dice lo que ha encontrado, o que no hay nada, o por qué ha fallado", async () => {
    const { api } = setup();
    await goTo("Proveedores de IA");
    const detect = vi.spyOn(api, "llmCliDetect");
    const button = await screen.findByRole("button", { name: "Detectar CLI instalados" });
    await userEvent.click(button);
    expect(await screen.findByText("Detectado en este Mac: Claude Code.")).toBeInTheDocument();
    detect.mockResolvedValueOnce({ claude: null, codex: null, opencode: null, claudeEnv: {}, codexEnv: {}, opencodeEnv: {} });
    await userEvent.click(button);
    expect(await screen.findByRole("alert")).toHaveTextContent(/No se ha encontrado Claude Code, Codex ni OpenCode/);
    detect.mockRejectedValueOnce({ code: "io", message: "no hay shell" });
    await userEvent.click(button);
    await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent("No se ha podido buscar en este Mac: no hay shell"));
  });

  it("nada en Proveedores apunta a carpetas o alias de quien escribió la app", async () => {
    setup();
    await goTo("Proveedores de IA");
    await advanced("Avanzado de Claude Code (CLI)");
    const section = await screen.findByRole("region", { name: "Proveedores de IA" });
    expect(section.textContent).not.toMatch(/claude-personal|CLAUDE_CONFIG_DIR|alias/i);
    expect(within(section).getByText(/Solo si tu instalación las necesita/)).toBeInTheDocument();
  });

  it("sin proveedor avisa de que habrá transcripción pero no resumen, y el primero que se añade se queda con todo", async () => {
    const { controls } = setup({ settings: { ...freshSettings(), onboarding: { completed: true } } });
    await goTo("Proveedores de IA");
    const section = await screen.findByRole("region", { name: "Proveedores de IA" });
    const note = within(section).getByRole("note", { name: "Sin proveedor de IA" });
    expect(note).toHaveTextContent("Aún no has elegido un proveedor de IA");
    expect(note).toHaveTextContent(/graba y transcribe tus reuniones igual, pero sin proveedor no hay resumen, tareas ni chat/);
    expect(within(section).queryByRole("group", { name: /Claude Code/ })).not.toBeInTheDocument();
    expect(within(section).getByLabelText("Proveedor para el acta")).toHaveValue("");
    await userEvent.click(within(section).getByRole("button", { name: "Añadir OpenRouter" }));
    expect(within(section).queryByRole("note", { name: "Sin proveedor de IA" })).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Guardar cambios" }));
    await waitFor(() => expect(controls.state.settings.llm).toMatchObject({ minutesProvider: "openrouter", chatProvider: "openrouter", namingProvider: "openrouter" }));
    expect(controls.state.settings.llm.providers.map((p) => p.id)).toEqual(["openrouter"]);
  });

  it("con proveedores pero nadie asignado al acta, también lo avisa", async () => {
    setup();
    await goTo("Proveedores de IA");
    await userEvent.selectOptions(await screen.findByLabelText("Proveedor para el acta"), "");
    expect(screen.getByRole("note", { name: "Sin proveedor de IA" })).toHaveTextContent("Nadie se encarga de escribir las actas");
  });

  it("edita el esfuerzo y las variables de entorno de Claude", async () => {
    const { controls } = setup();
    await goTo("Proveedores de IA");
    await advanced("Avanzado de Claude Code (CLI)");
    await userEvent.selectOptions(await screen.findByLabelText("Esfuerzo de Claude Code (CLI)"), "high");
    await userEvent.click(screen.getByRole("button", { name: "Añadir variable a Claude Code (CLI)" }));
    await userEvent.type(screen.getByLabelText("Nombre de la variable 1 de Claude Code (CLI)"), "FOO");
    await userEvent.type(screen.getByLabelText("Valor de la variable 1 de Claude Code (CLI)"), "bar");
    await userEvent.click(screen.getByRole("button", { name: "Guardar cambios" }));
    await waitFor(() => {
      const k = controls.state.settings.llm.providers[0]!.kind;
      expect(k.type === "claudeCli" && k.effort).toBe("high");
      expect(k.type === "claudeCli" && k.extraEnv).toEqual({ FOO: "bar" });
    });
    await userEvent.click(screen.getByRole("button", { name: "Quitar variable FOO de Claude Code (CLI)" }));
    await userEvent.selectOptions(screen.getByLabelText("Esfuerzo de Claude Code (CLI)"), "");
    await userEvent.click(screen.getByRole("button", { name: "Guardar cambios" }));
    await waitFor(() => {
      const k = controls.state.settings.llm.providers[0]!.kind;
      expect(k.type === "claudeCli" && k.effort).toBeNull();
      expect(k.type === "claudeCli" && k.extraEnv).toEqual({});
    });
  });

  it("guarda y borra la clave de API en el Llavero", async () => {
    const { api, controls } = setup();
    await goTo("Proveedores de IA");
    const set = vi.spyOn(api, "secretSet");
    const del = vi.spyOn(api, "secretDelete");
    await userEvent.click(await screen.findByRole("button", { name: "Añadir Otro servidor compatible" }));
    const input = await screen.findByLabelText("Clave de API de OpenAI compatible");
    expect(input).toHaveAttribute("type", "password");
    await userEvent.type(input, "sk-secreto");
    await userEvent.click(screen.getByRole("button", { name: "Guardar en el Llavero OpenAI compatible" }));
    await waitFor(() => expect(set).toHaveBeenCalledWith("custom", "sk-secreto"));
    await waitFor(() => expect(input).toHaveValue(""));
    await userEvent.click(screen.getByRole("button", { name: "Guardar cambios" }));
    await waitFor(() => {
      const k = controls.state.settings.llm.providers[1]!.kind;
      expect(k.type === "openAiCompatible" && k.apiKey).toEqual({ service: "acta", account: "provider:custom" });
    });
    expect(JSON.stringify(controls.state.settings)).not.toContain("sk-secreto");
    await userEvent.click(screen.getByRole("button", { name: "Borrar clave OpenAI compatible" }));
    await waitFor(() => expect(del).toHaveBeenCalledWith("custom"));
  });

  it("lista los modelos y descarga uno con progreso", async () => {
    const { api } = setup();
    await goTo("Modelos");
    const dl = vi.spyOn(api, "modelsDownload");
    const section = await screen.findByRole("region", { name: "Modelos" });
    const diar = await within(section).findByRole("listitem", { name: "Separar hablantes" });
    expect(within(await within(section).findByRole("listitem", { name: "Transcripción" })).getByText(/Descargado/)).toBeInTheDocument();
    await userEvent.click(within(diar).getByRole("button", { name: "Descargar Separar hablantes" }));
    expect(dl).toHaveBeenCalledWith("diarizer");
    await waitFor(() => expect(within(diar).getByText(/Descargado/)).toBeInTheDocument());
  });

  it("muestra el error de descarga de un modelo", async () => {
    const { controls } = setup();
    await goTo("Modelos");
    const section = await screen.findByRole("region", { name: "Modelos" });
    await within(section).findByRole("listitem", { name: "Detección de voz" });
    act(() => controls.emit("model:download", { name: "vad", fraction: 0.2, done: false, error: "sin red" }));
    expect(await within(section).findByRole("alert")).toHaveTextContent("sin red");
  });

  it("añade un proveedor compatible con OpenAI", async () => {
    setup();
    await goTo("Proveedores de IA");
    await userEvent.click(await screen.findByRole("button", { name: "Añadir Otro servidor compatible" }));
    expect(await screen.findByLabelText("URL base de OpenAI compatible")).toBeInTheDocument();
  });

  it("elige el proveedor por tarea", async () => {
    const { controls } = setup();
    await goTo("Proveedores de IA");
    const sel = await screen.findByLabelText("Proveedor para el acta");
    expect(sel).toHaveValue("claude-cli");
    await userEvent.selectOptions(sel, "");
    await userEvent.click(screen.getByRole("button", { name: "Guardar cambios" }));
    await waitFor(() => expect(controls.state.settings.llm.minutesProvider).toBeNull());
  });

  it("autoparada en minutos y horas", async () => {
    const { controls } = setup();
    await goTo("Grabación y autoparada");
    const silence = await screen.findByLabelText("Minutos de silencio antes de preguntar");
    expect(silence).toHaveValue(3);
    await userEvent.clear(silence);
    await userEvent.type(silence, "5");
    const max = screen.getByLabelText("Duración máxima (horas)");
    expect(max).toHaveValue(4);
    await userEvent.click(screen.getByRole("button", { name: "Guardar cambios" }));
    await waitFor(() => expect(controls.state.settings.autoStop.silencePromptAfterMs).toBe(300_000));
  });

  it("permisos: estado y acciones", async () => {
    const { api } = setup();
    await goTo("Permisos");
    const req = vi.spyOn(api, "permissionsRequest");
    const open = vi.spyOn(api, "openSystemSettings");
    const section = await screen.findByRole("region", { name: "Permisos" });
    const mic = within(section).getByRole("listitem", { name: "Micrófono" });
    expect(within(mic).getByText("Concedido")).toBeInTheDocument();
    const screenRec = within(section).getByRole("listitem", { name: "Grabación de pantalla" });
    await userEvent.click(within(screenRec).getByRole("button", { name: "Conceder" }));
    expect(req).toHaveBeenCalledWith("screenRecording");
    await userEvent.click(within(screenRec).getByRole("button", { name: "Abrir Ajustes del Sistema" }));
    expect(open).toHaveBeenCalledWith("screenRecording");
  });

  describe("permisos al día", () => {
    const row = async (name: string) => within(await screen.findByRole("region", { name: "Permisos" })).getByRole("listitem", { name });
    const grant = (controls: ReturnType<typeof createMockApi>["controls"], kind: string) => {
      controls.state.permissions = controls.state.permissions.map((p) => (p.kind === kind ? { ...p, status: "granted" } : p));
    };

    it("al volver a la ventana se vuelve a comprobar y la fila cambia", async () => {
      const { controls } = setup();
      await goTo("Permisos");
      expect(within(await row("Audio del sistema")).getByText("Sin comprobar")).toBeInTheDocument();
      grant(controls, "systemAudio");
      act(() => void window.dispatchEvent(new Event("focus")));
      await waitFor(async () => expect(within(await row("Audio del sistema")).getByText("Concedido")).toBeInTheDocument());
      expect(within(await row("Audio del sistema")).queryByRole("button", { name: "Abrir Ajustes del Sistema" })).not.toBeInTheDocument();
    });

    it("también al volver a ser visible (cambio de espacio o de app)", async () => {
      const { api, controls } = setup();
      await goTo("Permisos");
      await row("Accesibilidad");
      const status = vi.spyOn(api, "permissionsStatus");
      grant(controls, "accessibility");
      act(() => void document.dispatchEvent(new Event("visibilitychange")));
      await waitFor(async () => expect(within(await row("Accesibilidad")).getByText("Concedido")).toBeInTheDocument());
      expect(status).toHaveBeenCalled();
    });

    it("`permissions:changed` actualiza la fila en el acto", async () => {
      const { controls } = setup();
      await goTo("Permisos");
      await row("Grabación de pantalla");
      act(() => controls.emit("permissions:changed", { kind: "screenRecording", status: "granted" }));
      expect(within(await row("Grabación de pantalla")).getByText("Concedido")).toBeInTheDocument();
    });

    it("con la sección abierta sondea cada poco; fuera de ella, no", async () => {
      vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
      try {
        const { api, controls } = setup();
        const status = vi.spyOn(api, "permissionsStatus");
        await screen.findByRole("region", { name: "General" });
        act(() => void vi.advanceTimersByTime(6_500));
        expect(status).not.toHaveBeenCalled();
        await goTo("Permisos");
        grant(controls, "systemAudio");
        act(() => void vi.advanceTimersByTime(2_100));
        await waitFor(async () => expect(within(await row("Audio del sistema")).getByText("Concedido")).toBeInTheDocument());
      } finally {
        vi.useRealTimers();
      }
    });

    it("el aviso rojo de la lista desaparece cuando se concede lo que faltaba", async () => {
      const { controls } = setup();
      controls.state.permissions = controls.state.permissions.map((p) => (p.kind === "microphone" ? { ...p, status: "denied" } : p));
      act(() => void window.dispatchEvent(new Event("focus")));
      const nav = await screen.findByRole("navigation", { name: "Secciones de ajustes" });
      await waitFor(() => expect(nav.querySelector(".settings-nav-alert")).toBeInTheDocument());
      grant(controls, "microphone");
      act(() => void window.dispatchEvent(new Event("focus")));
      await waitFor(() => expect(nav.querySelector(".settings-nav-alert")).not.toBeInTheDocument());
    });
  });

  it("muestra el progreso de descarga de modelos", async () => {
    const { controls } = setup();
    await goTo("Modelos");
    const section = await screen.findByRole("region", { name: "Modelos" });
    await within(section).findByRole("listitem", { name: "Transcripción" });
    act(() => controls.emit("model:download", { name: "asr", fraction: 0.5, done: false, error: null }));
    expect(await screen.findByRole("progressbar", { name: "Descargando Transcripción" })).toHaveAttribute("aria-valuenow", "50");
    act(() => controls.emit("model:download", { name: "asr", fraction: 1, done: true, error: null }));
    await waitFor(() => expect(screen.queryByRole("progressbar")).not.toBeInTheDocument());
  });

  it("activar las capturas es opt-in explícito", async () => {
    const { controls } = setup();
    await goTo("Capturas");
    const toggle = await screen.findByRole("checkbox", { name: "Capturar la pantalla compartida" });
    expect(toggle).not.toBeChecked();
    await userEvent.click(toggle);
    await userEvent.click(screen.getByRole("button", { name: "Guardar cambios" }));
    await waitFor(() => expect(controls.state.settings.screen.enabled).toBe(true));
  });

  it("borra una huella de voz", async () => {
    const { controls } = setup({ voiceprints: [{ id: "v1", displayName: "Ana", samples: 3, meetings: 2, updatedAt: 0 }] });
    await goTo("General");
    await userEvent.click(await screen.findByRole("button", { name: "Olvidar la voz de Ana" }));
    await waitFor(() => expect(controls.state.voiceprints).toHaveLength(0));
  });

  it("si no se puede olvidar una voz, lo dice en vez de no hacer nada", async () => {
    const { api } = setup({ voiceprints: [{ id: "v1", displayName: "Ana", samples: 3, meetings: 2, updatedAt: 0 }] });
    vi.spyOn(api, "voiceprintDelete").mockRejectedValue({ code: "io", message: "base de datos ocupada" });
    await userEvent.click(await screen.findByRole("button", { name: "Olvidar la voz de Ana" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("No se ha podido olvidar esa voz: base de datos ocupada");
    expect(screen.getByRole("button", { name: "Olvidar la voz de Ana" })).toBeInTheDocument();
  });

  it("si pedir un permiso o abrir Ajustes del Sistema falla, lo dice", async () => {
    const { api } = setup();
    await goTo("Permisos");
    vi.spyOn(api, "permissionsRequest").mockRejectedValue({ code: "io", message: "el ayudante no responde" });
    const section = await screen.findByRole("region", { name: "Permisos" });
    await userEvent.click(within(within(section).getByRole("listitem", { name: "Grabación de pantalla" })).getByRole("button", { name: "Conceder" }));
    expect(await within(section).findByRole("alert")).toHaveTextContent("No se ha podido pedir el permiso: el ayudante no responde");
  });

  it("si no se puede saber qué modelos hay, lo dice y deja reintentar", async () => {
    const mock = createMockApi();
    vi.spyOn(mock.api, "modelsStatus").mockRejectedValueOnce({ code: "io", message: "sin carpeta de modelos" });
    render(
      <ApiProvider api={mock.api}>
        <SettingsContainer />
      </ApiProvider>,
    );
    await goTo("Modelos");
    const section = await screen.findByRole("region", { name: "Modelos" });
    expect(await within(section).findByRole("alert")).toHaveTextContent("sin carpeta de modelos");
    await userEvent.click(within(section).getByRole("button", { name: "Reintentar" }));
    expect(await within(section).findByRole("listitem", { name: "Transcripción" })).toBeInTheDocument();
    expect(within(section).queryByRole("alert")).not.toBeInTheDocument();
  });
});
