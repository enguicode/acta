//! `MeetingStore`: reuniones, transcript, actas y capturas.

pub mod timeline;

use crate::codec::*;
use crate::db::SqliteStore;
use crate::search::purge_index;
use crate::text::{fold_text, fts_and_query};
use acta_core::ids::*;
use acta_core::layout::DataLayout;
use acta_core::library::*;
use acta_core::minutes::{ActionItem, Decision, Escaleta, KeyMoment, Minutes, MinutesOutput, Tema};
use acta_core::recording::Track;
use acta_core::screen::{Capture, ScreenShare};
use acta_core::time::{MediaMs, UnixMs};
use acta_core::transcription::{NameSource, Speaker, Transcript, Utterance};
use acta_core::{PortError, PortResult};
use async_trait::async_trait;
use rusqlite::types::Value;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension, Row};

const MEETING_COLS: &str =
    "id,title,platform,app_bundle_id,started_at,ended_at,duration_ms,status,recovered,error,language,created_at,updated_at,favourite,error_provider,error_model,user_note";
const DEFAULT_LIMIT: u32 = 50;
const MAX_LIMIT: u32 = 200;

fn meeting_from_row(r: &Row<'_>) -> rusqlite::Result<Meeting> {
    Ok(Meeting {
        id: col_id(r, 0)?,
        title: r.get(1)?,
        platform: col_enum(r, 2)?,
        app_bundle_id: r.get(3)?,
        started_at: UnixMs(r.get(4)?),
        ended_at: r.get::<_, Option<i64>>(5)?.map(UnixMs),
        duration: MediaMs(col_u64(r, 6)?),
        status: col_enum(r, 7)?,
        recovered: col_bool(r, 8)?,
        error: r.get(9)?,
        language: r.get(10)?,
        created_at: UnixMs(r.get(11)?),
        updated_at: UnixMs(r.get(12)?),
        favourite: col_bool(r, 13)?,
        error_provider: r.get(14)?,
        error_model: r.get(15)?,
        user_note: r.get(16)?,
    })
}

fn load_meeting(conn: &Connection, id: MeetingId) -> PortResult<Option<Meeting>> {
    conn.query_row(&format!("SELECT {MEETING_COLS} FROM meetings WHERE id = ?1"), [id.to_string()], meeting_from_row)
        .optional()
        .map_err(sql_err)
}

/// Condición SQL «la reunión `id_expr` es favorita»: tiene estrella o está en
/// un grupo con estrella. La comparten el filtro de la lista, el alcance de
/// búsqueda y la limpieza.
pub(crate) fn favourite_sql(id_expr: &str) -> String {
    format!(
        "({id_expr} IN (SELECT id FROM meetings WHERE favourite = 1) \
          OR {id_expr} IN (SELECT gm.meeting_id FROM group_meetings gm JOIN groups g ON g.id = gm.group_id WHERE g.favourite = 1))"
    )
}

fn require_meeting(conn: &Connection, id: MeetingId) -> PortResult<()> {
    if exists(conn, "meetings", "id", &id.to_string())? {
        Ok(())
    } else {
        Err(PortError::NotFound(format!("meeting {id}")))
    }
}

fn speaker_from_row(r: &Row<'_>) -> rusqlite::Result<Speaker> {
    Ok(Speaker {
        id: col_id(r, 0)?,
        meeting_id: col_id(r, 1)?,
        label: r.get(2)?,
        display_name: r.get(3)?,
        is_me: col_bool(r, 4)?,
        track: col_enum(r, 5)?,
        diarization_label: r.get(6)?,
        name_source: col_enum(r, 7)?,
        confirmed: col_bool(r, 8)?,
        voiceprint_id: col_id_opt(r, 9)?,
    })
}

fn load_transcript(conn: &Connection, id: MeetingId) -> PortResult<Transcript> {
    let mid = id.to_string();
    let speakers = conn
        .prepare(
            "SELECT id,meeting_id,label,display_name,is_me,track,diarization_label,name_source,confirmed,voiceprint_id
             FROM speakers WHERE meeting_id = ?1 ORDER BY rowid",
        )
        .and_then(|mut st| st.query_map([&mid], speaker_from_row)?.collect::<Result<Vec<_>, _>>())
        .map_err(sql_err)?;
    let utterances = conn
        .prepare("SELECT idx,speaker_id,track,start_ms,end_ms,text,confidence FROM utterances WHERE meeting_id = ?1 ORDER BY idx")
        .and_then(|mut st| {
            st.query_map([&mid], |r| {
                Ok(Utterance {
                    idx: r.get(0)?,
                    speaker_id: col_id(r, 1)?,
                    track: col_enum(r, 2)?,
                    start: MediaMs(col_u64(r, 3)?),
                    end: MediaMs(col_u64(r, 4)?),
                    text: r.get(5)?,
                    confidence: r.get::<_, Option<f64>>(6)?.map(|c| c as f32),
                })
            })?
            .collect::<Result<Vec<_>, _>>()
        })
        .map_err(sql_err)?;
    Ok(Transcript { meeting_id: id, speakers, utterances })
}

