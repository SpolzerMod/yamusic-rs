//! macOS: окно через winit, папки ~/Library, один экземпляр через unix-сокет, диалоги
//! через osascript, автозапуск через LaunchAgent. Трея нет: закрытие окна закрывает
//! приложение, как и на Linux.

use slint::winit_030::WinitWindowAccessor;
use std::path::PathBuf;
use std::process::{Command, Stdio};

#[path = "unix.rs"]
mod unix;
pub use unix::{claim_instance, cleanup, on_wake, wake_existing};

/// Bundle identifier из Info.plist (tools/macos-app.sh) и имя LaunchAgent.
const BUNDLE_ID: &str = "io.github.spolzermod.yamusic-rs";

pub fn created(w: &slint::Window) -> bool {
    w.with_winit_window(|_| ()).is_some()
}

/// Значок в Dock берётся из бандла .app, у окна на macOS своего значка нет.
pub fn prepare(_: &slint::Window) {}

/// winit на macOS вместе с фокусом активирует и само приложение.
pub fn raise(w: &slint::Window) {
    w.set_minimized(false);
    w.with_winit_window(|w| w.focus_window());
}

pub fn scale(w: &slint::Window) -> f32 {
    w.with_winit_window(|w| w.scale_factor() as f32).unwrap_or(1.0)
}

/// Монитор с окном целиком: строку меню и Dock winit не вычитает.
pub fn work_area(w: &slint::Window) -> (i32, i32, i32, i32) {
    w.with_winit_window(|w| {
        let m = w.current_monitor()?;
        let (p, s) = (m.position(), m.size());
        Some((p.x, p.y, p.x + s.width as i32, p.y + s.height as i32))
    })
    .flatten()
    .unwrap_or_else(|| {
        let s = w.size();
        (0, 0, s.width as i32, s.height as i32)
    })
}

/// AppleScript-диалог. Заголовок и папка уходят аргументами, а не в текст скрипта:
/// кавычки в них ничего не сломают. Отмена — osascript завершается с ошибкой.
fn osascript(script: &str, args: &[&str]) -> Option<String> {
    let out = Command::new("osascript").arg("-e").arg(script).args(args).stderr(Stdio::null()).output().ok()?;
    let path = String::from_utf8_lossy(&out.stdout).trim_end_matches('\n').to_string();
    (out.status.success() && !path.is_empty()).then_some(path)
}

pub fn pick_image(_: &slint::Window, title: &str) -> Option<String> {
    osascript(
        r#"on run argv
  POSIX path of (choose file with prompt (item 1 of argv) of type {"public.jpeg", "public.png"})
end run"#,
        &[title],
    )
}

pub fn pick_folder(_: &slint::Window, title: &str, start: &str) -> Option<String> {
    let start = if std::path::Path::new(start).is_dir() { start.to_string() } else { home().display().to_string() };
    osascript(
        r#"on run argv
  POSIX path of (choose folder with prompt (item 1 of argv) default location (POSIX file (item 2 of argv)))
end run"#,
        &[title, &start],
    )
    .map(|p| if p.len() > 1 { p.trim_end_matches('/').to_string() } else { p })
}

pub fn open_url(url: &str) {
    if let Ok(mut child) = Command::new("open").arg(url).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
        std::thread::spawn(move || child.wait());
    }
}

/// Вернуть системе свободные страницы malloc, как malloc_trim на Linux.
pub fn trim_memory() {
    extern "C" {
        fn malloc_zone_pressure_relief(zone: *mut std::ffi::c_void, goal: usize) -> usize;
    }
    unsafe {
        malloc_zone_pressure_relief(std::ptr::null_mut(), 0);
    }
}

pub fn release_memory() {
    trim_memory();
}

/// В тёмном оформлении в глобальных настройках есть `AppleInterfaceStyle = Dark`,
/// в светлом ключа нет вовсе.
pub fn system_light_theme() -> bool {
    let out = Command::new("defaults").args(["read", "-g", "AppleInterfaceStyle"]).stderr(Stdio::null()).output();
    !matches!(out, Ok(o) if String::from_utf8_lossy(&o.stdout).trim().eq_ignore_ascii_case("dark"))
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

/// Finder показывает её как «Музыка», но на диске она всегда Music.
pub fn music_dir() -> PathBuf {
    home().join("Music")
}

/// Имя компьютера из «Основные → Общий доступ», а не сетевое имя хоста.
pub fn hostname() -> Option<String> {
    let out = Command::new("scutil").args(["--get", "ComputerName"]).stderr(Stdio::null()).output().ok()?;
    let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !name.is_empty()).then_some(name)
}

pub fn copy_text(text: &str) {
    if let Ok(mut c) = arboard::Clipboard::new() {
        let _ = c.set_text(text);
    }
}

fn agent_file() -> PathBuf {
    home().join("Library").join("LaunchAgents").join(format!("{BUNDLE_ID}.plist"))
}

pub fn autostart_enabled() -> bool {
    agent_file().exists()
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// LaunchAgent в ~/Library/LaunchAgents: launchd запустит приложение при входе в систему.
/// Без трея запускаемся с окном, а не скрытыми.
pub fn set_autostart(on: bool) -> bool {
    let file = agent_file();
    if !on {
        return std::fs::remove_file(&file).is_ok() || !file.exists();
    }
    let Ok(exe) = std::env::current_exe() else { return false };
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{BUNDLE_ID}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{}</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>ProcessType</key>
  <string>Interactive</string>
</dict>
</plist>
"#,
        xml_escape(&exe.to_string_lossy())
    );
    if std::fs::read_to_string(&file).is_ok_and(|old| old == plist) {
        return true;
    }
    file.parent().is_some_and(|d| std::fs::create_dir_all(d).is_ok()) && std::fs::write(&file, plist).is_ok()
}

/// Приложение могли перенести в другую папку: путь в LaunchAgent обновляем.
pub fn refresh_autostart() {
    if autostart_enabled() {
        set_autostart(true);
    }
}
