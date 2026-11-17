//! macOS: медиаклавиши, AirPods и «Исполняется» в Пункте управления через MediaPlayer.framework.
//! Аналог MPRIS на Linux (src/mpris.rs): состояние берём у плеера сами, команды отдаём в
//! поток интерфейса. Обработчики команд система зовёт в главном потоке, туда же ставим и
//! таймер, который переносит состояние плеера в MPNowPlayingInfoCenter.

use crate::api::Track;
use crate::config::log;
use crate::player::{Player, Shared};
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::AnyThread;
use objc2_app_kit::NSImage;
use objc2_core_foundation::CGSize;
use objc2_foundation::{NSData, NSDictionary, NSNumber, NSString};
use objc2_media_player::{
    MPChangePlaybackPositionCommandEvent, MPMediaItemArtwork, MPMediaItemPropertyAlbumTitle, MPMediaItemPropertyArtist,
    MPMediaItemPropertyArtwork, MPMediaItemPropertyPlaybackDuration, MPMediaItemPropertyTitle, MPNowPlayingInfoCenter,
    MPNowPlayingInfoPropertyElapsedPlaybackTime, MPNowPlayingInfoPropertyPlaybackRate, MPNowPlayingPlaybackState,
    MPRemoteCommand, MPRemoteCommandCenter, MPRemoteCommandEvent, MPRemoteCommandHandlerStatus,
};
use std::cell::RefCell;
use std::ptr::NonNull;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// Команда от системы. Время — в секундах. Набор тот же, что у MPRIS, чтобы gui.rs
/// обрабатывал обе платформы одним кодом.
pub enum Event {
    PlayPause,
    Play,
    Pause,
    Next,
    Prev,
    Seek(f32),
    // Перемотки на шаг, громкости, окна и выхода в Пункте управления нет.
    #[allow(dead_code)]
    SeekBy(f32),
    #[allow(dead_code)]
    Volume(f32),
    #[allow(dead_code)]
    Raise,
    #[allow(dead_code)]
    Quit,
}

/// Что сейчас показано в «Исполняется»: обновляем, только когда что-то поменялось.
#[derive(Default)]
struct Shown {
    track: Option<String>,
    playing: bool,
    /// Когда и на какой позиции последний раз сообщали время: по скорости 1.0 система
    /// дальше считает сама, а скачок — это перемотка.
    at: Option<(Instant, f32)>,
    artwork: Option<Retained<MPMediaItemArtwork>>,
}

thread_local! {
    static SHOWN: RefCell<Shown> = RefCell::new(Shown::default());
    static TIMER: slint::Timer = slint::Timer::default();
}

/// Подписаться на команды и начать показывать трек. Звать из главного потока.
pub fn spawn(player: Player, on_event: impl Fn(Event) + Send + 'static) {
    let on_event = Rc::new(on_event);
    unsafe {
        let center = MPRemoteCommandCenter::sharedCommandCenter();
        let simple = |cmd: Retained<MPRemoteCommand>, e: fn() -> Event| {
            let f = on_event.clone();
            let block = RcBlock::new(move |_: NonNull<MPRemoteCommandEvent>| {
                f(e());
                MPRemoteCommandHandlerStatus::Success
            });
            cmd.setEnabled(true);
            cmd.addTargetWithHandler(&block);
        };
        simple(center.togglePlayPauseCommand(), || Event::PlayPause);
        simple(center.playCommand(), || Event::Play);
        simple(center.pauseCommand(), || Event::Pause);
        simple(center.stopCommand(), || Event::Pause);
        simple(center.nextTrackCommand(), || Event::Next);
        simple(center.previousTrackCommand(), || Event::Prev);

        let f = on_event.clone();
        let seek = RcBlock::new(move |e: NonNull<MPRemoteCommandEvent>| {
            // Этот обработчик стоит только на changePlaybackPositionCommand, событие всегда такого типа.
            let e = e.cast::<MPChangePlaybackPositionCommandEvent>();
            f(Event::Seek(e.as_ref().positionTime().max(0.0) as f32));
            MPRemoteCommandHandlerStatus::Success
        });
        let pos = center.changePlaybackPositionCommand();
        pos.setEnabled(true);
        pos.addTargetWithHandler(&seek);
    }

    TIMER.with(|t| t.start(slint::TimerMode::Repeated, Duration::from_millis(500), move || follow(&player)));
}

