//! Apertura de la base de datos y migraciones.

use crate::codec::sql_err;
use acta_core::layout::DataLayout;
use acta_core::library::Embedder;
use acta_core::time::{Clock, SystemClock, UnixMs};
use acta_core::{PortError, PortResult};
use rusqlite::functions::FunctionFlags;
use rusqlite::Connection;
use std::path::Path;
use std::sync::{Arc, Mutex, Once};

/// Migraciones en orden. La posición + 1 es el `user_version` resultante.
pub const MIGRATIONS: &[&str] = &[
    include_str!("../../migrations/0001_init.sql"),
    include_str!("../../migrations/0002_screen_shares_titles_stale.sql"),
    include_str!("../../migrations/0003_favourites.sql"),
    include_str!("../../migrations/0004_usage_cleanup.sql"),
    include_str!("../../migrations/0005_error_provider.sql"),
    include_str!("../../migrations/0006_title_source.sql"),
    include_str!("../../migrations/0007_user_note.sql"),
];

/// Dimensión de los vectores en el esquema actual (multilingual-e5-small).
pub const EMBEDDING_DIMS: usize = 384;

static REGISTER_VEC: Once = Once::new();

/// Registra sqlite-vec como extensión automática para TODAS las conexiones
/// que se abran después en el proceso.
#[allow(unsafe_code)]
pub fn register_sqlite_vec() {
    REGISTER_VEC.call_once(|| {
        // SAFETY: `sqlite3_vec_init` es el punto de entrada de extensión de
        // sqlite-vec, enlazado estáticamente; `sqlite3_auto_extension` espera
        // exactamente esa firma (xEntryPoint). Se registra una sola vez.
        unsafe {
            type EntryPoint = unsafe extern "C" fn(
                *mut rusqlite::ffi::sqlite3,
                *mut *mut std::os::raw::c_char,
                *const rusqlite::ffi::sqlite3_api_routines,
            ) -> std::os::raw::c_int;
            let entry: EntryPoint = std::mem::transmute(sqlite_vec::sqlite3_vec_init as *const ());
            rusqlite::ffi::sqlite3_auto_extension(Some(entry));
        }
    });
}

/// Aplica las migraciones pendientes en una transacción cada una.
pub fn migrate(conn: &mut Connection) -> PortResult<u32> {
    let current: u32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).map_err(sql_err)?;
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(current as usize) {
        let version = i as u32 + 1;
        let tx = conn.transaction().map_err(sql_err)?;
        tx.execute_batch(sql).map_err(sql_err)?;
        tx.pragma_update(None, "user_version", version).map_err(sql_err)?;
        tx.commit().map_err(sql_err)?;
    }
    Ok(MIGRATIONS.len() as u32)
}

/// Abre (o crea) la base de datos con WAL, claves foráneas y sqlite-vec.
/// Registra además `acta_fold(text)`: minúsculas sin acentos (para LIKE).
pub fn open(path: &Path) -> PortResult<Connection> {
    register_sqlite_vec();
    let mut conn = Connection::open(path).map_err(sql_err)?;
    conn.execute_batch(
        "PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 5000;",
    )
    .map_err(sql_err)?;
    conn.create_scalar_function(
        "acta_fold",
        1,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        |ctx| Ok(crate::text::fold_text(&ctx.get::<String>(0)?)),
    )
    .map_err(sql_err)?;
    migrate(&mut conn)?;
    Ok(conn)
}

/// Adaptador que implementa todos los puertos de almacenamiento (WP2).
#[derive(Clone)]
pub struct SqliteStore {
    pub(crate) conn: Arc<Mutex<Connection>>,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) embedder: Option<Arc<dyn Embedder>>,
    pub(crate) layout: Option<DataLayout>,
}

impl SqliteStore {
    pub fn open(path: &Path) -> PortResult<Self> {
        Ok(Self {
            conn: Arc::new(Mutex::new(open(path)?)),
            clock: Arc::new(SystemClock),
            embedder: None,
            layout: None,
        })
    }

    /// Reloj inyectable (tests deterministas).
    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    /// Sin embedder la búsqueda funciona solo con FTS5 y no hay clustering.
    pub fn with_embedder(mut self, embedder: Arc<dyn Embedder>) -> Self {
        self.embedder = Some(embedder);
        self
    }

    /// Necesario para rellenar `MeetingDetail.audio` con rutas absolutas.
    pub fn with_layout(mut self, layout: DataLayout) -> Self {
        self.layout = Some(layout);
        self
    }

    pub(crate) fn now(&self) -> UnixMs {
        self.clock.now()
    }

    /// Ejecuta `f` con la conexión en un hilo de bloqueo.
    pub async fn with_conn<T, F>(&self, f: F) -> PortResult<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> PortResult<T> + Send + 'static,
    {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let mut guard = conn.lock().map_err(|_| PortError::Internal("sqlite mutex poisoned".into()))?;
            f(&mut guard)
        })
        .await
        .map_err(|e| PortError::Internal(format!("join: {e}")))?
    }
}
