# acta: arquitectura y contratos (v1, 16-nov-2026)

acta es un grabador de reuniones local para Mac. Está hecho en Rust con arquitectura hexagonal: Tauri v2 con React para la UI y un sidecar Swift para todo lo nativo de macOS. Este documento fija los **contratos** para que cinco agentes implementen en paralelo sin pisarse. El código de los contratos ya existe y compila. Ante cualquier duda, **manda el código**:

| Contrato | Fuente de verdad |
|---|---|
| Dominio y puertos | `crates/acta-core/src/**/{model,ports,mod}.rs` |
| Protocolo del sidecar | `crates/acta-mac-bridge/src/protocol.rs` ⇄ `sidecars/acta-mac/Sources/ActaMacProtocol/Protocol.swift` |
| Comandos y eventos de Tauri | `crates/acta-core/src/api/` ⇄ `apps/desktop/src/api/{types,contract}.ts` |
| Esquema de la base de datos | `crates/acta-store/migrations/*.sql` |
| Esquemas del LLM | `crates/acta-llm/schemas/*.json`, prompts en `crates/acta-llm/prompts/` |

> Windows llegará DESPUÉS. Los puertos están pensados para que se le pueda añadir un adaptador (ver tabla §2), pero **no se escribe nada de Windows ahora**.

---

## 1. Decisiones clave (resumen)

| Tema | Decisión | Por qué |
|---|---|---|
| Arquitectura | Hexagonal. `acta-core` no hace I/O: solo dominio, puertos y lógica pura. Carpetas por funcionalidad (screaming). | Se puede testear todo con fakes y Windows solo necesita adaptadores nuevos. |
| Audio | Lo escribe **Rust** (`hound`): WAV de 16 kHz, mono, s16 por pista, un segmento cada 30 s, primero `.tmp`, luego fsync y rename. El manifiesto se escribe de forma atómica. | Así no se repite el bug de Meetily, que lanzaba un proceso por chunk y no avisaba cuando se perdían segmentos. |
| Pistas | `mic` es "Yo" (quien usa el Mac) y `system` son los demás. Siempre separadas. | "Yo" sale gratis y solo hay que diarizar a los demás. |
| Transporte del audio | El sidecar envía frames PCM **s16le en base64** de 100 ms por stdout (evento `audio.frame`). | Rust se queda como dueño único del writer y de la recuperación. Son unos 85 KB/s con las dos pistas, nada para un pipe. |
| Sidecar | Swift (`acta-mac`) con JSON por líneas por stdin/stdout. Si recibe EOF en stdin, sale. | No puede quedar huérfano y el protocolo se puede testear. |
| Procesos hijo | `tokio::process` + `kill_on_drop(true)` + `wait()` en **todos** los caminos. Además `#![deny(clippy::zombie_processes)]`, `disallowed-methods` sobre `std::process::Command::spawn` y `scripts/guards.sh`. | Es la lección de Meetily. |
| Base de datos | **rusqlite 0.37 (bundled)** con FTS5 + sqlite-vec 0.1.x. Migraciones con `PRAGMA user_version`. | Es síncrono y simple (va con `spawn_blocking`). sqlx no aporta nada sin servidor. |
| Embeddings | fastembed-rs con `intfloat/multilingual-e5-small` (384 dims), detrás del puerto `Embedder` y de la feature `local-embeddings`. | Es multilingüe y ligero. bge-m3 (1024) queda como opción futura con una migración. |
| LLM | Puerto `LlmProvider` con seis adaptadores: `claude -p`, `codex exec`, `opencode run`, OpenAI-compatible, Anthropic y el **modelo local integrado** (§15, opcional). **De fábrica no hay ninguno activado**: la bienvenida detecta lo que hay en el Mac y el usuario elige. | Nada personal viaja como valor por defecto. Nunca se tocan las credenciales del CLI. |
| Procesado | Va en una cola de fondo y **no bloquea** una grabación nueva. | Así se pueden encadenar reuniones seguidas. |
| Firma | **Ninguna**: `bundle.macOS.signingIdentity: null`, `hardenedRuntime: false`. El sidecar se compila con `swift build --disable-get-task-allow-entitlement`. | No hay certificado de Apple Developer; ver `docs/03-instalacion.md` §3. |

---

## 2. Mapa de módulos

```text
acta/
├─ crates/
│  ├─ acta-core/        dominio + puertos + máquina de estados (sin I/O)
│  ├─ acta-audio/       writer de segmentos, recuperación, concat, watchdog, supervisor, pipeline
│  ├─ acta-store/       SQLite + FTS5 + sqlite-vec, repos, búsqueda híbrida, embeddings, clustering
│  ├─ acta-llm/         proveedores LLM, prompts, acta, chat, nombres de grupo, momentos clave
│  └─ acta-mac-bridge/  cliente del sidecar + adaptadores macOS de los puertos
├─ sidecars/acta-mac/   paquete Swift (process tap, mic, AX, ScreenCaptureKit, FluidAudio)
└─ apps/desktop/        Tauri v2: src/ (React+TS) y src-tauri/ (composición)
```

Funcionalidades de `acta-core/src/`:

| Carpeta | Contiene | Puertos |
|---|---|---|
| `recording/` | `Track`, `AudioFrame`, `SessionManifest`, `Level`, **máquina de estados**, `AutoStopPolicy`, `HealthWarning` | `AudioSource` |
| `detection/` | `MeetingApp`, `Platform`, `DetectionEvent`, `SpeakerSignal` | `MeetingDetector`, `SpeakerNameSource` |
| `transcription/` | `AsrSegment`, `DiarizationResult`, `Speaker`, `Utterance`, `Transcript`, `Voiceprint` | `Transcriber`, `Diarizer`, `LiveTranscriber` |
| `minutes/` | `Minutes`, `Decision`, `ActionItem`, `Escaleta`/`Tema`, `KeyMoment`, `TimelineEvent` | `MinutesGenerator`, `GroupNamer`, `ImportantMomentDetector` |
| `screen/` | `Capture`, `CaptureReason`, `ScreenEvent` | `ScreenCapturer` |
| `llm/` | `ProviderConfig`, `ProviderKind`, `LlmRequest`/`Response` | `LlmProvider`, `LlmProviderFactory` |
| `library/` | `Meeting*`, `Group*`, `Search*`, `Chunk`, `EmbedKind` | `MeetingStore`, `GroupStore`, `VoiceprintStore`, `SearchIndex`, `GroupSuggester`, `Embedder`, `SettingsStore` |
| `chat/` | `ChatScope`, `ChatThread`, `ChatMessage`, `Citation`, `ChatDelta` | `Retriever`, `ChatStore`, `ChatEngine` |
| `processing/` | `ProcessingStep`, `ProcessingProgress` | `ProcessingPipeline`, `ProgressSink` |
| `permissions/` | `PermissionKind`, `PermissionStatus` | `PermissionsProbe` |
| `settings/` | `Settings` con valores por defecto | — |
| `api/` | Contrato Tauri: `SessionSnapshot`, `ApiError`, nombres de comandos y eventos | — |
| raíz | `ids` (UUID v7 tipados), `time` (`UnixMs`, `MediaMs`, `Clock`), `layout` (rutas), `error` (`PortError`) | `Clock` |

Adaptador de cada puerto:

