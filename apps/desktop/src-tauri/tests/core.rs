#![allow(clippy::unwrap_used, clippy::expect_used)]
//! `AppCore` (la lógica de los comandos) con el store real en un directorio
//! temporal, sidecar "no disponible" y un LLM falso.

use acta_audio::supervisor::SupervisorConfig;
use acta_core::chat::ChatScope;
use acta_core::detection::Platform;
use acta_core::ids::*;
use acta_core::layout::DataLayout;
use acta_core::library::*;
use acta_core::llm::*;
use acta_core::recording::SessionState;
use acta_core::screen::{Capture, CaptureReason};
use acta_core::time::{MediaMs, SystemClock, UnixMs};
use acta_core::library::cleanup::DAY_MS;
use acta_core::{EventStream, PortError, PortResult};
use acta_desktop_lib::app::{AppCore, CoreDeps, Emit};
use acta_desktop_lib::native::NativePorts;
use acta_store::SqliteStore;
use acta_store::secrets::{InMemorySecretStore, SecretStore};
use async_trait::async_trait;
use std::sync::{Arc, Mutex};
use std::time::Duration;

mod support;

#[derive(Default)]
struct Events(Mutex<Vec<(String, serde_json::Value)>>);
impl Emit for Events {
    fn emit(&self, event: &str, payload: serde_json::Value) {
        self.0.lock().unwrap().push((event.to_owned(), payload));
    }
}

struct EchoProvider(ProviderId);
#[async_trait]
impl LlmProvider for EchoProvider {
    fn id(&self) -> &ProviderId {
        &self.0
    }
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities { streaming: true, native_json_schema: false, context_tokens: 100_000 }
    }
    async fn complete(&self, _r: LlmRequest) -> PortResult<LlmResponse> {
        Ok(LlmResponse { text: "ok".into(), json: None, model: Some("echo".into()), usage: Usage::default() })
    }
    async fn stream(&self, _r: LlmRequest) -> PortResult<EventStream<PortResult<String>>> {
        Ok(Box::pin(futures::stream::iter(vec![Ok("Se decidió ".to_string()), Ok("el plan.".to_string())])))
    }
}

struct EchoFactory;
impl LlmProviderFactory for EchoFactory {
    fn build(&self, config: &ProviderConfig) -> PortResult<Arc<dyn LlmProvider>> {
        Ok(Arc::new(EchoProvider(config.id.clone())))
    }
}

struct Rig {
    _dir: tempfile::TempDir,
    layout: DataLayout,
    core: AppCore,
    events: Arc<Events>,
    secrets: Arc<InMemorySecretStore>,
}

async fn rig() -> Rig {
    rig_with(NativePorts::unavailable("tests")).await
}

async fn rig_with(native: NativePorts) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let layout = DataLayout::new(dir.path());
    let store = Arc::new(SqliteStore::open(&layout.db_path()).unwrap().with_layout(layout.clone()));
    let events = Arc::new(Events::default());
    let secrets = Arc::new(InMemorySecretStore::default());
    let core = AppCore::start(CoreDeps {
        layout: layout.clone(),
        clock: Arc::new(SystemClock),
        store,
        secrets: secrets.clone(),
        factory: Some(Arc::new(EchoFactory)),
        local_llm: None,
        native,
        emitter: events.clone(),
        supervisor_config: SupervisorConfig { tick: Duration::from_millis(30), stop_drain: Duration::from_millis(200) },
    })
    .await
    .unwrap();
    // Una instalación ya configurada (los ajustes de fábrica van sin proveedores).
    core.settings_update(support::configured_settings()).await.unwrap();
    Rig { _dir: dir, layout, core, events, secrets }
}

async fn meeting(core: &AppCore, n: u128) -> MeetingId {
    let id = MeetingId::from_u128(n);
    core.store
        .create_meeting(NewMeeting {
            id,
            title: "Daily".into(),
            platform: Platform::Teams,
            app_bundle_id: None,
            started_at: UnixMs(1_700_000_000_000),
            language: "es".into(),
        })
        .await
        .unwrap();
    id
}

