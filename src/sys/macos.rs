//! macOS: нативное окно AppKit с «светофором», меню и Dock, системные диалоги NSOpenPanel,
//! буфер обмена NSPasteboard, пути ~/Library, автозапуск через LaunchAgent и один экземпляр
//! через unix-сокет.

use objc2::ffi::{class_addMethod, object_getClass};
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Bool, Imp, Sel};
use objc2::{define_class, msg_send, sel, AnyThread, MainThreadMarker, MainThreadOnly, Message};
use objc2_app_kit::{
    NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
    NSApplication, NSEventModifierFlags, NSImage, NSMenu, NSMenuItem, NSModalResponseOK,
    NSOpenPanel, NSPasteboard, NSPasteboardTypeString, NSScreen, NSView, NSWindow, NSWindowButton,
    NSWindowCollectionBehavior, NSWindowStyleMask, NSWindowTitleVisibility, NSWorkspace,
};
use objc2_foundation::{ns_string, NSArray, NSData, NSObject, NSString, NSURL, NSUserDefaults};
use slint::winit_030::winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::winit_030::WinitWindowAccessor;
use std::ffi::c_void;
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// Идентификатор пакета и LaunchAgent.
const BUNDLE_ID: &str = "io.github.spolzermod.yamusic-rs";
const APP_ID: &str = "yamusic-rs";

/// События от меню macOS и значка в Dock.
#[derive(Clone, Copy, Debug)]
pub enum AppEvent {
    Reopen,
    Settings,
    ToggleMini,
    PlayPause,
    Next,
    Prev,
    Quit,
}

static APP_CB: OnceLock<Box<dyn Fn(AppEvent) + Send + Sync>> = OnceLock::new();
static HOOKS_INSTALLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn emit(ev: AppEvent) {
    if let Some(cb) = APP_CB.get() {
        cb(ev);
    }
}

/// Зарегистрировать обработчик событий Dock и строки меню macOS.
/// Сами хуки AppKit ставятся в [`prepare`] после того, как winit завершит запуск приложения.
pub fn set_app_handler(on_event: impl Fn(AppEvent) + Send + Sync + 'static) {
    let _ = APP_CB.set(Box::new(on_event));
}

fn ns_view(w: &slint::Window) -> Option<Retained<NSView>> {
    w.with_winit_window(|ww| {
        let Ok(RawWindowHandle::AppKit(h)) = ww.window_handle().map(|h| h.as_raw()) else {
            return None;
        };
        let view: &NSView = unsafe { h.ns_view.cast().as_ref() };
        Some(view.retain())
    })
    .flatten()
}

fn ns_window(w: &slint::Window) -> Option<Retained<NSWindow>> {
    ns_view(w)?.window()
}

pub fn created(w: &slint::Window) -> bool {
    ns_window(w).is_some()
}

/// Один раз после создания окна: стиль с прозрачным заголовком и «светофором», значок в Dock,
/// строка меню и обработчик нажатия на значок в Dock.
pub fn prepare(w: &slint::Window) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    if let Some(win) = ns_window(w) {
        let mut mask = win.styleMask();
        mask.remove(NSWindowStyleMask::Borderless);
        mask |= NSWindowStyleMask::Titled
            | NSWindowStyleMask::Closable
            | NSWindowStyleMask::Miniaturizable
            | NSWindowStyleMask::Resizable
            | NSWindowStyleMask::FullSizeContentView;
        win.setStyleMask(mask);
        win.setTitleVisibility(NSWindowTitleVisibility::Hidden);
        win.setTitlebarAppearsTransparent(true);
        win.setMovableByWindowBackground(false);
        win.setHasShadow(true);
        let behavior = win.collectionBehavior() | NSWindowCollectionBehavior::FullScreenPrimary;
        win.setCollectionBehavior(behavior);
    }
    let app = NSApplication::sharedApplication(mtm);
    let icon_bytes = include_bytes!("../../packaging/icons/256.png");
    let data = NSData::with_bytes(icon_bytes);
    if let Some(img) = NSImage::initWithData(NSImage::alloc(), &data) {
        unsafe { app.setApplicationIconImage(Some(&img)) };
    }
    if !HOOKS_INSTALLED.swap(true, std::sync::atomic::Ordering::AcqRel) {
        install_delegate_hooks(&app);
        install_main_menu(&app, mtm);
    }
}

