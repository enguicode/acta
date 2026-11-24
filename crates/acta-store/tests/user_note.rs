#![allow(clippy::expect_used, clippy::unwrap_used)]
//! Nota personal del usuario en una reunión.

mod common;

use acta_core::library::*;
use common::*;

#[tokio::test]
async fn a_meeting_has_no_note_until_the_user_writes_one() {
    let (s, _d, _c) = store();
    seed_meeting(&s, 1, "Daily", T0, &["hola"]).await;
    assert_eq!(s.get_meeting(mid(1)).await.unwrap().unwrap().user_note, None);
}

#[tokio::test]
async fn the_note_persists_survives_other_patches_and_can_be_cleared() {
    let (s, _d, _c) = store();
    seed_meeting(&s, 1, "Daily", T0, &["hola"]).await;
    let m = s.patch_meeting(mid(1), MeetingPatch { user_note: Some(Some("Pedir presupuesto a Ana".into())), ..Default::default() }).await.unwrap();
    assert_eq!(m.user_note.as_deref(), Some("Pedir presupuesto a Ana"));
    s.patch_meeting(mid(1), MeetingPatch { title: Some("Otro".into()), ..Default::default() }).await.unwrap();
    let detail = s.get_meeting_detail(mid(1)).await.unwrap().unwrap();
    assert_eq!(detail.meeting.user_note.as_deref(), Some("Pedir presupuesto a Ana"));
    let m = s.patch_meeting(mid(1), MeetingPatch { user_note: Some(None), ..Default::default() }).await.unwrap();
    assert_eq!(m.user_note, None);
}
