import { useEffect, useRef, useState } from "react";
import type { Citation, LiveSegment, LlmOverride, MeetingDetail, ProcessingStep, Settings, VoiceprintSummary } from "../../api";
import { useApi, useApiEvent } from "../../app/ApiContext";
import { ChevronLeft, FolderOpen, Pencil, Trash2, TriangleAlert, X } from "lucide-react";
import { formatDay, formatDuration, formatTime, platformLabel } from "../../shared/format";
import { StarButton } from "../../shared/ui/Star";
import { AvatarStack, Badge, EmptyState, IconButton, PlatformIcon, Tabs, errorMessage, type TabDef } from "../../shared/ui/ui";
import { CapturesContainer } from "../captures/CapturesContainer";
import { ChatContainer } from "../chat/ChatContainer";
import { GroupPicker } from "../groups/GroupPicker";
import { EscaletaTab } from "../escaleta/EscaletaTab";
import { MinutesTab } from "../minutes/MinutesTab";
import { AudioPlayer } from "../player/AudioPlayer";
import { PlayerProvider, usePlayer } from "../player/PlayerContext";
import { TimelineTab } from "../timeline/TimelineTab";
import { TranscriptTab } from "../transcript/TranscriptTab";
import { providerById, providerDisplay, providerModel } from "../chat/providerLabel";
import { LiveLines } from "../live/LiveLines";
import { MeetingNote } from "./MeetingNote";
import { MeetingFilesMenu } from "./MeetingFilesMenu";
import { ProcessingStatus } from "./ProcessingStatus";
import { RetryMenu } from "./RetryMenu";
import { useProcessingTimeline } from "./useProcessingTimeline";
import { useMeetingDetail } from "./useMeetingDetail";
import { DeleteMeetingDialog } from "./DeleteMeetingDialog";

const capitalizeFirst = (s: string): string => s.charAt(0).toUpperCase() + s.slice(1);

/** Con transcripción ya hecha, reintentar no la repite. */
const RESUME_STEPS: ProcessingStep[] = ["summarizing", "indexing", "pruningCaptures"];
/** «Regenerar»: solo el acta y su índice. */
const MINUTES_STEPS: ProcessingStep[] = ["summarizing", "indexing"];

type TabKey = "resumen" | "transcript" | "escaleta" | "timeline" | "capturas" | "chat";

interface Props {
  meetingId: string;
  initialSeekMs?: number | null;
  onBack(): void;
  /** Para citas de otras reuniones. */
  onOpenMeeting(meetingId: string, atMs: number): void;
}

export function MeetingDetailContainer(props: Props) {
  const data = useMeetingDetail(props.meetingId);
  const { detail, error } = data;
  if (error && !detail) {
    return (
      <div className="page">
        <EmptyState icon={<TriangleAlert size={26} strokeWidth={1.7} />} title="No se ha podido abrir la reunión" hint={error}>
          <IconButton label="Volver" variant="glass" onClick={props.onBack}>
            <ChevronLeft size={17} strokeWidth={2.2} aria-hidden />
          </IconButton>
        </EmptyState>
      </div>
    );
  }
  if (!detail) return <p className="loading">Cargando…</p>;
  return (
    <PlayerProvider key={detail.meeting.id} audio={detail.audio}>
      <Body {...props} detail={detail} voiceprints={data.voiceprints} reload={data.reload} />
    </PlayerProvider>
  );
}

interface BodyProps extends Props {
  detail: MeetingDetail;
  voiceprints: VoiceprintSummary[];
  reload(): Promise<void>;
}

