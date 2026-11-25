//! Интеграция с системным плеером macOS (MediaPlayer.framework): медиаклавиши клавиатуры,
//! кнопки AirPods/Bluetooth-наушников, карточка «Исполняется» в Пункте управления и на экране
//! блокировки с обложкой и перемоткой.

use crate::api::Track;
use crate::player::{Player, Shared};
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Bool};
use objc2::{msg_send, AnyThread};
use objc2_app_kit::NSImage;
use objc2_foundation::{NSData, NSMutableDictionary, NSNumber, NSSize, NSString};
use slint::{Timer, TimerMode};
use std::cell::RefCell;
use std::ptr::NonNull;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

#[link(name = "MediaPlayer", kind = "framework")]
extern "C" {
    static MPMediaItemPropertyTitle: &'static NSString;
    static MPMediaItemPropertyArtist: &'static NSString;
    static MPMediaItemPropertyAlbumTitle: &'static NSString;
    static MPMediaItemPropertyPlaybackDuration: &'static NSString;
    static MPMediaItemPropertyArtwork: &'static NSString;
    static MPNowPlayingInfoPropertyElapsedPlaybackTime: &'static NSString;
    static MPNowPlayingInfoPropertyPlaybackRate: &'static NSString;
    static MPNowPlayingInfoPropertyMediaType: &'static NSString;
}

const STATUS_SUCCESS: isize = 0;
const PLAYBACK_PLAYING: usize = 1;
const PLAYBACK_PAUSED: usize = 2;
const PLAYBACK_STOPPED: usize = 3;
const MEDIA_TYPE_AUDIO: usize = 1;

/// Команда от медиаклавиш или Пункта управления macOS. Время — в секундах.
pub enum Event {
    PlayPause,
    Play,
    Pause,
    Next,
    Prev,
    Seek(f32),
}

struct State {
    last_track: Option<String>,
    last_state: usize,
    last_duration: f32,
    last_pos: Option<(Instant, f32)>,
    /// Текущая скачанная обложка для `last_track` и флаг, что её ещё нужно отдать системе.
    art: Option<(String, Arc<Vec<u8>>)>,
    art_dirty: bool,
    /// Токены и блоки обработчиков команд живут до конца работы приложения.
    _tokens: Vec<Retained<AnyObject>>,
    _blocks: Vec<RcBlock<dyn Fn(NonNull<AnyObject>) -> isize>>,
    _timer: Timer,
}

thread_local! {
    static NOW_PLAYING: RefCell<Option<State>> = const { RefCell::new(None) };
}

fn http_client() -> &'static reqwest::blocking::Client {
    static HTTP: OnceLock<reqwest::blocking::Client> = OnceLock::new();
    HTTP.get_or_init(|| {
        reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(10))
            .connect_timeout(Duration::from_secs(5))
            .build()
            .expect("nowplaying http client")
    })
}

/// Подключить медиаклавиши и синхронизацию «Исполняется» в главном потоке.
pub fn install(player: Player, on_event: impl Fn(Event) + Send + Sync + 'static) {
    let Some(center_cls) = AnyClass::get(c"MPRemoteCommandCenter") else { return };
    let center: Option<Retained<AnyObject>> = unsafe { msg_send![center_cls, sharedCommandCenter] };
    let Some(center) = center else { return };

    let cb: Arc<dyn Fn(Event) + Send + Sync> = Arc::new(on_event);
    let mut tokens = Vec::new();
    let mut blocks = Vec::new();

    let mut bind_simple = |cmd: Option<Retained<AnyObject>>, make: fn() -> Event| {
        let Some(cmd) = cmd else { return };
        let cb = cb.clone();
        let block = RcBlock::new(move |_ev: NonNull<AnyObject>| -> isize {
            cb(make());
            STATUS_SUCCESS
        });
        unsafe {
            let _: () = msg_send![&*cmd, setEnabled: Bool::YES];
            let token: Option<Retained<AnyObject>> = msg_send![&*cmd, addTargetWithHandler: &*block];
            if let Some(t) = token {
                tokens.push(t);
            }
        }
        blocks.push(block);
    };

    unsafe {
        bind_simple(msg_send![&*center, togglePlayPauseCommand], || Event::PlayPause);
        bind_simple(msg_send![&*center, playCommand], || Event::Play);
        bind_simple(msg_send![&*center, pauseCommand], || Event::Pause);
        bind_simple(msg_send![&*center, stopCommand], || Event::Pause);
        bind_simple(msg_send![&*center, nextTrackCommand], || Event::Next);
        bind_simple(msg_send![&*center, previousTrackCommand], || Event::Prev);

        if let Some(seek_cmd) = msg_send![&*center, changePlaybackPositionCommand] {
            let seek_cmd: Retained<AnyObject> = seek_cmd;
            let cb = cb.clone();
            let block = RcBlock::new(move |ev: NonNull<AnyObject>| -> isize {
                let pos: f64 = msg_send![ev.as_ref(), positionTime];
                if pos.is_finite() {
                    cb(Event::Seek(pos.max(0.0) as f32));
                }
                STATUS_SUCCESS
            });
            let _: () = msg_send![&*seek_cmd, setEnabled: Bool::NO];
            let token: Option<Retained<AnyObject>> = msg_send![&*seek_cmd, addTargetWithHandler: &*block];
            if let Some(t) = token {
                tokens.push(t);
            }
            blocks.push(block);
        }
    }

    let timer = Timer::default();
    let p = player.clone();
    timer.start(TimerMode::Repeated, Duration::from_millis(500), move || {
        sync(&p);
    });

    NOW_PLAYING.with(|cell| {
        *cell.borrow_mut() = Some(State {
            last_track: None,
            last_state: PLAYBACK_STOPPED,
            last_duration: 0.0,
            last_pos: None,
            art: None,
            art_dirty: false,
            _tokens: tokens,
            _blocks: blocks,
            _timer: timer,
        });
    });

    sync(&player);
}