fn load_decisions(conn: &Connection, id: MeetingId) -> PortResult<Vec<Decision>> {
    conn.prepare("SELECT id,text,at_ms FROM decisions WHERE meeting_id = ?1 ORDER BY rowid")
        .and_then(|mut st| {
            st.query_map([id.to_string()], |r| {
                Ok(Decision { id: col_id(r, 0)?, text: r.get(1)?, at: r.get::<_, Option<i64>>(2)?.map(|v| MediaMs(v.max(0) as u64)) })
            })?
            .collect::<Result<Vec<_>, _>>()
        })
        .map_err(sql_err)
}

fn load_action_items(conn: &Connection, id: MeetingId) -> PortResult<Vec<ActionItem>> {
    let mut items = conn
        .prepare(
            "SELECT id,meeting_id,text,owner_name,owner_speaker_id,is_mine,due,done,at_ms
             FROM action_items WHERE meeting_id = ?1 ORDER BY rowid",
        )
        .and_then(|mut st| {
            st.query_map([id.to_string()], |r| {
                Ok(ActionItem {
                    id: col_id(r, 0)?,
                    meeting_id: col_id(r, 1)?,
                    text: r.get(2)?,
                    owner_name: r.get(3)?,
                    owner_speaker_id: col_id_opt(r, 4)?,
                    is_mine: col_bool(r, 5)?,
                    due: r.get(6)?,
                    done: col_bool(r, 7)?,
                    at: r.get::<_, Option<i64>>(8)?.map(|v| MediaMs(v.max(0) as u64)),
                    capture_ids: vec![],
                })
            })?
            .collect::<Result<Vec<_>, _>>()
        })
        .map_err(sql_err)?;
    for it in &mut items {
        it.capture_ids = conn
            .prepare("SELECT capture_id FROM capture_action_items WHERE action_item_id = ?1 ORDER BY capture_id")
            .and_then(|mut st| st.query_map([it.id.to_string()], |r| col_id::<CaptureId>(r, 0))?.collect::<Result<Vec<_>, _>>())
            .map_err(sql_err)?;
    }
    Ok(items)
}

fn load_minutes(conn: &Connection, id: MeetingId) -> PortResult<Option<(Minutes, Escaleta, Vec<KeyMoment>)>> {
    let row = conn
        .query_row(
            "SELECT summary_md,escaleta,key_moments,provider,model,generated_at FROM minutes WHERE meeting_id = ?1",
            [id.to_string()],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    col_json::<Escaleta>(r, 1)?,
                    col_json::<Vec<KeyMoment>>(r, 2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, i64>(5)?,
                ))
            },
        )
        .optional()
        .map_err(sql_err)?;
    let Some((summary_md, escaleta, key_moments, provider, model, generated_at)) = row else {
        return Ok(None);
    };
    let minutes = Minutes {
        meeting_id: id,
        summary_md,
        decisions: load_decisions(conn, id)?,
        action_items: load_action_items(conn, id)?,
        provider: ProviderId(provider),
        model,
        generated_at: UnixMs(generated_at),
    };
    Ok(Some((minutes, escaleta, key_moments)))
}

fn load_screen_shares(conn: &Connection, id: MeetingId) -> PortResult<Vec<ScreenShare>> {
    conn.prepare("SELECT start_ms,end_ms,presenter FROM screen_shares WHERE meeting_id = ?1 ORDER BY start_ms, id")
        .and_then(|mut st| {
            st.query_map([id.to_string()], |r| {
                Ok(ScreenShare { start: MediaMs(col_u64(r, 0)?), end: r.get::<_, Option<i64>>(1)?.map(|v| MediaMs(v.max(0) as u64)), presenter: r.get(2)? })
            })?
            .collect::<Result<Vec<_>, _>>()
        })
        .map_err(sql_err)
}

fn capture_from_row(r: &Row<'_>) -> rusqlite::Result<Capture> {
    Ok(Capture {
        id: col_id(r, 0)?,
        meeting_id: col_id(r, 1)?,
        at: MediaMs(col_u64(r, 2)?),
        at_wall: UnixMs(r.get(3)?),
        path: r.get(4)?,
        reason: col_enum(r, 5)?,
        phash: r.get(6)?,
        width: r.get(7)?,
        height: r.get(8)?,
        keyword: r.get(9)?,
        topic_path: r.get(10)?,
        action_item_ids: vec![],
    })
}

const CAPTURE_COLS: &str = "id,meeting_id,at_ms,at_wall,path,reason,phash,width,height,keyword,topic_path";

fn load_captures(conn: &Connection, id: MeetingId) -> PortResult<Vec<Capture>> {
    let mut caps = conn
        .prepare(&format!("SELECT {CAPTURE_COLS} FROM captures WHERE meeting_id = ?1 ORDER BY at_ms, rowid"))
        .and_then(|mut st| st.query_map([id.to_string()], capture_from_row)?.collect::<Result<Vec<_>, _>>())
        .map_err(sql_err)?;
    for c in &mut caps {
        c.action_item_ids = conn
            .prepare("SELECT action_item_id FROM capture_action_items WHERE capture_id = ?1 ORDER BY action_item_id")
            .and_then(|mut st| st.query_map([c.id.to_string()], |r| col_id::<ActionItemId>(r, 0))?.collect::<Result<Vec<_>, _>>())
            .map_err(sql_err)?;
    }
    Ok(caps)
}

