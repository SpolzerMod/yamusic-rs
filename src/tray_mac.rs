//! Значок в меню-баре macOS, как у Windows-версии (tray.rs): клик сворачивает и разворачивает
//! окно, правый клик — меню с управлением плеером. Медиаклавиши и «Сейчас играет» — через
//! souvlaki (MPRemoteCommandCenter).

use crate::config::log;
use crate::i18n::tr;
use crate::player::{Cmd, Player};
use objc2::rc::Retained;
use objc2::runtime::NSObject;
use objc2::{define_class, msg_send, sel, AnyThread};
use objc2_foundation::{NSAppleEventDescriptor, NSAppleEventManager};
use souvlaki::{MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig};
use std::cell::RefCell;
use std::io::Write;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Toggle,
    Show,
    PlayPause,
    Prev,
    Next,
    Updates,
    Terms,
    Recommendations,
    Support,
    Quit,
}

/// Что показать в меню: окно свёрнуто или спрятано, играет ли трек, есть ли соседние.
pub struct MenuState {
    pub minimized: bool,
    pub playing: bool,
    pub can_prev: bool,
    pub can_next: bool,
}

/// Пункты меню, подписи и доступность которых зависят от состояния плеера.
struct Items {
    toggle: MenuItem,
    play: MenuItem,
    prev: MenuItem,
    next: MenuItem,
}

struct Tray {
    /// Значок и меню создаются уже в цикле событий (`create_icon`), до этого их нет.
    _icon: Option<TrayIcon>,
    items: Option<Items>,
    on_event: Rc<dyn Fn(Event)>,
    state: Box<dyn Fn() -> MenuState>,
}

thread_local! {
    static TRAY: RefCell<Option<Tray>> = const { RefCell::new(None) };
}

/// Управление «Сейчас играет» и медиаклавишами: им пользуются основной поток и поток метаданных.
static MEDIA: Mutex<Option<MediaControls>> = Mutex::new(None);

/// Плеер для команд из системы, которых нет среди событий трея (перемотка).
static PLAYER: OnceLock<Player> = OnceLock::new();

/// Сокет, через который второй запуск будит работающий экземпляр.
fn wake_socket() -> PathBuf {
    crate::config::dir().join("wake.sock")
}

/// Монохромный значок (шаблон): система сама красит его под светлое и тёмное меню-бар.
fn load_icon() -> Option<Icon> {
    let img = image::load_from_memory(include_bytes!("../icons/tray_mac.png")).ok()?.to_rgba8();
    let (w, h) = img.dimensions();
    Icon::from_rgba(img.into_raw(), w, h).ok()
}

fn item(id: &str, text: &str) -> MenuItem {
    MenuItem::with_id(id, tr(text), true, None)
}

/// Запоминает обработчики и запускает всё, что не зависит от значка: сокет второго запуска и
/// медиаклавиши. Значок создаётся позже, в цикле событий. События приходят в основной поток
/// через очередь Slint, а не из обработчика AppKit: так их обработка не вкладывается в цикл winit.
pub fn install(on_event: impl Fn(Event) + 'static, state: impl Fn() -> MenuState + 'static) {
    TRAY.with(|t| *t.borrow_mut() = Some(Tray { _icon: None, items: None, on_event: Rc::new(on_event), state: Box::new(state) }));
    listen_for_wake();
    attach_media_keys();
    // Как советует tray-icon, значок создаётся, когда цикл событий уже запущен. Заодно к этому
    // моменту AppKit поставил свои обработчики событий, и наш `reopen` их заменит, а не наоборот.
    slint::Timer::single_shot(Duration::ZERO, || {
        create_icon();
        handle_reopen();
    });
}

