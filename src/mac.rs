//! macOS: медиа-клавиши (F7/F8/F9), карточка «Сейчас играет» в Пункте управления и
//! нативные кнопки окна («светофор»). MediaPlayer framework — `MPRemoteCommandCenter`
//! и `MPNowPlayingInfoCenter`: то же, что делает оригинальное Electron-приложение через
//! MediaSession браузера. Без сторонних библиотек: прямые вызовы objc runtime.
//!
//! Все вызовы идут из главного потока (цикл событий slint/winit и обработчики команд
//! MediaPlayer вызываются на main run loop), поэтому thread_local для кэша безопасен.

use crate::api::Track;
use crate::player::{Cmd, Player};
use std::cell::RefCell;
use std::ffi::{c_char, c_void, CString};
use std::os::raw::{c_int, c_long, c_ulong};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

type Id = *mut c_void;
type Sel = *mut c_void;

extern "C" {
    fn objc_getClass(name: *const c_char) -> Id;
    fn sel_registerName(name: *const c_char) -> Sel;
    fn objc_msgSend();
    fn dlopen(filename: *const c_char, flag: c_int) -> *mut c_void;
    fn objc_allocateClassPair(superclass: Id, name: *const c_char, extra_bytes: usize) -> Id;
    fn class_addMethod(cls: Id, name: Sel, imp: *const c_void, types: *const c_char) -> bool;
    fn objc_registerClassPair(cls: Id);
}

/// Убедиться, что MediaPlayer.framework загружен (для objc_getClass его классов).
/// Фреймворк слинкован, но на всякий случай грузим явно — dlopen идемпотентен.
fn ensure_media_player() {
    static DONE: OnceLock<()> = OnceLock::new();
    DONE.get_or_init(|| unsafe {
        let h = dlopen(c("/System/Library/Frameworks/MediaPlayer.framework/MediaPlayer").as_ptr(), 2 /* RTLD_NOW */);
        if h.is_null() {
            crate::config::log("media: MediaPlayer.framework не загрузился");
        }
    });
}

fn c(s: &str) -> CString {
    CString::new(s).unwrap()
}

// objc_msgSend нужно приводить к точной сигнатуре каждого вызова (на arm64 variadic
// соглашение не используется). transmute между указателями на функции одного размера.
unsafe fn msg0(obj: Id, sel: &str) -> Id {
    let f: unsafe extern "C" fn(Id, Sel) -> Id = std::mem::transmute(objc_msgSend as unsafe extern "C" fn() as usize);
    f(obj, sel_registerName(c(sel).as_ptr()))
}

unsafe fn msg_set_id(obj: Id, sel: &str, arg: Id) {
    let f: unsafe extern "C" fn(Id, Sel, Id) = std::mem::transmute(objc_msgSend as unsafe extern "C" fn() as usize);
    f(obj, sel_registerName(c(sel).as_ptr()), arg);
}

unsafe fn msg_set_ulong(obj: Id, sel: &str, v: c_ulong) {
    let f: unsafe extern "C" fn(Id, Sel, c_ulong) = std::mem::transmute(objc_msgSend as unsafe extern "C" fn() as usize);
    f(obj, sel_registerName(c(sel).as_ptr()), v);
}

unsafe fn msg_set_bool(obj: Id, sel: &str, v: bool) {
    let f: unsafe extern "C" fn(Id, Sel, bool) = std::mem::transmute(objc_msgSend as unsafe extern "C" fn() as usize);
    f(obj, sel_registerName(c(sel).as_ptr()), v);
}

unsafe fn msg_set_long(obj: Id, sel: &str, v: c_long) {
    let f: unsafe extern "C" fn(Id, Sel, c_long) = std::mem::transmute(objc_msgSend as unsafe extern "C" fn() as usize);
    f(obj, sel_registerName(c(sel).as_ptr()), v);
}

unsafe fn msg_set_double(obj: Id, sel: &str, v: f64) {
    let f: unsafe extern "C" fn(Id, Sel, f64) = std::mem::transmute(objc_msgSend as unsafe extern "C" fn() as usize);
    f(obj, sel_registerName(c(sel).as_ptr()), v);
}

unsafe fn msg_responds(obj: Id, sel: Sel) -> bool {
    let f: unsafe extern "C" fn(Id, Sel, Sel) -> bool = std::mem::transmute(objc_msgSend as unsafe extern "C" fn() as usize);
    f(obj, sel_registerName(c("respondsToSelector:").as_ptr()), sel)
}

unsafe fn msg_target_action(obj: Id, sel: &str, target: Id, action: Sel) {
    let f: unsafe extern "C" fn(Id, Sel, Id, Sel) = std::mem::transmute(objc_msgSend as unsafe extern "C" fn() as usize);
    f(obj, sel_registerName(c(sel).as_ptr()), target, action);
}