/// Enlaza capturas con una tarea, solo las que existen y son de la misma reunión.
fn link_captures(conn: &Connection, meeting: &str, item: &str, capture_ids: &[CaptureId]) -> PortResult<()> {
    for c in capture_ids {
        conn.execute(
            "INSERT OR IGNORE INTO capture_action_items(capture_id, action_item_id)
             SELECT id, ?2 FROM captures WHERE id = ?1 AND meeting_id = ?3",
            params![c.to_string(), item, meeting],
        )
        .map_err(sql_err)?;
    }
    Ok(())
}

/// Recorre la escaleta y devuelve (ruta "2.1", ref de captura).
fn capture_topic_paths(temas: &[Tema], prefix: &str, out: &mut Vec<(String, String)>) {
    for (i, t) in temas.iter().enumerate() {
        let path = if prefix.is_empty() { format!("{}", i + 1) } else { format!("{prefix}.{}", i + 1) };
        for c in &t.capturas {
            out.push((path.clone(), c.clone()));
        }
        capture_topic_paths(&t.subtemas, &path, out);
    }
}

fn audio_paths(layout: &Option<DataLayout>, id: MeetingId) -> MeetingAudio {
    let Some(l) = layout else { return MeetingAudio::default() };
    let pick = |p: std::path::PathBuf| p.exists().then(|| p.to_string_lossy().into_owned());
    MeetingAudio {
        mic: pick(l.track_wav(id, Track::Mic)),
        system: pick(l.track_wav(id, Track::System)),
        mix: pick(l.mix_wav(id)),
    }
}

fn parse_cursor(c: &str) -> PortResult<(i64, i64)> {
    c.split_once(':')
        .and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?)))
        .ok_or_else(|| PortError::InvalidInput(format!("cursor inválido: {c}")))
}