/// Подогнать системное оформление окна и диалогов (тень, меню, NSOpenPanel) под тему приложения.
pub fn set_window_appearance(w: &slint::Window, mode: &str) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let appearance = match mode {
        "dark" => NSAppearance::appearanceNamed(unsafe { NSAppearanceNameDarkAqua }),
        "light" => NSAppearance::appearanceNamed(unsafe { NSAppearanceNameAqua }),
        _ => None,
    };
    let app = NSApplication::sharedApplication(mtm);
    app.setAppearance(appearance.as_deref());
    if let Some(win) = ns_window(w) {
        win.setAppearance(appearance.as_deref());
    }
}

pub fn is_fullscreen(w: &slint::Window) -> bool {
    w.with_winit_window(|ww| ww.fullscreen().is_some()).unwrap_or(false)
}

pub fn exit_fullscreen(w: &slint::Window) {
    w.with_winit_window(|ww| ww.set_fullscreen(None));
}

pub fn is_maximized(w: &slint::Window) -> bool {
    !is_fullscreen(w) && ns_window(w).is_some_and(|win| win.isZoomed())
}

pub fn minimize(w: &slint::Window) {
    w.set_minimized(true);
}

pub fn hide(w: &slint::Window) {
    if let Some(win) = ns_window(w) {
        win.orderOut(None);
    }
}

pub fn raise(w: &slint::Window) {
    w.set_minimized(false);
    if let Some(win) = ns_window(w) {
        if win.isMiniaturized() {
            win.deminiaturize(None);
        }
        win.makeKeyAndOrderFront(None);
    }
    if let Some(mtm) = MainThreadMarker::new() {
        let app = NSApplication::sharedApplication(mtm);
        #[allow(deprecated)]
        app.activateIgnoringOtherApps(true);
    }
}

pub fn scale(w: &slint::Window) -> f32 {
    w.with_winit_window(|ww| ww.scale_factor() as f32).unwrap_or(1.0)
}

/// В мини-плеере убираем заголовок и «светофор», чтобы кнопки громкости и закрытия у верхнего
/// края плашки не перехватывал системный заголовок, но оставляем скругление и тень.
pub fn set_mini_mode(w: &slint::Window, mini: bool) {
    use slint::winit_030::winit::window::WindowButtons;
    let Some(win) = ns_window(w) else { return };
    if mini {
        set_standard_buttons_hidden(&win, true);
        win.setStyleMask(NSWindowStyleMask::Borderless);
        win.setHasShadow(true);
        set_content_corner_radius(&win, 12.0);
        w.with_winit_window(|ww| {
            ww.set_resizable(false);
        });
    } else {
        set_content_corner_radius(&win, 0.0);
        let mask = NSWindowStyleMask::Titled
            | NSWindowStyleMask::Closable
            | NSWindowStyleMask::Miniaturizable
            | NSWindowStyleMask::Resizable
            | NSWindowStyleMask::FullSizeContentView;
        win.setStyleMask(mask);
        win.setTitleVisibility(NSWindowTitleVisibility::Hidden);
        win.setTitlebarAppearsTransparent(true);
        win.setHasShadow(true);
        set_standard_buttons_hidden(&win, false);
        w.with_winit_window(|ww| {
            ww.set_resizable(true);
            ww.set_enabled_buttons(WindowButtons::all());
        });
    }
    if let Some(view) = win.contentView() {
        let _ = win.makeFirstResponder(Some(&view));
    }
}

