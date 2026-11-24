//! # acta-desktop
//!
//! Raíz de composición:
//! 1. `DataLayout` en `~/Library/Application Support/acta/` y logs en `logs/acta.log`.
//! 2. `SqliteStore` (acta-store) + Llavero (`KeychainSecretStore`), sidecar
//!    `acta-mac` + adaptadores (acta-mac-bridge), proveedores LLM (acta-llm),
//!    supervisor + `DefaultPipeline` (acta-audio). Todo en [`app::AppCore`].
//! 3. Comandos Tauri (`commands`) → `AppCore`.
//! 4. `SupervisorEvent` → `app.emit(event_names::*)` y visibilidad de la pastilla.
//! 5. Ventana `overlay` convertida a NSPanel no activable (tauri-nspanel),
//!    visible en todos los Spaces y sobre pantalla completa.
//! 6. Icono en la barra de menús con "Grabar ahora".
//!
//! Procesos hijo: SOLO vía acta-mac-bridge / acta-llm (supervisados). Para
//! abrir URLs/Ajustes del sistema, `tauri-plugin-opener`.
#![deny(clippy::zombie_processes)]

pub mod app;
mod commands;
pub mod events;
pub mod export;
pub mod glass;
pub mod llm;
pub mod native;
pub mod overlay;
pub mod pipeline;
pub mod system;
pub mod instance;
mod tray;
pub mod unavailable;

use acta_audio::supervisor::{SupervisorConfig, SupervisorEvent};
use acta_core::layout::{APP_DIR_NAME, DataLayout};
use acta_core::library::MeetingStatus;
use acta_core::time::SystemClock;
use acta_core::PortResult;
use acta_store::SqliteStore;
use acta_store::secrets::KeychainSecretStore;
use app::{AppCore, CoreDeps, Emit};
use commands::Core;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Emitter, Manager, RunEvent, WindowEvent};

/// `~/Library/Application Support/acta`, o `ACTA_DATA_DIR` si está definida.
pub fn data_layout() -> Option<DataLayout> {
    system::data_root(std::env::var_os(system::DATA_DIR_ENV), dirs::data_dir()).map(DataLayout::new)
}

fn init_logging(layout: &DataLayout) -> Option<tracing_appender::non_blocking::WorkerGuard> {
    use tracing_subscriber::prelude::*;
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let file = tracing_appender::rolling::never(layout.logs_dir(), "acta.log");
    let (writer, guard) = tracing_appender::non_blocking(file);
    let res = tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().with_writer(writer).with_ansi(false))
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
        .try_init();
    res.ok().map(|()| guard)
}

struct TauriEmit(AppHandle);

impl Emit for TauriEmit {
    fn emit(&self, event: &str, payload: serde_json::Value) {
        if let Err(e) = self.0.emit(event, payload) {
            tracing::warn!("emit {event}: {e}");
        }
    }
}

async fn build_core(layout: DataLayout, handle: AppHandle) -> PortResult<AppCore> {
    let store = SqliteStore::open(&layout.db_path())?.with_layout(layout.clone());
    #[cfg(feature = "local-embeddings")]
    let store = store.with_embedder(Arc::new(acta_store::embeddings::FastEmbedder::new(
        layout.models_dir().join("fastembed"),
    )));
    let native = native::start(&layout).await;
    if let Some(e) = &native.error {
        tracing::error!("sidecar acta-mac no disponible: {e}");
    }
    AppCore::start(CoreDeps {
        layout,
        clock: Arc::new(SystemClock),
        store: Arc::new(store),
        secrets: Arc::new(KeychainSecretStore::new()),
        factory: None,
        local_llm: None,
        native,
        emitter: Arc::new(TauriEmit(handle)),
        supervisor_config: SupervisorConfig::default(),
    })
    .await
}

fn set_overlay(handle: &AppHandle, visible: bool) {
    #[cfg(target_os = "macos")]
    overlay::panel::set_visible(handle, visible);
    #[cfg(not(target_os = "macos"))]
    if let Some(w) = handle.get_webview_window("overlay") {
        let _ = if visible { w.show() } else { w.hide() };
    }
}

