import { describe, expect, it } from "vitest";
import { COMMANDS, EVENTS } from "./contract";
import { createMockApi } from "./mock";

describe("mock api", () => {
  it("records a manual session and stops it", async () => {
    const { api } = createMockApi();
    const seen: string[] = [];
    await api.on("session:state", (s) => seen.push(s.state.state));

    expect((await api.sessionGetState()).state.state).toBe("idle");
    await api.sessionStart();
    await api.sessionPause();
    await api.sessionResume();
    const stopped = await api.sessionStop();

    expect(stopped.state.state).toBe("stopping");
    expect(seen).toEqual(["recording", "paused", "recording", "stopping"]);
  });

  it("rejects pausing when idle with an ApiError", async () => {
    const { api } = createMockApi();
    await expect(api.sessionPause()).rejects.toMatchObject({ code: "invalidState" });
  });

  it("stores a secret as a keychain ref and reports models", async () => {
    const { api } = createMockApi();
    expect(await api.secretSet("openrouter", "sk-x")).toEqual({ service: "acta", account: "provider:openrouter" });
    await api.secretDelete("openrouter");
    const status = await api.modelsStatus();
    expect(status.map((m) => m.name)).toEqual(["asr", "diarizer", "vad"]);
  });

  it("emits download progress then done and marks the model downloaded", async () => {
    const { api } = createMockApi();
    const seen: number[] = [];
    let done = false;
    await api.on("model:download", (d) => {
      seen.push(d.fraction);
      done = d.done;
    });
    await api.modelsDownload("vad");
    expect(seen.length).toBeGreaterThanOrEqual(3);
    expect(done).toBe(true);
    expect((await api.modelsStatus()).find((m) => m.name === "vad")?.downloaded).toBe(true);
  });

  it("has Rust-equal defaults for effort and denylist", async () => {
    const { api } = createMockApi();
    const s = await api.settingsGet();
    expect(s.detection.denylist).toEqual([]);
    const k = s.llm.providers[0]!.kind;
    expect(k.type === "claudeCli" && k.effort).toBeNull();
  });

  it("starts with the onboarding done and no local model server running", async () => {
    const { api } = createMockApi();
    expect((await api.settingsGet()).onboarding).toEqual({ completed: true });
    const local = await api.llmDetectLocal();
    expect(local.ollama).toEqual({ running: false, baseUrl: "http://127.0.0.1:11434/v1", models: [] });
    expect(local.lmStudio).toEqual({ running: false, baseUrl: "http://127.0.0.1:1234/v1", models: [] });
  });

  it("lists the microphones with exactly one default (audio_inputs_list)", async () => {
    const { api } = createMockApi();
    const inputs = await api.audioInputsList();
    expect(COMMANDS.audioInputsList).toBe("audio_inputs_list");
    expect(inputs.filter((i) => i.isDefault)).toHaveLength(1);
    expect(inputs[0]).toEqual({ uid: expect.any(String), name: expect.any(String), isDefault: expect.any(Boolean), transport: expect.any(String) });
  });

  it("lists the models of a server by its base url and rejects an unknown one", async () => {
    const { api } = createMockApi({ remoteModels: { "https://api.nan.builders/v1": ["gemma4", "qwen3.6"] } });
    expect(await api.llmListModels("https://api.nan.builders/v1/", "nan")).toEqual(["gemma4", "qwen3.6"]);
    await expect(api.llmListModels("http://127.0.0.1:9/v1")).rejects.toMatchObject({ code: "unavailable" });
  });

  it("lists OpenCode models via llm_list_opencode_models and rejects an empty path", async () => {
    const { api } = createMockApi({ opencodeModels: ["provider/model-a", "provider/model-b"] });
    expect(COMMANDS.llmListOpencodeModels).toBe("llm_list_opencode_models");
    expect(await api.llmListOpencodeModels("/x/opencode", "opencode-cli")).toEqual(["provider/model-a", "provider/model-b"]);
    await expect(api.llmListOpencodeModels("   ")).rejects.toMatchObject({ code: "invalidInput" });
  });

  it("remembers the links it was asked to open in the browser", async () => {
    const { api, controls } = createMockApi();
    await api.openExternalUrl("https://cloud.nan.builders/r/YZ524NPK");
    expect(controls.state.opened).toEqual(["https://cloud.nan.builders/r/YZ524NPK"]);
  });

  it("lists the sample meeting with my open tasks", async () => {
    const { api } = createMockApi();
    const page = await api.meetingsList({});
    expect(page.items).toHaveLength(1);
    expect(page.items[0]?.myOpenTasks).toBe(1);
  });
});

describe("contract names", () => {
  it("uses snake_case commands and namespaced events like acta-core", () => {
    for (const name of Object.values(COMMANDS)) expect(name).toMatch(/^[a-z]+(_[a-z]+)*$/);
    for (const name of Object.values(EVENTS)) expect(name).toMatch(/^[a-z]+:[a-z-]+$/);
  });
});