fn set_standard_buttons_hidden(win: &NSWindow, hidden: bool) {
    for btn in [
        NSWindowButton::CloseButton,
        NSWindowButton::MiniaturizeButton,
        NSWindowButton::ZoomButton,
    ] {
        if let Some(b) = win.standardWindowButton(btn) {
            b.setHidden(hidden);
        }
    }
}

fn set_content_corner_radius(win: &NSWindow, radius: f64) {
    let Some(view) = win.contentView() else { return };
    view.setWantsLayer(true);
    let layer: *mut AnyObject = unsafe { msg_send![&*view, layer] };
    if !layer.is_null() {
        unsafe {
            let _: () = msg_send![layer, setCornerRadius: radius];
            let _: () = msg_send![layer, setMasksToBounds: Bool::new(radius > 0.0)];
        }
    }
}

/// Рабочая область экрана с окном (без строки меню и Dock), в физических пикселях winit.
pub fn work_area(w: &slint::Window) -> (i32, i32, i32, i32) {
    let k = f64::from(scale(w));
    if let Some(mtm) = MainThreadMarker::new() {
        let screens = NSScreen::screens(mtm);
        if let Some(primary) = screens.firstObject() {
            let primary_h = primary.frame().size.height;
            let screen = ns_window(w).and_then(|win| win.screen()).unwrap_or(primary);
            let v = screen.visibleFrame();
            let left = (v.origin.x * k).round() as i32;
            let top = ((primary_h - (v.origin.y + v.size.height)) * k).round() as i32;
            let right = ((v.origin.x + v.size.width) * k).round() as i32;
            let bottom = ((primary_h - v.origin.y) * k).round() as i32;
            return (left, top, right, bottom);
        }
    }
    let s = w.size();
    (0, 0, s.width as i32, s.height as i32)
}

#[link(name = "UniformTypeIdentifiers", kind = "framework")]
extern "C" {}

/// Системный диалог выбора картинки (JPEG, PNG). `None` — пользователь отказался.
pub fn pick_image(_: &slint::Window, title: &str) -> Option<String> {
    let mtm = MainThreadMarker::new()?;
    let panel = NSOpenPanel::openPanel(mtm);
    panel.setCanChooseFiles(true);
    panel.setCanChooseDirectories(false);
    panel.setAllowsMultipleSelection(false);
    panel.setTitle(Some(&NSString::from_str(title)));
    if let Some(cls) = AnyClass::get(c"UTType") {
        let mut types: Vec<Retained<AnyObject>> = Vec::new();
        for ext in ["jpg", "jpeg", "png"] {
            let ns_ext = NSString::from_str(ext);
            let ut: Option<Retained<AnyObject>> =
                unsafe { msg_send![cls, typeWithFilenameExtension: &*ns_ext] };
            if let Some(ut) = ut {
                types.push(ut);
            }
        }
        if !types.is_empty() {
            let refs: Vec<&AnyObject> = types.iter().map(|t| &**t).collect();
            let arr = NSArray::from_slice(&refs);
            let _: () = unsafe { msg_send![&*panel, setAllowedContentTypes: &*arr] };
        }
    }
    if panel.runModal() != NSModalResponseOK {
        return None;
    }
    panel.URL()?.path().map(|p| p.to_string())
}

/// Системный диалог выбора папки, открывается на `start`. `None` — пользователь отказался.
pub fn pick_folder(_: &slint::Window, title: &str, start: &str) -> Option<String> {
    let mtm = MainThreadMarker::new()?;
    let panel = NSOpenPanel::openPanel(mtm);
    panel.setCanChooseFiles(false);
    panel.setCanChooseDirectories(true);
    panel.setCanCreateDirectories(true);
    panel.setAllowsMultipleSelection(false);
    panel.setTitle(Some(&NSString::from_str(title)));
    if Path::new(start).is_dir() {
        let url = NSURL::fileURLWithPath(&NSString::from_str(start));
        panel.setDirectoryURL(Some(&url));
    }
    if panel.runModal() != NSModalResponseOK {
        return None;
    }
    panel.URL()?.path().map(|p| p.to_string())
}

