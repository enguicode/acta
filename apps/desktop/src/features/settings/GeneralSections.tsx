import { useEffect, useState } from "react";
import type { AppRule, InstalledApp, Platform, Settings, VoiceprintSummary } from "../../api";
import { appDisplayName, platformLabel } from "../../shared/format";
import { AppListEditor, type AppsSource, type PickedApp } from "../../shared/ui/AppListEditor";
import { ChipsInput } from "../../shared/ui/ChipsInput";
import { Avatar, Button, Disclosure, PlatformIcon, Segmented, Select } from "../../shared/ui/ui";
import { MEETING_PRESETS, PLATFORM_OPTIONS, guessRule } from "./appRules";
import { dictationApps } from "./dictationApps";
import { Field, Group, Row, Section, StepperRow, ToggleRow } from "./fields";
import { languageOptions } from "./languages";

interface SectionProps<K extends keyof Settings> {
  value: Settings[K];
  onChange(v: Settings[K]): void;
}

const MIN = 60_000;
const HOUR = 3_600_000;

const CLEANUP_AGES: { days: number; label: string }[] = [
  { days: 30, label: "1 mes" },
  { days: 60, label: "2 meses" },
  { days: 90, label: "3 meses" },
  { days: 180, label: "6 meses" },
  { days: 365, label: "1 año" },
];

interface GeneralProps {
  user: Settings["user"];
  cleanup: Settings["cleanup"];
  onCleanup(v: Settings["cleanup"]): void;
  transcription: Settings["transcription"];
  titles?: Settings["titles"];
  onTitles?(v: Settings["titles"]): void;
  onUser(v: Settings["user"]): void;
  onTranscription(v: Settings["transcription"]): void;
  voiceprints: VoiceprintSummary[];
  /** No se pudieron leer u olvidar las voces: se enseña en su grupo. */
  voiceprintsError?: string | null;
  onRetryVoiceprints?: () => void;
  onForget(id: string): void;
  onReplayOnboarding?: () => void;
}