describe("personal note (mock)", () => {
  it("saves, trims and clears the note", async () => {
    const { api, controls } = createMockApi();
    const id = controls.state.meetings[0]!.meeting.id;
    expect((await api.meetingSetNote(id, "  Pedir descuento  ")).userNote).toBe("Pedir descuento");
    expect((await api.meetingGet(id)).meeting.userNote).toBe("Pedir descuento");
    expect((await api.meetingSetNote(id, "  ")).userNote).toBeNull();
  });
  it("is in the command contract", () => {
    expect(COMMANDS.meetingSetNote).toBe("meeting_set_note");
  });
});

describe("favourites, cleanup and the built-in local model (mock)", () => {
  it("stars a meeting and a group and filters by favourites", async () => {
    const { api, controls } = createMockApi();
    const id = controls.state.meetings[0]!.meeting.id;
    expect((await api.meetingsList({})).items[0]!.favourite).toBe(false);
    expect((await api.meetingsList({ favouritesOnly: true })).items).toHaveLength(0);
    expect((await api.meetingSetFavourite(id, true)).favourite).toBe(true);
    expect((await api.meetingsList({ favouritesOnly: true })).items.map((m) => m.id)).toEqual([id]);
    await api.meetingSetFavourite(id, false);

    // Una reunión de un grupo con estrella también cuenta como favorita.
    const g = await api.groupCreate("Proyecto");
    await api.groupAddMeeting(g.id, id);
    expect((await api.groupSetFavourite(g.id, true)).favourite).toBe(true);
    expect((await api.meetingsList({ favouritesOnly: true })).items).toHaveLength(1);
    expect(await api.search({ text: "presupuesto", scope: { kind: "favourites" }, limit: 5 })).toHaveLength(1);
    await api.groupSetFavourite(g.id, false);
    expect(await api.search({ text: "presupuesto", scope: { kind: "favourites" }, limit: 5 })).toHaveLength(0);
  });

  it("suggests old meetings, never favourites, and deletes or keeps the chosen ones", async () => {
    const { api, controls } = createMockApi();
    const id = controls.state.meetings[0]!.meeting.id;
    controls.state.cleanup = [
      { meetingId: id, title: "Daily del equipo", startedAt: 1, duration: 60_000, reasons: [{ kind: "old", days: 120 }, { kind: "neverUsed" }], bytes: 5_000 },
    ];
    const s = await api.cleanupSuggestions();
    expect(s.candidates).toHaveLength(1);
    expect(s).toMatchObject({ totalBytes: 5_000, minAgeDays: 90 });

    await api.meetingSetFavourite(id, true);
    expect((await api.cleanupSuggestions()).candidates).toHaveLength(0);
    expect(await api.cleanupDelete([id])).toEqual({ deleted: [], skipped: [id], freedBytes: 0 });
    await api.meetingSetFavourite(id, false);

    await api.cleanupKeep([id]);
    expect((await api.cleanupSuggestions()).candidates).toHaveLength(0);
    controls.state.kept = [];
    expect(await api.cleanupDelete([id])).toEqual({ deleted: [id], skipped: [], freedBytes: 5_000 });
    expect(controls.state.meetings).toHaveLength(0);
  });

  it("turning suggestions off empties them", async () => {
    const { api, controls } = createMockApi();
    const id = controls.state.meetings[0]!.meeting.id;
    controls.state.cleanup = [{ meetingId: id, title: "x", startedAt: 1, duration: 1, reasons: [], bytes: 1 }];
    const settings = await api.settingsGet();
    expect(settings.cleanup).toEqual({ enabled: true, minAgeDays: 90, snoozedUntil: null });
    await api.settingsUpdate({ ...settings, cleanup: { ...settings.cleanup, enabled: false } });
    expect((await api.cleanupSuggestions()).candidates).toHaveLength(0);
  });

  it("offers three local tiers, recommends one for this Mac and downloads with stages", async () => {
    const { api } = createMockApi();
    const status = await api.localLlmStatus();
    expect(status.models.map((m) => m.model.tier)).toEqual(["tiny", "medium", "large"]);
    expect(status.models.filter((m) => m.recommended)).toHaveLength(1);
    expect(status.recommended).toBe(status.models.find((m) => m.recommended)!.model.id);
    expect(status.runtime.available).toBe(false);

    const stages: string[] = [];
    await api.on("localmodel:download", (e) => {
      if (stages[stages.length - 1] !== e.stage) stages.push(e.stage);
    });
    const tiny = status.models[0]!.model.id;
    await api.localLlmDownload(tiny);
    expect(stages).toEqual(["runtime", "model", "verifying", "done"]);
    const after = await api.localLlmStatus();
    expect(after.models[0]!.downloaded).toBe(true);
    expect(after.runtime.available).toBe(true);
    expect((await api.localLlmTest(tiny)).ok).toBe(true);

    await api.localLlmDelete(tiny);
    expect((await api.localLlmStatus()).models[0]!.downloaded).toBe(false);
    expect((await api.localLlmTest(tiny)).ok).toBe(false);
  });

  it("refuses a model that does not fit this Mac", async () => {
    const { api, controls } = createMockApi();
    controls.state.localLlm.models[2]!.fit = "tooBig";
    await expect(api.localLlmDownload(controls.state.localLlm.models[2]!.model.id)).rejects.toMatchObject({ code: "invalidInput" });
  });
});