pub fn open_url(url: &str) {
    let Some(u) = NSURL::URLWithString(&NSString::from_str(url)) else { return };
    NSWorkspace::sharedWorkspace().openURL(&u);
}

pub fn copy_text(text: &str) {
    let pb = NSPasteboard::generalPasteboard();
    pb.clearContents();
    let _ = pb.setString_forType(&NSString::from_str(text), unsafe { NSPasteboardTypeString });
}

extern "C" {
    fn malloc_zone_pressure_relief(zone: *mut c_void, goal: usize) -> usize;
    fn gethostname(name: *mut std::ffi::c_char, len: usize) -> std::ffi::c_int;
    fn getuid() -> u32;
}

/// Вернуть системе свободные страницы кучи.
pub fn trim_memory() {
    unsafe {
        malloc_zone_pressure_relief(std::ptr::null_mut(), 0);
    }
}

pub fn release_memory() {
    trim_memory();
}

/// Светлая ли системная тема macOS. При тёмной в UserDefaults лежит `AppleInterfaceStyle = "Dark"`.
pub fn system_light_theme() -> bool {
    let defaults = NSUserDefaults::standardUserDefaults();
    let style = defaults.stringForKey(ns_string!("AppleInterfaceStyle"));
    !style.is_some_and(|s| s.to_string().eq_ignore_ascii_case("dark"))
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()))
}

pub fn config_base() -> PathBuf {
    home().join("Library").join("Application Support")
}

pub fn cache_base() -> PathBuf {
    home().join("Library").join("Caches")
}

pub fn music_dir() -> PathBuf {
    home().join("Music")
}

pub fn hostname() -> Option<String> {
    let mut buf = [0u8; 256];
    let rc = unsafe { gethostname(buf.as_mut_ptr().cast(), buf.len()) };
    if rc != 0 {
        return None;
    }
    let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    let name = String::from_utf8_lossy(&buf[..len]).trim().to_string();
    let name = name.strip_suffix(".local").unwrap_or(&name).to_string();
    (!name.is_empty()).then_some(name)
}

fn autostart_file() -> PathBuf {
    home().join("Library").join("LaunchAgents").join(format!("{BUNDLE_ID}.plist"))
}

pub fn autostart_enabled() -> bool {
    autostart_file().exists()
}

fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

fn launch_agent_plist(exe: &Path) -> String {
    let exe = xml_escape(&exe.to_string_lossy());
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{BUNDLE_ID}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{exe}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
</dict>
</plist>
"#
    )
}

/// Автозапуск через LaunchAgent пользователя.
pub fn set_autostart(on: bool) -> bool {
    let file = autostart_file();
    if !on {
        return std::fs::remove_file(&file).is_ok() || !file.exists();
    }
    let Ok(exe) = std::env::current_exe() else { return false };
    let plist = launch_agent_plist(&exe);
    file.parent().is_some_and(|d| std::fs::create_dir_all(d).is_ok()) && std::fs::write(&file, plist).is_ok()
}

pub fn refresh_autostart() {
    if autostart_enabled() {
        set_autostart(true);
    }
}

/// Путь unix-сокета единственного экземпляра. На macOS `sun_path` ограничен 104 байтами.
fn socket_path() -> PathBuf {
    let candidate = std::env::temp_dir().join(format!("{APP_ID}.sock"));
    if candidate.as_os_str().len() < 96 {
        return candidate;
    }
    let uid = unsafe { getuid() };
    PathBuf::from(format!("/tmp/{APP_ID}-{uid}.sock"))
}

static LISTENER: Mutex<Option<UnixListener>> = Mutex::new(None);