#[tokio::test]
async fn api_keys_go_to_the_secret_store_never_to_settings() {
    let r = rig().await;
    let sref = r.core.secret_set("openrouter".into(), "  sk-123 ".into()).await.unwrap();
    assert_eq!((sref.service.as_str(), sref.account.as_str()), ("acta", "provider:openrouter"));
    assert_eq!(r.secrets.get(&sref).unwrap().as_deref(), Some("sk-123"));
    assert!(!serde_json::to_string(&r.core.settings_get().await.unwrap()).unwrap().contains("sk-123"));
    r.core.secret_delete("openrouter".into()).await.unwrap();
    assert_eq!(r.secrets.get(&sref).unwrap(), None);
    assert!(matches!(r.core.secret_set("x".into(), " ".into()).await, Err(PortError::InvalidInput(_))));
}

#[tokio::test]
async fn settings_update_persists_and_is_used() {
    let r = rig().await;
    let mut s = r.core.settings_get().await.unwrap();
    s.detection.denylist = vec!["com.example.dictation".into()];
    if let ProviderKind::ClaudeCli { effort, extra_env, .. } = &mut s.llm.providers[0].kind {
        *effort = Some("high".into());
        extra_env.insert("CLAUDE_CONFIG_DIR".into(), "/Users/e/.claude-otro".into());
    }
    r.core.settings_update(s.clone()).await.unwrap();
    use acta_core::library::SettingsStore;
    assert_eq!(r.core.store.load_settings().await.unwrap(), s);
    assert_eq!(r.core.settings_get().await.unwrap(), s);
}

#[tokio::test]
async fn deleting_a_meeting_removes_rows_and_folder() {
    let r = rig().await;
    let id = meeting(&r.core, 1).await;
    let dir = r.layout.meeting_dir(id);
    std::fs::create_dir_all(dir.join("audio")).unwrap();
    std::fs::write(dir.join("audio/mix.wav"), b"x").unwrap();
    r.core.meeting_delete(id).await.unwrap();
    assert!(!dir.exists());
    assert!(matches!(r.core.meeting_get(id).await, Err(PortError::NotFound(_))));
}

#[tokio::test]
async fn deleting_a_capture_removes_its_file() {
    let r = rig().await;
    let id = meeting(&r.core, 2).await;
    let rel = "captures/000001000-manual.jpg";
    std::fs::create_dir_all(r.layout.captures_dir(id)).unwrap();
    std::fs::write(r.layout.meeting_dir(id).join(rel), b"jpg").unwrap();
    let cid = CaptureId::from_u128(9);
    r.core
        .store
        .add_capture(Capture {
            id: cid,
            meeting_id: id,
            at: MediaMs(1_000),
            at_wall: UnixMs(1),
            path: rel.into(),
            reason: CaptureReason::Manual,
            phash: "0".repeat(16),
            width: 1,
            height: 1,
            keyword: None,
            topic_path: None,
            action_item_ids: vec![],
        })
        .await
        .unwrap();
    r.core.capture_delete(cid).await.unwrap();
    assert!(!r.layout.meeting_dir(id).join(rel).exists());
    assert!(r.core.store.list_captures(id).await.unwrap().is_empty());
}

#[tokio::test]
async fn renaming_a_meeting_announces_it() {
    let r = rig().await;
    let id = meeting(&r.core, 3).await;
    let m = r.core.meeting_update(id, "  Retro  ".into()).await.unwrap();
    assert_eq!(m.title, "Retro");
    assert!(r.events.0.lock().unwrap().iter().any(|(n, p)| n == "meeting:updated" && p["title"] == "Retro"));
    assert!(matches!(r.core.meeting_update(id, " ".into()).await, Err(PortError::InvalidInput(_))));
}

#[tokio::test]
async fn the_personal_note_is_saved_trimmed_cleared_when_blank_and_announced() {
    let r = rig().await;
    let id = meeting(&r.core, 3).await;
    let m = r.core.meeting_set_note(id, "  Pedir descuento  ".into()).await.unwrap();
    assert_eq!(m.user_note.as_deref(), Some("Pedir descuento"));
    assert!(r.events.0.lock().unwrap().iter().any(|(n, _)| n == "meeting:updated"));
    let detail = r.core.meeting_get(id).await.unwrap();
    assert_eq!(detail.meeting.user_note.as_deref(), Some("Pedir descuento"));
    assert_eq!(r.core.meeting_set_note(id, "   ".into()).await.unwrap().user_note, None);
    assert!(matches!(r.core.meeting_set_note(MeetingId::from_u128(999), "x".into()).await, Err(PortError::NotFound(_))));
}

