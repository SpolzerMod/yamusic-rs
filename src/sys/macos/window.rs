//! Окно и системная тема macOS.
use slint::winit_030::WinitWindowAccessor;
use std::process::{Command, Stdio};

pub fn created(w: &slint::Window) -> bool {
    w.with_winit_window(|_| ()).is_some()
}

/// Значок macOS берёт из пакета .app.
pub fn prepare(_: &slint::Window) {}

/// Показать окно работающего экземпляра.
pub fn raise(w: &slint::Window) {
    w.set_minimized(false);
    w.with_winit_window(|w| w.focus_window());
}

pub fn scale(w: &slint::Window) -> f32 {
    w.with_winit_window(|w| w.scale_factor() as f32).unwrap_or(1.0)
}

/// Размер монитора; если он неизвестен, используем размер окна.
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

pub fn system_light_theme() -> bool {
    !Command::new("/usr/bin/defaults")
        .args(["read", "-g", "AppleInterfaceStyle"])
        .stderr(Stdio::null())
        .output()
        .is_ok_and(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).trim() == "Dark")
}