unsafe fn msg_number(cls: Id, v: f64) -> Id {
    let f: unsafe extern "C" fn(Id, Sel, f64) -> Id = std::mem::transmute(objc_msgSend as unsafe extern "C" fn() as usize);
    f(cls, sel_registerName(c("numberWithDouble:").as_ptr()), v)
}

unsafe fn msg_string(cls: Id, s: &str) -> Id {
    let f: unsafe extern "C" fn(Id, Sel, *const c_char) -> Id = std::mem::transmute(objc_msgSend as unsafe extern "C" fn() as usize);
    f(cls, sel_registerName(c("stringWithUTF8String:").as_ptr()), c(s).as_ptr())
}

unsafe fn msg_double(obj: Id, sel: &str) -> f64 {
    let f: unsafe extern "C" fn(Id, Sel) -> f64 = std::mem::transmute(objc_msgSend as unsafe extern "C" fn() as usize);
    f(obj, sel_registerName(c(sel).as_ptr()))
}

unsafe fn msg_dict(cls: Id, objects: *const Id, keys: *const Id, count: c_ulong) -> Id {
    let f: unsafe extern "C" fn(Id, Sel, *const Id, *const Id, c_ulong) -> Id = std::mem::transmute(objc_msgSend as unsafe extern "C" fn() as usize);
    f(cls, sel_registerName(c("dictionaryWithObjects:forKeys:count:").as_ptr()), objects, keys, count)
}

// ---------- медиа-клавиши ----------

static PLAYER: OnceLock<Player> = OnceLock::new();

fn player() -> Option<&'static Player> {
    PLAYER.get()
}

// Обработчики команд MediaPlayer обязаны возвращать MPRemoteCommandHandlerStatus
// (NSInteger): 0 = Success. Сигнатура метода в objc — "q@:@", не "v@:@".
const STATUS_SUCCESS: c_long = 0;

extern "C" fn on_play(_self: Id, _sel: Sel, _cmd: Id) -> c_long {
    if let Some(p) = player() {
        p.send(Cmd::Resume);
    }
    STATUS_SUCCESS
}

extern "C" fn on_pause(_self: Id, _sel: Sel, _cmd: Id) -> c_long {
    if let Some(p) = player() {
        p.send(Cmd::Pause);
    }
    STATUS_SUCCESS
}

extern "C" fn on_toggle(_self: Id, _sel: Sel, _cmd: Id) -> c_long {
    if let Some(p) = player() {
        p.send(Cmd::PlayPause);
    }
    STATUS_SUCCESS
}

extern "C" fn on_next(_self: Id, _sel: Sel, _cmd: Id) -> c_long {
    if let Some(p) = player() {
        p.send(Cmd::Next);
    }
    STATUS_SUCCESS
}

extern "C" fn on_prev(_self: Id, _sel: Sel, _cmd: Id) -> c_long {
    if let Some(p) = player() {
        p.send(Cmd::Prev);
    }
    STATUS_SUCCESS
}

extern "C" fn on_seek(_self: Id, _sel: Sel, event: Id) -> c_long {
    unsafe {
        let n = msg0(event, "playbackPosition");
        if !n.is_null() {
            let pos = msg_double(n, "doubleValue");
            if let Some(p) = player() {
                p.send(Cmd::Seek(pos as f32));
            }
        }
    }
    STATUS_SUCCESS
}

/// Зарегистрировать обработчики медиа-клавиш и команд Пункта управления.
/// Вызывать один раз из главного потока после создания плеера.
pub fn init_media_keys(p: Player) {
    let _ = PLAYER.set(p);
    ensure_media_player();
    unsafe {
        let cls = objc_allocateClassPair(objc_getClass(c("NSObject").as_ptr()), c("YamusicMediaKeys").as_ptr(), 0);
        if cls.is_null() {
            crate::config::log("media: класс не создался");
            return;
        }
        let add = |name: &str, imp: extern "C" fn(Id, Sel, Id) -> c_long| {
            class_addMethod(
                cls,
                sel_registerName(c(name).as_ptr()),
                imp as *const c_void,
                c("q@:@").as_ptr(),
            );
        };
        add("play:", on_play);
        add("pause:", on_pause);
        add("togglePlayPause:", on_toggle);
        add("nextTrack:", on_next);
        add("previousTrack:", on_prev);
        add("changePlaybackPosition:", on_seek);
        objc_registerClassPair(cls);

        let target = msg0(msg0(cls, "alloc"), "init");
        let center = msg0(objc_getClass(c("MPRemoteCommandCenter").as_ptr()), "sharedCommandCenter");
        if target.is_null() || center.is_null() {
            crate::config::log("media: не удалось получить центр команд");
            return;
        }
        for (command, action) in [
            ("playCommand", "play:"),
            ("pauseCommand", "pause:"),
            ("togglePlayPauseCommand", "togglePlayPause:"),
            ("nextTrackCommand", "nextTrack:"),
            ("previousTrackCommand", "previousTrack:"),
            ("changePlaybackPositionCommand", "changePlaybackPosition:"),
        ] {
            let cmd = msg0(center, command);
            if !cmd.is_null() {
                msg_target_action(cmd, "addTarget:action:", target, sel_registerName(c(action).as_ptr()));
            }
        }
    }
}