#[test]
fn the_note_command_is_in_the_contract() {
    assert_eq!(acta_core::api::command_names::MEETING_SET_NOTE, "meeting_set_note");
}

#[tokio::test]
async fn chat_streams_deltas_and_saves_both_messages() {
    let r = rig().await;
    let id = meeting(&r.core, 4).await;
    let thread = r.core.chat_thread_create(ChatScope::Meeting { meeting_id: id }).await.unwrap();
    assert_eq!(thread.title, "Daily");
    let res = r.core.chat_send(thread.id, "¿Qué se decidió?".into(), None, None).await.unwrap();
    for _ in 0..100 {
        if r.core.chat_messages_list(thread.id).await.unwrap().len() == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let msgs = r.core.chat_messages_list(thread.id).await.unwrap();
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[1].id, res.assistant_message_id);
    assert!(msgs[1].content.contains("el plan"));
    let deltas: Vec<_> = r.events.0.lock().unwrap().iter().filter(|(n, _)| n == "chat:delta").cloned().collect();
    assert!(deltas.last().is_some_and(|(_, p)| p["done"] == true));
}

#[tokio::test]
async fn provider_test_and_unknown_provider() {
    let r = rig().await;
    let ok = r.core.llm_provider_test("claude-cli".into()).await.unwrap();
    assert!(ok.ok, "{}", ok.message);
    assert!(matches!(r.core.llm_provider_test("nope".into()).await, Err(PortError::NotFound(_))));
}

#[tokio::test]
async fn cli_detection_reads_alias_env_from_an_interactive_shell() {
    let mut r = rig().await;
    let dir = tempfile::tempdir().unwrap();
    let shell = dir.path().join("zsh");
    std::fs::write(
        &shell,
        "#!/bin/sh\nif [ \"$1\" = \"-ic\" ]; then case \"$2\" in *claude*) echo \"alias claude='CLAUDE_CONFIG_DIR=\\\"\\$HOME/.claude-personal\\\" /opt/homebrew/bin/claude'\";; *) exit 1;; esac; else exit 1; fi\n",
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o755)).unwrap();
    r.core.shell = shell;
    r.core.home = "/Users/edu".into();
    let d = r.core.llm_cli_detect().await.unwrap();
    assert_eq!(d.claude.as_deref(), Some("/opt/homebrew/bin/claude"));
    assert_eq!(d.claude_env.get("CLAUDE_CONFIG_DIR").map(String::as_str), Some("/Users/edu/.claude-personal"));
    assert_eq!(d.codex, None);
}

#[tokio::test]
async fn without_the_sidecar_recording_fails_cleanly_and_library_still_works() {
    let r = rig().await;
    let snap = r.core.session_start(None).await.unwrap();
    // El audio no arranca → se para con error y se intenta procesar lo que haya.
    assert!(!matches!(snap.state, SessionState::Recording(_)), "{:?}", snap.state);
    let mut failed = false;
    for _ in 0..200 {
        if matches!(r.core.session_get_state().await.unwrap().state, SessionState::Failed { .. }) {
            failed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(failed, "acaba en failed");
    let page = r.core.meetings_list(MeetingQuery::default()).await.unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].status, MeetingStatus::Failed);
    assert!(matches!(r.core.models_status().await, Err(PortError::Unavailable(_))));
    assert!(matches!(r.core.models_download("bogus".into()).await, Err(PortError::InvalidInput(_))));
    assert!(r.core.app_info().data_dir.ends_with(r.layout.root().file_name().unwrap().to_str().unwrap()));
}