fn create_icon() {
    let items = Items { toggle: item("toggle", "Свернуть"), play: item("play", "Воспроизведение"), prev: item("prev", "Предыдущая песня"), next: item("next", "Следующая песня") };
    let (updates, terms, recs, support, quit) = (
        item("updates", "Проверить обновления"),
        item("terms", "Пользовательское соглашение"),
        item("recs", "Правила рекомендаций"),
        item("support", "Чат с поддержкой"),
        item("quit", "Закрыть приложение"),
    );
    let about = Submenu::with_items(tr("О приложении"), true, &[&terms, &recs, &support]);
    let sep = PredefinedMenuItem::separator;
    let menu = Menu::new();
    let built = about.and_then(|about| {
        menu.append_items(&[&items.toggle, &updates, &sep(), &items.play, &items.prev, &items.next, &sep(), &about, &sep(), &quit])
    });
    if let Err(e) = built {
        log(format!("меню в меню-баре не собралось: {e}"));
        return;
    }
    let Some(icon) = load_icon() else {
        log("значок меню-бара не загрузился");
        return;
    };
    let tray = TrayIconBuilder::new()
        .with_icon_templated(icon)
        .with_tooltip(env!("APP_NAME"))
        .with_menu(Box::new(menu))
        // Левый клик, как у Windows, сворачивает и разворачивает окно; меню — по правому.
        .with_menu_on_left_click(false)
        .build();
    let tray = match tray {
        Ok(t) => t,
        Err(e) => {
            log(format!("значок в меню-баре не создался: {e}"));
            return;
        }
    };
    MenuEvent::set_event_handler(Some(|e: MenuEvent| {
        emit(match e.id.0.as_str() {
            "toggle" => Event::Toggle,
            "updates" => Event::Updates,
            "play" => Event::PlayPause,
            "prev" => Event::Prev,
            "next" => Event::Next,
            "terms" => Event::Terms,
            "recs" => Event::Recommendations,
            "support" => Event::Support,
            "quit" => Event::Quit,
            _ => return,
        })
    }));
    TrayIconEvent::set_event_handler(Some(|e: TrayIconEvent| match e {
        TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } => emit(Event::Toggle),
        // Обработчик зовётся в основном потоке прямо перед показом меню: подписи успевают обновиться.
        TrayIconEvent::Click { button: MouseButton::Right, button_state: MouseButtonState::Down, .. } => refresh(),
        _ => {}
    }));
    TRAY.with(|t| {
        if let Some(t) = t.borrow_mut().as_mut() {
            t._icon = Some(tray);
            t.items = Some(items);
        }
    });
    refresh();
}

pub fn remove() {
    MenuEvent::set_event_handler(None::<fn(MenuEvent)>);
    TrayIconEvent::set_event_handler(None::<fn(TrayIconEvent)>);
    TRAY.with(|t| t.borrow_mut().take());
    let _ = std::fs::remove_file(wake_socket());
    if let Some(mut c) = MEDIA.lock().unwrap().take() {
        let _ = c.set_playback(MediaPlayback::Stopped);
        let _ = c.detach();
    }
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "YamusicReopenHandler"]
    struct ReopenHandler;

    impl ReopenHandler {
        #[unsafe(method(handleReopen:withReplyEvent:))]
        fn handle_reopen(&self, _event: &NSAppleEventDescriptor, _reply: &NSAppleEventDescriptor) {
            emit(Event::Show);
        }
    }
);

/// Повторный запуск .app и клик по значку в Dock: пока приложение работает, второй процесс
/// macOS не запускает, а шлёт ему событие «reopen» (aevt/rapp). Winit его не обрабатывает,
/// поэтому окно, спрятанное в меню-бар, само не вернулось бы.
fn handle_reopen() {
    const CORE_EVENT_CLASS: u32 = u32::from_be_bytes(*b"aevt");
    const REOPEN_APPLICATION: u32 = u32::from_be_bytes(*b"rapp");
    let handler: Retained<ReopenHandler> = unsafe { msg_send![ReopenHandler::alloc(), init] };
    let manager = NSAppleEventManager::sharedAppleEventManager();
    unsafe {
        let _: () = msg_send![
            &*manager,
            setEventHandler: &*handler,
            andSelector: sel!(handleReopen:withReplyEvent:),
            forEventClass: CORE_EVENT_CLASS,
            andEventID: REOPEN_APPLICATION
        ];
    }
    // Менеджер событий объект не удерживает, а обработчик нужен до конца работы.
    std::mem::forget(handler);
}

/// Для второго запуска: попросить работающий экземпляр показать окно.
pub fn wake_existing() -> bool {
    UnixStream::connect(wake_socket()).and_then(|mut s| s.write_all(b"1")).is_ok()
}

/// Слушаем сокет: каждое подключение — просьба показать окно.
fn listen_for_wake() {
    let path = wake_socket();
    // Блокировка экземпляра уже наша, так что сокет остался от завершившегося процесса.
    let _ = std::fs::remove_file(&path);
    let listener = match UnixListener::bind(&path) {
        Ok(l) => l,
        Err(e) => {
            log(format!("сокет второго запуска: {e}"));
            return;
        }
    };
    std::thread::Builder::new()
        .name("wake".into())
        .spawn(move || {
            for _ in listener.incoming().flatten() {
                emit(Event::Show);
            }
        })
        .ok();
}

fn emit(e: Event) {
    let _ = slint::invoke_from_event_loop(move || {
        let f = TRAY.with(|t| t.borrow().as_ref().map(|t| t.on_event.clone()));
        if let Some(f) = f {
            f(e);
        }
    });
}