export function GeneralSection({ user, cleanup, onCleanup, transcription, titles, onTitles, onUser, onTranscription, voiceprints, voiceprintsError, onRetryVoiceprints, onForget, onReplayOnboarding }: GeneralProps) {
  const ages = CLEANUP_AGES.some((a) => a.days === cleanup.minAgeDays) ? CLEANUP_AGES : [...CLEANUP_AGES, { days: cleanup.minAgeDays, label: `${cleanup.minAgeDays} días` }].sort((a, b) => a.days - b.days);
  return (
    <Section title="General" hint="Todo se procesa en tu Mac. Lo único que sale es el texto que le mandas a tu proveedor de IA para escribir el acta.">
      <Group title="Tú" foot="acta usa estos nombres para saber qué tareas y menciones son tuyas. No salen de tu Mac, salvo dentro del texto de la reunión que se envía al proveedor de IA que hayas elegido.">
        <Row label="Tu nombre" hint={user.displayName.trim() ? undefined : "Sin tu nombre, acta no puede saber qué tareas son tuyas."}>
          <input aria-label="Tu nombre" placeholder="Nombre y apellido" autoComplete="off" value={user.displayName} onChange={(e) => onUser({ ...user, displayName: e.target.value })} />
        </Row>
        <div className="row row-stack">
          <span className="row-text">
            <span className="row-label">También te llaman</span>
            <span className="row-hint">Apodos o formas cortas con las que te nombran en las reuniones. Intro o coma para añadir.</span>
          </span>
          <ChipsInput label="Otros nombres" placeholder="Álex, Alejandra" values={user.aliases} onChange={(aliases) => onUser({ ...user, aliases })} />
        </div>
      </Group>

      <Group title="Transcripción">
        <Row label="Idioma de las reuniones" hint="Dirige la transcripción y el idioma del acta. La interfaz de acta está, por ahora, en español.">
          <Select label="Idioma de las reuniones" value={transcription.language} onChange={(e) => onTranscription({ ...transcription, language: e.target.value })}>
            {languageOptions(transcription.language).map((l) => (
              <option key={l.value} value={l.value}>
                {l.label}
              </option>
            ))}
          </Select>
        </Row>
        <ToggleRow label="Separar hablantes" hint="Distingue quién habla entre los demás participantes." checked={transcription.diarization} onChange={(diarization) => onTranscription({ ...transcription, diarization })} />
        <ToggleRow label="Transcripción en vivo" hint="Vista previa mientras grabas y detección de momentos importantes." checked={transcription.livePreview} onChange={(livePreview) => onTranscription({ ...transcription, livePreview })} />
      </Group>

      {titles && onTitles ? (
        <Group title="Títulos de las reuniones" foot="acta usa el nombre de la reunión que muestra Teams. Si no lo hay, puede buscarlo en tu Calendario; si tampoco, lo saca de lo que se habló. Un título que cambies tú no se toca nunca.">
          <ToggleRow
            label="Usar el calendario para titular reuniones"
            hint="Busca en el Calendario de macOS (donde suele estar Outlook) el evento a la hora de la reunión. La primera vez pide permiso."
            checked={titles.calendar}
            onChange={(calendar) => onTitles({ ...titles, calendar })}
          />
        </Group>
      ) : null}

      <Group title="Voces recordadas" foot="Cuando confirmas un nombre en un transcript, acta recuerda esa voz para la próxima reunión.">
        {voiceprintsError ? (
          <Row
            label={
              <span role="alert" className="error">
                {voiceprintsError}
              </span>
            }
          >
            {onRetryVoiceprints ? (
              <Button small capsule onClick={onRetryVoiceprints}>
                Reintentar
              </Button>
            ) : null}
          </Row>
        ) : null}
        {voiceprints.length === 0 && !voiceprintsError ? <Row label={<span className="muted">Todavía no recuerdo ninguna voz</span>} /> : null}
        {voiceprints.map((v) => (
          <Row key={v.id} icon={<Avatar name={v.displayName} />} label={v.displayName} hint={v.meetings === 1 ? "1 reunión" : `${v.meetings} reuniones`}>
            <Button small aria-label={`Olvidar la voz de ${v.displayName}`} onClick={() => onForget(v.id)}>
              Olvidar
            </Button>
          </Row>
        ))}
      </Group>

      <Group title="Limpieza" foot="acta solo sugiere: nunca borra nada por su cuenta. Las favoritas, las de un grupo favorito y las que tienen tareas tuyas pendientes no se sugieren nunca.">
        <ToggleRow
          label="Sugerir borrar reuniones antiguas"
          hint="Un aviso discreto en la lista cuando hay reuniones viejas que no has vuelto a abrir, buscar ni usar en el chat."
          checked={cleanup.enabled}
          onChange={(enabled) => onCleanup({ ...cleanup, enabled, snoozedUntil: enabled ? cleanup.snoozedUntil : null })}
        />
        {cleanup.enabled ? (
          <Row label="Considerar antigua una reunión con más de">
            <Select label="Antigüedad para sugerir" value={String(cleanup.minAgeDays)} onChange={(e) => onCleanup({ ...cleanup, minAgeDays: Number(e.target.value) })}>
              {ages.map((a) => (
                <option key={a.days} value={a.days}>
                  {a.label}
                </option>
              ))}
            </Select>
          </Row>
        ) : null}
      </Group>

      {onReplayOnboarding ? (
        <Group title="Primeros pasos">
          <Row label="Bienvenida" hint="El recorrido inicial: permisos, modelos de voz, proveedor de IA y detección.">
            <Button capsule onClick={onReplayOnboarding}>
              Volver a ver la bienvenida
            </Button>
          </Row>
        </Group>
      ) : null}

      <Disclosure label="Avanzado">
        <Group>
          <StepperRow
            label="Similitud mínima de voz (%)"
            title="Similitud mínima para reconocer una voz"
            hint="Más alto: menos confusiones, pero reconoce a menos gente."
            unit="%"
            max={100}
            value={transcription.voiceprintMinSimilarityPct}
            onChange={(n) => onTranscription({ ...transcription, voiceprintMinSimilarityPct: n })}
          />
        </Group>
      </Disclosure>
    </Section>
  );
}

interface RecordingProps {
  autoStop: Settings["autoStop"];
  recording: Settings["recording"];
  onAutoStop(v: Settings["autoStop"]): void;
  onRecording(v: Settings["recording"]): void;
}