#[tokio::test]
async fn audio_inputs_need_the_sidecar_and_serialize_in_camel_case() {
    let r = rig().await;
    assert!(matches!(r.core.audio_inputs_list().await, Err(PortError::Unavailable(_))));
    let dto = acta_core::api::AudioInput { uid: "u".into(), name: "Logitech StreamCam".into(), is_default: false, transport: "usb".into() };
    assert_eq!(serde_json::to_value(&dto).unwrap(), serde_json::json!({"uid":"u","name":"Logitech StreamCam","isDefault":false,"transport":"usb"}));
    assert_eq!(acta_core::api::command_names::AUDIO_INPUTS_LIST, "audio_inputs_list");
}

// ---------------------------------------------------------------------------
// Permisos antes de grabar
// ---------------------------------------------------------------------------

use acta_core::permissions::{PermissionKind, PermissionState, PermissionStatus, PermissionsProbe};

/// TCC de mentira: estado inicial por permiso y qué contesta el usuario al diálogo.
struct FakeTcc {
    status: Vec<(PermissionKind, PermissionStatus)>,
    answer: PermissionStatus,
    requested: Mutex<Vec<PermissionKind>>,
}
#[async_trait]
impl PermissionsProbe for FakeTcc {
    async fn status(&self) -> PortResult<Vec<PermissionState>> {
        Ok(self.status.iter().map(|(kind, status)| PermissionState { kind: *kind, status: *status, target: None }).collect())
    }
    async fn request(&self, kind: PermissionKind) -> PortResult<PermissionStatus> {
        self.requested.lock().unwrap().push(kind);
        Ok(self.answer)
    }
}

async fn rig_with_tcc(status: Vec<(PermissionKind, PermissionStatus)>, answer: PermissionStatus) -> (Rig, Arc<FakeTcc>) {
    let tcc = Arc::new(FakeTcc { status, answer, requested: Mutex::default() });
    let mut native = NativePorts::unavailable("tests");
    native.permissions = tcc.clone();
    (rig_with(native).await, tcc)
}

/// Tras reinstalar, macOS vuelve a preguntar por el micro. La captura se
/// quedaba bloqueada en ese diálogo con la pastilla en «Grabando 00:00» y la
/// reunión acababa vacía: hay que preguntar ANTES de empezar la sesión.
#[tokio::test]
async fn recording_asks_for_undetermined_permissions_before_starting() {
    use PermissionKind::*;
    let (r, tcc) = rig_with_tcc(
        vec![(Microphone, PermissionStatus::NotDetermined), (SystemAudio, PermissionStatus::NotDetermined)],
        PermissionStatus::Denied,
    )
    .await;
    let err = r.core.session_start(None).await.unwrap_err();
    assert!(matches!(&err, PortError::PermissionDenied(m) if m.contains("Micrófono")), "{err:?}");
    assert_eq!(tcc.requested.lock().unwrap().clone(), vec![Microphone]);
    assert_eq!(r.core.session_get_state().await.unwrap().state, SessionState::Idle, "no se llega a empezar");
    assert!(r.core.meetings_list(MeetingQuery::default()).await.unwrap().items.is_empty());
}

#[tokio::test]
async fn granted_dialogs_let_the_recording_start() {
    use PermissionKind::*;
    let (r, tcc) = rig_with_tcc(
        vec![(Microphone, PermissionStatus::NotDetermined), (SystemAudio, PermissionStatus::NotDetermined)],
        PermissionStatus::Granted,
    )
    .await;
    // El audio es "no disponible" en este banco: basta con ver que se pasó de los permisos.
    let _ = r.core.session_start(None).await;
    assert_eq!(tcc.requested.lock().unwrap().clone(), vec![Microphone, SystemAudio]);
    assert_eq!(r.core.meetings_list(MeetingQuery::default()).await.unwrap().items.len(), 1);
}

#[tokio::test]
async fn a_denied_permission_fails_fast_and_granted_ones_never_prompt() {
    use PermissionKind::*;
    let (r, tcc) =
        rig_with_tcc(vec![(Microphone, PermissionStatus::Granted), (SystemAudio, PermissionStatus::Denied)], PermissionStatus::Granted).await;
    let err = r.core.session_start(None).await.unwrap_err();
    assert!(matches!(&err, PortError::PermissionDenied(m) if m.contains("Audio del sistema")), "{err:?}");
    assert!(tcc.requested.lock().unwrap().is_empty());

    let (r, tcc) =
        rig_with_tcc(vec![(Microphone, PermissionStatus::Granted), (SystemAudio, PermissionStatus::Unknown)], PermissionStatus::Denied).await;
    let _ = r.core.session_start(None).await;
    assert!(tcc.requested.lock().unwrap().is_empty());
    assert_eq!(r.core.meetings_list(MeetingQuery::default()).await.unwrap().items.len(), 1);
}

