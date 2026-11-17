//! Публикация состояния аудиопотока в Now Playing без обновления каждый кадр.
use crate::player::Shared;
use serde::Serialize;
use std::{cell::RefCell, time::Instant};

#[derive(Clone, Debug, Serialize, PartialEq)]
struct Snapshot {
    id: String,
    title: String,
    artist: String,
    album: String,
    cover: String,
    duration: f32,
    position: f32,
    playing: bool,
}
fn finite_seconds(value: f32) -> f32 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    }
}
impl Snapshot {
    fn from_state(s: &Shared) -> Self {
        let t = s.track.as_ref();
        Self {
            id: t.map(|t| t.full_id()).unwrap_or_default(),
            title: t
                .map(|t| {
                    if t.version.is_empty() {
                        t.title.clone()
                    } else {
                        format!("{} ({})", t.title, t.version)
                    }
                })
                .unwrap_or_default(),
            artist: t.map(|t| t.artists.clone()).unwrap_or_default(),
            album: t.map(|t| t.album_title.clone()).unwrap_or_default(),
            cover: t.and_then(|t| t.cover_url(400)).unwrap_or_default(),
            duration: finite_seconds(if s.duration > 0.0 {
                s.duration
            } else {
                t.map_or(0.0, |t| t.duration_ms as f32 / 1000.0)
            }),
            position: finite_seconds(s.position),
            playing: s.playing && !s.loading && t.is_some(),
        }
    }
    fn changed(&self, last: &Self, elapsed: f32) -> bool {
        let expected = last.position + if last.playing { elapsed } else { 0.0 };
        self.id != last.id
            || self.title != last.title
            || self.artist != last.artist
            || self.album != last.album
            || self.cover != last.cover
            || self.duration != last.duration
            || self.playing != last.playing
            || (self.position - expected).abs() > 1.0
    }
}
thread_local! { static LAST: RefCell<Option<(Snapshot, Instant)>> = const { RefCell::new(None) }; }
unsafe extern "C" {
    fn ym_media_update(bytes: *const u8, len: usize);
}
pub fn update(state: &Shared) {
    let snapshot = Snapshot::from_state(state);
    LAST.with(|last| {
        let mut last = last.borrow_mut();
        if last
            .as_ref()
            .is_some_and(|(s, at)| !snapshot.changed(s, at.elapsed().as_secs_f32()))
        {
            return;
        }
        if let Ok(json) = serde_json::to_vec(&snapshot) {
            // SAFETY: native код копирует буфер синхронно, вызов из GUI-потока.
            unsafe {
                ym_media_update(json.as_ptr(), json.len());
            }
            *last = Some((snapshot, Instant::now()));
        }
    });
}
pub(super) fn reset() {
    LAST.with(|last| *last.borrow_mut() = None);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn progress_does_not_republish_but_seek_and_pause_do() {
        let mut s = Snapshot::from_state(&Shared::default());
        s.id = "test".into();
        s.playing = true;
        s.duration = 200.0;
        s.position = 10.0;
        let mut next = s.clone();
        next.position = 13.0;
        assert!(!next.changed(&s, 3.0));
        next.position = 50.0;
        assert!(next.changed(&s, 3.0));
        next.position = 13.0;
        next.playing = false;
        assert!(next.changed(&s, 3.0));
    }
    #[test]
    fn non_finite_positions_are_sanitized() {
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -5.0] {
            assert_eq!(finite_seconds(value), 0.0);
        }
    }
}
