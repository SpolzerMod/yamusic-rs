//! Тонкие обёртки над AppKit — то же, что win.rs для Windows. «Дескриптор» окна здесь —
//! указатель на NSView окна winit; все вызовы идут из основного потока.

use objc2::rc::Retained;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSApplication, NSModalResponseOK, NSOpenPanel, NSScreen, NSView, NSWindow};
use objc2_foundation::{NSArray, NSString, NSURL};
use std::path::PathBuf;

/// Идентификатор приложения: тот же, что в Info.plist (tools/bundle_macos.sh), он же имя агента автозапуска.
const BUNDLE_ID: &str = "io.github.spolzermod.yamusic-rs";

fn window(hwnd: isize) -> Option<Retained<NSWindow>> {
    MainThreadMarker::new()?;
    if hwnd == 0 {
        return None;
    }
    // SAFETY: адрес NSView отдаёт winit, пока окно живо; gui.rs спрашивает его заново при каждом вызове.
    unsafe { (&*(hwnd as *const NSView)).window() }
}

pub fn is_visible(hwnd: isize) -> bool {
    // Как IsWindowVisible: свёрнутое в Dock окно остаётся видимым.
    window(hwnd).is_some_and(|w| w.isVisible() || w.isMiniaturized())
}

pub fn is_iconic(hwnd: isize) -> bool {
    window(hwnd).is_some_and(|w| w.isMiniaturized())
}

pub fn show(hwnd: isize) {
    if let Some(w) = window(hwnd) {
        if w.isMiniaturized() {
            w.deminiaturize(None);
        }
        w.makeKeyAndOrderFront(None);
    }
}

pub fn hide(hwnd: isize) {
    if let Some(w) = window(hwnd) {
        w.orderOut(None);
    }
}

/// Сделать приложение активным: окно, спрятанное в меню-бар, иначе не выйдет на передний план.
pub fn force_foreground(hwnd: isize) {
    let _ = hwnd;
    if let Some(mtm) = MainThreadMarker::new() {
        #[allow(deprecated)]
        NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
    }
}

/// Масштаб экрана с окном (2.0 на Retina).
pub fn dpi_scale(hwnd: isize) -> f32 {
    window(hwnd).map_or(1.0, |w| w.backingScaleFactor() as f32)
}

/// Значок окна задаёт .app (Info.plist), на macOS отдельно его ставить не нужно.
pub fn set_icon(_hwnd: isize) {}

/// Углы окна на macOS скругляет система.
pub fn round_corners(_hwnd: isize) {}

/// Системный диалог выбора картинки (JPEG, PNG). `None` — пользователь отказался.
pub fn pick_image(_owner: isize, title: &str) -> Option<String> {
    let panel = open_panel(title)?;
    panel.setCanChooseFiles(true);
    panel.setCanChooseDirectories(false);
    let types = NSArray::from_retained_slice(&[NSString::from_str("jpg"), NSString::from_str("jpeg"), NSString::from_str("png")]);
    #[allow(deprecated)]
    panel.setAllowedFileTypes(Some(&types));
    run_panel(&panel)
}

/// Системный диалог выбора папки, открывается на `start`. `None` — пользователь отказался.
pub fn pick_folder(_owner: isize, title: &str, start: &str) -> Option<String> {
    let panel = open_panel(title)?;
    panel.setCanChooseFiles(false);
    panel.setCanChooseDirectories(true);
    panel.setCanCreateDirectories(true);
    if !start.is_empty() {
        panel.setDirectoryURL(Some(&NSURL::fileURLWithPath(&NSString::from_str(start))));
    }
    run_panel(&panel)
}

fn open_panel(title: &str) -> Option<Retained<NSOpenPanel>> {
    let panel = NSOpenPanel::openPanel(MainThreadMarker::new()?);
    panel.setAllowsMultipleSelection(false);
    panel.setMessage(Some(&NSString::from_str(title)));
    Some(panel)
}

