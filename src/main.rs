//! Яндекс Музыка RS — неофициальный клиент Яндекс Музыки. Точка входа: один экземпляр
//! на пользователя, выбор рендерера, запуск окна.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod api;
mod assets;
mod auth;
mod client;
mod config;
mod discord;
mod downloads;
mod eq;
mod flac;
mod gui;
mod i18n;
mod images;
mod landing;
mod net;
mod palette;
mod player;
mod secret;
mod tags;
mod tray;
mod update;
mod win;
mod yandex_id;

/// Занять мьютекс единственного экземпляра. Если он уже чей-то — отпустить свою ссылку,
/// чтобы мьютекс исчез вместе с тем процессом.
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

fn main() -> Result<(), slint::PlatformError> {
    // После обновления старая версия ещё закрывается: ждём её до 15 секунд.
    let after_update = std::env::args().any(|a| a == update::AFTER_UPDATE);
    let mut claimed = claim_instance();
    for _ in 0..60 {
        if claimed || !after_update {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
        claimed = claim_instance();
    }
    if !claimed {
        // Второй запуск просто показывает уже открытое окно.
        if tray::wake_existing() {
            return Ok(());
        }
        let h: isize = std::fs::read_to_string(config::dir().join("hwnd")).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
        if h != 0 && win::is_window(h) {
            win::show(h);
            win::force_foreground(h);
        }
        return Ok(());
    }
    // Skia без видеокарты: память не зависит от драйвера (с OpenGL или D3D окно занимает
    // в 2–3 раза больше). Режим работы («Плавный» или «Лёгкий») меняет только анимации
    // и кэши и переключается на лету (gui::apply_mode).
    slint::BackendSelector::new().backend_name("winit".into()).renderer_name("skia-software".into()).select()?;
    update::cleanup();
    win::refresh_autostart();
    let start_hidden = std::env::args().any(|a| a == "--tray");
    gui::run(start_hidden)
}