// ---------- «Сейчас играет» ----------

struct Last {
    key: String,
    at: Instant,
}

thread_local! {
    static LAST: RefCell<Option<Last>> = const { RefCell::new(None) };
}

/// Обновить карточку в Пункте управления. Звать из sync_player (главный поток).
/// Пересобираем словарь при смене трека/состояния и раз в 2 секунды для позиции.
pub fn now_playing(track: Option<&Track>, playing: bool, position: f32, duration: f32) {
    unsafe {
        let center = msg0(objc_getClass(c("MPNowPlayingInfoCenter").as_ptr()), "defaultCenter");
        if center.is_null() {
            return;
        }
        let Some(t) = track else {
            if LAST.with(|l| l.borrow_mut().take()).is_some() {
                msg_set_id(center, "setNowPlayingInfo:", std::ptr::null_mut());
            }
            return;
        };
        let key = format!("{}\u{1}{}\u{1}{}\u{1}{duration}\u{1}{playing}", t.title, t.artists, t.album_title);
        let fresh = LAST.with(|l| {
            let mut l = l.borrow_mut();
            match l.as_ref() {
                Some(last) if last.key == key && last.at.elapsed() < Duration::from_secs(2) => false,
                _ => {
                    *l = Some(Last { key, at: Instant::now() });
                    true
                }
            }
        });
        if !fresh {
            return;
        }
        let nsstring = objc_getClass(c("NSString").as_ptr());
        let nsnumber = objc_getClass(c("NSNumber").as_ptr());
        // Ключи nowPlayingInfo — строковые значения констант MPMediaItemProperty* /
        // MPNowPlayingInfoProperty* (создаём сами, чтобы не зависеть от extern-символов).
        let keys = [
            msg_string(nsstring, "title"),
            msg_string(nsstring, "artist"),
            msg_string(nsstring, "albumTitle"),
            msg_string(nsstring, "playbackDuration"),
            // У MPNowPlayingInfoProperty* значение константы — полное имя строкой
            // (в отличие от MPMediaItem*, где короткое «title», «artist»).
            msg_string(nsstring, "MPNowPlayingInfoPropertyElapsedPlaybackTime"),
            msg_string(nsstring, "MPNowPlayingInfoPropertyPlaybackRate"),
        ];
        let values = [
            msg_string(nsstring, &t.title),
            msg_string(nsstring, &t.artists),
            msg_string(nsstring, &t.album_title),
            msg_number(nsnumber, duration as f64),
            msg_number(nsnumber, position as f64),
            msg_number(nsnumber, if playing { 1.0 } else { 0.0 }),
        ];
        let dict = msg_dict(objc_getClass(c("NSDictionary").as_ptr()), values.as_ptr(), keys.as_ptr(), keys.len() as c_ulong);
        msg_set_id(center, "setNowPlayingInfo:", dict);
    }
}

// ---------- нативные кнопки окна ----------

/// Желаемый NSWindowStyleMask: Titled(1) | Closable(2) | Miniaturizable(4) |
/// Resizable(8) | FullSizeContentView(1 << 15).
const WINDOW_STYLE: c_ulong = 1 | 2 | 4 | 8 | (1 << 15);

/// Вернуть окну нативный вид: «светофор», прозрачный тайтлбар на всю высоту
/// (как `titleBarStyle: hiddenInset` у Electron-оригинала) и скруглённые углы ~10pt.
unsafe fn restyle_window(win: Id) {
    if win.is_null() {
        return;
    }
    // macOS сбрасывает styleMask при выходе из полноэкранного режима — ставим всегда.
    msg_set_ulong(win, "setStyleMask:", WINDOW_STYLE);
    msg_set_bool(win, "setTitlebarAppearsTransparent:", true);
    msg_set_long(win, "setTitleVisibility:", 1); // NSWindowTitleHidden
    // Сервер скругляет стандартные окна сам, но окно winit создаётся borderless,
    // поэтому задаём радиус самому окну.
    let corner_sel = sel_registerName(c("_setCornerRadius:").as_ptr());
    if msg_responds(win, corner_sel) {
        msg_set_double(win, "_setCornerRadius:", 10.0);
    }
}