fn escape_like(s: &str) -> String {
    s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

fn list_meetings_sync(conn: &Connection, q: &MeetingQuery) -> PortResult<Page<MeetingSummary>> {
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT) as usize;
    let mut sql = String::from("SELECT m.rowid_alias, m.id, m.title, m.platform, m.started_at, m.duration_ms, m.status, m.favourite FROM meetings m WHERE 1=1");
    let mut args: Vec<Value> = Vec::new();
    if let Some(p) = &q.platform {
        sql.push_str(" AND m.platform = ?");
        args.push(Value::Text(enc(p)));
    }
    if let Some(f) = q.from {
        sql.push_str(" AND m.started_at >= ?");
        args.push(Value::Integer(f.0));
    }
    if let Some(t) = q.to {
        sql.push_str(" AND m.started_at <= ?");
        args.push(Value::Integer(t.0));
    }
    if let Some(g) = q.group_id {
        sql.push_str(" AND EXISTS (SELECT 1 FROM group_meetings gm WHERE gm.group_id = ? AND gm.meeting_id = m.id)");
        args.push(Value::Text(g.to_string()));
    }
    if q.favourites_only {
        sql.push_str(&format!(" AND {}", favourite_sql("m.id")));
    }
    let text = q.text.as_deref().map(str::trim).filter(|t| !t.is_empty());
    let fts = text.and_then(fts_and_query);
    if let Some(t) = text {
        sql.push_str(" AND (acta_fold(m.title) LIKE ? ESCAPE '\\'");
        args.push(Value::Text(format!("%{}%", escape_like(&fold_text(t)))));
        if let Some(f) = &fts {
            sql.push_str(" OR m.id IN (SELECT u.meeting_id FROM utterances u WHERE u.id IN (SELECT rowid FROM utterances_fts WHERE utterances_fts MATCH ?))");
            args.push(Value::Text(f.clone()));
        }
        sql.push(')');
    }
    if let Some(c) = &q.cursor {
        let (started, rowid) = parse_cursor(c)?;
        sql.push_str(" AND (m.started_at < ? OR (m.started_at = ? AND m.rowid_alias < ?))");
        args.extend([Value::Integer(started), Value::Integer(started), Value::Integer(rowid)]);
    }
    sql.push_str(" ORDER BY m.started_at DESC, m.rowid_alias DESC LIMIT ?");
    args.push(Value::Integer(limit as i64 + 1));

    struct Row0 {
        rowid: i64,
        id: MeetingId,
        title: String,
        platform: acta_core::detection::Platform,
        started: i64,
        duration: u64,
        status: MeetingStatus,
        favourite: bool,
    }
    let mut rows = conn
        .prepare(&sql)
        .and_then(|mut st| {
            st.query_map(params_from_iter(args.iter()), |r| {
                Ok(Row0 {
                    rowid: r.get(0)?,
                    id: col_id(r, 1)?,
                    title: r.get(2)?,
                    platform: col_enum(r, 3)?,
                    started: r.get(4)?,
                    duration: col_u64(r, 5)?,
                    status: col_enum(r, 6)?,
                    favourite: col_bool(r, 7)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()
        })
        .map_err(sql_err)?;
    let has_more = rows.len() > limit;
    rows.truncate(limit);
    let next_cursor = has_more.then(|| rows.last().map(|r| format!("{}:{}", r.started, r.rowid))).flatten();

    let mut items = Vec::with_capacity(rows.len());
    for r in rows {
        let mid = r.id.to_string();
        let speaker_names = conn
            .prepare("SELECT DISTINCT COALESCE(display_name, label) AS n FROM speakers WHERE meeting_id = ?1 ORDER BY n")
            .and_then(|mut st| st.query_map([&mid], |x| x.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>())
            .map_err(sql_err)?;
        let my_open_tasks: u32 = conn
            .query_row("SELECT count(*) FROM action_items WHERE meeting_id = ?1 AND is_mine = 1 AND done = 0", [&mid], |x| x.get(0))
            .map_err(sql_err)?;
        let capture_count: u32 = conn
            .query_row("SELECT count(*) FROM captures WHERE meeting_id = ?1 AND reason <> 'candidate'", [&mid], |x| x.get(0))
            .map_err(sql_err)?;
        let group_ids = conn
            .prepare("SELECT group_id FROM group_meetings WHERE meeting_id = ?1 ORDER BY added_at, group_id")
            .and_then(|mut st| st.query_map([&mid], |x| col_id::<GroupId>(x, 0))?.collect::<Result<Vec<_>, _>>())
            .map_err(sql_err)?;
        let snippet = match &fts {
            Some(f) => conn
                .query_row(
                    "SELECT snippet(utterances_fts, 0, '[', ']', '…', 12) FROM utterances_fts
                     JOIN utterances u ON u.id = utterances_fts.rowid
                     WHERE utterances_fts MATCH ?1 AND u.meeting_id = ?2 ORDER BY rank LIMIT 1",
                    params![f, mid],
                    |x| x.get::<_, String>(0),
                )
                .optional()
                .map_err(sql_err)?,
            None => None,
        };
        items.push(MeetingSummary {
            id: r.id,
            title: r.title,
            platform: r.platform,
            started_at: UnixMs(r.started),
            duration: MediaMs(r.duration),
            status: r.status,
            speaker_names,
            my_open_tasks,
            group_ids,
            capture_count,
            snippet,
            favourite: r.favourite,
        });
    }
    Ok(Page { items, next_cursor })
}

#[async_trait]
impl MeetingStore for SqliteStore {
    async fn create_meeting(&self, m: NewMeeting) -> PortResult<Meeting> {
        let now = self.now().0;
        self.with_conn(move |conn| {
            if exists(conn, "meetings", "id", &m.id.to_string())? {
                return Err(PortError::InvalidInput(format!("meeting {} already exists", m.id)));
            }
            conn.execute(
                "INSERT INTO meetings(id,title,platform,app_bundle_id,started_at,status,language,created_at,updated_at)
                 VALUES (?1,?2,?3,?4,?5,'recording',?6,?7,?7)",
                params![m.id.to_string(), m.title, enc(&m.platform), m.app_bundle_id, m.started_at.0, m.language, now],
            )
            .map_err(sql_err)?;
            load_meeting(conn, m.id)?.ok_or_else(|| PortError::Internal("meeting vanished".into()))
        })
        .await
    }

    async fn patch_meeting(&self, id: MeetingId, patch: MeetingPatch) -> PortResult<Meeting> {
        let now = self.now().0;
        self.with_conn(move |conn| {
            let mut m = load_meeting(conn, id)?.ok_or_else(|| PortError::NotFound(format!("meeting {id}")))?;
            let title_edited = patch.title.is_some();
            if let Some(t) = patch.title {
                m.title = t;
            }
            if let Some(e) = patch.ended_at {
                m.ended_at = Some(e);
            }
            if let Some(d) = patch.duration {
                m.duration = d;
            }
            if let Some(s) = patch.status {
                m.status = s;
            }
            if let Some(r) = patch.recovered {
                m.recovered = r;
            }
            if let Some(n) = patch.user_note {
                m.user_note = n;
                conn.execute("UPDATE meetings SET user_note=?2 WHERE id=?1", params![id.to_string(), m.user_note]).map_err(sql_err)?;
            }
            let clear_llm = matches!(patch.error, Some(None));
            if let Some(e) = patch.error {
                m.error = e;
            }
            if clear_llm {
                m.error_provider = None;
                m.error_model = None;
                conn.execute("UPDATE meetings SET error_provider=NULL, error_model=NULL WHERE id=?1", [id.to_string()]).map_err(sql_err)?;
            }
            conn.execute(
                "UPDATE meetings SET title=?2, ended_at=?3, duration_ms=?4, status=?5, recovered=?6, error=?7, updated_at=?8, title_edited = title_edited OR ?9 WHERE id=?1",
                params![id.to_string(), m.title, m.ended_at.map(|t| t.0), m.duration.0 as i64, enc(&m.status), i64::from(m.recovered), m.error, now, i64::from(title_edited)],
            )
            .map_err(sql_err)?;
            m.updated_at = UnixMs(now);
            Ok(m)
        })
        .await
    }

    async fn get_meeting(&self, id: MeetingId) -> PortResult<Option<Meeting>> {
        self.with_conn(move |conn| load_meeting(conn, id)).await
    }

    async fn get_meeting_detail(&self, id: MeetingId) -> PortResult<Option<MeetingDetail>> {
        let layout = self.layout.clone();
        self.with_conn(move |conn| {
            let Some(meeting) = load_meeting(conn, id)? else { return Ok(None) };
            let transcript = load_transcript(conn, id)?;
            let captures = load_captures(conn, id)?;
            let shares = load_screen_shares(conn, id)?;
            let (minutes, escaleta) = match load_minutes(conn, id)? {
                Some((m, e, _)) => (Some(m), Some(e)),
                None => (None, None),
            };
            let timeline = timeline::derive_timeline(
                &escaleta.clone().unwrap_or_default(),
                minutes.as_ref().map_or(&[][..], |m| &m.decisions),
                minutes.as_ref().map_or(&[][..], |m| &m.action_items),
                &captures,
                &shares,
            );
            let groups = crate::groups::groups_for_meeting(conn, id)?;
            Ok(Some(MeetingDetail { meeting, transcript, minutes, escaleta, captures, timeline, groups, audio: audio_paths(&layout, id) }))
        })
        .await
    }

    async fn list_meetings(&self, query: MeetingQuery) -> PortResult<Page<MeetingSummary>> {
        self.with_conn(move |conn| list_meetings_sync(conn, &query)).await
    }

    async fn delete_meeting(&self, id: MeetingId) -> PortResult<()> {
        self.with_conn(move |conn| {
            require_meeting(conn, id)?;
            let tx = conn.transaction().map_err(sql_err)?;
            purge_index(&tx, id)?;
            tx.execute("DELETE FROM meetings WHERE id = ?1", [id.to_string()]).map_err(sql_err)?;
            drop_from_suggestions(&tx, id)?;
            tx.commit().map_err(sql_err)
        })
        .await
    }

    async fn set_meeting_favourite(&self, id: MeetingId, favourite: bool) -> PortResult<Meeting> {
        self.with_conn(move |conn| {
            // No toca `updated_at`: marcar no es editar.
            conn.execute("UPDATE meetings SET favourite = ?2 WHERE id = ?1", params![id.to_string(), i64::from(favourite)]).map_err(sql_err)?;
            load_meeting(conn, id)?.ok_or_else(|| PortError::NotFound(format!("meeting {id}")))
        })
        .await
    }

    async fn touch_meeting(&self, id: MeetingId, touch: MeetingTouch, at: UnixMs) -> PortResult<()> {
        let column = match touch {
            MeetingTouch::Opened => "last_opened_at",
            MeetingTouch::Used => "last_used_at",
        };
        self.with_conn(move |conn| {
            conn.execute(
                &format!("UPDATE meetings SET {column} = MAX(COALESCE({column}, 0), ?2) WHERE id = ?1"),
                params![id.to_string(), at.0],
            )
            .map_err(sql_err)?;
            Ok(())
        })
        .await
    }

    async fn set_cleanup_keep(&self, id: MeetingId, keep: bool) -> PortResult<()> {
        self.with_conn(move |conn| {
            let n = conn
                .execute("UPDATE meetings SET cleanup_keep = ?2 WHERE id = ?1", params![id.to_string(), i64::from(keep)])
                .map_err(sql_err)?;
            if n == 0 {
                return Err(PortError::NotFound(format!("meeting {id}")));
            }
            Ok(())
        })
        .await
    }

    async fn cleanup_facts(&self) -> PortResult<Vec<MeetingCleanupFacts>> {
        self.with_conn(move |conn| {
            // Palabras = espacios + 1 por frase no vacía (suficiente para «poco contenido»).
            let sql = format!(
                "SELECT m.id, m.title, m.started_at, m.duration_ms, m.status, m.favourite,
                        {in_fav_group}, m.cleanup_keep, m.last_opened_at, m.last_used_at,
                        (SELECT count(*) FROM action_items a WHERE a.meeting_id = m.id AND a.is_mine = 1 AND a.done = 0),
                        (SELECT COALESCE(SUM(length(trim(u.text)) - length(replace(trim(u.text), ' ', '')) + 1), 0)
                           FROM utterances u WHERE u.meeting_id = m.id AND trim(u.text) <> ''),
                        EXISTS(SELECT 1 FROM minutes mi WHERE mi.meeting_id = m.id)
                 FROM meetings m ORDER BY m.started_at, m.rowid_alias",
                in_fav_group = "EXISTS(SELECT 1 FROM group_meetings gm JOIN groups g ON g.id = gm.group_id WHERE gm.meeting_id = m.id AND g.favourite = 1)"
            );
            conn.prepare(&sql)
                .and_then(|mut st| {
                    st.query_map([], |r| {
                        Ok(MeetingCleanupFacts {
                            id: col_id(r, 0)?,
                            title: r.get(1)?,
                            started_at: UnixMs(r.get(2)?),
                            duration: MediaMs(col_u64(r, 3)?),
                            status: col_enum(r, 4)?,
                            favourite: col_bool(r, 5)?,
                            in_favourite_group: col_bool(r, 6)?,
                            keep: col_bool(r, 7)?,
                            last_opened_at: r.get::<_, Option<i64>>(8)?.map(UnixMs),
                            last_used_at: r.get::<_, Option<i64>>(9)?.map(UnixMs),
                            my_open_tasks: r.get(10)?,
                            word_count: r.get(11)?,
                            has_minutes: col_bool(r, 12)?,
                        })
                    })?
                    .collect::<Result<Vec<_>, _>>()
                })
                .map_err(sql_err)
        })
        .await
    }

    async fn save_transcript(&self, t: Transcript) -> PortResult<()> {
        self.with_conn(move |conn| {
            require_meeting(conn, t.meeting_id)?;
            let mid = t.meeting_id.to_string();
            let tx = conn.transaction().map_err(sql_err)?;
            tx.execute("DELETE FROM utterances WHERE meeting_id = ?1", [&mid]).map_err(sql_err)?;
            tx.execute("DELETE FROM speakers WHERE meeting_id = ?1", [&mid]).map_err(sql_err)?;
            for s in &t.speakers {
                tx.execute(
                    "INSERT INTO speakers(id,meeting_id,label,display_name,is_me,track,diarization_label,name_source,confirmed,voiceprint_id)
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                    params![
                        s.id.to_string(), mid, s.label, s.display_name, i64::from(s.is_me), enc(&s.track),
                        s.diarization_label, enc(&s.name_source), i64::from(s.confirmed), s.voiceprint_id.map(|v| v.to_string())
                    ],
                )
                .map_err(sql_err)?;
            }
            for u in &t.utterances {
                tx.execute(
                    "INSERT INTO utterances(meeting_id,idx,speaker_id,track,start_ms,end_ms,text,confidence) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                    params![mid, u.idx, u.speaker_id.to_string(), enc(&u.track), u.start.0 as i64, u.end.0 as i64, u.text, u.confidence],
                )
                .map_err(sql_err)?;
            }
            tx.commit().map_err(sql_err)
        })
        .await
    }

    async fn get_transcript(&self, id: MeetingId) -> PortResult<Option<Transcript>> {
        self.with_conn(move |conn| {
            if !exists(conn, "meetings", "id", &id.to_string())? {
                return Ok(None);
            }
            load_transcript(conn, id).map(Some)
        })
        .await
    }

    async fn rename_speaker(&self, meeting_id: MeetingId, speaker_id: SpeakerId, display_name: String, confirmed: bool) -> PortResult<()> {
        self.with_conn(move |conn| {
            let n = conn
                .execute(
                    "UPDATE speakers SET display_name=?3, name_source=?4, confirmed=?5 WHERE id=?1 AND meeting_id=?2",
                    params![speaker_id.to_string(), meeting_id.to_string(), display_name, enc(&NameSource::User), i64::from(confirmed)],
                )
                .map_err(sql_err)?;
            if n == 0 {
                return Err(PortError::NotFound(format!("speaker {speaker_id} in meeting {meeting_id}")));
            }
            Ok(())
        })
        .await
    }

    async fn link_speaker_voiceprint(&self, speaker_id: SpeakerId, voiceprint_id: Option<VoiceprintId>) -> PortResult<()> {
        self.with_conn(move |conn| {
            let n = conn
                .execute("UPDATE speakers SET voiceprint_id=?2 WHERE id=?1", params![speaker_id.to_string(), voiceprint_id.map(|v| v.to_string())])
                .map_err(sql_err)?;
            if n == 0 {
                return Err(PortError::NotFound(format!("speaker {speaker_id}")));
            }
            Ok(())
        })
        .await
    }

    async fn save_minutes(&self, output: MinutesOutput) -> PortResult<()> {
        self.with_conn(move |conn| {
            let m = &output.minutes;
            require_meeting(conn, m.meeting_id)?;
            let mid = m.meeting_id.to_string();
            let tx = conn.transaction().map_err(sql_err)?;
            tx.execute("DELETE FROM minutes WHERE meeting_id = ?1", [&mid]).map_err(sql_err)?;
            tx.execute("DELETE FROM decisions WHERE meeting_id = ?1", [&mid]).map_err(sql_err)?;
            let done_texts: std::collections::HashSet<String> = tx
                .prepare("SELECT text FROM action_items WHERE meeting_id = ?1 AND done = 1")
                .and_then(|mut st| st.query_map([&mid], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>())
                .map_err(sql_err)?
                .into_iter()
                .map(|t| fold_text(&t).trim().to_owned())
                .collect();
            tx.execute("DELETE FROM action_items WHERE meeting_id = ?1", [&mid]).map_err(sql_err)?;
            if let Some(title) = output.suggested_title.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
                tx.execute("UPDATE meetings SET title = ?2 WHERE id = ?1 AND title_edited = 0 AND title_source IS NULL", params![mid, title]).map_err(sql_err)?;
            }
            tx.execute(
                "INSERT INTO minutes(meeting_id,summary_md,escaleta,key_moments,provider,model,generated_at) VALUES (?1,?2,?3,?4,?5,?6,?7)",
                params![
                    mid, m.summary_md,
                    serde_json::to_string(&output.escaleta)?, serde_json::to_string(&output.key_moments)?,
                    m.provider.0, m.model, m.generated_at.0
                ],
            )
            .map_err(sql_err)?;
            for d in &m.decisions {
                tx.execute(
                    "INSERT INTO decisions(id,meeting_id,text,at_ms) VALUES (?1,?2,?3,?4)",
                    params![d.id.to_string(), mid, d.text, d.at.map(|a| a.0 as i64)],
                )
                .map_err(sql_err)?;
            }
            for a in &m.action_items {
                // El responsable solo se enlaza si el hablante existe en esta reunión.
                let owner = match a.owner_speaker_id {
                    Some(s) => tx
                        .query_row("SELECT id FROM speakers WHERE id=?1 AND meeting_id=?2", params![s.to_string(), mid], |r| r.get::<_, String>(0))
                        .optional()
                        .map_err(sql_err)?,
                    None => None,
                };
                tx.execute(
                    "INSERT INTO action_items(id,meeting_id,text,owner_name,owner_speaker_id,is_mine,due,done,at_ms) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                    params![a.id.to_string(), mid, a.text, a.owner_name, owner, i64::from(a.is_mine), a.due, i64::from(a.done || done_texts.contains(fold_text(&a.text).trim())), a.at.map(|v| v.0 as i64)],
                )
                .map_err(sql_err)?;
                link_captures(&tx, &mid, &a.id.to_string(), &a.capture_ids)?;
            }
            let mut paths = Vec::new();
            capture_topic_paths(&output.escaleta.temas, "", &mut paths);
            for (path, cref) in paths {
                if let Ok(cid) = cref.parse::<CaptureId>() {
                    tx.execute("UPDATE captures SET topic_path=?1 WHERE id=?2 AND meeting_id=?3", params![path, cid.to_string(), mid]).map_err(sql_err)?;
                }
            }
            tx.commit().map_err(sql_err)
        })
        .await
    }

    async fn save_screen_shares(&self, meeting_id: MeetingId, shares: Vec<ScreenShare>) -> PortResult<()> {
        self.with_conn(move |conn| {
            require_meeting(conn, meeting_id)?;
            let mid = meeting_id.to_string();
            let tx = conn.transaction().map_err(sql_err)?;
            tx.execute("DELETE FROM screen_shares WHERE meeting_id = ?1", [&mid]).map_err(sql_err)?;
            for sh in &shares {
                tx.execute(
                    "INSERT INTO screen_shares(meeting_id,start_ms,end_ms,presenter) VALUES (?1,?2,?3,?4)",
                    params![mid, sh.start.0 as i64, sh.end.map(|e| e.0 as i64), sh.presenter],
                )
                .map_err(sql_err)?;
            }
            tx.commit().map_err(sql_err)
        })
        .await
    }

    async fn get_minutes(&self, id: MeetingId) -> PortResult<Option<Minutes>> {
        self.with_conn(move |conn| Ok(load_minutes(conn, id)?.map(|(m, _, _)| m))).await
    }

    async fn set_action_item_done(&self, id: ActionItemId, done: bool) -> PortResult<()> {
        self.with_conn(move |conn| {
            let n = conn.execute("UPDATE action_items SET done=?2 WHERE id=?1", params![id.to_string(), i64::from(done)]).map_err(sql_err)?;
            if n == 0 {
                return Err(PortError::NotFound(format!("action item {id}")));
            }
            Ok(())
        })
        .await
    }

    async fn add_capture(&self, c: Capture) -> PortResult<()> {
        self.with_conn(move |conn| {
            require_meeting(conn, c.meeting_id)?;
            let tx = conn.transaction().map_err(sql_err)?;
            tx.execute(
                &format!("INSERT INTO captures({CAPTURE_COLS}) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)"),
                params![
                    c.id.to_string(), c.meeting_id.to_string(), c.at.0 as i64, c.at_wall.0, c.path, enc(&c.reason),
                    c.phash, c.width, c.height, c.keyword, c.topic_path
                ],
            )
            .map_err(sql_err)?;
            link_items(&tx, &c)?;
            tx.commit().map_err(sql_err)
        })
        .await
    }

    async fn update_capture(&self, c: Capture) -> PortResult<()> {
        self.with_conn(move |conn| {
            let tx = conn.transaction().map_err(sql_err)?;
            let n = tx
                .execute(
                    "UPDATE captures SET at_ms=?2, at_wall=?3, path=?4, reason=?5, phash=?6, width=?7, height=?8, keyword=?9, topic_path=?10 WHERE id=?1",
                    params![c.id.to_string(), c.at.0 as i64, c.at_wall.0, c.path, enc(&c.reason), c.phash, c.width, c.height, c.keyword, c.topic_path],
                )
                .map_err(sql_err)?;
            if n == 0 {
                return Err(PortError::NotFound(format!("capture {}", c.id)));
            }
            tx.execute("DELETE FROM capture_action_items WHERE capture_id=?1", [c.id.to_string()]).map_err(sql_err)?;
            link_items(&tx, &c)?;
            tx.commit().map_err(sql_err)
        })
        .await
    }

    async fn list_captures(&self, meeting_id: MeetingId) -> PortResult<Vec<Capture>> {
        self.with_conn(move |conn| load_captures(conn, meeting_id)).await
    }

    async fn get_capture(&self, id: CaptureId) -> PortResult<Option<Capture>> {
        self.with_conn(move |conn| {
            let Some(mut c) = conn
                .query_row(&format!("SELECT {CAPTURE_COLS} FROM captures WHERE id = ?1"), [id.to_string()], capture_from_row)
                .optional()
                .map_err(sql_err)?
            else {
                return Ok(None);
            };
            c.action_item_ids = conn
                .prepare("SELECT action_item_id FROM capture_action_items WHERE capture_id = ?1 ORDER BY action_item_id")
                .and_then(|mut st| st.query_map([c.id.to_string()], |r| col_id::<ActionItemId>(r, 0))?.collect::<Result<Vec<_>, _>>())
                .map_err(sql_err)?;
            Ok(Some(c))
        })
        .await
    }

    async fn delete_capture(&self, id: CaptureId) -> PortResult<()> {
        self.with_conn(move |conn| {
            let n = conn.execute("DELETE FROM captures WHERE id=?1", [id.to_string()]).map_err(sql_err)?;
            if n == 0 {
                return Err(PortError::NotFound(format!("capture {id}")));
            }
            Ok(())
        })
        .await
    }
}

/// Enlaza la captura con las tareas indicadas que existan en su reunión.
fn link_items(conn: &Connection, c: &Capture) -> PortResult<()> {
    for a in &c.action_item_ids {
        conn.execute(
            "INSERT OR IGNORE INTO capture_action_items(capture_id, action_item_id)
             SELECT ?1, id FROM action_items WHERE id = ?2 AND meeting_id = ?3",
            params![c.id.to_string(), a.to_string(), c.meeting_id.to_string()],
        )
        .map_err(sql_err)?;
    }
    Ok(())
}

/// Una reunión borrada sale de las sugerencias de grupo; una sugerencia que
/// se queda con menos de dos reuniones ya no es un grupo y se quita.
fn drop_from_suggestions(tx: &rusqlite::Transaction<'_>, id: MeetingId) -> PortResult<()> {
    let rows: Vec<(String, String)> = tx
        .prepare("SELECT id, meeting_ids FROM group_suggestions")
        .and_then(|mut st| st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<Vec<_>, _>>())
        .map_err(sql_err)?;
    for (sid, json) in rows {
        let mut ids: Vec<MeetingId> = serde_json::from_str(&json).unwrap_or_default();
        let before = ids.len();
        ids.retain(|m| *m != id);
        if ids.len() == before {
            continue;
        }
        if ids.len() < 2 {
            tx.execute("DELETE FROM group_suggestions WHERE id = ?1", [&sid]).map_err(sql_err)?;
        } else {
            tx.execute("UPDATE group_suggestions SET meeting_ids = ?2 WHERE id = ?1", params![sid, serde_json::to_string(&ids)?]).map_err(sql_err)?;
        }
    }
    Ok(())
}

impl crate::SqliteStore {
    /// Título automático (Teams, Calendario): solo si el usuario no lo editó,
    /// y sin marcarlo como editado. Devuelve la reunión si cambió.
    pub async fn set_auto_title(&self, id: MeetingId, title: String) -> PortResult<Option<Meeting>> {
        self.with_conn(move |conn| {
            let n = conn
                .execute("UPDATE meetings SET title=?2, title_source='app' WHERE id=?1 AND title_edited=0 AND title<>?2", params![id.to_string(), title])
                .map_err(sql_err)?;
            if n == 0 { Ok(None) } else { load_meeting(conn, id) }
        })
        .await
    }

    /// Proveedor y modelo del resumen que falló (se borran al limpiar el error).
    pub async fn set_meeting_error_llm(&self, id: MeetingId, provider: Option<String>, model: Option<String>) -> PortResult<()> {
        self.with_conn(move |conn| {
            conn.execute("UPDATE meetings SET error_provider=?2, error_model=?3 WHERE id=?1", params![id.to_string(), provider, model]).map_err(sql_err)?;
            Ok(())
        })
        .await
    }
}