fn follow(player: &Player) {
    let s = player.state();
    let id = s.track.as_ref().map(|t| t.id.clone());
    let (changed, refresh) = SHOWN.with(|sh| {
        let mut sh = sh.borrow_mut();
        let changed = sh.track != id;
        if changed {
            sh.track = id.clone();
            sh.artwork = None;
            sh.at = None;
        }
        let expected = sh.at.map(|(at, pos)| if sh.playing { pos + at.elapsed().as_secs_f32() } else { pos });
        let jumped = expected.is_some_and(|e| (e - s.position).abs() > 2.0);
        let refresh = changed || sh.playing != s.playing || jumped || sh.at.is_none();
        sh.playing = s.playing;
        if refresh {
            sh.at = Some((Instant::now(), s.position));
        }
        (changed, refresh)
    });
    if changed {
        if let Some(t) = &s.track {
            fetch_artwork(t);
        }
    }
    if refresh {
        publish(&s);
    }
}

/// Записать текущее состояние в MPNowPlayingInfoCenter.
fn publish(s: &Shared) {
    unsafe {
        let center = MPNowPlayingInfoCenter::defaultCenter();
        let Some(t) = &s.track else {
            center.setNowPlayingInfo(None);
            center.setPlaybackState(MPNowPlayingPlaybackState::Stopped);
            return;
        };
        let title = if t.version.is_empty() { t.title.clone() } else { format!("{} ({})", t.title, t.version) };
        let duration = if s.duration > 0.0 { f64::from(s.duration) } else { t.duration_ms as f64 / 1000.0 };
        let mut keys: Vec<&NSString> = vec![
            MPMediaItemPropertyTitle,
            MPMediaItemPropertyArtist,
            MPMediaItemPropertyPlaybackDuration,
            MPNowPlayingInfoPropertyElapsedPlaybackTime,
            MPNowPlayingInfoPropertyPlaybackRate,
        ];
        let mut values: Vec<Retained<AnyObject>> = vec![
            NSString::from_str(&title).into_super().into_super(),
            NSString::from_str(&t.artists).into_super().into_super(),
            NSNumber::new_f64(duration).into_super().into_super().into_super(),
            NSNumber::new_f64(f64::from(s.position.max(0.0))).into_super().into_super().into_super(),
            NSNumber::new_f64(if s.playing { 1.0 } else { 0.0 }).into_super().into_super().into_super(),
        ];
        if !t.album_title.is_empty() {
            keys.push(MPMediaItemPropertyAlbumTitle);
            values.push(NSString::from_str(&t.album_title).into_super().into_super());
        }
        if let Some(art) = SHOWN.with(|sh| sh.borrow().artwork.clone()) {
            keys.push(MPMediaItemPropertyArtwork);
            values.push(art.into_super().into_super());
        }
        let info = NSDictionary::from_retained_objects(&keys, &values);
        center.setNowPlayingInfo(Some(&info));
        center.setPlaybackState(if s.playing { MPNowPlayingPlaybackState::Playing } else { MPNowPlayingPlaybackState::Paused });
    }
}

/// Скачать обложку в фоне и показать, если трек к тому времени не сменился.
fn fetch_artwork(t: &Track) {
    let Some(url) = t.cover_url(400) else { return };
    let id = t.id.clone();
    let started = std::thread::Builder::new().name("nowplaying-cover".into()).spawn(move || {
        let bytes = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(20))
            .build()
            .and_then(|c| c.get(&url).send())
            .and_then(|r| r.error_for_status())
            .and_then(|r| r.bytes());
        let bytes = match bytes {
            Ok(b) => b.to_vec(),
            Err(e) => return log(format!("обложка для «Исполняется»: {}", crate::config::chain(&e))),
        };
        let _ = slint::invoke_from_event_loop(move || set_artwork(&id, &bytes));
    });
    if let Err(e) = started {
        log(format!("обложка для «Исполняется»: поток не запустился: {e}"));
    }
}

fn set_artwork(id: &str, bytes: &[u8]) {
    let current = SHOWN.with(|sh| sh.borrow().track.as_deref() == Some(id));
    if !current {
        return;
    }
    let Some(image) = NSImage::initWithData(NSImage::alloc(), &NSData::with_bytes(bytes)) else { return };
    let size = image.size();
    let size = if size.width > 0.0 && size.height > 0.0 { size } else { CGSize::new(400.0, 400.0) };
    let handler_image = image.clone();
    let handler = RcBlock::new(move |_: CGSize| NonNull::from(&*handler_image));
    let artwork = unsafe { MPMediaItemArtwork::initWithBoundsSize_requestHandler(MPMediaItemArtwork::alloc(), size, &handler) };
    SHOWN.with(|sh| {
        let mut sh = sh.borrow_mut();
        sh.artwork = Some(artwork);
        // Пусть следующий тик таймера переотправит словарь уже с обложкой.
        sh.at = None;
    });
}
