//! Яндекс Музыка RS — неофициальный клиент Яндекс Музыки. Точка входа: один экземпляр
//! на пользователя, выбор рендерера, запуск окна.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod api;
mod assets;
mod auth;
mod client;
mod config;
mod discord;
mod eq;
mod gui;
mod i18n;
mod images;
mod landing;
mod mpris;
mod net;
mod palette;
mod player;
mod secret;
mod tags;
mod tray;
mod update;
mod win;
mod yandex_id;

/// Сокет единственного экземпляра: занимает первый процесс и слушает «wake».
#[cfg(not(target_os = "windows"))]
static WAKE_LISTENER: std::sync::OnceLock<std::sync::Mutex<Option<std::os::unix::net::UnixListener>>> =
    std::sync::OnceLock::new();

/// Занять мьютекс единственного экземпляра. Если он уже чей-то — отпустить свою ссылку,
/// чтобы мьютекс исчез вместе с тем процессом.
#[cfg(windows)]
fn claim_instance() -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS};
    use windows_sys::Win32::System::Threading::CreateMutexW;
    let name = win::wide(r"Local\yamusic_rs_single_instance");
    unsafe {
        let m = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
        if GetLastError() == ERROR_ALREADY_EXISTS {
            CloseHandle(m);
            return false;
        }
        true
    }
}

/// Linux: тот же контракт через Unix-сокет в `XDG_RUNTIME_DIR`. Сокет живого экземпляра
/// отвечает на подключение; файл от умершего процесса удаляем и занимаем заново.
#[cfg(not(target_os = "windows"))]
fn claim_instance() -> bool {
    use std::os::unix::net::UnixListener;
    let slot = WAKE_LISTENER.get_or_init(|| std::sync::Mutex::new(None));
    let Ok(mut guard) = slot.lock() else { return false };
    // Уже слушаем (повторная попытка после ожидания обновления) — экземпляр наш.
    if guard.is_some() {
        return true;
    }
    if wake_socket_alive() {
        return false;
    }
    // Fallback-путь (без XDG_RUNTIME_DIR) — внутри каталога данных приложения,
    // который ещё никто не создал: создаём, иначе bind молча не удастся.
    let _ = std::fs::create_dir_all(crate::config::dir());
    let _ = std::fs::remove_file(instance_socket());
    match UnixListener::bind(instance_socket()) {
        Ok(l) => {
            *guard = Some(l);
            true
        }
        Err(e) => {
            crate::config::log(format!("не занять сокет единственного экземпляра: {e}"));
            false
        }
    }
}

/// Путь управляющего сокета единственного экземпляра.
#[cfg(not(target_os = "windows"))]
fn instance_socket() -> std::path::PathBuf {
    std::env::var("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| crate::config::dir())
        .join("yamusic-rs.sock")
}

/// Живёт ли сейчас чужой экземпляр (сокет кто-то слушает).
#[cfg(not(target_os = "windows"))]
fn wake_socket_alive() -> bool {
    use std::io::Write;
    use std::os::unix::net::UnixStream;
    let Ok(mut s) = UnixStream::connect(instance_socket()) else { return false };
    s.write_all(b"wake").is_ok()
}

/// Слушать сокет в фоне: второе подключение просит показать окно. Ничего не делает,
/// если сокет у нас не занят (тогда экземпляр второй и его сокет должен умереть с ним).
#[cfg(not(target_os = "windows"))]
fn spawn_wake_listener() {
    use std::io::Read;
    let Some(listener) = WAKE_LISTENER.get().and_then(|s| s.lock().ok()).and_then(|mut s| s.take()) else { return };
    let _ = std::thread::Builder::new().name("wake-socket".into()).spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut buf = [0u8; 16];
            let mut stream = stream;
            let n = stream.read(&mut buf).unwrap_or(0);
            if buf[..n].starts_with(b"wake") {
                let _ = crate::gui::wake();
            }
        }
    });
}

/// Попросить уже работающий экземпляр показать окно.
#[cfg(not(target_os = "windows"))]
fn wake_running() {
    let _ = wake_socket_alive();
}

/// Попросить уже работающий экземпляр показать окно.
#[cfg(windows)]
fn wake_running() {
    // Второй запуск просто показывает уже открытое окно.
    if tray::wake_existing() {
        return;
    }
    let h: isize = std::fs::read_to_string(config::dir().join("hwnd")).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
    if h != 0 && win::is_window(h) {
        win::show(h);
        win::force_foreground(h);
    }
}

fn main() -> Result<(), slint::PlatformError> {
    // После обновления старая версия ещё закрывается: ждём её до 15 секунд.
    // (Только Windows: на Linux обновление — это установка deb пакетным менеджером.)
    #[cfg(windows)]
    let after_update = std::env::args().any(|a| a == update::AFTER_UPDATE);
    #[cfg(not(target_os = "windows"))]
    let after_update = false;
    let mut claimed = claim_instance();
    for _ in 0..60 {
        if claimed || !after_update {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
        claimed = claim_instance();
    }
    if !claimed {
        wake_running();
        return Ok(());
    }
    // Рендер без видеокарты (Windows): память не зависит от драйвера (с OpenGL или D3D
    // окно занимает в 2–3 раза больше). На Linux — femtovg: Mesa 26 на целевой машине
    // даёт OpenGL 4.6, а сборка без Skia быстрее и легче. Режим работы («Плавный» или
    // «Лёгкий») меняет только анимации и кэши и переключается на лету (gui::apply_mode).
    slint::BackendSelector::new().backend_name("winit".into()).renderer_name(renderer_name().into()).select()?;
    // Имя приложения для Wayland: по нему GNOME находит .desktop (значок в доке,
    // подпись окна) и связывает окно с MPRIS-плеером.
    #[cfg(not(target_os = "windows"))]
    match slint::set_xdg_app_id("yamusic-rs") {
        Ok(()) => crate::config::log("окно: app-id yamusic-rs"),
        Err(e) => crate::config::log(format!("окно: app-id не задан — {e}")),
    }
    // Убрать exe прошлой версии после self-update — это только Windows.
    #[cfg(windows)]
    update::cleanup();
    win::refresh_autostart();
    // `--hidden` — родное имя для Linux; `--tray` оставлен как алиас (автозапуск оригинала).
    let start_hidden = std::env::args().any(|a| a == "--hidden" || a == "--tray");
    #[cfg(not(target_os = "windows"))]
    spawn_wake_listener();
    gui::run(start_hidden)
}

#[cfg(windows)]
fn renderer_name() -> &'static str {
    "skia-software"
}

#[cfg(not(target_os = "windows"))]
fn renderer_name() -> &'static str {
    "femtovg"
}