export function RecordingSection({ autoStop, recording, onAutoStop, onRecording }: RecordingProps) {
  return (
    <Section title="Grabación y autoparada" hint="Evita grabaciones eternas cuando la llamada acaba y nadie pulsa parar.">
      <Group title="Parada automática" foot="Si hay silencio, acta pregunta en la pastilla. Si nadie responde, para sola y guarda la reunión.">
        <StepperRow
          label="Minutos de silencio antes de preguntar"
          title="Preguntar tras un silencio de"
          unit="min"
          min={1}
          value={autoStop.silencePromptAfterMs / MIN}
          onChange={(n) => onAutoStop({ ...autoStop, silencePromptAfterMs: Math.round(n * MIN) })}
        />
        <StepperRow
          label="Minutos sin respuesta antes de parar"
          title="Parar si nadie responde en"
          unit="min"
          min={1}
          value={autoStop.autoStopAfterUnansweredMs / MIN}
          onChange={(n) => onAutoStop({ ...autoStop, autoStopAfterUnansweredMs: Math.round(n * MIN) })}
        />
        <StepperRow label="Duración máxima (horas)" title="Duración máxima de una grabación" unit="h" step={0.5} min={0.5} value={autoStop.maxDurationMs / HOUR} onChange={(n) => onAutoStop({ ...autoStop, maxDurationMs: Math.round(n * HOUR) })} />
      </Group>

      <Disclosure label="Avanzado">
        <Group>
          <StepperRow
            label="Umbral de silencio (dBFS)"
            title="Umbral de silencio"
            hint="Por debajo de este nivel se considera que nadie habla."
            unit="dBFS"
            min={-90}
            max={0}
            value={autoStop.silenceThresholdDbfs}
            onChange={(n) => onAutoStop({ ...autoStop, silenceThresholdDbfs: n })}
          />
          <ToggleRow
            label="Borrar los fragmentos al terminar"
            hint="El audio se graba en trozos de 30 s por seguridad. Al acabar se unen; actívalo para no guardar también los trozos."
            checked={recording.deleteSegmentsAfterConcat}
            onChange={(deleteSegmentsAfterConcat) => onRecording({ ...recording, deleteSegmentsAfterConcat })}
          />
        </Group>
      </Disclosure>
    </Section>
  );
}

/** Tras elegir una app de reunión: qué tipo de llamada es y si es un navegador. Parte de lo más probable. */
function RuleStep({ app, onAdd, onCancel }: { app: PickedApp; onAdd(rule: AppRule): void; onCancel(): void }) {
  const [rule, setRule] = useState<AppRule>(() => guessRule(app.bundleId, app.name));
  useEffect(() => setRule(guessRule(app.bundleId, app.name)), [app.bundleId, app.name]);
  return (
    <div className="rule-step" role="group" aria-label={`Tipo de reunión de ${app.name}`}>
      <p className="rule-step-title">
        ¿Qué reuniones haces con <strong>{app.name}</strong>?
      </p>
      <Segmented small label="Tipo de reunión" options={PLATFORM_OPTIONS} value={rule.platform} onChange={(platform: Platform) => setRule({ ...rule, platform })} />
      <label className="check rule-step-browser">
        <input
          type="checkbox"
          className="check-box"
          aria-label="Es un navegador"
          checked={rule.isBrowser}
          // En un navegador solo cuenta una pestaña de Google Meet.
          onChange={(e) => setRule({ ...rule, isBrowser: e.target.checked, platform: e.target.checked ? "meet" : rule.platform })}
        />
        Es un navegador: solo cuenta cuando la pestaña es de Google Meet
      </label>
      <div className="rule-step-actions">
        <Button small capsule variant="ghost" onClick={onCancel}>
          Cancelar
        </Button>
        <Button small capsule variant="primary" autoFocus onClick={() => onAdd(rule)}>
          Añadir a la lista
        </Button>
      </div>
    </div>
  );
}

interface DetectionProps extends SectionProps<"detection"> {
  /** Las apps de este Mac, para elegirlas por su nombre e icono. */
  apps: AppsSource;
}