/// Очистить карточку «Исполняется» при выходе из приложения.
pub fn clear() {
    let Some(cls) = AnyClass::get(c"MPNowPlayingInfoCenter") else { return };
    let center: Option<Retained<AnyObject>> = unsafe { msg_send![cls, defaultCenter] };
    let Some(center) = center else { return };
    unsafe {
        let nil: *const AnyObject = std::ptr::null();
        let _: () = msg_send![&*center, setNowPlayingInfo: nil];
        let _: () = msg_send![&*center, setPlaybackState: PLAYBACK_STOPPED];
    }
}

fn sync(player: &Player) {
    let s = player.state();
    let fetch_cover = NOW_PLAYING.with(|cell| {
        let mut guard = cell.borrow_mut();
        let st = guard.as_mut()?;

        let id = s.track.as_ref().map(|t| t.id.clone());
        let track_changed = id != st.last_track;
        let state = playback_state(&s);
        let state_changed = state != st.last_state;
        let dur = effective_duration(&s);
        let dur_changed = (dur - st.last_duration).abs() > 0.5;
        let expected = st
            .last_pos
            .map(|(at, pos)| if s.playing { pos + at.elapsed().as_secs_f32() } else { pos });
        let jumped = expected.is_some_and(|e| (e - s.position).abs() > 1.5);

        let mut to_fetch = None;
        if track_changed {
            st.last_track = id.clone();
            st.last_duration = 0.0;
            st.last_pos = None;
            st.art = None;
            st.art_dirty = false;
            set_track_commands_enabled(s.track.is_some());
            if let Some(t) = &s.track {
                if let Some(url) = t.cover_url(400) {
                    to_fetch = Some((t.id.clone(), url));
                }
            }
        }

        if track_changed || state_changed || dur_changed || jumped || st.art_dirty {
            let art = st.art.as_ref().and_then(|(aid, b)| (Some(aid) == id.as_ref()).then(|| b.clone()));
            publish_info(s.track.as_ref(), state, s.position, dur, art);
            st.last_state = state;
            st.last_duration = dur;
            st.art_dirty = false;
        }
        st.last_pos = Some((Instant::now(), s.position));
        to_fetch
    });

    if let Some((track_id, url)) = fetch_cover {
        let p = player.clone();
        std::thread::spawn(move || {
            let Ok(resp) = http_client().get(&url).send() else { return };
            if !resp.status().is_success() {
                return;
            }
            let Ok(bytes) = resp.bytes() else { return };
            if bytes.is_empty() {
                return;
            }
            let data = Arc::new(bytes.to_vec());
            let _ = slint::invoke_from_event_loop(move || {
                let updated = NOW_PLAYING.with(|cell| {
                    let mut guard = cell.borrow_mut();
                    let Some(st) = guard.as_mut() else { return false };
                    if st.last_track.as_deref() == Some(track_id.as_str()) {
                        st.art = Some((track_id, data));
                        st.art_dirty = true;
                        true
                    } else {
                        false
                    }
                });
                if updated {
                    sync(&p);
                }
            });
        });
    }
}

fn set_track_commands_enabled(enabled: bool) {
    let Some(cls) = AnyClass::get(c"MPRemoteCommandCenter") else { return };
    let center: Option<Retained<AnyObject>> = unsafe { msg_send![cls, sharedCommandCenter] };
    let Some(center) = center else { return };
    let flag = Bool::new(enabled);
    unsafe {
        if let Some(cmd) = msg_send![&*center, nextTrackCommand] {
            let cmd: Retained<AnyObject> = cmd;
            let _: () = msg_send![&*cmd, setEnabled: flag];
        }
        if let Some(cmd) = msg_send![&*center, previousTrackCommand] {
            let cmd: Retained<AnyObject> = cmd;
            let _: () = msg_send![&*cmd, setEnabled: flag];
        }
        if let Some(cmd) = msg_send![&*center, changePlaybackPositionCommand] {
            let cmd: Retained<AnyObject> = cmd;
            let _: () = msg_send![&*cmd, setEnabled: flag];
        }
    }
}