fn run_panel(panel: &NSOpenPanel) -> Option<String> {
    if panel.runModal() != NSModalResponseOK {
        return None;
    }
    Some(panel.URL()?.path()?.to_string())
}

/// Рабочая область экрана с окном (без меню-бара и Dock), в физических пикселях: лево, верх, право, низ.
/// Координаты те же, что у `set_position` winit: начало у левого верхнего угла основного экрана.
pub fn work_area(hwnd: isize) -> (i32, i32, i32, i32) {
    let Some(mtm) = MainThreadMarker::new() else { return (0, 0, 0, 0) };
    let w = window(hwnd);
    let screens = NSScreen::screens(mtm);
    let (Some(screen), Some(primary)) = (w.as_ref().and_then(|w| w.screen()).or_else(|| NSScreen::mainScreen(mtm)), screens.firstObject())
    else {
        return (0, 0, 0, 0);
    };
    let k = screen.backingScaleFactor();
    let top_left = primary.frame().size.height;
    let v = screen.visibleFrame();
    let px = |p: f64| (p * k).round() as i32;
    (px(v.origin.x), px(top_left - (v.origin.y + v.size.height)), px(v.origin.x + v.size.width), px(top_left - v.origin.y))
}

pub fn open_url(url: &str) {
    if !crate::client::is_https(url) {
        crate::config::log(format!("ссылка не открыта, не https: {}", url.split(':').next().unwrap_or("")));
        return;
    }
    let _ = std::process::Command::new("open").arg(url).status();
}

/// На macOS систему просить вернуть память не нужно.
pub fn trim_memory() {}

pub fn release_memory() {}

/// Светлая ли тема macOS.
pub fn system_light_theme() -> bool {
    let Some(mtm) = MainThreadMarker::new() else { return true };
    let names = unsafe { NSArray::from_slice(&[NSAppearanceNameAqua, NSAppearanceNameDarkAqua]) };
    let best = NSApplication::sharedApplication(mtm).effectiveAppearance().bestMatchFromAppearancesWithNames(&names);
    !best.is_some_and(|n| &*n == unsafe { NSAppearanceNameDarkAqua })
}

fn agent_path() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var("HOME").ok()?).join("Library/LaunchAgents").join(format!("{BUNDLE_ID}.plist")))
}

/// Агент запуска: этот exe с `--tray`, то есть сразу в меню-баре, без окна.
fn agent_plist(exe: &str) -> String {
    let exe = exe.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
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
    <string>--tray</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
</dict>
</plist>
"#
    )
}

/// Записан ли автозапуск вместе с macOS.
pub fn autostart_enabled() -> bool {
    agent_path().is_some_and(|p| p.exists())
}

/// Включить или выключить автозапуск: агент запуска в ~/Library/LaunchAgents, права не нужны.
pub fn set_autostart(on: bool) -> bool {
    let Some(path) = agent_path() else { return false };
    if !on {
        return match std::fs::remove_file(&path) {
            Ok(()) => true,
            Err(e) => e.kind() == std::io::ErrorKind::NotFound,
        };
    }
    let Ok(exe) = std::env::current_exe() else { return false };
    let plist = agent_plist(&exe.display().to_string());
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    std::fs::write(path, plist).is_ok()
}

/// Приложение перенесли — поправить путь в автозапуске, если он включён.
pub fn refresh_autostart() {
    if autostart_enabled() {
        set_autostart(true);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn agent_plist() {
        let p = super::agent_plist("/Applications/A & B.app/Contents/MacOS/yamusic-rs");
        assert!(p.contains("<string>/Applications/A &amp; B.app/Contents/MacOS/yamusic-rs</string>"));
        assert!(p.contains("<string>--tray</string>"));
        assert!(p.contains(&format!("<string>{}</string>", super::BUNDLE_ID)));
    }
}