/// Первичная стилизация окна. `ns_view` — NSView окна winit; вызывать из главного
/// потока после появления winit-окна.
pub fn native_window_buttons(ns_view: Id) {
    unsafe {
        let win = msg0(ns_view, "window");
        restyle_window(win);
    }
}

/// После выхода из полноэкранного режима macOS сбрасывает styleMask и «светофор»
/// пропадает — ловим уведомление и возращаем нативный вид. Вызывать один раз.
pub fn init_window_observers() {
    unsafe {
        let cls = objc_allocateClassPair(objc_getClass(c("NSObject").as_ptr()), c("YamusicWindowObserver").as_ptr(), 0);
        if cls.is_null() {
            return;
        }
        let add = |name: &str, imp: extern "C" fn(Id, Sel, Id)| {
            class_addMethod(cls, sel_registerName(c(name).as_ptr()), imp as *const c_void, c("v@:@").as_ptr());
        };
        add("windowDidExitFullScreen:", on_exit_fullscreen);
        add("restyleNow:", on_restyle_now);
        objc_registerClassPair(cls);
        let observer = msg0(msg0(cls, "alloc"), "init");
        let center = msg0(objc_getClass(c("NSNotificationCenter").as_ptr()), "defaultCenter");
        if observer.is_null() || center.is_null() {
            return;
        }
        let name = msg_string(objc_getClass(c("NSString").as_ptr()), "NSWindowDidExitFullScreenNotification");
        // addObserver:selector:name:object: — object nil, то есть все окна приложения.
        let f: unsafe extern "C" fn(Id, Sel, Id, Sel, Id, Id) = std::mem::transmute(objc_msgSend as unsafe extern "C" fn() as usize);
        f(
            center,
            sel_registerName(c("addObserver:selector:name:object:").as_ptr()),
            observer,
            sel_registerName(c("windowDidExitFullScreen:").as_ptr()),
            name,
            std::ptr::null_mut(),
        );
    }
}

extern "C" fn on_exit_fullscreen(_self: Id, _sel: Sel, note: Id) {
    unsafe {
        let win = msg0(note, "object");
        // Перестилизуем не сразу, а через 0.12 с: winit тем же уведомлением/делегатом
        // успевает сбросить styleMask, и наш `setStyleMask` должен идти последним.
        // performSelector:withObject:afterDelay: выполняется на main-очереди — можно.
        let f: unsafe extern "C" fn(Id, Sel, Sel, Id, f64) = std::mem::transmute(objc_msgSend as unsafe extern "C" fn() as usize);
        f(
            _self,
            sel_registerName(c("performSelector:withObject:afterDelay:").as_ptr()),
            sel_registerName(c("restyleNow:").as_ptr()),
            win,
            0.12,
        );
    }
}

extern "C" fn on_restyle_now(_self: Id, _sel: Sel, win: Id) {
    unsafe {
        restyle_window(win);
    }
}

// ---------- рабочая область экрана (для мини-плеера) ----------

/// NSRect: origin{x,y} + size{w,h} — 4 CGFloat (f64 на arm64).
#[repr(C)]
struct NsRect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

unsafe fn ns_rect(obj: Id, sel: &str) -> NsRect {
    let f: unsafe extern "C" fn(Id, Sel) -> NsRect = std::mem::transmute(objc_msgSend as unsafe extern "C" fn() as usize);
    f(obj, sel_registerName(c(sel).as_ptr()))
}

/// Рабочая область основного экрана в физических пикселях, начало — левый верхний
/// угол основного экрана (как PhysicalPosition у winit): лево, верх, право, низ.
/// `visibleFrame` в Cocoa — в точках и с началом в левом нижнем углу, пересчитываем.
pub fn work_area() -> (i32, i32, i32, i32) {
    unsafe {
        let screen = msg0(objc_getClass(c("NSScreen").as_ptr()), "mainScreen");
        if screen.is_null() {
            return (0, 0, 1920, 1080);
        }
        let vf = ns_rect(screen, "visibleFrame");
        let k = msg_double(screen, "backingScaleFactor");
        // Высота основного экрана в точках: frame первой из [NSScreen screens].
        let screens = msg0(objc_getClass(c("NSScreen").as_ptr()), "screens");
        let first = if screens.is_null() { std::ptr::null_mut() } else { msg0(screens, "firstObject") };
        let h_pts = if first.is_null() { vf.y + vf.h } else { ns_rect(first, "frame").h };
        let left = (vf.x * k) as i32;
        let right = ((vf.x + vf.w) * k) as i32;
        let top = ((h_pts - (vf.y + vf.h)) * k) as i32;
        let bottom = ((h_pts - vf.y) * k) as i32;
        (left, top, right, bottom)
    }
}