| Puerto | macOS (ahora) | Windows (después, NO ahora) |
|---|---|---|
| `AudioSource` | `acta-mac-bridge::audio` (process tap + mic en Swift) | cpal + WASAPI process loopback |
| `MeetingDetector` | `acta-mac-bridge::detection` | CapabilityAccessManager + UI Automation |
| `SpeakerNameSource` | `acta-mac-bridge::speakers` (AX Teams) | UI Automation |
| `ScreenCapturer` | `acta-mac-bridge::screen` (ScreenCaptureKit) | Windows.Graphics.Capture |
| `Transcriber`, `Diarizer`, `LiveTranscriber` | `acta-mac-bridge::transcription` (FluidAudio) | whisper-rs / ONNX |
| `PermissionsProbe` | `acta-mac-bridge::permissions` | — |
| Stores, `Embedder`, `Retriever` | `acta-store` | igual |
| `LlmProvider` y compañía | `acta-llm` | igual |
| `ProcessingPipeline` | `acta-audio::processing::DefaultPipeline` | igual |

### Dos relojes (no se mezclan)

- `UnixMs`: hora de pared. La usan las señales en vivo (detección, hablante activo, capturas) y los metadatos.
- `MediaMs`: posición en el audio final, **sin pausas**. La usan las utterances, la escaleta, el timeline y los saltos de reproducción.
- Para pasar de una a otra se usa `SessionManifest::wall_to_media`.

---

## 3. Máquina de estados de la sesión

`acta_core::recording::machine` es **pura**: `SessionState::transition(event, &mut IdSource) -> Result<Transition{next, effects}, TransitionError>`. El supervisor (WP1) ejecuta los `Effect` en orden. Si llega un evento inválido, devuelve `Err` y el estado no cambia: el supervisor lo registra y lo ignora. Está especificada como tests en `crates/acta-core/tests/state_machine.rs`: 21 pasan y 3 están `#[ignore]` porque son TODO de WP1.

```text
           MeetingDetected                    Accept / StartRequested
  Idle ───────────────────▶ Detected ─────────────────────────────────┐
   ▲ ▲  Decline/Expired/Ended │                                         ▼
   │ └────────────────────────┘                    ┌───────────── Recording ◀──┐
   │                                     Pause     │   ▲  MeetingEnded/Silence  │ Resume
   │ RecoveryRequired                              ▼   │  → abre "¿Parar?"      │
   ├────────▶ Recovering                         Paused┘                         │
   │             │ RecoveryFinished                │ Stop / AutoStopDue / Fatal  │
   │             ▼                                 ▼                             │
   │         Processing ◀──── CaptureFinalized ── Stopping ◀────────────────────┘
   │             │ ProcessingFinished                     Stop / AutoStopDue / Fatal
   │  Dismiss    ▼
   ├──────── Ready
   └──────── Failed ◀── ProcessingFailed / RecoveryFailed
```

Reglas que no son obvias:

| Regla | Detalle |
|---|---|
| No bloquea | Desde `Processing`, `Ready` y `Failed` se puede pasar a `Detected` o `Recording`. Los eventos de procesado de **otra** reunión se ignoran (`Ok`, sin efectos). |
| Pregunta de parada | `MeetingEnded` o `SilenceDetected` en `Recording` abren `stopPrompt`. Si ya hay una pregunta abierta, no se abre otra. Accept lleva a `Stopping{MeetingEndedConfirmed}`. Decline cierra la pregunta y se sigue grabando. |
| Fatal | Si hay un `Fatal` en `Recording` o `Paused`, se pasa a `Stopping{Error}`: se procesa lo que haya, **no se tira el audio**. |
| Prompt caducado | Una respuesta a un `promptId` que no es el abierto da `Err`. |
| Ids | Los `MeetingId` y `PromptId` vienen de `IdSource`, que se inyecta para que los tests sean deterministas. |

### Autoparada (`recording::autostop`, la implementa WP1)

| # | Regla | Por defecto |
|---|---|---|
| 1 | Tope duro de tiempo grabado (sin contar pausas): `AutoStopDue{MaxDuration}` | 4 h |
| 2 | Silencio en **ambas** pistas (`rms < -50 dBFS`) sin pregunta abierta: `SilenceDetected` | 3 min |
| 3 | Pregunta "¿Parar?" sin responder **y** silencio en ambas pistas: `AutoStopDue{AutoSilence}` | 2 min |
| 4 | Si alguien habla, el contador de silencio vuelve a cero. Tras un "No", no se vuelve a preguntar hasta que haya habido voz y después otra vez el silencio de la regla 2. | — |

### Watchdog de salud (`recording::health` + `acta-audio::watchdog`)

| Aviso | Disparo | Por defecto |
|---|---|---|
| `trackStalled` | Una pista no entrega frames | 3 s |
| `trackAllZeros` | Solo **ceros exactos** (es la firma de TCC denegado) | 10 s |
| `segmentsStalled` | No se finaliza ningún segmento | 30 s + 10 s |
| `diskLow` | Poco espacio libre en el volumen de datos | < 1 GB |
| `overrun` / `sidecarRestarted` / `writeFailed` / `permissionMissing` | Cuando ocurren | — |

Cada aviso se emite al aparecer y otra vez con `cleared: true` al resolverse. La pastilla lo muestra (punto rojo + `message` en castellano).

---

## 4. Protocolo sidecar ⇄ Rust (v1)

### 4.1 Transporte

| Canal | Contenido |
|---|---|
| stdin (Rust → sidecar) | Una línea JSON por comando: `{"id":7,"cmd":"capture.start","args":{…}}`. Los comandos sin argumentos van **sin** `args`. |
| stdout (sidecar → Rust) | Respuesta: `{"type":"response","id":7,"ok":true,"result":{…}}` o `{"type":"response","id":7,"ok":false,"error":{"code":"…","message":"…"}}`. Evento: `{"type":"event","ts":<unix ms>,"event":"<nombre>","data":{…}}`. |
| stderr | Logs del sidecar en texto libre. Rust los vuelca a `logs/acta-mac.log`. Nunca lleva protocolo. |

- Claves en snake_case. En Swift se usa `.convertToSnakeCase`, así que las propiedades se escriben `bundleIds`, **no** `bundleIDs`.
- Cada línea ocupa como máximo 1 MiB. Las respuestas pueden llegar **desordenadas** porque cada comando corre en su propia tarea, así que se emparejan por `id`.
- Códigos de error: `permission_denied`, `not_found`, `unsupported`, `invalid_args`, `busy`, `cancelled`, `internal`.

### 4.2 Comandos