/// Los comandos de la pantalla de proveedores también están en el contrato.
#[test]
fn provider_ui_commands_are_in_the_contract() {
    use acta_core::api::command_names as c;
    assert_eq!(c::LLM_LIST_MODELS, "llm_list_models");
    assert_eq!(c::LLM_LIST_OPENCODE_MODELS, "llm_list_opencode_models");
    assert_eq!(c::LLM_DETECT_LOCAL, "llm_detect_local");
    assert_eq!(c::OPEN_EXTERNAL_URL, "open_external_url");
}

// ----------------------------------------------------------------------
// Favoritas y limpieza de reuniones antiguas
// ----------------------------------------------------------------------

/// Reunión de hace tres años (la de `meeting`) con `bytes` de audio en disco.
async fn old_meeting(r: &Rig, n: u128, bytes: usize) -> MeetingId {
    let id = meeting(&r.core, n).await;
    r.core
        .store
        .patch_meeting(id, MeetingPatch { status: Some(MeetingStatus::Ready), ..MeetingPatch::default() })
        .await
        .unwrap();
    let dir = r.layout.meeting_dir(id);
    std::fs::create_dir_all(dir.join("audio")).unwrap();
    std::fs::write(dir.join("audio/mix.wav"), vec![0u8; bytes]).unwrap();
    id
}

#[tokio::test]
async fn stars_on_meetings_and_groups_persist() {
    let r = rig().await;
    let id = meeting(&r.core, 20).await;
    assert!(r.core.meeting_set_favourite(id, true).await.unwrap().favourite);
    assert!(r.events.0.lock().unwrap().iter().any(|(n, _)| n == "meeting:updated"));
    let page = r.core.meetings_list(MeetingQuery { favourites_only: true, ..MeetingQuery::default() }).await.unwrap();
    assert_eq!(page.items.len(), 1);
    let g = r.core.group_create("Proyecto".into(), None).await.unwrap();
    assert!(r.core.group_set_favourite(g.id, true).await.unwrap().favourite);
    assert!(matches!(r.core.meeting_set_favourite(MeetingId::from_u128(999), true).await, Err(PortError::NotFound(_))));
}

#[tokio::test]
async fn old_unused_meetings_are_suggested_with_their_size_and_never_the_protected_ones() {
    let r = rig().await;
    let plain = old_meeting(&r, 30, 1_000).await;
    let starred = old_meeting(&r, 31, 500).await;
    r.core.meeting_set_favourite(starred, true).await.unwrap();
    let in_group = old_meeting(&r, 32, 500).await;
    let g = r.core.group_create("G".into(), None).await.unwrap();
    r.core.group_add_meeting(g.id, in_group).await.unwrap();
    r.core.group_set_favourite(g.id, true).await.unwrap();
    let opened = old_meeting(&r, 33, 500).await;
    r.core.meeting_get(opened).await.unwrap(); // abrirla hoy la protege

    let s = r.core.cleanup_suggestions().await.unwrap();
    assert_eq!(s.candidates.iter().map(|c| c.meeting_id).collect::<Vec<_>>(), vec![plain]);
    assert_eq!((s.candidates[0].bytes, s.total_bytes, s.min_age_days), (1_000, 1_000, 90));
    assert!(s.candidates[0].reasons.contains(&CleanupReason::NeverUsed));

    // «Mantener» la quita para siempre; apagar las sugerencias las vacía.
    r.core.cleanup_keep(vec![plain]).await.unwrap();
    assert!(r.core.cleanup_suggestions().await.unwrap().candidates.is_empty());
    let another = old_meeting(&r, 34, 10).await;
    assert_eq!(r.core.cleanup_suggestions().await.unwrap().candidates[0].meeting_id, another);
    let mut settings = r.core.settings_get().await.unwrap();
    settings.cleanup.enabled = false;
    r.core.settings_update(settings).await.unwrap();
    assert!(r.core.cleanup_suggestions().await.unwrap().candidates.is_empty());
}