export function DetectionSection({ value, onChange, apps }: DetectionProps) {
  const [pending, setPending] = useState<PickedApp | null>(null);
  const has = (id: string) => value.rules.some((r) => r.bundleId.toLowerCase() === id.toLowerCase());
  const addRule = (rule: AppRule) => {
    setPending(null);
    if (!has(rule.bundleId)) onChange({ ...value, rules: [...value.rules, rule] });
  };
  const installed = (id: string): InstalledApp | undefined => apps.apps?.find((a) => a.bundleId.toLowerCase() === id.toLowerCase());
  // Las habituales, con un toque: solo las instaladas (si no se sabe qué hay instalado, todas).
  const presets = MEETING_PRESETS.filter((p) => !has(p.bundleId) && p.bundleId !== pending?.bundleId && (!apps.apps || installed(p.bundleId)));
  const presetApps: InstalledApp[] = presets.map((p) => installed(p.bundleId) ?? { bundleId: p.bundleId, name: appDisplayName(p.bundleId), path: "", running: false, iconPath: null });
  const dictation = dictationApps(apps.apps ?? [], value.denylist);

  return (
    <Section
      title="Detección"
      hint={
        value.autoRecord
          ? "Cuando una de estas apps entra en una llamada, acta empieza a grabar sola; si la paras durante los primeros segundos, la descarta."
          : "Cuando una de estas apps entra en una llamada, acta te pregunta si quieres grabarla. Nunca graba sin preguntar."
      }
    >
      <Group>
        <ToggleRow label="Detectar reuniones automáticamente" hint="Si lo apagas, solo se graba cuando pulses «Grabar ahora»." checked={value.enabled} onChange={(enabled) => onChange({ ...value, enabled })} />
        {value.enabled ? (
          <ToggleRow
            label="Grabar automáticamente al detectar una llamada"
            hint="Empieza a grabar nada más detectar la llamada; si la paras durante la ventana de descarte, borra la reunión y su audio."
            checked={value.autoRecord}
            onChange={(autoRecord) => onChange({ ...value, autoRecord })}
          />
        ) : null}
        {value.enabled && value.autoRecord ? (
          <StepperRow
            label="Segundos para poder descartar"
            title="Ventana de descarte al iniciar"
            hint="Si paras la grabación antes de ese tiempo, se descarta todo. 0 s guarda siempre."
            unit="s"
            min={0}
            max={120}
            step={5}
            value={value.autoRecordDelayS}
            onChange={(autoRecordDelayS) => onChange({ ...value, autoRecordDelayS })}
          />
        ) : null}
      </Group>

      <AppListEditor
        title="Apps de reunión"
        foot="Solo estas apps pueden disparar la pregunta. En un navegador, solo cuenta si la pestaña es de Google Meet."
        emptyText="Ninguna app: la detección no saltará nunca"
        entries={value.rules.map((r) => ({ bundleId: r.bundleId, detail: `${platformLabel(r.platform)}${r.isBrowser ? " · en el navegador" : ""}`, fallbackIcon: <PlatformIcon platform={r.platform} size="sm" /> }))}
        source={apps}
        removeLabel={(name) => `Quitar ${name}`}
        suggestions={presetApps}
        suggestLabel={(name) => `Añadir ${name}`}
        // Elegida a mano se pregunta qué es; las habituales ya se sabe.
        onPick={setPending}
        onSuggestion={(app) => addRule(MEETING_PRESETS.find((p) => p.bundleId === app.bundleId) ?? guessRule(app.bundleId, app.name))}
        onRemove={(id) => onChange({ ...value, rules: value.rules.filter((x) => x.bundleId !== id) })}
      >
        {pending ? <RuleStep app={pending} onAdd={addRule} onCancel={() => setPending(null)} /> : null}
      </AppListEditor>

      <AppListEditor
        title="Apps ignoradas"
        foot="Apps que usan el micrófono pero no son reuniones, como las de dictado. acta nunca preguntará por ellas."
        emptyText="Ninguna"
        entries={value.denylist.map((bundleId) => ({ bundleId }))}
        source={apps}
        addLabel="Añadir app"
        removeLabel={(name) => `Dejar de ignorar ${name}`}
        suggestions={dictation}
        suggestLabel={(name) => `Ignorar ${name}`}
        onPick={(app) => {
          if (!value.denylist.some((id) => id.toLowerCase() === app.bundleId.toLowerCase())) onChange({ ...value, denylist: [...value.denylist, app.bundleId] });
        }}
        onRemove={(id) => onChange({ ...value, denylist: value.denylist.filter((x) => x !== id) })}
      />

      <Disclosure label="Avanzado">
        <div className="card advanced">
          <Field label="Direcciones de Google Meet" hint="Una por línea. Si la pestaña activa contiene alguna, cuenta como reunión.">
            <textarea
              className="input-mono"
              aria-label="URLs de Meet"
              rows={2}
              // Sin controlar (se guarda al salir): la clave lo repone al descartar los cambios.
              key={value.meetUrlPatterns.join("\n")}
              defaultValue={value.meetUrlPatterns.join("\n")}
              onBlur={(e) => onChange({ ...value, meetUrlPatterns: e.target.value.split("\n").map((l) => l.trim()).filter(Boolean) })}
            />
          </Field>
        </div>
      </Disclosure>
    </Section>
  );
}

export function ScreenSection({ value, onChange }: SectionProps<"screen">) {
  return (
    <Section title="Capturas" hint="Guarda las diapositivas que se comparten en la llamada y las enlaza con el tema y las tareas. Está apagado por defecto.">
      <Group foot="Necesita el permiso de Grabación de pantalla. Solo captura la ventana compartida, nunca tu escritorio.">
        <ToggleRow label="Capturar la pantalla compartida" hint="Una captura cada vez que cambia la diapositiva." checked={value.enabled} onChange={(enabled) => onChange({ ...value, enabled })} />
        <ToggleRow label="Borrar las candidatas que no se usen" hint="Al terminar, la IA elige las útiles y el resto se elimina." checked={value.pruneCandidates} onChange={(pruneCandidates) => onChange({ ...value, pruneCandidates })} />
      </Group>
      <Group title="Palabras que fuerzan una captura" foot="Si alguien las dice mientras se comparte pantalla, se guarda ese momento.">
        <div className="row row-stack">
          <ChipsInput label="Palabras clave" placeholder="Añadir palabra…" values={value.keywords} onChange={(keywords) => onChange({ ...value, keywords })} />
        </div>
      </Group>
    </Section>
  );
}