/// Supervisor → eventos Tauri, pastilla y tareas tras procesar.
fn forward_events(handle: AppHandle, core: Core) {
    let mut rx = core.supervisor.subscribe();
    tauri::async_runtime::spawn(async move {
        loop {
            let ev = match rx.recv().await {
                Ok(ev) => ev,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    tracing::debug!("{n} eventos de sesión descartados");
                    continue;
                }
                Err(_) => break,
            };
            let (name, payload) = events::to_tauri(&ev);
            if let Err(e) = handle.emit(name, payload) {
                tracing::warn!("emit {name}: {e}");
            }
            match &ev {
                SupervisorEvent::Snapshot(s) => {
                    set_overlay(&handle, overlay::overlay_visible(&s.state));
                    tray::update(&handle, s);
                }
                SupervisorEvent::MeetingUpdated(m) if m.status == MeetingStatus::Recording => {
                    let warm = core.clone();
                    tauri::async_runtime::spawn(async move { warm.warm_models().await });
                    // Teams tarda en poner el nombre de la reunión en su ventana.
                    let (core, id) = (core.clone(), m.meeting_id);
                    tauri::async_runtime::spawn(async move {
                        for wait in [5u64, 55, 180] {
                            tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
                            let app = match core.session_get_state().await.map(|s| s.state) {
                                Ok(acta_core::recording::SessionState::Recording(r)) if r.meeting_id == id => r.app,
                                Ok(acta_core::recording::SessionState::Paused { recording, .. }) if recording.meeting_id == id => recording.app,
                                _ => None,
                            };
                            match core.resolve_title(id, app.as_ref()).await {
                                Ok(Some(t)) => {
                                    tracing::info!(%id, title = %t, "título de la reunión");
                                    break;
                                }
                                Ok(None) => {}
                                Err(e) => tracing::warn!(%id, "título automático: {e}"),
                            }
                        }
                    });
                }
                SupervisorEvent::MeetingUpdated(m) if m.status == MeetingStatus::Ready => {
                    let core = core.clone();
                    tauri::async_runtime::spawn(async move { core.after_processing().await });
                }
                _ => {}
            }
        }
    });
}