| `cmd` | `args` | `result` |
|---|---|---|
| `hello` | `protocol_version` | `sidecar_version, protocol_version, macos_version, capabilities[]` |
| `shutdown` | — | `{}` y luego el sidecar sale con código 0 |
| `permissions.status` | — | `permissions[{kind, status, target?}]` |
| `permissions.request` | `kind, target?` | `status` |
| `devices.list` | — | `inputs[], outputs[]` con `{uid, name, is_default, transport}` |
| `apps.list` | — | `apps[{pid, bundle_id, name, audio_pids[]}]` |
| `detect.start` | `rules[{bundle_id, platform, is_browser}], meet_url_patterns[], poll_ms` | `{}` |
| `detect.stop` | — | `{}` |
| `capture.start` | `session_id, sample_rate=16000, frame_ms=100, mic{enabled, device_uid?}, system{enabled, pids[], bundle_ids[]}` | `started_at_unix_ms, mic_device?, system_pids[]` |
| `capture.pause` / `capture.resume` / `capture.stop` | — | `{}` |
| `capture.retarget` | `pids[], bundle_ids[]` | `{}` |
| `speakers.watch` | `pid, bundle_id` | `{}` |
| `speakers.unwatch` | — | `{}` |
| `screen.start` | `session_id, pid, bundle_id, window_title_hint?, out_dir, candidates_dir, fps, phash_threshold, stable_frames, candidate_every_s, ring_seconds, max_width, format="jpeg"` | `{}` |
| `screen.snapshot` | `reason, at_unix_ms, keyword?` | `frame?` (igual que el evento `screen.frame`) |
| `screen.stop` | — | `{}` |
| `asr.transcribe` | `job_id, wav_path, out_path, language?, model="parakeet-tdt-0.6b-v3"` | `out_path, segments, duration_ms`. Escribe un `AsrFile`. |
| `asr.live.start` | `tracks[], language?` | `{}` |
| `asr.live.stop` | — | `{}` |
| `diarize.run` | `job_id, wav_path, out_path, num_speakers?, min_speakers?, max_speakers?` | `out_path, speakers`. Escribe un `DiarizationFile`. |
| `voiceprint.embed` | `wav_path, spans[{start_ms, end_ms}]` | `embedding[], model` |
| `models.status` | — | `models[{name, downloaded, bytes?}]` |
| `models.download` | `name` (`asr` \| `diarizer` \| `vad`) | `{}`. El progreso llega por `models.progress`. |
| `job.cancel` | `job_id` | `{}`. El trabajo responde con el error `cancelled`. |
| `system.profile` | — | `full_name?, preferred_languages[]` (`NSFullUserName`, `Locale.preferredLanguages`) |
| `apps.installed` | `icons_dir, icon_px` | `apps[{bundle_id, name, path, running, icon_path?}]`. Un nivel de `/Applications`, `/Applications/Utilities`, `/System/Applications`, `/System/Applications/Utilities` y `~/Applications`, más las apps normales en marcha. Escribe `<icons_dir>/<bundle id>.png` solo si falta o la app es más nueva. |

### 4.3 Eventos

| `event` | `data` |
|---|---|
| `audio.frame` | `track, seq, pts_us, sample_rate, channels=1, format="s16le", data` (base64). `pts_us` es relativo a `capture.start`, con reloj host común a las dos pistas. |
| `audio.device_changed` | `track, device_uid?, device_name?, reason` (`default_input_changed` \| `default_output_changed` \| `device_removed` \| `route_changed`) |
| `audio.track_state` | `track, state` (`running` \| `stalled` \| `restarting` \| `stopped` \| `failed`), `message?` |
| `audio.overrun` | `track, dropped_frames` |
| `detect.signal` | `signal` (`mic_in_use` \| `mic_released` \| `meeting_window_open` \| `meeting_window_closed` \| `meet_tab_open` \| `meet_tab_closed` \| `audio_pids_changed`), `app{pid, bundle_id, name, audio_pids[]}, window_title?, url?` |
| `speakers.roster` | `pid, participants[{name, is_self, muted?}]` |
| `speakers.active` | `pid, names[]` (solo cuando cambia, con lista vacía si no habla nadie) |
| `screen.share_state` | `sharing, presenter?, source` (`ax` \| `frame`) |
| `screen.frame` | `path` (absoluta, ya renombrada), `at_unix_ms, reason, phash` (hex de 16), `width, height, keyword?` |
| `asr.partial` | `track, text, at_unix_ms, is_final` |
| `job.progress` | `job_id, fraction, stage?` |
| `models.progress` | `name, fraction, done, error?` |
| `permissions.changed` | `kind, status` |
| `log` | `level, message` |
| `fatal` | `message` (justo antes de salir) |

Ficheros de salida, escritos con `.tmp` + rename:

- `AsrFile` (`transcript/asr-<track>.json`): `{model, language?, duration_ms, segments[{start_ms, end_ms, text, confidence?, words[{start_ms, end_ms, text}]}]}`.
- `DiarizationFile` (`transcript/diarization.json`): `{embedding_model, turns[{label, start_ms, end_ms}], speakers[{label, embedding[], total_ms}]}`.

### 4.4 Supervisión (acta-mac-bridge)

1. Lanzamiento con `tokio::process::Command` y `kill_on_drop(true)`, con stdin, stdout y stderr en pipe. Primero `hello`, y si `protocol_version != 1` se aborta.
2. Una tarea lee stdout con `BufReader::lines` (tope de 1 MiB por línea), empareja las respuestas por `id` con oneshot y publica los eventos en un `broadcast`.
3. Si el hijo muere solo, se hace `wait()` (siempre) y se reinicia con backoff, como mucho 5 veces en 60 s. Si estaba grabando, se repite `capture.start` y se emite `AudioEvent::Restarted`. El writer rellena el hueco con silencio y se avisa con `sidecarRestarted`.
4. Para cerrar: `shutdown`, espera de 2 s y después `start_kill()` + `wait()`.
5. Lado Swift: sale con EOF en stdin (si el padre muere, se cierra el pipe), sale si cambia `getppid()` y sale si falla la escritura en stdout. Hay un solo escritor de stdout (actor `Output`). El callback de audio **nunca** bloquea: escribe en un ring buffer por pista y, si se llena, descarta lo más antiguo y emite `audio.overrun`.
6. TCC: el sidecar va en `Contents/MacOS/acta-mac` y lo lanza la app, así que los permisos se atribuyen a **acta.app**, que es el proceso responsable.

### 4.5 Detección: cómo se fusionan las señales (acta-mac-bridge)

| Plataforma | Empieza cuando | Termina cuando |
|---|---|---|
| Teams (`com.microsoft.teams2`) | `mic_in_use` de un proceso de Teams + `meeting_window_open` (AX) mantenidos `debounce_ms`. Confianza `High`. Solo `mic_in_use`: `Low`. | `mic_released` + `meeting_window_closed`, o una de las dos durante 10 s |
| Meet (navegadores de la lista) | `meet_tab_open` (AppleScript, URL que encaja con `meet_url_patterns`) + `mic_in_use` del navegador | `meet_tab_closed` o `mic_released` durante 10 s |

Solo cuentan las apps de `Settings.detection.rules` (lista blanca), así que Typeless y Orca no disparan la detección. Hay que emitir **un solo** `MeetingStarted` por reunión.

---

## 5. Audio: writer, manifiesto y recuperación (WP1)

1. `SegmentWriter::create` crea `meetings/<id>/audio/{mic,system}/` y escribe `manifest.json` con status `recording`.
2. `push(frame)`: convierte `pts_us` en posición de sample. Si hay un hueco, mete ceros (y lo apunta en `padded_samples`). Si un frame se solapa, se recorta. Las dos pistas quedan **alineadas sample a sample**.
3. Escribe en `seg-NNNNNN.wav.tmp` y hace flush cada `flush_every_ms` (1 s). Al llegar a 30 s: finaliza la cabecera, fsync, rename a `.wav` y manifiesto atómico (`manifest.json.tmp`, fsync, rename, fsync del directorio).
4. Si falla un segmento: aviso `writeFailed`, **el índice avanza y el buffer se vacía**. Nunca se reintenta con el mismo nombre.
5. Pausa: cierra el segmento en curso y cierra el `RecordingSpan`. Al reanudar abre un span nuevo con `media_start` igual al total grabado.
6. `finish`: cierra todo y deja el manifiesto en `stopped`. Esto genera `CaptureFinalized`.
7. Recuperación al arrancar: `scan_incomplete` busca manifiestos en `recording` o `paused`. Para cada uno: `RecoveryRequired`, se reescribe la cabecera de cada `.tmp` según su tamaño real (descartando un sample incompleto), rename, manifiesto en `stopped` y se procesa.
8. `concat` (hound) genera `audio/mic.wav`, `audio/system.wav` y `audio/mix.wav` (mezcla mono con limitador para reproducir). **Nunca ffmpeg.**

---

## 6. Procesado tras la reunión (WP1 orquesta, cada paso usa puertos)

