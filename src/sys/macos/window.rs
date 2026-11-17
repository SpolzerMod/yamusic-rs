//! Окно и системная тема macOS.
use slint::winit_030::WinitWindowAccessor;

pub fn created(w: &slint::Window) -> bool {
    w.with_winit_window(|_| ()).is_some()
}

/// Значок macOS берёт из пакета .app.
pub fn prepare(_: &slint::Window) {
    super::native::prepare_window();
}

/// Показать окно работающего экземпляра.
pub fn raise(w: &slint::Window) {
    w.set_minimized(false);
    w.with_winit_window(|w| {
        w.set_visible(true);
        w.focus_window();
    });
    super::native::activate();
}

pub fn scale(w: &slint::Window) -> f32 {
    w.with_winit_window(|w| w.scale_factor() as f32).unwrap_or(1.0)
}

/// Размер монитора; если он неизвестен, используем размер окна.
pub fn work_area(w: &slint::Window) -> (i32, i32, i32, i32) {
    w.with_winit_window(|w| {
        let m = w.current_monitor()?;
        let (p, s) = (m.position(), m.size());
        if let Some((l, t, r, b)) = super::native::visible_area() {
            let scale = m.scale_factor();
            Some((
                p.x + (l * scale) as i32,
                p.y + (t * scale) as i32,
                p.x + (r * scale) as i32,
                p.y + (b * scale) as i32,
            ))
        } else {
            Some((p.x, p.y, p.x + s.width as i32, p.y + s.height as i32))
        }
    })
    .flatten()
    .unwrap_or_else(|| {
        let s = w.size();
        (0, 0, s.width as i32, s.height as i32)
    })
}

pub fn system_light_theme() -> bool {
    super::native::light_theme()
}
pub fn ensure_visible() {
    super::native::ensure_visible();
}

/// Не уничтожаем окно: Dock и системные команды должны вернуть тот же плеер.
pub fn hide(w: &slint::Window) {
    w.with_winit_window(|w| w.set_visible(false));
}

pub fn edit(w: &slint::Window, key: char) {
    use slint::platform::{Key, WindowEvent};
    // Cocoa меню поглощает сочетание; направляем его в обработчик текста Slint.
    let modifier: slint::SharedString = Key::Control.into();
    w.dispatch_event(WindowEvent::KeyPressed { text: modifier.clone() });
    w.dispatch_event(WindowEvent::KeyPressed {
        text: key.to_string().into(),
    });
    w.dispatch_event(WindowEvent::KeyReleased {
        text: key.to_string().into(),
    });
    w.dispatch_event(WindowEvent::KeyReleased { text: modifier });
}