pub fn run() {
    let layout = data_layout().unwrap_or_else(|| DataLayout::new(std::env::temp_dir().join(APP_DIR_NAME)));
    for dir in [layout.root().to_path_buf(), layout.logs_dir(), layout.models_dir(), layout.meetings_dir()] {
        let _ = std::fs::create_dir_all(dir);
    }
    let _log_guard = init_logging(&layout);
    // Una segunda acta crearía un segundo icono en la barra de menús.
    let _instance = match instance::InstanceLock::acquire(layout.root()) {
        instance::Acquire::Owned(lock) => Some(lock),
        instance::Acquire::AlreadyRunning => {
            tracing::warn!("acta ya está abierta: se cierra esta segunda instancia");
            return;
        }
        instance::Acquire::Unavailable => {
            tracing::warn!("no se pudo tomar el candado de instancia única: se arranca sin él");
            None
        }
    };

    let positions_file = layout.root().join("overlay-positions.json");
    let builder = tauri::Builder::default().plugin(tauri_plugin_opener::init());
    #[cfg(target_os = "macos")]
    let builder = builder.plugin(tauri_nspanel::init());

    let app = builder
        .setup(move |app| {
            let handle = app.handle().clone();
            let core: Core = Arc::new(tauri::async_runtime::block_on(build_core(layout.clone(), handle.clone()))?);
            app.manage(core.clone());
            #[cfg(target_os = "macos")]
            if let Err(e) = overlay::panel::install(&handle) {
                tracing::error!("no se pudo convertir la pastilla en panel: {e}");
            }
            if let Some(w) = handle.get_webview_window("overlay") {
                overlay::place(&w, &layout.root().join("overlay-positions.json"));
            }
            glass::install(&handle);
            if let Err(e) = tray::install(&handle) {
                tracing::error!("no se pudo crear el icono de la barra de menús: {e}");
            }
            forward_events(handle.clone(), core.clone());
            // El modelo local se descarga de memoria cuando lleva un rato sin uso.
            let idle_core = core.clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                    idle_core.local_llm.stop_if_idle().await;
                }
            });
            // Permisos que el usuario concede en Ajustes del Sistema mientras acta
            // está abierta: se avisa a la UI (`permissions:changed`) sin esperar al foco.
            let perm_handle = handle.clone();
            tauri::async_runtime::spawn(async move {
                use acta_core::permissions::{PermissionKind, PermissionStatus};
                let mut last = system::app_level_permissions();
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                    let now = system::app_level_permissions();
                    for (kind, before, after) in [
                        (PermissionKind::Accessibility, last.accessibility, now.accessibility),
                        (PermissionKind::ScreenRecording, last.screen_recording, now.screen_recording),
                    ] {
                        if before != after {
                            let status = if after { PermissionStatus::Granted } else { PermissionStatus::NotDetermined };
                            let payload = acta_core::api::PermissionChanged { kind, status };
                            let _ = perm_handle.emit(acta_core::api::event_names::PERMISSIONS_CHANGED, serde_json::to_value(payload).unwrap_or_default());
                        }
                    }
                    last = now;
                }
            });
            // Estado inicial (p. ej. recuperando una grabación tras un cierre).
            let h = handle.clone();
            tauri::async_runtime::spawn(async move {
                if let Ok(s) = core.session_get_state().await {
                    set_overlay(&h, overlay::overlay_visible(&s.state));
                    tray::update(&h, &s);
                }
                core.after_processing().await;
            });
            Ok(())
        })
        .on_window_event(move |window, event| {
            if window.label() == "overlay"
                && let WindowEvent::Moved(_) = event
            {
                overlay::remember(window, &positions_file);
            }
            // Cerrar la ventana principal la oculta: acta sigue en la barra de menús.
            if window.label() == "main"
                && let WindowEvent::CloseRequested { api, .. } = event
            {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::session_get_state,
            commands::session_start,
            commands::session_pause,
            commands::session_resume,
            commands::session_stop,
            commands::session_dismiss,
            commands::prompt_answer,
            commands::capture_manual,
            commands::meetings_list,
            commands::meeting_get,
            commands::meeting_update,
            commands::meeting_set_note,
            commands::meeting_delete,
            commands::meeting_set_favourite,
            commands::group_set_favourite,
            commands::cleanup_suggestions,
            commands::cleanup_delete,
            commands::cleanup_keep,
            commands::meeting_reprocess,
            commands::meeting_retry,
            commands::meeting_search_transcript,
            commands::groups_suggest_title,
            commands::llm_providers_available,
            commands::chat_cancel,
            commands::live_cancel,
            commands::live_transcript_get,
            commands::live_ask,
            commands::speaker_rename,
            commands::action_item_set_done,
            commands::capture_delete,
            commands::groups_list,
            commands::group_create,
            commands::group_update,
            commands::group_delete,
            commands::group_add_meeting,
            commands::group_remove_meeting,
            commands::group_suggestions_list,
            commands::group_suggestions_refresh,
            commands::group_suggestion_accept,
            commands::group_suggestion_dismiss,
            commands::search,
            commands::chat_threads_list,
            commands::chat_thread_create,
            commands::chat_thread_delete,
            commands::chat_messages_list,
            commands::chat_send,
            commands::voiceprints_list,
            commands::voiceprint_delete,
            commands::settings_get,
            commands::settings_update,
            commands::llm_provider_test,
            commands::llm_cli_detect,
            commands::secret_set,
            commands::secret_delete,
            commands::local_llm_status,
            commands::local_llm_download,
            commands::local_llm_cancel,
            commands::local_llm_delete,
            commands::local_llm_test,
            commands::models_status,
            commands::audio_inputs_list,
            commands::system_profile,
            commands::apps_list,
            commands::models_download,
            commands::permissions_status,
            commands::permissions_request,
            commands::open_system_settings,
            commands::health_get,
            commands::app_info,
            commands::overlay_set_expanded,
            commands::main_window_show,
            commands::providers_ui::llm_list_models,
            commands::providers_ui::llm_list_opencode_models,
            commands::providers_ui::llm_detect_local,
            commands::providers_ui::open_external_url,
            commands::downloads::meeting_export_transcript,
            commands::downloads::meeting_export_audio,
            commands::downloads::open_meeting_folder,
            commands::downloads::open_data_folder,
        ])
        .build(tauri::generate_context!());
    let app = match app {
        Ok(a) => a,
        Err(e) => {
            tracing::error!("acta no pudo arrancar: {e}");
            return;
        }
    };
    let stopping = AtomicBool::new(false);
    app.run(move |handle, event| {
        #[cfg(target_os = "macos")]
        if let RunEvent::Reopen { .. } = &event {
            // El Dock debe recuperar la ventana principal aunque la pastilla
            // siga visible (has_visible_windows no distingue entre ambas).
            if let Err(e) = commands::show_main(handle, None) {
                tracing::warn!("no se pudo reabrir la ventana principal: {e}");
            }
        }
        if let RunEvent::ExitRequested { .. } = event
            && !stopping.swap(true, Ordering::SeqCst)
            && let Some(core) = handle.try_state::<Core>()
        {
            // Cierra la grabación en curso (se procesará al volver) y el sidecar.
            let core = core.inner().clone();
            tauri::async_runtime::block_on(async move {
                let _ = tokio::time::timeout(std::time::Duration::from_secs(8), core.shutdown()).await;
            });
        }
    });
}