| Paso (`ProcessingStep`) | Qué hace | Puerto |
|---|---|---|
| `concatenating` | §5.8 | (I/O de acta-audio) |
| `transcribing` | ASR de `mic.wav` y `system.wav` | `Transcriber` |
| `diarizing` | Solo `system.wav` | `Diarizer` |
| `namingSpeakers` | §6.4 | `VoiceprintStore` + señales AX guardadas durante la reunión |
| `summarizing` | Acta completa (§7) | `MinutesGenerator` |
| `indexing` | Chunks, FTS y vectores, más el embedding medio de la reunión | `SearchIndex` |
| `pruningCaptures` | Promueve las candidatas cercanas a `key_moments` (`llmPick`) y borra el resto | `MeetingStore` |

Durante la grabación, el supervisor guarda las `SpeakerSignal` (roster y hablante activo) en `meetings/<id>/live/speakers.jsonl` para usarlas en el paso de nombres.

### 6.4 Fusión del transcript

- Pista `mic`: hablante "Yo" (`is_me`, `name_source = me`).
- Pista `system`: cada utterance se asigna al turno de diarización con el que más se solapa.
- Nombre de cada hablante diarizado, por este orden:
  1. Huella de voz con similitud ≥ 0,75 y el mismo `model`.
  2. Hablante activo de Teams con mayor solape temporal.
  3. "Speaker N".
- `confirmed` solo lo pone el usuario. La huella **solo se aprende** cuando el usuario confirma el nombre (`speaker_rename` con `remember: true`).
- Eco: las utterances de `system` que se solapan ≥ 80 % con una de `mic` de texto casi igual se descartan.
- Limpieza con LLM (`transcription.llmCleanup`, encendida de fábrica; solo si hay proveedor): tras la fusión y antes del acta, `TranscriptCleaner` corrige el `text` de cada utterance (jerga con el glosario, puntuación, mayúsculas) en tandas de ≤ 40 segmentos / 6 000 caracteres. La respuesta debe traer los mismos ids en el mismo orden; si no, tiempo agotado o error, esa tanda se queda con el texto crudo y no se cachea. Caché `transcript/cleanup.json` (texto → limpio) invalidada por glosario, idioma y versión del prompt.
- Troceo del audio para el ASR: el sidecar puntúa la pista con Silero VAD, corta en pausas (≥ 3 bloques de 256 ms por debajo de 0,8; tope 13,5 s cortando en el punto más callado), descarta el silencio y decodifica cada tramo en una pasada con tiempos exactos del fichero. Sin VAD, pista entera como antes (`ACTA_ASR_VAD=0` lo fuerza).

---

## 7. LLM: proveedores, prompts y esquemas (WP3)

### 7.1 Contrato

- `LlmRequest { system, messages, json_schema?, max_tokens?, temperature? }`.
- El proveedor usa el esquema de forma nativa si puede. El llamador **siempre** valida con serde. Si falla, hace un único reintento con "repara este JSON".
- Si el transcript no cabe en `capabilities().context_tokens`, se hace map-reduce: se resume por bloques de unos 20 min y se fusiona con el mismo esquema.

### 7.2 Proveedores

| Proveedor | Invocación |
|---|---|
| `claudeCli` | `<binary_path> -p --safe-mode --output-format json --json-schema '<schema>' --tools "" --strict-mcp-config --no-session-persistence --system-prompt '<sys>' [--model m]`. El prompt va **por stdin** y el `cwd` es un directorio temporal vacío. El resultado es `.result` del JSON. **No usar `--bare`**: ignora el login OAuth del usuario. |
| `codexCli` | `<binary_path> exec --skip-git-repo-check --ephemeral --sandbox read-only --output-schema <fichero> -o <fichero_salida> [-m m] -`, con el prompt por stdin y `cwd` temporal. |
| `opencodeCli` | `<binary_path> run --archived --format json [-m m]`, con el prompt por stdin (incluye el esquema JSON cuando aplica) y un `opencode.json` temporal que desactiva snapshots y deniega herramientas. Si `model` está vacío, resuelve el modelo de `~/.config/opencode/opencode.json(c)` o el último no gratuito de `~/.local/state/opencode/model.json`; `opencode models` pobla el selector de Ajustes y el reintento. |
| `openAiCompatible` | `POST {base_url}/chat/completions` con `response_format: {type: json_schema, strict: true}` y streaming SSE. |
| `builtIn` | `{modelId}`. acta arranca su propio `llama-server` y le habla por la misma API compatible con OpenAI (§15). |
| `anthropic` | Messages API. La salida estructurada va con una herramienta cuyo `input_schema` es el esquema. La clave está en el Llavero (`SecretRef`). |

Reglas comunes de los CLI:

- Se lanzan **solo** con `acta_llm::process::run_supervised` (ya hecho y testeado).
- Variables extra con `extra_env` (por ejemplo `CLAUDE_CONFIG_DIR`, si el `claude` del usuario es un alias que la fija).
- Las apps GUI **no heredan el PATH**. `llm_cli_detect` resuelve las rutas con `zsh -lc 'command -v claude'`, también supervisado.

### 7.3 Esquemas

**`acta.minutes.v1`** (`schemas/minutes.v1.json`, compatible con el modo estricto de OpenAI):

```json
{ "titulo_sugerido": "string|null",
  "resumen": "markdown",
  "decisiones": [{ "texto": "", "ts_ms": "int|null" }],
  "tareas": [{ "texto": "", "responsable": "string|null", "es_mia": true,
               "fecha_limite": "string|null", "ts_ms": "int|null", "capturas": ["c1"] }],
  "escaleta": { "temas": [{ "titulo": "", "resumen": "", "inicio_ms": 0, "fin_ms": 0,
                            "subtemas": ["<tema>"], "capturas": ["c1"] }] },
  "momentos_clave": [{ "ts_ms": 0, "motivo": "", "tema": "string|null" }] }
```

- Entrada (`prompts/minutes.user.md`): metadatos, la lista de capturas como `c1 [mm:ss] slide_change` y el transcript en líneas `[mm:ss|ms] Nombre: texto`. El usuario aparece como `<nombre> (yo)` o, si no ha puesto nombre en Ajustes, solo como `Yo`.
- Idioma: `Settings.transcription.language` es `"auto"` o un código ISO 639-1. Dirige la pista de idioma del ASR (`settings::language::asr_hint`; `auto` o un idioma que Parakeet no conoce = que lo detecte) y el idioma en que se escriben acta, chat, preguntas en vivo y nombres de grupo (`auto` = el que se habla en la reunión).
- `es_mia = true` solo si el responsable es el usuario (`Settings.user` + aliases).
- acta-llm traduce las refs `cN` a `CaptureId` y las `ts_ms` a `MediaMs` para construir `MinutesOutput`.

**`acta.group_name.v1`**: `{ "nombre": "2-5 palabras", "descripcion": "una frase" }`.

**Chat**: sin esquema, texto en streaming con citas `[n]`.

- Reunión: el transcript entero en contexto.
- Grupo o todo: el `Retriever` saca los 12 mejores chunks, que van numerados `[n] Reunión · fecha · mm:ss`. Las `[n]` de la respuesta se convierten en `Citation{n, meeting_id, at, …}`.
- Se descartan las citas a números que no existen.

**El acta habla de la reunión, no de la grabación** (`minutes/meta.rs`): el prompt prohíbe comentar la grabación, la calidad de la transcripción, las etiquetas de hablante o la herramienta. Como un modelo pequeño no siempre obedece, después se quitan esas frases con una lista corta de patrones (fixture `tests/fixtures/minutes_meta_commentary.json`, sacado de un acta real). Con menos de 8 palabras transcritas no se llama al LLM: el resumen es «Reunión muy breve, sin contenido que resumir». Vale para todos los proveedores.

