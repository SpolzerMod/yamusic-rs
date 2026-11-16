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
mod fade;
mod gui;
mod i18n;
mod images;
mod landing;
#[cfg(target_os = "macos")]
mod mac;
mod net;
mod palette;
mod player;
mod secret;
mod stream;
mod tags;
mod tray;
mod update;
mod win;
mod yandex_id;

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

/// На macOS второй запуск .app не создаёт процесс (LaunchServices показывает уже
/// открытое окно), отдельного мьютекса не нужно: считаем себя первым.
#[cfg(not(windows))]
fn claim_instance() -> bool {
    true
}

fn main() -> Result<(), slint::PlatformError> {
    // В release stderr теряется: панику пишем в лог (сообщение + место),
    // чтобы краши можно было разбирать по log.txt без crash report'ов.
    std::panic::set_hook(Box::new(|info| {
        config::log(format!("PANIC: {info}"));
    }));
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
    // Дефолт — CPU (skia-software): так задумал автор (память и совместимость
    // не зависят от драйвера). GPU (Skia через wgpu: macOS — Metal, Windows —
    // DX12/Vulkan) включается в настройках; замер на macOS дал CPU 25.1% → 4.3%,
    // RSS 178 → 137 МБ, но на слабых машинах решение остаётся за пользователем.
    // Env YAMUSIC_RENDERER перекрывает конфиг; при ошибке выбора GPU — fallback.
    let want = std::env::var("YAMUSIC_RENDERER").unwrap_or_default();
    let gpu_on = config::Config::load().gpu;
    let renderer = match want.as_str() {
        "skia-software" | "skia" | "skia-opengl" | "skia-wgpu" => want.clone(),
        _ => if gpu_on { "skia-wgpu".to_string() } else { "skia-software".to_string() },
    };
    config::log(format!("renderer: {renderer}"));
    let selected = slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name(renderer.clone())
        .select();
    if selected.is_err() && renderer != "skia-software" {
        config::log(format!("renderer {renderer} недоступен, fallback на skia-software"));
        slint::BackendSelector::new().backend_name("winit".into()).renderer_name("skia-software".into()).select()?;
    } else {
        selected?;
    }
    update::cleanup();
    win::refresh_autostart();
    // На macOS трея нет: --tray игнорируем, иначе окно спрячется без возможности вернуть.
    #[cfg(windows)]
    let start_hidden = std::env::args().any(|a| a == "--tray");
    #[cfg(not(windows))]
    let start_hidden = false;
    gui::run(start_hidden)
}