#[tokio::test]
async fn the_configured_age_decides_what_counts_as_old() {
    let r = rig().await;
    let id = MeetingId::from_u128(40);
    let ten_days_ago = UnixMs(acta_core::time::Clock::now(&SystemClock).0 - 10 * DAY_MS);
    r.core
        .store
        .create_meeting(NewMeeting { id, title: "Reciente".into(), platform: Platform::Teams, app_bundle_id: None, started_at: ten_days_ago, language: "es".into() })
        .await
        .unwrap();
    r.core.store.patch_meeting(id, MeetingPatch { status: Some(MeetingStatus::Ready), ..MeetingPatch::default() }).await.unwrap();
    assert!(r.core.cleanup_suggestions().await.unwrap().candidates.is_empty());
    let mut settings = r.core.settings_get().await.unwrap();
    settings.cleanup.min_age_days = 7;
    r.core.settings_update(settings).await.unwrap();
    let s = r.core.cleanup_suggestions().await.unwrap();
    assert_eq!((s.candidates.len(), s.min_age_days), (1, 7));
}

#[tokio::test]
async fn deleting_selected_meetings_frees_disk_and_never_touches_favourites() {
    let r = rig().await;
    let a = old_meeting(&r, 50, 4_000).await;
    let b = old_meeting(&r, 51, 6_000).await;
    let starred = old_meeting(&r, 52, 9_000).await;
    r.core.meeting_set_favourite(starred, true).await.unwrap();
    let ghost = MeetingId::from_u128(59);

    let res = r.core.cleanup_delete(vec![a, b, starred, ghost]).await.unwrap();
    assert_eq!(res.deleted, vec![a, b]);
    assert_eq!(res.skipped, vec![starred, ghost]);
    assert_eq!(res.freed_bytes, 10_000);
    assert!(!r.layout.meeting_dir(a).exists() && !r.layout.meeting_dir(b).exists());
    assert!(r.layout.meeting_dir(starred).join("audio/mix.wav").exists());
    assert!(matches!(r.core.meeting_get(a).await, Err(PortError::NotFound(_))));
    assert!(r.core.meeting_get(starred).await.is_ok());
}

#[tokio::test]
async fn searching_marks_the_meetings_it_finds_as_used() {
    use acta_core::recording::Track;
    use acta_core::transcription::{NameSource, Speaker, Transcript, Utterance};
    let r = rig().await;
    let id = old_meeting(&r, 60, 10).await;
    let sp = SpeakerId::from_u128(1);
    r.core
        .store
        .save_transcript(Transcript {
            meeting_id: id,
            speakers: vec![Speaker { id: sp, meeting_id: id, label: "Yo".into(), display_name: None, is_me: true, track: Track::Mic, diarization_label: None, name_source: NameSource::Me, confirmed: false, voiceprint_id: None }],
            utterances: vec![Utterance { idx: 0, speaker_id: sp, track: Track::Mic, start: MediaMs(0), end: MediaMs(900), text: "revisamos el presupuesto".into(), confidence: None }],
        })
        .await
        .unwrap();
    r.core.store.index_meeting(id).await.unwrap();
    assert_eq!(r.core.cleanup_suggestions().await.unwrap().candidates.len(), 1);
    let hits = r.core.search(SearchQuery { text: "presupuesto".into(), scope: SearchScope::All, limit: 5 }).await.unwrap();
    assert_eq!(hits.len(), 1);
    assert!(r.core.cleanup_suggestions().await.unwrap().candidates.is_empty(), "usada hoy: ya no se sugiere");
}

#[test]
fn favourites_and_cleanup_commands_are_in_the_contract() {
    use acta_core::api::command_names as c;
    assert_eq!(
        [c::MEETING_SET_FAVOURITE, c::GROUP_SET_FAVOURITE, c::CLEANUP_SUGGESTIONS, c::CLEANUP_DELETE, c::CLEANUP_KEEP],
        ["meeting_set_favourite", "group_set_favourite", "cleanup_suggestions", "cleanup_delete", "cleanup_keep"]
    );
}