/// Занять сокет единственного экземпляра. Если файл остался после падения, но не отвечает,
/// считаем его брошенным и пересоздаём.
pub fn claim_instance() -> bool {
    let path = socket_path();
    let listener = match UnixListener::bind(&path) {
        Ok(l) => l,
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            if UnixStream::connect(&path).is_ok() {
                return false;
            }
            let _ = std::fs::remove_file(&path);
            match UnixListener::bind(&path) {
                Ok(l) => l,
                Err(_) => return true,
            }
        }
        Err(_) => return true,
    };
    *LISTENER.lock().unwrap() = Some(listener);
    true
}

/// Второй запуск: попросить работающий экземпляр показать окно.
pub fn wake_existing() {
    if let Ok(mut s) = UnixStream::connect(socket_path()) {
        let _ = s.write_all(b"show");
    }
}

/// Звать `f` из фонового потока, когда второй запуск просит показать окно.
pub fn on_wake(f: impl Fn() + Send + 'static) {
    let Some(listener) = LISTENER.lock().unwrap().take() else { return };
    std::thread::spawn(move || {
        for mut s in listener.incoming().flatten() {
            let mut buf = [0u8; 4];
            if s.read(&mut buf).is_ok_and(|n| n > 0) {
                f();
            }
        }
    });
}

pub fn cleanup() {
    let _ = std::fs::remove_file(socket_path());
}

// ---------- Меню macOS и делегат приложения (Dock) ----------

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "YaMusicMacMenuTarget"]
    struct MenuTarget;

    impl MenuTarget {
        #[unsafe(method(openSettings:))]
        fn open_settings(&self, _sender: Option<&AnyObject>) {
            emit(AppEvent::Settings);
        }

        #[unsafe(method(toggleMini:))]
        fn toggle_mini(&self, _sender: Option<&AnyObject>) {
            emit(AppEvent::ToggleMini);
        }

        #[unsafe(method(showMainWindow:))]
        fn show_main_window(&self, _sender: Option<&AnyObject>) {
            emit(AppEvent::Reopen);
        }

        #[unsafe(method(mediaPlayPause:))]
        fn media_play_pause(&self, _sender: Option<&AnyObject>) {
            emit(AppEvent::PlayPause);
        }

        #[unsafe(method(mediaNext:))]
        fn media_next(&self, _sender: Option<&AnyObject>) {
            emit(AppEvent::Next);
        }

        #[unsafe(method(mediaPrev:))]
        fn media_prev(&self, _sender: Option<&AnyObject>) {
            emit(AppEvent::Prev);
        }

        #[unsafe(method(quitApp:))]
        fn quit_app(&self, _sender: Option<&AnyObject>) {
            emit(AppEvent::Quit);
        }
    }
);

thread_local! {
    static MENU_TARGET: std::cell::OnceCell<Retained<MenuTarget>> = const { std::cell::OnceCell::new() };
    static DOCK_MENU: std::cell::OnceCell<Retained<NSMenu>> = const { std::cell::OnceCell::new() };
}

fn menu_target(mtm: MainThreadMarker) -> Retained<MenuTarget> {
    MENU_TARGET.with(|cell| {
        cell.get_or_init(|| unsafe { msg_send![MenuTarget::alloc(mtm), init] }).clone()
    })
}

fn menu_item(
    mtm: MainThreadMarker,
    title: &str,
    action: Option<Sel>,
    key: &str,
    mods: Option<NSEventModifierFlags>,
    target: Option<&AnyObject>,
) -> Retained<NSMenuItem> {
    let item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str(title),
            action,
            &NSString::from_str(key),
        )
    };
    if let Some(m) = mods {
        item.setKeyEquivalentModifierMask(m);
    }
    if let Some(t) = target {
        unsafe { item.setTarget(Some(t)) };
    }
    item
}