/// Подписи и доступность пунктов по текущему состоянию плеера.
fn refresh() {
    TRAY.with(|t| {
        let t = t.borrow();
        let Some((t, items)) = t.as_ref().and_then(|t| Some((t, t.items.as_ref()?))) else { return };
        let s = (t.state)();
        items.toggle.set_text(tr(if s.minimized { "Развернуть" } else { "Свернуть" }));
        items.play.set_text(tr(if s.playing { "Пауза" } else { "Воспроизведение" }));
        items.prev.set_enabled(s.can_prev);
        items.next.set_enabled(s.can_next);
    });
}

/// Медиаклавиши клавиатуры и пульт в Пункте управления. macOS отдаёт их приложению, которое
/// сообщило «Сейчас играет» (см. `now_playing`).
fn attach_media_keys() {
    let cfg = PlatformConfig { dbus_name: "yamusic_rs", display_name: env!("APP_NAME"), hwnd: None };
    let mut controls = match MediaControls::new(cfg) {
        Ok(c) => c,
        Err(e) => {
            log(format!("медиаклавиши: {e}"));
            return;
        }
    };
    if controls.attach(on_media).is_err() {
        return;
    }
    *MEDIA.lock().unwrap() = Some(controls);
}

fn on_media(e: MediaControlEvent) {
    // Перемотка из Пункта управления: позиция в секундах, команда плееру напрямую.
    if let MediaControlEvent::SetPosition(MediaPosition(pos)) = e {
        if let Some(p) = PLAYER.get() {
            p.send(Cmd::Seek(pos.as_secs_f32()));
        }
        return;
    }
    let _ = slint::invoke_from_event_loop(move || {
        let Some((f, playing)) = TRAY.with(|t| t.borrow().as_ref().map(|t| (t.on_event.clone(), (t.state)().playing))) else { return };
        // Событий «играть» и «пауза» в меню нет, поэтому сверяемся с состоянием плеера.
        f(match e {
            MediaControlEvent::Toggle => Event::PlayPause,
            MediaControlEvent::Play if !playing => Event::PlayPause,
            MediaControlEvent::Pause | MediaControlEvent::Stop if playing => Event::PlayPause,
            MediaControlEvent::Next => Event::Next,
            MediaControlEvent::Previous => Event::Prev,
            _ => return,
        });
    });
}

/// «Сейчас играет» в Пункте управления и на экране блокировки: трек, исполнитель, альбом,
/// обложка, пауза и позиция. Как и Discord, смотрит на состояние плеера из своего потока.
pub fn now_playing(player: Player) {
    let _ = PLAYER.set(player.clone());
    std::thread::Builder::new()
        .name("now-playing".into())
        .spawn(move || {
            let mut track_id: Option<String> = None;
            // Когда, где и в каком режиме опубликована позиция: по ней macOS сам двигает ползунок.
            let mut published: Option<(Instant, f32, bool)> = None;
            loop {
                std::thread::sleep(Duration::from_millis(500));
                let mut guard = MEDIA.lock().unwrap();
                let Some(media) = guard.as_mut() else { continue };
                let s = player.state();
                let id = s.track.as_ref().map(|t| t.id.clone());
                if id != track_id {
                    published = None;
                    match &s.track {
                        Some(t) => {
                            let title = if t.version.is_empty() { t.title.clone() } else { format!("{} ({})", t.title, t.version) };
                            let cover = t.cover_url(400);
                            let _ = media.set_metadata(MediaMetadata {
                                title: Some(&title),
                                artist: Some(&t.artists),
                                album: Some(&t.album_title).filter(|a| !a.is_empty()).map(|a| a.as_str()),
                                cover_url: cover.as_deref(),
                                duration: (s.duration > 1.0).then(|| Duration::from_secs_f32(s.duration)),
                            });
                        }
                        None => {
                            // Вышли из аккаунта или очередь пуста: старый трек из Пункта управления убираем.
                            let _ = media.set_metadata(MediaMetadata::default());
                            let _ = media.set_playback(MediaPlayback::Stopped);
                        }
                    }
                    track_id = id;
                }
                if s.track.is_none() {
                    continue;
                }
                let expected = published.map(|(at, pos, playing)| if playing { pos + at.elapsed().as_secs_f32() } else { pos });
                let moved = published.is_none_or(|(_, _, playing)| playing != s.playing) || expected.is_some_and(|e| (e - s.position).abs() > 2.0);
                if moved {
                    let progress = Some(MediaPosition(Duration::from_secs_f32(s.position.max(0.0))));
                    let _ = media.set_playback(if s.playing { MediaPlayback::Playing { progress } } else { MediaPlayback::Paused { progress } });
                    published = Some((Instant::now(), s.position, s.playing));
                }
            }
        })
        .ok();
}