// ----------------------------------------------------------------------
// Modelo local integrado
// ----------------------------------------------------------------------

use acta_core::llm::local::{GIB, HardwareInfo, LocalDownloadStage, ModelFit, RuntimeSource};
use acta_llm::local::{LocalLlm, LocalLlmConfig};

/// `llama-server` de mentira (Perl): 200 en `/health` y una respuesta fija de chat.
const FAKE_LLAMA_SERVER: &str = r#"#!/usr/bin/perl
use strict; use IO::Socket::INET;
my $port = 0;
for (my $i = 0; $i < @ARGV; $i++) { $port = $ARGV[$i + 1] if $ARGV[$i] eq '--port'; }
my $srv = IO::Socket::INET->new(LocalAddr => '127.0.0.1', LocalPort => $port, Listen => 16, ReuseAddr => 1) or die "bind: $!";
while (my $c = $srv->accept) {
  my ($head, $len) = ('', 0);
  while (my $line = <$c>) { $head .= $line; $len = $1 if $line =~ /^content-length:\s*(\d+)/i; last if $line =~ /^\r?\n$/; }
  my $body = ''; read($c, $body, $len) if $len > 0;
  my $out = $head =~ m{^GET /health} ? '{"status":"ok"}'
    : '{"model":"qwen-local","choices":[{"message":{"role":"assistant","content":"ok"}}],"usage":{"prompt_tokens":3,"completion_tokens":1}}';
  print $c "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: " . length($out) . "\r\nConnection: close\r\n\r\n$out";
  close($c);
}
"#;

const TINY: &str = "qwen3.5-2b-q4_k_m";