fn publish_info(
    track: Option<&Track>,
    state: usize,
    position: f32,
    duration: f32,
    art: Option<Arc<Vec<u8>>>,
) {
    let Some(cls) = AnyClass::get(c"MPNowPlayingInfoCenter") else { return };
    let center: Option<Retained<AnyObject>> = unsafe { msg_send![cls, defaultCenter] };
    let Some(center) = center else { return };

    let Some(t) = track else {
        unsafe {
            let nil: *const AnyObject = std::ptr::null();
            let _: () = msg_send![&*center, setNowPlayingInfo: nil];
            let _: () = msg_send![&*center, setPlaybackState: PLAYBACK_STOPPED];
        }
        return;
    };

    let dict = NSMutableDictionary::<NSString, AnyObject>::new();
    let title = format_title(t);
    set_obj(&dict, unsafe { MPMediaItemPropertyTitle }, &NSString::from_str(&title));
    set_obj(&dict, unsafe { MPMediaItemPropertyArtist }, &NSString::from_str(&t.artists));
    if !t.album_title.is_empty() {
        set_obj(&dict, unsafe { MPMediaItemPropertyAlbumTitle }, &NSString::from_str(&t.album_title));
    }
    if duration > 0.0 {
        let num = NSNumber::new_f64(f64::from(duration));
        set_obj(&dict, unsafe { MPMediaItemPropertyPlaybackDuration }, &num);
    }
    let elapsed = NSNumber::new_f64(f64::from(position.max(0.0)));
    set_obj(&dict, unsafe { MPNowPlayingInfoPropertyElapsedPlaybackTime }, &elapsed);
    let rate = NSNumber::new_f64(if state == PLAYBACK_PLAYING { 1.0 } else { 0.0 });
    set_obj(&dict, unsafe { MPNowPlayingInfoPropertyPlaybackRate }, &rate);
    let media_type = NSNumber::new_usize(MEDIA_TYPE_AUDIO);
    set_obj(&dict, unsafe { MPNowPlayingInfoPropertyMediaType }, &media_type);

    if let Some(bytes) = art {
        if let Some(artwork) = make_artwork(bytes) {
            set_obj(&dict, unsafe { MPMediaItemPropertyArtwork }, &artwork);
        }
    }

    unsafe {
        let _: () = msg_send![&*center, setNowPlayingInfo: &*dict];
        let _: () = msg_send![&*center, setPlaybackState: state];
    }
}

fn set_obj(dict: &NSMutableDictionary<NSString, AnyObject>, key: &NSString, val: &AnyObject) {
    unsafe {
        let _: () = msg_send![dict, setObject: val, forKey: key];
    }
}

fn make_artwork(bytes: Arc<Vec<u8>>) -> Option<Retained<AnyObject>> {
    let cls = AnyClass::get(c"MPMediaItemArtwork")?;
    let bounds = NSSize::new(400.0, 400.0);
    // Создаём NSImage внутри обработчика из байтов: MediaPlayer может вызывать блок
    // на своей очереди, а Arc<Vec<u8>> потокобезопасен.
    let handler = RcBlock::new(move |_size: NSSize| -> *mut NSImage {
        let data = NSData::with_bytes(&bytes);
        match NSImage::initWithData(NSImage::alloc(), &data) {
            Some(img) => Retained::autorelease_return(img),
            None => std::ptr::null_mut(),
        }
    });
    unsafe {
        let alloc: *mut AnyObject = msg_send![cls, alloc];
        let obj: *mut AnyObject = msg_send![alloc, initWithBoundsSize: bounds, requestHandler: &*handler];
        Retained::from_raw(obj)
    }
}

fn format_title(t: &Track) -> String {
    format_title_parts(&t.title, &t.version)
}

fn format_title_parts(title: &str, version: &str) -> String {
    if version.is_empty() {
        title.to_string()
    } else {
        format!("{title} ({version})")
    }
}

fn playback_state(s: &Shared) -> usize {
    match (&s.track, s.playing) {
        (None, _) => PLAYBACK_STOPPED,
        (Some(_), true) => PLAYBACK_PLAYING,
        (Some(_), false) => PLAYBACK_PAUSED,
    }
}

fn effective_duration(s: &Shared) -> f32 {
    pick_duration(s.duration, s.track.as_ref().map_or(0, |t| t.duration_ms))
}

fn pick_duration(duration: f32, track_ms: u64) -> f32 {
    if duration > 0.0 {
        duration
    } else if track_ms > 0 {
        track_ms as f32 / 1000.0
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_includes_version() {
        assert_eq!(format_title_parts("Track", ""), "Track");
        assert_eq!(format_title_parts("Track", "Remix"), "Track (Remix)");
    }

    #[test]
    fn duration_falls_back_to_track_ms() {
        assert_eq!(pick_duration(0.0, 215_000), 215.0);
        assert_eq!(pick_duration(214.5, 215_000), 214.5);
        assert_eq!(pick_duration(0.0, 0), 0.0);
    }
}