function Body({ meetingId, initialSeekMs, onBack, onOpenMeeting, detail, voiceprints, reload }: BodyProps) {
  const api = useApi();
  const player = usePlayer();
  const [tab, setTabState] = useState<TabKey>("resumen");
  /** La persona ya eligió pestaña: no se la cambiamos. */
  const picked = useRef(false);
  const setTab = (t: TabKey) => {
    picked.current = true;
    setTabState(t);
  };
  const [settings, setSettings] = useState<Settings | null>(null);
  const [liveSegments, setLiveSegments] = useState<LiveSegment[]>([]);
  const timeline = useProcessingTimeline(meetingId);
  const [editing, setEditing] = useState(false);
  const [title, setTitle] = useState("");
  const [actionError, setActionError] = useState<string | null>(null);
  const [confirmingDelete, setConfirmingDelete] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const { seek } = player;

  useEffect(() => {
    if (initialSeekMs != null) seek(initialSeekMs);
    // Solo al abrir.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [meetingId]);

  useEffect(() => {
    let live = true;
    void api.settingsGet().then((s) => live && setSettings(s));
    return () => {
      live = false;
    };
  }, [api]);

  // Sin transcripción definitiva todavía: si hubo directo, se enseña ya lo que se tomó.
  const provisional = detail.meeting.status !== "ready" && detail.transcript.utterances.length === 0;
  useEffect(() => {
    if (!provisional) {
      setLiveSegments([]);
      return;
    }
    let live = true;
    api.liveTranscriptGet(meetingId).then(
      (t) => {
        if (!live) return;
        setLiveSegments([...t.segments]);
        if (t.segments.length > 0 && !picked.current) setTabState("transcript");
      },
      () => {},
    );
    return () => {
      live = false;
    };
  }, [api, meetingId, provisional]);

  useApiEvent("meeting:updated", (u) => u.meetingId === meetingId && void reload());
  useApiEvent("capture:saved", (c) => c.meetingId === meetingId && void reload());

  const { meeting } = detail;
  const run = async (f: () => Promise<unknown>) => {
    try {
      setActionError(null);
      await f();
      await reload();
    } catch (e) {
      setActionError(errorMessage(e));
    }
  };

  const tabs: TabDef<TabKey>[] = [
    { key: "resumen", label: "Resumen" },
    { key: "transcript", label: "Transcript" },
    { key: "escaleta", label: "Escaleta" },
    { key: "timeline", label: "Timeline" },
    { key: "capturas", label: "Capturas", count: detail.captures.length },
    { key: "chat", label: "Chat" },
  ];

  const hasTranscript = detail.transcript.utterances.length > 0;
  const retry = (steps: ProcessingStep[] | null, llm: LlmOverride | null) => void run(() => api.meetingRetry({ meetingId: meeting.id, steps, providerId: llm?.providerId ?? null, model: llm?.model ?? null }));
  const defaultProvider = settings ? providerById(settings, settings.llm.minutesProvider) : null;
  const minutesLlm = defaultProvider ? { provider: defaultProvider.label, model: providerModel(defaultProvider) } : undefined;
  const author = settings && detail.minutes ? providerById(settings, detail.minutes.provider) : null;
  const generatedBy = detail.minutes ? (author ? providerDisplay(author, detail.minutes.model) : null) : null;

  const onCitation = (c: Citation) => (c.meetingId === meetingId ? player.seek(c.at) : onOpenMeeting(c.meetingId, c.at));

  const people = detail.transcript.speakers.map((s) => s.displayName ?? s.label);
  const activity = detail.transcript.utterances.map((u) => ({ start: u.start, end: u.end }));

  return (
    <div className="detail-wrap">
      <article className="page detail">
        <header className="detail-head">
          <div className="detail-bar">
            <IconButton label="Volver" variant="glass" onClick={onBack}>
              <ChevronLeft size={17} strokeWidth={2.2} aria-hidden />
            </IconButton>
            <span className="spacer" />
            <GroupPicker
              memberOf={detail.groups.map((g) => g.id)}
              onAdd={(ids) =>
                run(async () => {
                  for (const id of ids) await api.groupAddMeeting(id, meeting.id);
                })
              }
            />
            <MeetingFilesMenu
              meetingId={meeting.id}
              hasTranscript={detail.transcript.utterances.length > 0}
              hasAudio={!!detail.audio.mix}
              onResult={(r) => {
                setActionError("error" in r ? r.error : null);
                setNotice("ok" in r && r.ok ? r.ok : null);
              }}
            />
            <StarButton className="star-glass" on={meeting.favourite} name={meeting.title} size={17} onToggle={(f) => void run(() => api.meetingSetFavourite(meeting.id, f))} />
            <IconButton
              label="Eliminar reunión"
              variant="glass"
              onClick={() => setConfirmingDelete(true)}
            >
              <Trash2 size={15} strokeWidth={1.9} aria-hidden />
            </IconButton>
          </div>
          <div className="detail-hero">
            <PlatformIcon platform={meeting.platform} size="lg" />
            <div className="detail-hero-text">
              <div className="detail-title">
                {editing ? (
                  <form
                    onSubmit={(e) => {
                      e.preventDefault();
                      const t = title.trim();
                      setEditing(false);
                      if (t && t !== meeting.title) void run(() => api.meetingUpdate(meeting.id, t));
                    }}
                  >
                    <input aria-label="Título" autoFocus value={title} onChange={(e) => setTitle(e.target.value)} onBlur={() => setEditing(false)} />
                  </form>
                ) : (
                  <>
                    <h1 className="selectable">{meeting.title}</h1>
                    <IconButton
                      label="Editar título"
                      small
                      onClick={() => {
                        setTitle(meeting.title);
                        setEditing(true);
                      }}
                    >
                      <Pencil size={13} strokeWidth={2} aria-hidden />
                    </IconButton>
                  </>
                )}
              </div>
              <p className="detail-meta">
                <span>{capitalizeFirst(formatDay(meeting.startedAt))}</span>
                <span>{formatTime(meeting.startedAt)}</span>
                <span>{formatDuration(meeting.duration)}</span>
                <span>{platformLabel(meeting.platform)}</span>
                {meeting.recovered ? <span>Recuperada tras un cierre inesperado</span> : null}
              </p>
            </div>
          </div>
          {people.length > 0 || detail.groups.length > 0 ? (
            <div className="detail-people">
              {people.length > 0 ? (
                <span className="people">
                  <AvatarStack names={people} max={6} />
                  <span className="people-names">{people.join(", ")}</span>
                </span>
              ) : null}
              {detail.groups.map((g) => (
                <Badge key={g.id} tone="accent" icon={<FolderOpen size={11} strokeWidth={2.2} aria-hidden />}>
                  {g.name}
                  <button type="button" className="badge-x" aria-label={`Quitar de ${g.name}`} onClick={() => void run(() => api.groupRemoveMeeting(g.id, meeting.id))}>
                    <X size={10} strokeWidth={2.6} aria-hidden />
                  </button>
                </Badge>
              ))}
            </div>
          ) : null}
          <ProcessingStatus
            status={meeting.status}
            step={timeline?.step}
            fraction={timeline?.fraction}
            timings={timeline?.timings}
            error={meeting.error}
            llm={minutesLlm}
            loadProviders={() => api.llmProvidersAvailable()}
            onRetry={(llm) => retry(hasTranscript ? RESUME_STEPS : null, llm)}
          />
          <MeetingNote key={meeting.id} saved={meeting.userNote} onSave={(note) => void run(() => api.meetingSetNote(meeting.id, note))} />
          {actionError ? <p role="alert" className="inline-error">{actionError}</p> : null}
          {notice ? <p role="status" className="muted">{notice}</p> : null}
        </header>
        <div className="sticky-bar sticky-bar-center">
          <div className="tabs-capsule glass">
            <Tabs tabs={tabs} active={tab} onChange={setTab} label="Secciones de la reunión" />
          </div>
        </div>
        <div className="page-body tab-body" role="tabpanel">
          {tab === "resumen" ? (
            <MinutesTab
              minutes={detail.minutes}
              pending={meeting.status === "processing"}
              generatedBy={generatedBy}
              regenerate={detail.minutes && meeting.status === "ready" ? <RetryMenu quiet label="Regenerar el acta" menuLabel="Regenerar con…" loadProviders={() => api.llmProvidersAvailable()} onRetry={(llm) => retry(MINUTES_STEPS, llm)} /> : null}
              onReadTranscript={hasTranscript || liveSegments.length > 0 ? () => setTab("transcript") : undefined}
              onSeek={player.seek}
              onToggleTask={(id, done) => void run(() => api.actionItemSetDone(id, done))}
            />
          ) : null}
          {tab === "transcript" && provisional && liveSegments.length > 0 ? (
            <div className="transcript">
              <p className="provisional-note">Transcripción provisional, tomada en directo. La definitiva, con nombres y mejor puntuación, llega cuando termine el procesado.</p>
              <div className="card lines-card">
                <LiveLines segments={liveSegments} />
              </div>
            </div>
          ) : tab === "transcript" ? (
            <TranscriptTab
              transcript={detail.transcript}
              voiceprintNames={voiceprints.map((v) => v.displayName)}
              currentMs={player.currentMs}
              onSeek={player.seek}
              onRename={(sid, name, remember) => void run(() => api.speakerRename(meeting.id, sid, name, remember))}
              onSearch={(q) => api.meetingSearchTranscript(meeting.id, q).catch(() => null)}
            />
          ) : null}
          {tab === "escaleta" ? <EscaletaTab escaleta={detail.escaleta} title={meeting.title} onSeek={player.seek} /> : null}
          {tab === "timeline" ? <TimelineTab events={detail.timeline} duration={meeting.duration} currentMs={player.hasAudio ? player.currentMs : undefined} decisions={detail.minutes?.decisions} actionItems={detail.minutes?.actionItems} onSeek={player.seek} /> : null}
          {tab === "capturas" ? <CapturesContainer detail={detail} onSeek={player.seek} onChanged={() => void reload()} /> : null}
          {tab === "chat" ? (
            <ChatContainer
              scope={{ kind: "meeting", meetingId: meeting.id }}
              placeholder="esta reunión"
              suggestions={["¿Qué tareas tengo?", "¿Qué se decidió?", "Resume los desacuerdos", "¿Quién se comprometió a qué?"]}
              onCitation={onCitation}
            />
          ) : null}
        </div>
      </article>
      <div className="player-dock">
        <AudioPlayer activity={activity} durationMs={meeting.duration} />
      </div>
      {confirmingDelete ? (
        <DeleteMeetingDialog title={meeting.title} onClose={() => setConfirmingDelete(false)} onDelete={() => api.meetingDelete(meeting.id)} onDeleted={onBack} />
      ) : null}
    </div>
  );
}
