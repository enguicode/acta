#![allow(clippy::expect_used, clippy::unwrap_used)]
//! Repositorios: reuniones, transcript, actas, capturas, grupos, huellas, chat, ajustes.

mod common;

use acta_core::chat::*;
use acta_core::detection::Platform;
use acta_core::ids::*;
use acta_core::layout::DataLayout;
use acta_core::library::*;
use acta_core::minutes::*;
use acta_core::recording::Track;
use acta_core::screen::{Capture, CaptureReason, ScreenShare};
use acta_core::settings::Settings;
use acta_core::time::{MediaMs, UnixMs};
use acta_core::transcription::{NameSource, Voiceprint};
use acta_core::PortError;
use common::*;
use pretty_assertions::assert_eq;

// ---------- reuniones ----------

#[tokio::test]
async fn create_get_patch_meeting_uses_injected_clock() {
    let (s, _d, clock) = store();
    let m = s.create_meeting(new_meeting(1, "Daily", T0)).await.unwrap();
    assert_eq!(m.status, MeetingStatus::Recording);
    assert_eq!(m.created_at, UnixMs(T0));
    assert_eq!(m.duration, MediaMs(0));
    assert!(!m.recovered);

    clock.advance_ms(5_000);
    let p = s
        .patch_meeting(
            mid(1),
            MeetingPatch {
                title: Some("Daily de producto".into()),
                ended_at: Some(UnixMs(T0 + 4_000)),
                duration: Some(MediaMs(3_500)),
                status: Some(MeetingStatus::Failed),
                recovered: Some(true),
                error: Some(Some("boom".into())),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!((p.title.as_str(), p.duration, p.status, p.recovered), ("Daily de producto", MediaMs(3_500), MeetingStatus::Failed, true));
    assert_eq!(p.error.as_deref(), Some("boom"));
    assert_eq!(p.updated_at, UnixMs(T0 + 5_000));
    assert_eq!(p.created_at, UnixMs(T0));

    // `Some(None)` borra el error; `None` lo deja.
    let p = s.patch_meeting(mid(1), MeetingPatch { error: Some(None), ..Default::default() }).await.unwrap();
    assert_eq!(p.error, None);
    assert_eq!(s.get_meeting(mid(1)).await.unwrap().unwrap(), p);
}

#[tokio::test]
async fn missing_meeting_semantics_and_duplicate_create() {
    let (s, _d, _c) = store();
    assert_eq!(s.get_meeting(mid(9)).await.unwrap(), None);
    assert!(matches!(s.patch_meeting(mid(9), MeetingPatch::default()).await, Err(PortError::NotFound(_))));
    s.create_meeting(new_meeting(1, "A", T0)).await.unwrap();
    assert!(matches!(s.create_meeting(new_meeting(1, "A", T0)).await, Err(PortError::InvalidInput(_))));
}

#[tokio::test]
async fn list_meetings_orders_newest_first_and_paginates_with_cursor() {
    let (s, _d, _c) = store();
    for n in 1..=5u128 {
        s.create_meeting(new_meeting(n, &format!("R{n}"), T0 + n as i64 * 1000)).await.unwrap();
    }
    let p1 = s.list_meetings(MeetingQuery { limit: Some(2), ..Default::default() }).await.unwrap();
    assert_eq!(p1.items.iter().map(|m| m.title.as_str()).collect::<Vec<_>>(), vec!["R5", "R4"]);
    let c1 = p1.next_cursor.clone().expect("cursor");
    let p2 = s.list_meetings(MeetingQuery { limit: Some(2), cursor: Some(c1), ..Default::default() }).await.unwrap();
    assert_eq!(p2.items.iter().map(|m| m.title.as_str()).collect::<Vec<_>>(), vec!["R3", "R2"]);
    let p3 = s.list_meetings(MeetingQuery { limit: Some(2), cursor: p2.next_cursor, ..Default::default() }).await.unwrap();
    assert_eq!(p3.items.iter().map(|m| m.title.as_str()).collect::<Vec<_>>(), vec!["R1"]);
    assert_eq!(p3.next_cursor, None);
    assert!(matches!(
        s.list_meetings(MeetingQuery { cursor: Some("basura".into()), ..Default::default() }).await,
        Err(PortError::InvalidInput(_))
    ));
}

#[tokio::test]
async fn list_meetings_filters_by_platform_range_and_group() {
    let (s, _d, _c) = store();
    s.create_meeting(new_meeting(1, "Teams viejo", T0)).await.unwrap();
    let mut meet = new_meeting(2, "Meet", T0 + 10_000);
    meet.platform = Platform::Meet;
    s.create_meeting(meet).await.unwrap();
    s.create_meeting(new_meeting(3, "Teams nuevo", T0 + 20_000)).await.unwrap();

    let by_platform = s.list_meetings(MeetingQuery { platform: Some(Platform::Meet), ..Default::default() }).await.unwrap();
    assert_eq!(by_platform.items.len(), 1);
    let by_range = s.list_meetings(MeetingQuery { from: Some(UnixMs(T0 + 5_000)), to: Some(UnixMs(T0 + 15_000)), ..Default::default() }).await.unwrap();
    assert_eq!(by_range.items.iter().map(|m| m.title.as_str()).collect::<Vec<_>>(), vec!["Meet"]);

    let g = s.create_group("Proyecto".into(), None).await.unwrap();
    s.add_meeting(g.id, mid(3)).await.unwrap();
    let by_group = s.list_meetings(MeetingQuery { group_id: Some(g.id), ..Default::default() }).await.unwrap();
    assert_eq!(by_group.items.len(), 1);
    assert_eq!(by_group.items[0].group_ids, vec![g.id]);
}

#[tokio::test]
async fn list_meetings_text_search_is_accent_insensitive_over_title_and_transcript() {
    let (s, _d, _c) = store();
    seed_meeting(&s, 1, "Revisión trimestral", T0, &["Hablemos de la planificación", "Vale"]).await;
    seed_meeting(&s, 2, "Otra cosa", T0 + 1000, &["Nada relevante"]).await;

    let by_title = s.list_meetings(MeetingQuery { text: Some("revision".into()), ..Default::default() }).await.unwrap();
    assert_eq!(by_title.items.len(), 1);
    assert_eq!(by_title.items[0].id, mid(1));

    let by_body = s.list_meetings(MeetingQuery { text: Some("PLANIFICACION".into()), ..Default::default() }).await.unwrap();
    assert_eq!(by_body.items.len(), 1);
    assert_eq!(by_body.items[0].id, mid(1));
    assert!(by_body.items[0].snippet.as_deref().unwrap_or("").to_lowercase().contains("planificación"));

    // Una consulta con sintaxis FTS rara no rompe nada.
    let weird = s.list_meetings(MeetingQuery { text: Some("\"(( OR".into()), ..Default::default() }).await.unwrap();
    assert!(weird.items.is_empty());
}

#[tokio::test]
async fn summary_reports_speakers_open_tasks_and_captures() {
    let (s, _d, _c) = store();
    seed_meeting(&s, 1, "R", T0, &["Hola", "Buenas"]).await;
    s.rename_speaker(mid(1), sid(12), "Ana García".into(), true).await.unwrap();
    s.save_minutes(minutes_output(1, "t1")).await.unwrap();
    s.add_capture(capture(1, 1, 5_000, CaptureReason::SlideChange)).await.unwrap();

    let list = s.list_meetings(MeetingQuery::default()).await.unwrap();
    let row = &list.items[0];
    assert_eq!(row.speaker_names, vec!["Ana García".to_string(), "Yo".to_string()]);
    assert_eq!(row.my_open_tasks, 1);
    assert_eq!(row.capture_count, 1);

    s.set_action_item_done(ActionItemId::from_u128(900), true).await.unwrap();
    let list = s.list_meetings(MeetingQuery::default()).await.unwrap();
    assert_eq!(list.items[0].my_open_tasks, 0);
}

// ---------- transcript ----------

#[tokio::test]
async fn save_transcript_replaces_everything_and_roundtrips() {
    let (s, _d, _c) = store();
    seed_meeting(&s, 1, "R", T0, &["uno", "dos", "tres"]).await;
    let t = s.get_transcript(mid(1)).await.unwrap().unwrap();
    assert_eq!(t.speakers.len(), 2);
    assert_eq!(t.utterances.iter().map(|u| u.text.as_str()).collect::<Vec<_>>(), vec!["uno", "dos", "tres"]);
    assert_eq!(t.utterances[1].track, Track::System);
    assert_eq!(t.utterances[1].start, MediaMs(10_000));
    assert_eq!(t.utterances[0].confidence, Some(0.9));

    // Reemplazo total.
    seed_replace(&s).await;
    let t = s.get_transcript(mid(1)).await.unwrap().unwrap();
    assert_eq!(t.utterances.len(), 1);
    assert_eq!(t.speakers.len(), 1);
    // FTS coherente: lo viejo ya no se encuentra, lo nuevo sí.
    let n_old: i64 = s.with_conn(|c| Ok(c.query_row("SELECT count(*) FROM utterances_fts WHERE utterances_fts MATCH 'uno'", [], |r| r.get(0)).unwrap())).await.unwrap();
    let n_new: i64 = s.with_conn(|c| Ok(c.query_row("SELECT count(*) FROM utterances_fts WHERE utterances_fts MATCH 'nuevo'", [], |r| r.get(0)).unwrap())).await.unwrap();
    assert_eq!((n_old, n_new), (0, 1));

    assert_eq!(s.get_transcript(mid(77)).await.unwrap(), None);
}

async fn seed_replace(s: &acta_store::SqliteStore) {
    s.save_transcript(acta_core::transcription::Transcript {
        meeting_id: mid(1),
        speakers: vec![speaker(11, 1, "Yo", Track::Mic, true)],
        utterances: vec![utt(0, 11, Track::Mic, 0, 1000, "texto nuevo")],
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn rename_speaker_marks_user_source_and_link_voiceprint() {
    let (s, _d, _c) = store();
    seed_meeting(&s, 1, "R", T0, &["a", "b"]).await;
    s.rename_speaker(mid(1), sid(12), "Ana".into(), true).await.unwrap();
    let t = s.get_transcript(mid(1)).await.unwrap().unwrap();
    let ana = t.speakers.iter().find(|x| x.id == sid(12)).unwrap();
    assert_eq!((ana.display_name.as_deref(), ana.name_source, ana.confirmed), (Some("Ana"), NameSource::User, true));

    s.upsert_voiceprint(voiceprint(1, "Ana", vec![1.0, 0.0], "m1")).await.unwrap();
    s.link_speaker_voiceprint(sid(12), Some(VoiceprintId::from_u128(1))).await.unwrap();
    let t = s.get_transcript(mid(1)).await.unwrap().unwrap();
    assert_eq!(t.speakers.iter().find(|x| x.id == sid(12)).unwrap().voiceprint_id, Some(VoiceprintId::from_u128(1)));
    s.link_speaker_voiceprint(sid(12), None).await.unwrap();

    assert!(matches!(s.rename_speaker(mid(1), sid(999), "X".into(), true).await, Err(PortError::NotFound(_))));
    // Un hablante de otra reunión no se puede renombrar con este meeting_id.
    seed_meeting(&s, 2, "R2", T0, &["x"]).await;
    assert!(matches!(s.rename_speaker(mid(2), sid(12), "X".into(), true).await, Err(PortError::NotFound(_))));
}

// ---------- actas ----------

fn minutes_output(meeting: u128, _tag: &str) -> MinutesOutput {
    let m = mid(meeting);
    MinutesOutput {
        minutes: Minutes {
            meeting_id: m,
            summary_md: "## Resumen\nTodo bien".into(),
            decisions: vec![Decision { id: DecisionId::from_u128(800), text: "Aprobar presupuesto".into(), at: Some(MediaMs(15_000)) }],
            action_items: vec![
                ActionItem {
                    id: ActionItemId::from_u128(900),
                    meeting_id: m,
                    text: "Enviar informe".into(),
                    owner_name: Some("Edu".into()),
                    owner_speaker_id: Some(sid(meeting * 10 + 1)),
                    is_mine: true,
                    due: Some("2026-11-20".into()),
                    done: false,
                    at: Some(MediaMs(20_000)),
                    capture_ids: vec![CaptureId::from_u128(500), CaptureId::from_u128(404)],
                },
                ActionItem {
                    id: ActionItemId::from_u128(901),
                    meeting_id: m,
                    text: "Revisar contrato".into(),
                    owner_name: Some("Ana".into()),
                    owner_speaker_id: None,
                    is_mine: false,
                    due: None,
                    done: false,
                    at: None,
                    capture_ids: vec![],
                },
            ],
            provider: ProviderId::from("claude-cli"),
            model: Some("sonnet".into()),
            generated_at: UnixMs(T0 + 99),
        },
        escaleta: Escaleta {
            temas: vec![Tema {
                titulo: "Presupuesto".into(),
                resumen: "r".into(),
                inicio_ms: 0,
                fin_ms: 30_000,
                subtemas: vec![Tema { titulo: "Capex".into(), resumen: "r".into(), inicio_ms: 5_000, fin_ms: 25_000, subtemas: vec![], capturas: vec![CaptureId::from_u128(500).to_string()] }],
                capturas: vec![],
            }],
        },
        key_moments: vec![KeyMoment { at: MediaMs(15_000), reason: "decisión".into(), topic: Some("Presupuesto".into()) }],
        suggested_title: None,
    }
}

fn capture(meeting: u128, n: u128, at: u64, reason: CaptureReason) -> Capture {
    Capture {
        id: CaptureId::from_u128(500 + n - 1),
        meeting_id: mid(meeting),
        at: MediaMs(at),
        at_wall: UnixMs(T0 + at as i64),
        path: format!("captures/{at:09}-slide_change.jpg"),
        reason,
        phash: "ab".repeat(8),
        width: 1280,
        height: 720,
        keyword: None,
        topic_path: None,
        action_item_ids: vec![],
    }
}

#[tokio::test]
async fn minutes_roundtrip_replace_and_done_flag() {
    let (s, _d, _c) = store();
    seed_meeting(&s, 1, "R", T0, &["a", "b"]).await;
    assert_eq!(s.get_minutes(mid(1)).await.unwrap(), None);
    s.add_capture(capture(1, 1, 5_000, CaptureReason::SlideChange)).await.unwrap();

    s.save_minutes(minutes_output(1, "v1")).await.unwrap();
    let got = s.get_minutes(mid(1)).await.unwrap().unwrap();
    assert_eq!(got.summary_md, "## Resumen\nTodo bien");
    assert_eq!(got.decisions.len(), 1);
    assert_eq!(got.action_items.len(), 2);
    let mine = got.action_items.iter().find(|a| a.is_mine).unwrap();
    // La captura 500 existe y se enlaza; la 404 no existe y se ignora sin romper.
    assert_eq!(mine.capture_ids, vec![CaptureId::from_u128(500)]);
    assert_eq!(mine.due.as_deref(), Some("2026-11-20"));
    assert_eq!(got.model.as_deref(), Some("sonnet"));
    assert_eq!(got.provider, ProviderId::from("claude-cli"));
    assert_eq!(got.generated_at, UnixMs(T0 + 99));

    s.set_action_item_done(ActionItemId::from_u128(900), true).await.unwrap();
    assert!(s.get_minutes(mid(1)).await.unwrap().unwrap().action_items.iter().any(|a| a.done));
    assert!(matches!(s.set_action_item_done(ActionItemId::from_u128(12345), true).await, Err(PortError::NotFound(_))));

    // Reemplazo: no se duplican decisiones ni tareas.
    s.save_minutes(minutes_output(1, "v2")).await.unwrap();
    assert_eq!(s.get_minutes(mid(1)).await.unwrap().unwrap().action_items.len(), 2);
    assert!(matches!(s.save_minutes(minutes_output(42, "x")).await, Err(PortError::NotFound(_))));
}

#[tokio::test]
async fn save_minutes_links_captures_to_topic_paths() {
    let (s, _d, _c) = store();
    seed_meeting(&s, 1, "R", T0, &["a"]).await;
    s.add_capture(capture(1, 1, 6_000, CaptureReason::SlideChange)).await.unwrap();
    s.save_minutes(minutes_output(1, "v")).await.unwrap();
    let caps = s.list_captures(mid(1)).await.unwrap();
    assert_eq!(caps[0].topic_path.as_deref(), Some("1.1"));
    assert_eq!(caps[0].action_item_ids, vec![ActionItemId::from_u128(900)]);
}

#[tokio::test]
async fn meeting_detail_assembles_everything_including_audio_paths() {
    let dir = tempfile::tempdir().unwrap();
    let layout = DataLayout::new(dir.path());
    let clock = acta_core::time::ManualClock::new(UnixMs(T0));
    let s = acta_store::SqliteStore::open(&dir.path().join("acta.db")).unwrap().with_clock(std::sync::Arc::new(clock)).with_layout(layout.clone());
    seed_meeting(&s, 1, "R", T0, &["a", "b"]).await;
    s.add_capture(capture(1, 1, 5_000, CaptureReason::SlideChange)).await.unwrap();
    s.save_minutes(minutes_output(1, "v")).await.unwrap();
    let g = s.create_group("G".into(), None).await.unwrap();
    s.add_meeting(g.id, mid(1)).await.unwrap();

    // Solo existe el mix.
    std::fs::create_dir_all(layout.mix_wav(mid(1)).parent().unwrap()).unwrap();
    std::fs::write(layout.mix_wav(mid(1)), b"RIFF").unwrap();

    let d = s.get_meeting_detail(mid(1)).await.unwrap().unwrap();
    assert_eq!(d.meeting.title, "R");
    assert_eq!(d.transcript.utterances.len(), 2);
    assert!(d.minutes.is_some() && d.escaleta.is_some());
    assert_eq!(d.captures.len(), 1);
    assert_eq!(d.groups.len(), 1);
    assert_eq!(d.groups[0].meeting_count, 1);
    assert!(d.timeline.iter().any(|e| e.kind == TimelineKind::Topic));
    assert!(d.timeline.iter().any(|e| e.kind == TimelineKind::Decision));
    assert!(d.timeline.iter().any(|e| e.kind == TimelineKind::Capture));
    assert_eq!(d.audio.mix.as_deref(), Some(layout.mix_wav(mid(1)).to_str().unwrap()));
    assert_eq!((d.audio.mic.clone(), d.audio.system.clone()), (None, None));
    assert_eq!(s.get_meeting_detail(mid(5)).await.unwrap(), None);
}

// ---------- capturas ----------

#[tokio::test]
async fn capture_crud_orders_by_media_time() {
    let (s, _d, _c) = store();
    s.create_meeting(new_meeting(1, "R", T0)).await.unwrap();
    s.add_capture(capture(1, 2, 9_000, CaptureReason::Keyword)).await.unwrap();
    s.add_capture(capture(1, 1, 3_000, CaptureReason::Candidate)).await.unwrap();
    let list = s.list_captures(mid(1)).await.unwrap();
    assert_eq!(list.iter().map(|c| c.at.0).collect::<Vec<_>>(), vec![3_000, 9_000]);

    // Promover candidata a llmPick con tema.
    let mut c = list[0].clone();
    c.reason = CaptureReason::LlmPick;
    c.topic_path = Some("2".into());
    s.update_capture(c.clone()).await.unwrap();
    assert_eq!(s.list_captures(mid(1)).await.unwrap()[0], c);
    assert_eq!(s.get_capture(c.id).await.unwrap(), Some(c.clone()));

    s.delete_capture(c.id).await.unwrap();
    assert_eq!(s.get_capture(c.id).await.unwrap(), None);
    assert_eq!(s.list_captures(mid(1)).await.unwrap().len(), 1);
    assert!(matches!(s.delete_capture(c.id).await, Err(PortError::NotFound(_))));
    assert!(matches!(s.update_capture(c).await, Err(PortError::NotFound(_))));
    assert!(matches!(s.add_capture(capture(9, 3, 1, CaptureReason::Manual)).await, Err(PortError::NotFound(_))));
}

#[tokio::test]
async fn delete_meeting_cascades_to_every_table() {
    let (s, _d, _c) = store();
    seed_meeting(&s, 1, "R", T0, &["hola mundo"]).await;
    s.add_capture(capture(1, 1, 1000, CaptureReason::Manual)).await.unwrap();
    s.save_minutes(minutes_output(1, "v")).await.unwrap();
    s.delete_meeting(mid(1)).await.unwrap();
    for table in ["utterances", "speakers", "captures", "minutes", "decisions", "action_items"] {
        let n: i64 = s.with_conn(move |c| Ok(c.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0)).unwrap())).await.unwrap();
        assert_eq!(n, 0, "{table}");
    }
    let fts: i64 = s.with_conn(|c| Ok(c.query_row("SELECT count(*) FROM utterances_fts WHERE utterances_fts MATCH 'hola'", [], |r| r.get(0)).unwrap())).await.unwrap();
    assert_eq!(fts, 0);
    assert!(matches!(s.delete_meeting(mid(1)).await, Err(PortError::NotFound(_))));
}

#[tokio::test]
async fn the_failed_provider_and_model_are_kept_until_the_error_is_cleared() {
    let (s, _d, _c) = store();
    s.create_meeting(new_meeting(1, "R", T0)).await.unwrap();
    s.set_meeting_error_llm(mid(1), Some("nan".into()), Some("deepseek-v4-pro".into())).await.unwrap();
    s.patch_meeting(mid(1), MeetingPatch { error: Some(Some("falló".into())), ..Default::default() }).await.unwrap();
    let m = s.get_meeting(mid(1)).await.unwrap().unwrap();
    assert_eq!((m.error_provider.as_deref(), m.error_model.as_deref()), (Some("nan"), Some("deepseek-v4-pro")));
    let m = s.patch_meeting(mid(1), MeetingPatch { error: Some(None), ..Default::default() }).await.unwrap();
    assert_eq!((m.error_provider, m.error_model), (None, None));
    assert_eq!(s.get_meeting(mid(1)).await.unwrap().unwrap().error_provider, None);
}

// ---------- grupos ----------

#[tokio::test]
async fn group_lifecycle() {
    let (s, _d, clock) = store();
    s.create_meeting(new_meeting(1, "A", T0)).await.unwrap();
    s.create_meeting(new_meeting(2, "B", T0)).await.unwrap();
    let g = s.create_group("Proyecto X".into(), Some("desc".into())).await.unwrap();
    assert_eq!((g.kind, g.meeting_count, g.created_at), (GroupKind::Manual, 0, UnixMs(T0)));

    s.add_meeting(g.id, mid(1)).await.unwrap();
    s.add_meeting(g.id, mid(1)).await.unwrap(); // idempotente
    s.add_meeting(g.id, mid(2)).await.unwrap();
    assert_eq!(s.list_groups().await.unwrap()[0].meeting_count, 2);

    clock.advance_ms(10);
    let u = s.update_group(g.id, Some("Proyecto Y".into()), None).await.unwrap();
    assert_eq!((u.name.as_str(), u.description.as_deref(), u.updated_at), ("Proyecto Y", Some("desc"), UnixMs(T0 + 10)));

    GroupStore::remove_meeting(&s, g.id, mid(1)).await.unwrap();
    assert_eq!(s.list_groups().await.unwrap()[0].meeting_count, 1);

    assert!(matches!(s.add_meeting(g.id, mid(99)).await, Err(PortError::NotFound(_))));
    assert!(matches!(s.add_meeting(GroupId::from_u128(77), mid(1)).await, Err(PortError::NotFound(_))));
    assert!(matches!(s.update_group(GroupId::from_u128(77), None, None).await, Err(PortError::NotFound(_))));

    s.delete_group(g.id).await.unwrap();
    assert!(s.list_groups().await.unwrap().is_empty());
    // Las reuniones sobreviven.
    assert!(s.get_meeting(mid(1)).await.unwrap().is_some());
}

#[tokio::test]
async fn suggestions_save_replace_accept_dismiss() {
    let (s, _d, _c) = store();
    for n in 1..=3u128 {
        s.create_meeting(new_meeting(n, &format!("R{n}"), T0)).await.unwrap();
    }
    let sug = |n: u128, ids: Vec<u128>| GroupSuggestion {
        id: GroupId::from_u128(n),
        name: None,
        description: None,
        meeting_ids: ids.into_iter().map(mid).collect(),
        cohesion: 0.9,
        created_at: UnixMs(T0),
    };
    s.save_suggestions(vec![sug(100, vec![1, 2, 3])]).await.unwrap();
    // Reemplaza las pendientes.
    s.save_suggestions(vec![sug(101, vec![1, 2, 3]), sug(102, vec![2, 3])]).await.unwrap();
    assert_eq!(s.list_suggestions().await.unwrap().len(), 2);

    let g = s.accept_suggestion(GroupId::from_u128(101), Some("Roadmap".into())).await.unwrap();
    assert_eq!((g.id, g.name.as_str(), g.kind, g.meeting_count), (GroupId::from_u128(101), "Roadmap", GroupKind::Suggested, 3));
    assert_eq!(s.list_suggestions().await.unwrap().len(), 1);

    s.dismiss_suggestion(GroupId::from_u128(102)).await.unwrap();
    assert!(s.list_suggestions().await.unwrap().is_empty());
    assert!(matches!(s.accept_suggestion(GroupId::from_u128(555), None).await, Err(PortError::NotFound(_))));
    assert!(matches!(s.dismiss_suggestion(GroupId::from_u128(555)).await, Err(PortError::NotFound(_))));

    // Sin nombre en la sugerencia ni en la llamada: no se inventa uno.
    s.save_suggestions(vec![sug(103, vec![1, 2, 3])]).await.unwrap();
    assert!(matches!(s.accept_suggestion(GroupId::from_u128(103), None).await, Err(PortError::InvalidInput(_))));
}

#[tokio::test]
async fn deleting_a_meeting_drops_it_from_suggestions_and_drops_suggestions_left_too_small() {
    let (s, _d, _c) = store();
    for n in 1..=4u128 {
        s.create_meeting(new_meeting(n, &format!("R{n}"), T0)).await.unwrap();
    }
    let sug = |n: u128, ids: Vec<u128>| GroupSuggestion {
        id: GroupId::from_u128(n),
        name: Some("G".into()),
        description: None,
        meeting_ids: ids.into_iter().map(mid).collect(),
        cohesion: 0.9,
        created_at: UnixMs(T0),
    };
    s.save_suggestions(vec![sug(100, vec![1, 2, 3, 4]), sug(101, vec![1, 2])]).await.unwrap();
    s.delete_meeting(mid(1)).await.unwrap();
    let left = s.list_suggestions().await.unwrap();
    assert_eq!(left.len(), 1, "una sugerencia con una sola reunión ya no es un grupo");
    assert_eq!(left[0].meeting_ids, vec![mid(2), mid(3), mid(4)]);
}

// ---------- huellas ----------

fn voiceprint(n: u128, name: &str, emb: Vec<f32>, model: &str) -> Voiceprint {
    Voiceprint { id: VoiceprintId::from_u128(n), display_name: name.into(), embedding: emb, model: model.into(), samples: 1, created_at: UnixMs(T0), updated_at: UnixMs(T0) }
}

#[tokio::test]
async fn voiceprints_upsert_list_delete_and_unlink_speakers() {
    let (s, _d, _c) = store();
    seed_meeting(&s, 1, "R", T0, &["a", "b"]).await;
    s.upsert_voiceprint(voiceprint(1, "Ana", vec![1.0, 0.0, 0.0], "m1")).await.unwrap();
    s.link_speaker_voiceprint(sid(12), Some(VoiceprintId::from_u128(1))).await.unwrap();

    let mut v = voiceprint(1, "Ana García", vec![0.9, 0.1, 0.0], "m1");
    v.samples = 3;
    v.updated_at = UnixMs(T0 + 50);
    s.upsert_voiceprint(v).await.unwrap();
    let list = s.list_voiceprints().await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!((list[0].display_name.as_str(), list[0].samples, list[0].meetings, list[0].updated_at), ("Ana García", 3, 1, UnixMs(T0 + 50)));

    s.delete_voiceprint(VoiceprintId::from_u128(1)).await.unwrap();
    assert!(s.list_voiceprints().await.unwrap().is_empty());
    let t = s.get_transcript(mid(1)).await.unwrap().unwrap();
    assert_eq!(t.speakers.iter().find(|x| x.id == sid(12)).unwrap().voiceprint_id, None);
    assert!(matches!(s.delete_voiceprint(VoiceprintId::from_u128(1)).await, Err(PortError::NotFound(_))));
}

#[tokio::test]
async fn voiceprint_validation_rejects_empty_or_non_finite() {
    let (s, _d, _c) = store();
    assert!(matches!(s.upsert_voiceprint(voiceprint(1, "x", vec![], "m")).await, Err(PortError::InvalidInput(_))));
    assert!(matches!(s.upsert_voiceprint(voiceprint(1, "x", vec![f32::NAN, 1.0], "m")).await, Err(PortError::InvalidInput(_))));
}

#[tokio::test]
async fn nearest_voiceprint_is_conservative_and_model_scoped() {
    let (s, _d, _c) = store();
    s.upsert_voiceprint(voiceprint(1, "Ana", vec![1.0, 0.0, 0.0], "m1")).await.unwrap();
    s.upsert_voiceprint(voiceprint(2, "Luis", vec![0.0, 1.0, 0.0], "m1")).await.unwrap();
    s.upsert_voiceprint(voiceprint(3, "Otro modelo", vec![0.0, 0.0, 1.0], "m2")).await.unwrap();

    let (v, sim) = s.nearest_voiceprint(&[0.95, 0.05, 0.0], "m1", 0.75).await.unwrap().unwrap();
    assert_eq!(v.display_name, "Ana");
    assert!(sim > 0.99);
    // Por debajo del umbral: nada (nunca se adivina).
    assert!(s.nearest_voiceprint(&[0.6, 0.6, 0.5], "m1", 0.9).await.unwrap().is_none());
    // Otro modelo no se mezcla.
    assert!(s.nearest_voiceprint(&[0.0, 0.0, 1.0], "m1", 0.5).await.unwrap().is_none());
    // Dimensión distinta: se ignora, no explota.
    assert!(s.nearest_voiceprint(&[1.0, 0.0], "m1", 0.5).await.unwrap().is_none());
}

#[tokio::test]
async fn rank_voiceprints_returns_sorted_suggestions_only() {
    let (s, _d, _c) = store();
    s.upsert_voiceprint(voiceprint(1, "Ana", vec![1.0, 0.0], "m1")).await.unwrap();
    s.upsert_voiceprint(voiceprint(2, "Luis", vec![0.8, 0.6], "m1")).await.unwrap();
    s.upsert_voiceprint(voiceprint(3, "Pedro", vec![0.0, 1.0], "m1")).await.unwrap();
    let ranked = s.rank_voiceprints(&[1.0, 0.0], "m1", 0.75, 3).await.unwrap();
    assert_eq!(ranked.iter().map(|(v, _)| v.display_name.as_str()).collect::<Vec<_>>(), vec!["Ana", "Luis"]);
    assert!(ranked[0].1 >= ranked[1].1);
}

// ---------- chat ----------

#[tokio::test]
async fn chat_threads_messages_and_scopes() {
    let (s, _d, clock) = store();
    s.create_meeting(new_meeting(1, "R", T0)).await.unwrap();
    let t1 = s.create_thread(ChatScope::Meeting { meeting_id: mid(1) }, "Sobre R".into()).await.unwrap();
    clock.advance_ms(10);
    let t2 = s.create_thread(ChatScope::All, "Todo".into()).await.unwrap();

    assert_eq!(s.list_threads(None).await.unwrap().iter().map(|t| t.id).collect::<Vec<_>>(), vec![t2.id, t1.id]);
    assert_eq!(s.list_threads(Some(ChatScope::All)).await.unwrap().len(), 1);
    assert_eq!(s.list_threads(Some(ChatScope::Meeting { meeting_id: mid(1) })).await.unwrap()[0].scope, ChatScope::Meeting { meeting_id: mid(1) });

    let cite = Citation { n: 1, meeting_id: mid(1), meeting_title: "R".into(), at: MediaMs(1000), end: MediaMs(2000), snippet: "x".into() };
    let msg = |n: u128, role, content: &str, at: i64, cites: Vec<Citation>| ChatMessage {
        id: ChatMessageId::from_u128(n),
        thread_id: t1.id,
        role,
        content: content.into(),
        citations: cites,
        provider: Some(ProviderId::from("claude-cli")),
        created_at: UnixMs(at),
    };
    s.append_message(msg(2, ChatRole::Assistant, "respuesta [1]", T0 + 20, vec![cite.clone()])).await.unwrap();
    s.append_message(msg(1, ChatRole::User, "pregunta", T0 + 10, vec![])).await.unwrap();
    let msgs = s.list_messages(t1.id).await.unwrap();
    assert_eq!(msgs.iter().map(|m| m.content.as_str()).collect::<Vec<_>>(), vec!["pregunta", "respuesta [1]"]);
    assert_eq!(msgs[1].citations, vec![cite]);
    assert_eq!(msgs[1].provider, Some(ProviderId::from("claude-cli")));

    // Añadir un mensaje sube el updated_at del hilo.
    assert_eq!(s.list_threads(None).await.unwrap()[0].id, t1.id);

    let mut orphan = msg(3, ChatRole::User, "x", T0, vec![]);
    orphan.thread_id = ChatThreadId::from_u128(999);
    assert!(matches!(s.append_message(orphan).await, Err(PortError::NotFound(_))));

    s.delete_thread(t1.id).await.unwrap();
    assert!(s.list_messages(t1.id).await.unwrap().is_empty());
    assert!(matches!(s.delete_thread(t1.id).await, Err(PortError::NotFound(_))));
}

// ---------- ajustes ----------

/// Una instalación anterior a los valores neutros conserva SUS ajustes: lo
/// guardado manda sobre los valores de fábrica (que solo son para instalaciones nuevas).
#[tokio::test]
async fn settings_saved_by_an_older_version_load_as_they_were() {
    let (s, _d, _c) = store();
    let stored = r#"{"user":{"displayName":"Edu","aliases":["Eduardo","Edu Navajas"]},
        "transcription":{"language":"es","diarization":true,"livePreview":true,"voiceprintMinSimilarityPct":75},
        "llm":{"providers":[
          {"id":"claude-cli","label":"Claude Code (CLI)","enabled":true,"timeoutS":600,
           "kind":{"type":"claudeCli","binaryPath":"/opt/homebrew/bin/claude","model":null,"effort":null,"extraEnv":{"CLAUDE_CONFIG_DIR":"/Users/someone/.claude-personal"}}},
          {"id":"nan","label":"NaN Builders","enabled":true,"timeoutS":600,
           "kind":{"type":"openAiCompatible","baseUrl":"https://api.nan.builders/v1","model":"deepseek-v4-pro","apiKey":{"service":"acta","account":"provider:nan"}}}],
          "minutesProvider":"claude-cli","chatProvider":"nan","namingProvider":"claude-cli"},
        "onboarding":{"completed":true}}"#;
    s.with_conn(move |conn| {
        conn.execute("INSERT INTO settings(key,value) VALUES ('app', ?1)", [stored]).map_err(|e| acta_core::PortError::Internal(e.to_string()))?;
        Ok(())
    })
    .await
    .unwrap();
    let got = s.load_settings().await.unwrap();
    assert_eq!((got.user.display_name.as_str(), got.user.aliases.len()), ("Edu", 2));
    assert_eq!(got.transcription.language, "es");
    assert_eq!(got.llm.providers.iter().map(|p| (p.id.0.as_str(), p.enabled)).collect::<Vec<_>>(), [("claude-cli", true), ("nan", true)]);
    assert_eq!((got.llm.minutes_provider.clone(), got.llm.chat_provider.clone()), (Some("claude-cli".into()), Some("nan".into())));
    assert!(got.onboarding.completed);
    // Guardarlos de nuevo no los cambia.
    s.save_settings(got.clone()).await.unwrap();
    assert_eq!(s.load_settings().await.unwrap(), got);
}

#[tokio::test]
async fn settings_default_then_roundtrip_with_secret_refs_only() {
    use acta_core::llm::{ProviderConfig, ProviderKind, SecretRef};
    let (s, _d, _c) = store();
    assert_eq!(s.load_settings().await.unwrap(), Settings::default());

    let mut st = Settings::default();
    st.user.display_name = "Eduardo".into();
    st.llm.providers.push(ProviderConfig {
        id: ProviderId::from("openrouter"),
        label: "OpenRouter".into(),
        enabled: true,
        kind: ProviderKind::OpenAiCompatible {
            base_url: "https://openrouter.ai/api/v1".into(),
            model: "x".into(),
            api_key: Some(SecretRef { service: "acta".into(), account: "provider:openrouter".into() }),
        },
        timeout_s: 120,
    });
    s.save_settings(st.clone()).await.unwrap();
    assert_eq!(s.load_settings().await.unwrap(), st);

    // Lo que hay en disco es solo la referencia al Llavero.
    let raw: String = s.with_conn(|c| Ok(c.query_row("SELECT value FROM settings WHERE key='app'", [], |r| r.get(0)).unwrap())).await.unwrap();
    assert!(raw.contains("provider:openrouter"));
    assert!(!raw.contains("sk-"));
}

// ---------- pantalla compartida, título sugerido, tareas al reprocesar ----------

#[tokio::test]
async fn screen_shares_replace_roundtrip_in_timeline_and_cascade() {
    let (s, _d, _c) = store();
    seed_meeting(&s, 1, "R", T0, &["a"]).await;
    let share = |a: u64, b: Option<u64>, p: Option<&str>| ScreenShare { start: MediaMs(a), end: b.map(MediaMs), presenter: p.map(Into::into) };
    s.save_screen_shares(mid(1), vec![share(1_000, Some(2_000), None), share(3_000, None, Some("Ana"))]).await.unwrap();
    let tl = s.get_meeting_detail(mid(1)).await.unwrap().unwrap().timeline;
    let shares: Vec<_> = tl.iter().filter(|e| e.kind == TimelineKind::ScreenShare).collect();
    assert_eq!(shares.len(), 2);
    assert_eq!((shares[0].at, shares[0].end, shares[0].label.as_str()), (MediaMs(1_000), Some(MediaMs(2_000)), "Pantalla compartida"));
    assert_eq!((shares[1].end, shares[1].label.as_str(), shares[1].ref_id.clone()), (None, "Comparte: Ana", None));

    // Reemplaza, no acumula.
    s.save_screen_shares(mid(1), vec![share(9_000, None, None)]).await.unwrap();
    let n = s.get_meeting_detail(mid(1)).await.unwrap().unwrap().timeline.iter().filter(|e| e.kind == TimelineKind::ScreenShare).count();
    assert_eq!(n, 1);
    assert!(matches!(s.save_screen_shares(mid(42), vec![]).await, Err(PortError::NotFound(_))));

    s.delete_meeting(mid(1)).await.unwrap();
    let left: i64 = s.with_conn(|c| Ok(c.query_row("SELECT count(*) FROM screen_shares", [], |r| r.get(0)).unwrap())).await.unwrap();
    assert_eq!(left, 0);
}

#[tokio::test]
async fn suggested_title_applies_unless_user_edited() {
    let (s, _d, _c) = store();
    seed_meeting(&s, 1, "Reunión sin título", T0, &["a"]).await;
    let mut out = minutes_output(1, "v");
    out.suggested_title = Some("  Presupuesto 2027  ".into());
    s.save_minutes(out.clone()).await.unwrap();
    assert_eq!(s.get_meeting(mid(1)).await.unwrap().unwrap().title, "Presupuesto 2027");

    // Una sugerencia vacía no pisa nada; otra sugerencia sí (el usuario no editó).
    out.suggested_title = Some("   ".into());
    s.save_minutes(out.clone()).await.unwrap();
    assert_eq!(s.get_meeting(mid(1)).await.unwrap().unwrap().title, "Presupuesto 2027");
    out.suggested_title = Some("Otro".into());
    s.save_minutes(out.clone()).await.unwrap();
    assert_eq!(s.get_meeting(mid(1)).await.unwrap().unwrap().title, "Otro");

    // Edición del usuario: se respeta para siempre.
    s.patch_meeting(mid(1), MeetingPatch { title: Some("Mi título".into()), ..Default::default() }).await.unwrap();
    out.suggested_title = Some("Machacar".into());
    s.save_minutes(out).await.unwrap();
    assert_eq!(s.get_meeting(mid(1)).await.unwrap().unwrap().title, "Mi título");

    // Parchear otras cosas no cuenta como edición de título.
    seed_meeting(&s, 2, "T2", T0, &["a"]).await;
    s.patch_meeting(mid(2), MeetingPatch { status: Some(MeetingStatus::Ready), ..Default::default() }).await.unwrap();
    let mut o2 = minutes_output(2, "v");
    o2.minutes.decisions.clear();
    o2.minutes.action_items.clear();
    o2.suggested_title = Some("Sugerido".into());
    s.save_minutes(o2).await.unwrap();
    assert_eq!(s.get_meeting(mid(2)).await.unwrap().unwrap().title, "Sugerido");
}

#[tokio::test]
async fn reprocessing_keeps_done_flags_by_folded_text() {
    let (s, _d, _c) = store();
    seed_meeting(&s, 1, "R", T0, &["a", "b"]).await;
    s.save_minutes(minutes_output(1, "v1")).await.unwrap();
    s.set_action_item_done(ActionItemId::from_u128(900), true).await.unwrap();

    // Nueva generación: ids nuevos, texto reformulado en mayúsculas/acentos; la otra tarea cambia.
    let mut v2 = minutes_output(1, "v2");
    v2.minutes.action_items[0].id = ActionItemId::from_u128(950);
    v2.minutes.action_items[0].text = "  ENVIAR INFORME ".into();
    v2.minutes.action_items[1].id = ActionItemId::from_u128(951);
    v2.minutes.action_items[1].text = "Llamar a proveedor".into();
    s.save_minutes(v2).await.unwrap();
    let items = s.get_minutes(mid(1)).await.unwrap().unwrap().action_items;
    assert_eq!(items.iter().map(|a| a.done).collect::<Vec<_>>(), vec![true, false]);

    // Un `done=true` entrante se mantiene.
    let mut v3 = minutes_output(1, "v3");
    v3.minutes.action_items[1].done = true;
    s.save_minutes(v3).await.unwrap();
    let items = s.get_minutes(mid(1)).await.unwrap().unwrap().action_items;
    assert_eq!(items.iter().filter(|a| a.done).count(), 2);
}

/// Orden del título: Teams/Calendario mandan sobre el del LLM; el del
/// usuario sobre todos.
#[tokio::test]
async fn a_title_from_teams_or_the_calendar_is_not_replaced_by_the_llm() {
    let (s, _d, _c) = store();
    seed_meeting(&s, 1, "Reunión de Teams", T0, &["a"]).await;
    let m = s.set_auto_title(mid(1), "Daily Talento".into()).await.unwrap().unwrap();
    assert_eq!(m.title, "Daily Talento");
    let mut out = minutes_output(1, "v");
    out.suggested_title = Some("Seguimiento diario de subcuentas".into());
    s.save_minutes(out).await.unwrap();
    assert_eq!(s.get_meeting(mid(1)).await.unwrap().unwrap().title, "Daily Talento");
    // El usuario sí puede cambiarlo, y entonces ya no se toca automáticamente.
    s.patch_meeting(mid(1), MeetingPatch { title: Some("Mío".into()), ..Default::default() }).await.unwrap();
    assert!(s.set_auto_title(mid(1), "Otro".into()).await.unwrap().is_none());
}