**Detector de momentos importantes** (en vivo, solo con `live_preview`):

1. Normaliza el texto (minúsculas, NFD sin diacríticos).
2. Busca palabra completa o prefijo de `Settings.screen.keywords`: "importante", "apuntad", "esto es clave", "fecha", "plazo", "deadline", "important", "key point", "take note", "due date".
3. La misma palabra no vuelve a disparar en 20 s.
4. Si hay coincidencia: `screen.snapshot{reason: keyword}`.

---

## 8. Capturas de la pantalla compartida (opt-in)

Requiere el permiso de Grabación de pantalla y `Settings.screen.enabled = true` (desactivado por defecto).

1. **Qué se captura**: la ventana de reunión de Teams con `SCContentFilter(desktopIndependentWindow:)` y `SCStream` a ≈1 fps (`minimumFrameInterval = 1 s`), reducida a `max_width`.
2. **Cuándo hay alguien compartiendo**:
   - Por AX: elementos de "está presentando" o la vista de contenido compartido (`source: ax`).
   - Fallback: una región grande y estable que cambia por bloques (`source: frame`).
   - Esto genera `screen.share_state`. Cuando el propio usuario comparte su pantalla queda fuera del MVP.
3. **Cambio de diapositiva**: dHash de 64 bits por frame. Si la distancia de Hamming con la última guardada es ≥ `phash_threshold` (10) y la imagen se mantiene estable `stable_frames` (2) frames seguidos, se guarda como `slide_change` en `captures/`.
4. **Candidatas**: mientras se comparte, se guarda una cada `candidate_every_s` (30 s) en `captures/candidates/`, con calidad menor.
5. **Anillo en memoria**: los últimos `ring_seconds` (20 s) de frames, para que `screen.snapshot{at_unix_ms}` guarde el frame más cercano de forma retroactiva. Lo usan `keyword` y `manual`.
6. **Pasada del LLM**: `momentos_clave` hace que en `pruningCaptures` se promueva a `llmPick` la candidata más cercana (±15 s). El resto de candidatas se borran si `prune_candidates` está activo.
7. **Vínculos**:
   - Cada captura se asocia a la ruta del tema de la escaleta que la contiene (`topic_path` "2.1").
   - Las refs `cN` que el LLM puso en `tareas[].capturas` se guardan en `capture_action_items`.
   - Las capturas aparecen en el timeline como `capture`.
8. Ficheros JPEG (calidad 0,8), escritos con `.tmp` + rename. El sidecar los deja como `captures/<unix_ms>-<reason>.jpg` (candidatas: `captures/candidates/<unix_ms>.jpg`) y el supervisor los **renombra** a `captures/<media_ms 9 dígitos>-<reason>.jpg` con `wall_to_media` del manifiesto en memoria (`DataLayout::capture_rel_path`). La UI muestra `<dataDir>/meetings/<id>/<path>`; `topicPath` es la ruta 1-based del tema más profundo ("2.1").

---

## 9. Base de datos (WP2)

Esquema completo en `crates/acta-store/migrations/0001_init.sql`. Ya se aplica y está testeado: el test comprueba FTS5 sin acentos y `vec_version()`.

| Tabla | Clave | Notas |
|---|---|---|
| `meetings` | `id` TEXT UNIQUE + `rowid_alias` INTEGER PK | `rowid_alias` sirve para `vec_meetings` |
| `speakers` | `id` | FK a meeting y a `voiceprint_id` |
| `utterances` | `id` INTEGER | `(meeting_id, idx)` es único. Triggers que la mantienen en `utterances_fts` |
| `utterances_fts` | FTS5 | `unicode61 remove_diacritics 2` |
| `minutes` | `meeting_id` | `escaleta` y `key_moments` en JSON |
| `decisions`, `action_items` | `id` | índice `(is_mine, done)` para "mis tareas" |
| `captures` + `capture_action_items` | `id` | `path` relativo a la carpeta de la reunión |
| `groups`, `group_meetings`, `group_suggestions` | `id` | |
| `chunks` + `chunks_fts` | `id` | ventanas de 45-60 s o ≤ 300 tokens |
| `vec_chunks`, `vec_meetings` | vec0 `float[384] distance_metric=cosine` | cambiar de modelo obliga a una migración nueva |
| `embedding_meta`, `settings`, `chat_threads`, `chat_messages`, `llm_runs` | | `settings['app']` guarda el JSON de `Settings` |

Migraciones `0003` y `0004`: `meetings.favourite`, `groups.favourite`, `meetings.last_opened_at`, `meetings.last_used_at` y `meetings.cleanup_keep` (§16).

Búsqueda híbrida:

1. BM25 (FTS5) top 50 y vectores (k = 50), fusionados con RRF (k = 60).
2. Filtro por `SearchScope` (todo, favoritas, grupo o reunión). «Favoritas» = reuniones con estrella + reuniones de grupos con estrella (`meetings::favourite_sql`, la misma condición para el filtro de la lista, la búsqueda y la limpieza).
3. Snippet con `snippet()` de FTS5.

Grupos sugeridos:

1. Clustering aglomerativo por coseno sobre `vec_meetings`, con umbral 0,80 y mínimo 3 reuniones por cluster. Solo entran las reuniones indexadas (con embedding) que no están ya en ningún grupo, y como mucho las 600 más recientes.
   - `group_suggestions_refresh` reintenta indexar las pendientes; si no hay sugerencias y sigue habiendo reuniones sin indexar, devuelve un error (no una lista vacía).
2. Se devuelven sin nombre. La app los nombra con `GroupNamer` y los guarda con `save_suggestions`.

---

## 10. Directorio de datos y permisos

```text
~/Library/Application Support/acta/
  acta.db (+ -wal, -shm)
  logs/acta.log  logs/acta-mac.log
  models/  (fastembed/, fluidaudio/, llm/<modelo>.gguf)
  runtime/llama/<versión>/llama-server  (+ dylib; llama-server.pid)
  cache/app-icons/<bundle id>.png  (iconos de `apps_list`; se regeneran)
  meetings/<meeting_id>/
    manifest.json
    audio/{mic,system}/seg-000000.wav[.tmp]   audio/{mic,system,mix}.wav
    transcript/{asr-mic,asr-system,diarization}.json
    live/speakers.jsonl
    captures/*.jpg   captures/candidates/*.jpg
    llm/minutes.json   llm/raw-<kind>-<n>.txt
```

Las rutas se calculan **siempre** con `acta_core::layout::DataLayout`: nadie concatena rutas a mano.

| Permiso | Para qué | Clave en Info.plist | ¿Opcional? |
|---|---|---|---|
| Micrófono | Pista "Yo" | `NSMicrophoneUsageDescription` | No |
| Audio del sistema (process tap, macOS 14.2+) | Pista "otros" | `NSAudioCaptureUsageDescription` | No |
| Accesibilidad | Ventana de reunión, roster y hablante activo de Teams, detección de "presentando" | — (Ajustes del sistema) | Sí, aunque mejora mucho la detección y los nombres |
| Automatización (Apple Events) | URL de Meet en Chrome, Arc, Edge y Safari | `NSAppleEventsUsageDescription` | Sí |
| Grabación de pantalla | Capturas de pantalla compartida | `NSScreenCaptureUsageDescription` | Sí (opt-in) |

---

## 11. Contrato Tauri (WP5 ⇄ WP6)

Convenciones:

- Comandos en snake_case. Los argumentos se pasan en camelCase desde JS y Tauri los convierte.
- Todo devuelve `Result<T, ApiError{code, message}>`, con `message` en castellano.
- DTO en camelCase, salvo `Escaleta`.

Tabla completa de comandos: `crates/acta-core/src/api/commands.rs` (documentación del módulo). Resumen:

| Área | Comandos |
|---|---|
| Sesión | `session_get_state`, `session_start{app?}`, `session_pause`, `session_resume`, `session_stop`, `session_dismiss`, `prompt_answer{promptId, answer}`, `capture_manual` (todos devuelven `SessionSnapshot`, salvo `capture_manual`) |
| Reuniones | `meetings_list{query}`, `meeting_get{id}`, `meeting_update{id, title}`, `meeting_delete{id}`, `meeting_set_favourite{id, favourite}`, `meeting_reprocess{request}`, `speaker_rename{meetingId, speakerId, displayName, remember}`, `action_item_set_done{id, done}`, `capture_delete{id}` |
| Limpieza | `cleanup_suggestions`, `cleanup_delete{ids}`, `cleanup_keep{ids}` (§16) |
| Modelo local | `local_llm_status`, `local_llm_download{modelId}`, `local_llm_cancel{modelId}`, `local_llm_delete{modelId}`, `local_llm_test{modelId}` (§15) |
| Grupos | `groups_list`, `group_create`, `group_update`, `group_delete`, `group_set_favourite{id, favourite}`, `group_add_meeting`, `group_remove_meeting`, `group_suggestions_list`, `group_suggestions_refresh`, `group_suggestion_accept`, `group_suggestion_dismiss` |
| Búsqueda y chat | `search{query}`, `chat_threads_list`, `chat_thread_create`, `chat_thread_delete`, `chat_messages_list`, `chat_send{threadId, text, providerId?, model?}` (proveedor y modelo solo para esa respuesta; devuelve `ChatSendResult` y luego llegan `chat:delta`) |
| Huellas | `voiceprints_list`, `voiceprint_delete{id}` |
| Ajustes y diagnóstico | `settings_get`, `settings_update{settings}`, `llm_provider_test{providerId}`, `llm_cli_detect`, `secret_set{providerId, value}`, `secret_delete{providerId}`, `models_status`, `models_download{name}`, `permissions_status`, `permissions_request{kind}`, `open_system_settings{kind}`, `health_get`, `app_info`, `system_profile` (nombre de la cuenta e idiomas de macOS, para la bienvenida), `apps_list` (apps instaladas y en marcha con icono) |
| Ventanas | `overlay_set_expanded{expanded}`, `main_window_show{meetingId?}` |

| Evento | Payload | Frecuencia |
|---|---|---|
| `session:state` | `SessionSnapshot` (la pregunta abierta va **dentro** de `state`) | en cada transición y 1 Hz mientras graba |
| `session:levels` | `Levels` | ≈10 Hz mientras graba |
| `session:health` | `HealthWarning` | cuando un aviso aparece o se limpia |
| `processing:progress` | `ProcessingProgress` | por paso o fracción |
| `meeting:updated` | `MeetingUpdated` | cambio de estado o de título |
| `capture:saved` | `Capture` | durante la reunión |
| `chat:delta` | `ChatDelta` | streaming |
| `live:partial` | `LivePartial` | solo con previsualización en vivo |
| `model:download` | `ModelDownload` | descargas |
| `localmodel:download` | `LocalLlmDownload{modelId, stage, receivedBytes, totalBytes, fraction, done, error?}` | descarga del modelo local integrado |
| `main:navigate` | `MainNavigate{meetingId}` | `main_window_show{meetingId}` (la pastilla abre una reunión) |

Ventanas (`tauri.conf.json`, con `macOSPrivateApi: true`):

- `main`: 1200×800 (mínimo 900×600), transparente, `titleBarStyle: "Overlay"` con título oculto y semáforos en (20, 24). Material `windowEffects: ["liquidGlassRegular", "sidebar"]` que sigue el estado activo de la ventana: en macOS 26 monta un `NSGlassEffectView`; en macOS 14-15 cae al `NSVisualEffectView` `sidebar`.
- `overlay` (la pastilla): 280×56, sin decoraciones ni sombra, transparente, `alwaysOnTop`, `visibleOnAllWorkspaces`, `focus: false`, `acceptFirstMouse`, oculta al arrancar. Material `["liquidGlassRegular", "hudWindow"]` con radio 28 y tinte `#101014b3`.
- Capacidad `default` (ventanas `main` y `overlay`): `core:default`, `core:window:allow-start-dragging` (arrastrar por la barra de título superpuesta) y `opener:default`.
- WP6 convierte `overlay` en un **NSPanel no activable** con `tauri-nspanel` (`NSWindowStyleMaskNonactivatingPanel`, nivel por encima de pantalla completa, `canJoinAllSpaces` + `fullScreenAuxiliary`). `overlay_set_expanded` la amplía a 360×150 para las preguntas **manteniendo su centro horizontal y su borde superior**.
- **Trampa (macOS 26.5)**: no llamar a `set_effects` sobre la pastilla DESPUÉS de convertirla en NSPanel. La llamada no vuelve y se salta el resto de `setup` (bandeja, eventos, estado inicial). El efecto declarado en la configuración sobrevive a la conversión. Detalle en `src-tauri/src/glass.rs`.
- Variables de entorno de desarrollo: `ACTA_DATA_DIR` (otra carpeta de datos), `ACTA_MAC_SIDECAR` (otro binario del sidecar), `VITE_ACTA_DEMO=1` (datos de demostración, solo en desarrollo y nunca dentro de un build de producción), `ACTA_THEME`, `ACTA_OVERLAY_PREVIEW` y `ACTA_PREVIEW_AT` (ayudas de diseño, ver `glass.rs`).
- Audio y capturas se sirven por el asset protocol (`convertFileSrc`), con scope `$HOME/Library/Application Support/acta/**`.

---

## 12. Plan de implementación (work packages en paralelo)

### 12.1 Reglas para todos

1. **Cada WP solo toca sus carpetas** (tabla 12.2). Hay que respetarlo sobre todo con `Cargo.toml` raíz, `acta-core/src/**/{model,ports}.rs`, `acta-core/src/api/**`, `protocol.rs`/`Protocol.swift` y `apps/desktop/src/api/**`: son el **contrato congelado** y cualquier cambio pasa por el orquestador. Si te falta algo en el contrato, pídelo; no lo inventes en otro sitio.
2. **TDD estricto**: primero el test que falla y luego el código. Comandos que se pueden ejecutar: `cargo test -p <crate>`, `cargo clippy -p <crate> --all-targets -- -D warnings`, `scripts/guards.sh`, `swift build --disable-get-task-allow-entitlement`, `pnpm exec vitest run`, `pnpm exec tsc --noEmit`.
3. **Prohibido**: `tauri build`, `pnpm build`, empaquetar, `codesign` en cualquier forma, ffmpeg, `std::process::Command::spawn`, `#[allow(clippy::zombie_processes)]` y commits (los hace el orquestador).
4. Las dependencias ya están en `[workspace.dependencies]`. En tu crate se usan como `dep = { workspace = true }`.
5. Los puertos devuelven `PortError`. Para lo que falte, `PortError::NotImplemented`. **Nunca** `todo!()` en caminos de producción.
6. Hay que guardar en engram los descubrimientos que no sean obvios.

### 12.2 Propiedad de carpetas

