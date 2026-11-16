//! Платформенный слой для macOS. Окном управляет slint/winit, HWND здесь всегда 0,
//! поэтому win-функции для HWND — заглушки. Рабочие части: `open_url` (команда
//! `open`), тема (`defaults`), диалог выбора файла (`osascript`) и автозапуск
//! (LaunchAgent в `~/Library/LaunchAgents`, аналог HKCU\Run — без прав администратора).

use std::path::PathBuf;

// ---------- заглушки: окном управляет slint ----------
// Часть из них сейчас не вызывается вне Windows (вызовы в gui.rs закрыты cfg),
// но они оставлены для парности API с win::windows.

#[allow(dead_code)]
pub fn is_visible(_hwnd: isize) -> bool {
    false
}

pub fn is_iconic(_hwnd: isize) -> bool {
    false
}

pub fn show(_hwnd: isize) {}

#[allow(dead_code)]
pub fn hide(_hwnd: isize) {}

pub fn force_foreground(_hwnd: isize) {}

pub fn is_window(_hwnd: isize) -> bool {
    false
}

/// Скруглённые углы у окна macOS свои, DWM не нужен.
#[allow(dead_code)]
pub fn round_corners(_hwnd: isize) {}

/// Значок окна на macOS берётся из .app-бандла (tools/make_app.sh).
#[allow(dead_code)]
pub fn set_icon(_hwnd: isize) {}

/// SetProcessWorkingSetSize — только Windows; память на macOS отдаётся сама.
#[allow(dead_code)]
pub fn trim_memory() {}

pub fn release_memory() {}

// ---------- рабочие реализации ----------

pub fn open_url(url: &str) {
    let _ = std::process::Command::new("open").arg(url).spawn();
}

/// Системный диалог выбора картинки (JPEG, PNG). `None` — пользователь отказался.
/// `osascript` печатает POSIX-путь без кавычек.
pub fn pick_image(_owner: isize, title: &str) -> Option<String> {
    let t = title.replace('\\', "\\\\").replace('"', "\\\"");
    let script = format!("POSIX path of (choose file with prompt \"{t}\" of type {{\"public.jpeg\", \"public.png\"}})");
    let out = std::process::Command::new("osascript").arg("-e").arg(script).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Системный диалог выбора папки (macOS: `choose folder` через osascript).
pub fn pick_folder(_owner: isize, title: &str, start: &str) -> Option<String> {
    let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    let def = if start.is_empty() {
        String::new()
    } else {
        format!(" default location (POSIX file \"{}\")", esc(start))
    };
    let script = format!("POSIX path of (choose folder with prompt \"{}\"{def})", esc(title));
    let out = std::process::Command::new("osascript").arg("-e").arg(script).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    // osascript возвращает путь с завершающим «/» — уберём, кроме корня.
    Some(if s.len() > 1 { s.trim_end_matches('/').to_string() } else { s })
}

/// Рабочая область экрана в физических пикселях (лево, верх, право, низ).
/// На macOS считает `crate::mac` (NSScreen visibleFrame); на остальных unix — заглушка.
pub fn work_area(_hwnd: isize) -> (i32, i32, i32, i32) {
    #[cfg(target_os = "macos")]
    {
        crate::mac::work_area()
    }
    #[cfg(not(target_os = "macos"))]
    {
        (0, 0, 1920, 1080)
    }
}

/// Светлая ли тема macOS: `defaults` отвечает «Dark» при тёмной теме,
/// а на светлой ругается, что ключа нет.
pub fn system_light_theme() -> bool {
    std::process::Command::new("defaults")
        .args(["read", "-g", "AppleInterfaceStyle"])
        .output()
        .is_ok_and(|o| !o.status.success())
}

// ---------- автозапуск: LaunchAgent ----------

const LAUNCH_LABEL: &str = "ru.spolzermod.yamusic-rs";

fn launch_agent_path() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    let p = PathBuf::from(home).join("Library").join("LaunchAgents").join(format!("{LAUNCH_LABEL}.plist"));
    std::fs::create_dir_all(p.parent()?).ok()?;
    Some(p)
}

/// Записан ли автозапуск.
pub fn autostart_enabled() -> bool {
    launch_agent_path().is_some_and(|p| p.exists())
}

/// Включить или выключить автозапуск (plist в ~/Library/LaunchAgents).
/// Бинарник запускается обычным окном: трея на macOS пока нет.
pub fn set_autostart(on: bool) -> bool {
    let Some(p) = launch_agent_path() else { return false };
    if !on {
        return match std::fs::remove_file(&p) {
            Ok(()) => true,
            Err(_) => !p.exists(),
        };
    }
    let Ok(exe) = std::env::current_exe() else { return false };
    let plist = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict><key>Label</key><string>{LAUNCH_LABEL}</string><key>ProgramArguments</key><array><string>{}</string></array><key>RunAtLoad</key><true/></dict></plist>\n",
        exe.display()
    );
    std::fs::write(&p, plist).is_ok()
}

/// Бинарник перенесли — поправить путь в автозапуске, если он включён.
pub fn refresh_autostart() {
    if autostart_enabled() {
        set_autostart(true);
    }
}