/// Нативное меню macOS: пункты приложения (Cmd+,, Cmd+H, Cmd+Q), управление воспроизведением
/// и окном (Cmd+M, Cmd+W, Ctrl+Cmd+F). Пункты Edit (Cmd+C/V/A) намеренно не вешаем на меню,
/// чтобы клавиши доходили до полей ввода Slint.
fn install_main_menu(app: &NSApplication, mtm: MainThreadMarker) {
    let target = menu_target(mtm);
    let target_obj: &AnyObject = &target;
    let name = env!("APP_NAME");

    let menubar = NSMenu::new(mtm);

    // Меню приложения.
    let app_item = NSMenuItem::new(mtm);
    let app_menu = NSMenu::new(mtm);
    app_menu.addItem(&menu_item(
        mtm,
        &format!("О приложении «{name}»"),
        Some(sel!(orderFrontStandardAboutPanel:)),
        "",
        None,
        None,
    ));
    app_menu.addItem(&NSMenuItem::separatorItem(mtm));
    app_menu.addItem(&menu_item(
        mtm,
        "Настройки…",
        Some(sel!(openSettings:)),
        ",",
        None,
        Some(target_obj),
    ));
    app_menu.addItem(&NSMenuItem::separatorItem(mtm));
    app_menu.addItem(&menu_item(
        mtm,
        &format!("Скрыть «{name}»"),
        Some(sel!(hide:)),
        "h",
        None,
        None,
    ));
    app_menu.addItem(&menu_item(
        mtm,
        "Скрыть остальные",
        Some(sel!(hideOtherApplications:)),
        "h",
        Some(NSEventModifierFlags::Option | NSEventModifierFlags::Command),
        None,
    ));
    app_menu.addItem(&menu_item(
        mtm,
        "Показать все",
        Some(sel!(unhideAllApplications:)),
        "",
        None,
        None,
    ));
    app_menu.addItem(&NSMenuItem::separatorItem(mtm));
    app_menu.addItem(&menu_item(
        mtm,
        &format!("Завершить «{name}»"),
        Some(sel!(quitApp:)),
        "q",
        None,
        Some(target_obj),
    ));
    app_item.setSubmenu(Some(&app_menu));
    menubar.addItem(&app_item);

    // Воспроизведение.
    let play_item = NSMenuItem::new(mtm);
    let play_menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), ns_string!("Воспроизведение"));
    play_menu.addItem(&menu_item(
        mtm,
        "Пауза / Воспроизведение",
        Some(sel!(mediaPlayPause:)),
        "",
        None,
        Some(target_obj),
    ));
    play_menu.addItem(&menu_item(
        mtm,
        "Следующий трек",
        Some(sel!(mediaNext:)),
        "",
        None,
        Some(target_obj),
    ));
    play_menu.addItem(&menu_item(
        mtm,
        "Предыдущий трек",
        Some(sel!(mediaPrev:)),
        "",
        None,
        Some(target_obj),
    ));
    play_item.setSubmenu(Some(&play_menu));
    menubar.addItem(&play_item);

    // Окно.
    let win_item = NSMenuItem::new(mtm);
    let win_menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), ns_string!("Окно"));
    win_menu.addItem(&menu_item(
        mtm,
        "Свернуть",
        Some(sel!(performMiniaturize:)),
        "m",
        None,
        None,
    ));
    win_menu.addItem(&menu_item(mtm, "Изменить масштаб", Some(sel!(performZoom:)), "", None, None));
    win_menu.addItem(&menu_item(
        mtm,
        "Перейти в полноэкранный режим",
        Some(sel!(toggleFullScreen:)),
        "f",
        Some(NSEventModifierFlags::Control | NSEventModifierFlags::Command),
        None,
    ));
    win_menu.addItem(&menu_item(
        mtm,
        "Мини-плеер",
        Some(sel!(toggleMini:)),
        "",
        None,
        Some(target_obj),
    ));
    win_menu.addItem(&NSMenuItem::separatorItem(mtm));
    win_menu.addItem(&menu_item(
        mtm,
        "Показать окно",
        Some(sel!(showMainWindow:)),
        "",
        None,
        Some(target_obj),
    ));
    win_menu.addItem(&menu_item(
        mtm,
        "Закрыть окно",
        Some(sel!(performClose:)),
        "w",
        None,
        None,
    ));
    win_item.setSubmenu(Some(&win_menu));
    menubar.addItem(&win_item);

    app.setMainMenu(Some(&menubar));
    app.setWindowsMenu(Some(&win_menu));
}