/// Core con un modelo "ya descargado" y un llama-server falso (sin red).
async fn rig_with_local_model() -> Rig {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let layout = DataLayout::new(dir.path());
    let server = dir.path().join("fake/llama-server");
    std::fs::create_dir_all(server.parent().unwrap()).unwrap();
    std::fs::write(&server, FAKE_LLAMA_SERVER).unwrap();
    std::fs::set_permissions(&server, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut cfg = LocalLlmConfig::for_layout(&layout);
    cfg.search.env_override = Some(server);
    cfg.search.exe_dir = None;
    cfg.search.homebrew = vec![];
    std::fs::create_dir_all(&cfg.models_dir).unwrap();
    std::fs::write(cfg.models_dir.join("Qwen3.5-2B-Q4_K_M.gguf"), b"gguf de mentira").unwrap();

    let store = Arc::new(SqliteStore::open(&layout.db_path()).unwrap().with_layout(layout.clone()));
    let events = Arc::new(Events::default());
    let secrets = Arc::new(InMemorySecretStore::default());
    let core = AppCore::start(CoreDeps {
        layout: layout.clone(),
        clock: Arc::new(SystemClock),
        store,
        secrets: secrets.clone(),
        factory: None,
        local_llm: Some(Arc::new(LocalLlm::new(cfg))),
        native: NativePorts::unavailable("tests"),
        emitter: events.clone(),
        supervisor_config: SupervisorConfig { tick: Duration::from_millis(30), stop_drain: Duration::from_millis(200) },
    })
    .await
    .unwrap();
    // Una instalación ya configurada (los ajustes de fábrica van sin proveedores).
    core.settings_update(support::configured_settings()).await.unwrap();
    Rig { _dir: dir, layout, core, events, secrets }
}

#[tokio::test]
async fn local_model_status_reads_this_mac_and_offers_three_tiers() {
    let r = rig().await;
    let s = r.core.local_llm_status().await.unwrap();
    assert!(s.hardware.ram_bytes >= GIB, "{:?}", s.hardware);
    assert!(!s.hardware.chip.is_empty());
    assert!(s.hardware.free_disk_bytes.is_some());
    assert_eq!(s.models.len(), 3);
    assert!(s.models.iter().all(|m| !m.downloaded));
    assert_eq!(s.runtime.pinned_version, "b12444");
}

#[tokio::test]
async fn a_model_that_does_not_fit_this_mac_is_never_downloaded() {
    let mut r = rig().await;
    r.core.hardware_override =
        Some(HardwareInfo { ram_bytes: 6 * GIB, chip: "Apple M1".into(), apple_silicon: true, free_disk_bytes: Some(100 * GIB) });
    let s = r.core.local_llm_status().await.unwrap();
    assert_eq!(s.models.iter().map(|m| m.fit).collect::<Vec<_>>(), [ModelFit::Tight, ModelFit::TooBig, ModelFit::TooBig]);
    assert_eq!(s.recommended.as_deref(), Some(TINY));
    let err = r.core.local_llm_download("qwen3.5-9b-q4_k_m".into()).await.unwrap_err();
    assert!(matches!(&err, PortError::InvalidInput(m) if m.contains("memoria")), "{err}");
    // Y sin disco tampoco.
    r.core.hardware_override =
        Some(HardwareInfo { ram_bytes: 32 * GIB, chip: "Apple M2 Pro".into(), apple_silicon: true, free_disk_bytes: Some(GIB) });
    let err = r.core.local_llm_download(TINY.into()).await.unwrap_err();
    assert!(matches!(&err, PortError::InvalidInput(m) if m.contains("espacio")), "{err}");
    assert!(matches!(r.core.local_llm_download("nope".into()).await, Err(PortError::NotFound(_))));
}

#[tokio::test]
async fn the_built_in_model_downloads_tests_runs_as_a_provider_and_deletes() {
    let r = rig_with_local_model().await;
    let s = r.core.local_llm_status().await.unwrap();
    assert!(s.models[0].downloaded);
    assert_eq!(s.runtime.source, Some(RuntimeSource::Env));

    // «Descargar» con todo ya en disco termina enseguida y lo anuncia.
    r.core.local_llm_download(TINY.into()).await.unwrap();
    let mut done = None;
    for _ in 0..200 {
        done = r.events.0.lock().unwrap().iter().find(|(n, p)| n == "localmodel:download" && p["done"] == true).map(|(_, p)| p.clone());
        if done.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let done = done.expect("evento final");
    assert_eq!(done["modelId"], TINY);
    assert_eq!(done["stage"], serde_json::to_value(LocalDownloadStage::Done).unwrap());
    assert!(done["error"].is_null());

    // «Probar» arranca el servidor y responde.
    let t = r.core.local_llm_test(TINY.into()).await.unwrap();
    assert!(t.ok, "{}", t.message);
    assert!(t.message.contains("qwen-local"));
    assert_eq!(r.core.local_llm_status().await.unwrap().running.as_deref(), Some(TINY));

    // Como proveedor de Ajustes pasa por la fábrica de verdad.
    let mut settings = r.core.settings_get().await.unwrap();
    settings.llm.providers.push(ProviderConfig {
        id: "builtin".into(),
        label: "Modelo local integrado".into(),
        enabled: true,
        kind: ProviderKind::BuiltIn { model_id: TINY.into() },
        timeout_s: 30,
    });
    r.core.settings_update(settings).await.unwrap();
    assert!(r.core.llm_provider_test("builtin".into()).await.unwrap().ok);

    r.core.local_llm_delete(TINY.into()).await.unwrap();
    let s = r.core.local_llm_status().await.unwrap();
    assert!(!s.models[0].downloaded && s.running.is_none());
    let t = r.core.local_llm_test(TINY.into()).await.unwrap();
    assert!(!t.ok && t.message.contains("no está descargado"), "{}", t.message);
    r.core.shutdown().await;
}

#[test]
fn local_model_commands_and_event_are_in_the_contract() {
    use acta_core::api::{command_names as c, event_names as e};
    assert_eq!(
        [c::LOCAL_LLM_STATUS, c::LOCAL_LLM_DOWNLOAD, c::LOCAL_LLM_CANCEL, c::LOCAL_LLM_DELETE, c::LOCAL_LLM_TEST],
        ["local_llm_status", "local_llm_download", "local_llm_cancel", "local_llm_delete", "local_llm_test"]
    );
    assert_eq!(e::LOCAL_LLM_DOWNLOAD, "localmodel:download");
}