| WP | Carpetas (escritura) | Entrega |
|---|---|---|
| **WP1** core + audio | `crates/acta-audio/**` · `crates/acta-core/src/recording/{machine,autostop,health}.rs` (**solo la lógica**, los tipos están congelados) · `crates/acta-core/tests/**` | `SegmentWriter`, manifiesto atómico, `recovery`, `concat`/`mixdown`, `levels`, `Watchdog`, `AutoStopPolicy`, supervisor (`SupervisorHandle`, `SupervisorEvent`), `DefaultPipeline` con la fusión de §6.4. Tests con fakes de todos los puertos. |
| **WP2** storage + search | `crates/acta-store/**` | Impl de `MeetingStore`, `GroupStore`, `VoiceprintStore`, `SearchIndex`, `Retriever`, `ChatStore`, `SettingsStore`, `GroupSuggester` sobre `SqliteStore`; timeline derivado; `Embedder` fastembed (feature `local-embeddings`); RRF; clustering. Migraciones nuevas solo como `0002_*.sql`. |
| **WP3** llm | `crates/acta-llm/**` | 4 proveedores + `DefaultProviderFactory`, `MinutesGenerator` (map-reduce si hace falta), `ChatEngine` con citas, `GroupNamer`, `ImportantMomentDetector`. Tests con un `LlmProvider` falso y, para los CLI, con un script falso (`/bin/sh`) que imite la salida JSON. |
| **WP4** sidecar Swift + bridge | `sidecars/acta-mac/**` · `crates/acta-mac-bridge/**` (sin `protocol.rs`, que está congelado) | Swift: process tap (14.2+, AudioCap como referencia) y mic con remuestreo a 16 kHz mono s16, ring buffer, cambio de dispositivo (AirPods), detección (mic por proceso, ventana de Teams por AX, URL de Meet por AppleScript), roster y hablante activo por AX, ScreenCaptureKit + dHash, FluidAudio ASR/diarización/huellas, permisos. Rust: `SidecarClient` supervisado y todos los adaptadores de puertos de la tabla §2. |
| **WP5** UI | `apps/desktop/src/**` salvo `src/api/**` (contrato) · `apps/desktop/package.json` (solo dependencias de UI) | Funcionalidades en `src/features/<feature>/` con patrón contenedor/presentacional: lista de reuniones, detalle (transcript por hablante + reproductor con saltos, escaleta en árbol, timeline, capturas, resumen y "mis tareas"/otros), chat, grupos, ajustes (proveedores, modelos, permisos, autoparada), pastilla (temporizador, 2 medidores, pausa/parar, preguntas expandidas). Todo en castellano de España. Tests con `createMockApi`. |
| **WP6** integración (después de WP1-4) | `apps/desktop/src-tauri/**` | Composición, todos los comandos de §11, `SupervisorEvent` → `emit`, NSPanel de la pastilla (añadir `tauri-nspanel` a **su** Cargo.toml), flujo detección → pregunta, `externalBin` del sidecar + script de copia, Llavero (`keyring`), `llm_cli_detect`. |

### 12.3 Lo que cada WP debe saber de los demás

- **WP1 ↔ WP4**: `AudioEvent::Frame.pts_us` usa el reloj host común a las dos pistas. `Restarted` reinicia el origen de `pts_us`. El bridge decodifica el base64 y entrega `Vec<i16>`.
- **WP1 ↔ WP2**: `save_transcript` reemplaza todo. `index_meeting` se llama después de `save_minutes`.
- **WP1 ↔ WP3**: `MinutesGenerator::generate` recibe utterances en `MediaMs` y devuelve `MinutesOutput` con `CaptureId` ya traducidos.
- **WP5 ↔ WP6**: la UI solo habla con `src/api`. Las preguntas se leen de `snapshot.state` (`detected.prompt` o `recording.stopPrompt`). No hay evento aparte para las preguntas.
- **Todos**: los `Settings` traen valores por defecto (`Settings::default()` ⇄ `defaultSettings()` del mock). Son **neutros**: sin nombre ni alias, idioma `"auto"` y sin ningún proveedor LLM (los pone la bienvenida con `system_profile`, `llm_cli_detect` y `llm_detect_local`). Los ajustes ya guardados se leen tal cual: los valores de fábrica solo afectan a instalaciones nuevas (test `fresh_defaults_contain_no_personal_strings`).

### 12.4 Orden recomendado

WP1 a WP5 en paralelo, y después WP6. Primer hito integrado (fase 1 de `00-analisis.md`): grabación manual con la pastilla, dos pistas a prueba de crash, Parakeet, yo/otros, resumen y tareas con `claude -p`.

---

## 13. Verificación

- `scripts/verify.sh` hace guards, `cargo check`, clippy con `-D warnings`, `cargo test`, `swift build` sin codesign, `tsc` y `vitest`.
- Checklist manual: `docs/02-verificacion-manual.md`.

---

## 14. Integración (WP6) y cambios de contrato

WP6 es ahora dueño de la integración y de los contratos. Cambios hechos (Rust, Swift y TS sincronizados):

| Dónde | Cambio | Por qué |
|---|---|---|
| `llm::ProviderKind::ClaudeCli` | `effort: Option<String>` → `claude -p --effort <nivel>` | WP3 |
| `settings::DetectionSettings` | `denylist: Vec<String>` (bundle ids que nunca disparan; se quitan de `rules` y se filtran los eventos) | WP5 |
| `api::CliDetection` | `claude_env`, `codex_env` (variables del alias con `$HOME` expandido). La detección prueba `zsh -lc` y, si no, `zsh -ic` (lee `~/.zshrc`) | El `claude` del usuario puede ser un alias con `CLAUDE_CONFIG_DIR` |
| `api` | Comandos `secret_set`, `secret_delete` (Llavero, cuenta `provider:<id>`), `models_status`, `models_download`; DTO `ModelStatus`; evento `main:navigate` | WP5 |
| `library::SearchIndex` | `remove_meeting` → `unindex_meeting`; `stale_meetings()` | Choque de nombres con `GroupStore::remove_meeting`; reindexar reuniones cuyo embedding falló |
| `library::MeetingStore` | `save_screen_shares`, `get_capture` | Timeline con `screenShare`; borrar el fichero de una captura |
| `screen::ScreenShare` | Nuevo `{start, end?, presenter?}` en tiempo media | Timeline |
| `layout` | `live/screen.jsonl`, `transcript/segments-<track>.json`, `transcript/diarization-result.json`, `llm/minutes.json`, `capture_rel_path` | Cachés del pipeline para reprocesar pasos sueltos |
| store (migración `0002`) | `meetings.title_edited`, `meetings.index_stale`, tabla `screen_shares`; `save_minutes` aplica `suggested_title` si el usuario no editó el título y conserva `done` de tareas con el mismo texto | WP2 |
| `Protocol.swift` / `protocol.rs` | `init` públicos en todos los structs (adiós a `Wire.make`); constante `speakerEmbeddingModel` / `SPEAKER_EMBEDDING_MODEL` = `fluidaudio-wespeaker-community1-256` | WP4 |
| `SegmentWriter` | `manifest()`, `recorded_ms()`, `meeting_id()`. Al reanudar ya rebasaba el origen de `pts_us` (sin rellenar el salto): test de regresión añadido | WP4 |

Piezas de WP6:

- `acta-audio::supervisor`: actor único (máquina de estados + efectos). Los frames NO pasan por el actor: una tarea los escribe en el writer y al parar se vacía el stream antes de cerrar. Niveles a ≈10 Hz, snapshot a 1 Hz, watchdog con espacio libre (`statvfs`), autoparada, caducidad de "¿Grabar?", señales en vivo a `live/*.jsonl`, capturas renombradas a tiempo media, momentos importantes → `screen.snapshot{keyword}`. Cola de procesado propia: nunca bloquea grabar otra. Al arrancar recupera sesiones a medias y reencola las que quedaron procesando.
- `acta-audio::processing::DefaultPipeline`: pasos con cachés en disco (se puede reprocesar uno suelto). Si falla el resumen se indexa igualmente y la reunión queda `failed` con el paso que falló.
- `apps/desktop/src-tauri`: `AppCore` (lógica de comandos, testeada sin Tauri), comandos finos, NSPanel no activable (`tauri-nspanel`, nivel Status, todos los Spaces, pantalla completa), icono en la barra de menús ("Grabar ahora"), sidecar localizado en `Contents/MacOS/acta-mac` (externalBin solo en `tauri.bundle.conf.json`, porque `tauri-build` falla en `cargo check` si el binario no existe). Sin sidecar la app abre igual y explica el error en cada acción nativa.
- Embeddings: feature `local-embeddings` de la app (la activa `scripts/install-mac.sh`). Sin ella no hay embedder (solo FTS5), para no mezclar vectores de modelos distintos.
- Instalación: `docs/03-instalacion.md`. Verificación manual: `docs/02-verificacion-manual.md`.

---

## 15. Modelo local integrado (opcional)

acta puede descargar un modelo y ejecutarlo ella misma, sin Ollama ni nada instalado. Es **opcional**: se ofrece en el paso de IA de la bienvenida y en Ajustes › Proveedores de IA › «Modelo local integrado (opcional)».

### 15.1 Motor: `llama-server` supervisado, no llama.cpp dentro del proceso

| Opción | Veredicto |
|---|---|
| llama.cpp en proceso (`llama-cpp-2`) | Descartado. Compila llama.cpp con CMake en cada build (en este Mac ni hay `cmake`), mete `unsafe` y C++ en el proceso que graba, y si el modelo se queda sin memoria **muere la app con la grabación en curso**. |
| `llama-server` como proceso hijo | **Elegido.** Binario oficial con Metal, API compatible con OpenAI (se reutiliza el proveedor `openAiCompatible`), `response_format: json_schema` convertido en gramática, y si se cae, se cae solo él. Al pararlo, la memoria vuelve al sistema. |

- acta **no compila ni firma** el binario: descarga el paquete oficial de macOS arm64 de una versión **fijada** de `ggml-org/llama.cpp` (`b12444`, 17 MB) con su SHA-256, y extrae solo `llama-server`, sus `.dylib` y la licencia (`local/runtime.rs`).
- Orden de búsqueda: `ACTA_LLAMA_SERVER` → junto al ejecutable de la app → el descargado → Homebrew (`brew install llama.cpp`, respaldo si la red bloquea GitHub).
- Supervisión (`local/server.rs`): `tokio::process` + `kill_on_drop(true)` y `wait()` en todos los caminos. Escucha solo en `127.0.0.1`, en un puerto libre. Arranca con la primera petición, se para tras 5 min sin uso y al cerrar la app. Su PID se apunta en `runtime/llama/llama-server.pid` y al arrancar se mata el huérfano de un cierre brusco (solo si su línea de comando es un `llama-server`).
- Argumentos: `-m <gguf> --host 127.0.0.1 --port <p> -c <contexto> -ngl 99 -np 1 --reasoning-budget 0 --alias <id> --jinja --no-webui`.

### 15.2 Catálogo (verificado en Hugging Face el 16-nov-2026)

Lógica pura en `acta_core::llm::local`. Cada modelo va fijado a un commit del repositorio y a su SHA-256.

| Nivel | Modelo (repo · fichero) | En disco | Memoria | Contexto | Licencia |
|---|---|---|---|---|---|
| Ligero | `unsloth/Qwen3.5-2B-GGUF` · `Qwen3.5-2B-Q4_K_M.gguf` | 1,28 GB | ≈ 2,5 GB | 16 384 | Apache-2.0 |
| Equilibrado | `unsloth/Qwen3.5-4B-GGUF` · `Qwen3.5-4B-Q4_K_M.gguf` | 2,74 GB | ≈ 4,5 GB | 16 384 | Apache-2.0 |
| Avanzado | `unsloth/Qwen3.5-9B-GGUF` · `Qwen3.5-9B-Q4_K_M.gguf` | 5,68 GB | ≈ 8,5 GB | 24 576 | Apache-2.0 |

### 15.3 Recomendación según el Mac

`sysctl -n hw.memsize machdep.cpu.brand_string hw.optional.arm64` (supervisado) + espacio libre (`statvfs`).

| Memoria | Ligero (mín. 4, holgado 8) | Equilibrado (8 / 16) | Avanzado (16 / 24) | Recomendado |
|---|---|---|---|---|
| < 8 GB | justo | no se ofrece | no se ofrece | Ligero |
| 8 GB | holgado | justo | no se ofrece | Ligero |
| 16 GB | holgado | holgado | justo | Equilibrado |
| ≥ 24 GB (este Mac: M2 Pro, 32 GB) | holgado | holgado | holgado | Avanzado |

- Se recomienda el más capaz que va holgado; si ninguno, el más ligero que funciona. Intel: todos «justo» y se recomienda el ligero.
- Sin disco (tamaño + 2 GB de margen) no se puede descargar. La velocidad estimada sale de la generación (M1…M5) y de la variante (Pro, Max, Ultra); está calibrada con la medida real de este Mac.

### 15.4 Descarga

`local/download.rs`: escribe en `<fichero>.part`, reanuda con `Range`, vuelve a empezar si el servidor lo ignora, y solo renombra al nombre final con el SHA-256 correcto (si no coincide, borra el `.part`). «Pausar» deja lo bajado para reanudar. Primero llama.cpp (si acta no tiene el suyo), después el modelo.

### 15.5 Salida estructurada y transcripts largos

El esquema `acta.minutes.v1` viaja en `response_format` y `llama-server` lo convierte en una gramática: el JSON sale válido a la primera también con el modelo ligero. `capabilities().context_tokens` es el contexto del modelo, así que el map-reduce de §7.1 trocea solo.

---

## 16. Favoritas y limpieza de reuniones antiguas

**Favoritas**: estrella en reuniones y en grupos (fila de la lista, cabecera de la reunión, vista del grupo y ⌘K, con ⌘D). Filtro «Favoritas» en la lista de reuniones y «Favoritos» en la de grupos. La búsqueda acepta el alcance `favourites`. Marcar no cambia `updated_at`.

**Limpieza** (`acta_core::library::cleanup`, determinista, sin LLM). Una reunión se sugiere si cumple todo:

1. Tiene más de `Settings.cleanup.minAgeDays` días (90 por defecto).
2. No tiene estrella, no está en un grupo con estrella, no tiene «mis tareas» abiertas y no se pulsó «Mantener».
3. No se ha abierto (`last_opened_at`: `meeting_get`), ni ha salido en una búsqueda ni se ha usado en el chat (`last_used_at`) en ese mismo plazo.
4. No está grabando ni procesando.

Los motivos que se enseñan: antigüedad, nunca usada o sin usar desde hace N días, corta (< 3 min), poco contenido (< 80 palabras) y fallida.

- La UI enseña un aviso que se puede cerrar («Recordármelo más tarde»: calla una semana, `cleanup.snoozedUntil`) y una hoja de revisión para elegir, mantener o borrar con confirmación.
- `cleanup_delete` borra filas, índice y carpeta (audio, capturas, transcript), dice cuánto libera y **se salta siempre las favoritas** aunque se le pidan. acta nunca borra por su cuenta.
- Ajustes › General › Limpieza: activar o desactivar y elegir la antigüedad.