fn dock_menu(mtm: MainThreadMarker) -> Retained<NSMenu> {
    DOCK_MENU.with(|cell| {
        cell.get_or_init(|| {
            let target = menu_target(mtm);
            let target_obj: &AnyObject = &target;
            let menu = NSMenu::new(mtm);
            menu.addItem(&menu_item(
                mtm,
                "Пауза / Воспроизведение",
                Some(sel!(mediaPlayPause:)),
                "",
                None,
                Some(target_obj),
            ));
            menu.addItem(&menu_item(
                mtm,
                "Следующий трек",
                Some(sel!(mediaNext:)),
                "",
                None,
                Some(target_obj),
            ));
            menu.addItem(&menu_item(
                mtm,
                "Предыдущий трек",
                Some(sel!(mediaPrev:)),
                "",
                None,
                Some(target_obj),
            ));
            menu.addItem(&NSMenuItem::separatorItem(mtm));
            menu.addItem(&menu_item(
                mtm,
                "Показать окно",
                Some(sel!(showMainWindow:)),
                "",
                None,
                Some(target_obj),
            ));
            menu
        })
        .clone()
    })
}

unsafe extern "C-unwind" fn on_reopen(
    _this: *mut AnyObject,
    _cmd: Sel,
    _app: *mut AnyObject,
    _has_visible_windows: Bool,
) -> Bool {
    emit(AppEvent::Reopen);
    Bool::NO
}

unsafe extern "C-unwind" fn on_dock_menu(
    _this: *mut AnyObject,
    _cmd: Sel,
    _app: *mut AnyObject,
) -> *mut NSMenu {
    let Some(mtm) = MainThreadMarker::new() else {
        return std::ptr::null_mut();
    };
    Retained::autorelease_return(dock_menu(mtm))
}

/// Расширяем класс делегата winit методами reopen и меню Dock, не подменяя сам объект делегата
/// (winit проверяет его класс через `is_kind_of::<ApplicationDelegate>()`).
fn install_delegate_hooks(app: &NSApplication) {
    let Some(delegate) = app.delegate() else { return };
    let delegate_ptr: *const AnyObject = Retained::as_ptr(&delegate).cast();
    unsafe {
        let cls = object_getClass(delegate_ptr) as *mut AnyClass;
        if cls.is_null() {
            return;
        }
        let reopen_imp: Imp = std::mem::transmute::<
            unsafe extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject, Bool) -> Bool,
            Imp,
        >(on_reopen);
        let _ = class_addMethod(
            cls,
            sel!(applicationShouldHandleReopen:hasVisibleWindows:),
            reopen_imp,
            c"B@:@B".as_ptr(),
        );
        let dock_imp: Imp = std::mem::transmute::<
            unsafe extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject) -> *mut NSMenu,
            Imp,
        >(on_dock_menu);
        let _ = class_addMethod(
            cls,
            sel!(applicationDockMenu:),
            dock_imp,
            c"@@:@".as_ptr(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plist_escapes_special_chars() {
        let p = launch_agent_plist(Path::new("/Applications/A & B <app>/yamusic-rs"));
        assert!(p.contains("<string>/Applications/A &amp; B &lt;app&gt;/yamusic-rs</string>"));
        assert!(p.contains(BUNDLE_ID));
        assert!(p.contains("<key>RunAtLoad</key>"));
    }

    #[test]
    fn socket_path_fits_sun_path() {
        assert!(socket_path().as_os_str().len() < 104);
    }
}
