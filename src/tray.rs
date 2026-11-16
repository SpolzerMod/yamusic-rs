//! Трей (значок в области уведомлений). На Windows — живой значок с меню
//! (`windows`), на macOS приложение живёт в Dock, и функции — заглушки:
//! события не приходят, `wake_existing` всегда «не нашёл».

/// События трея: клики и пункты меню. Приходят в основной поток через очередь Slint.
#[cfg_attr(not(windows), allow(dead_code))] // вне Windows трей не работает
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Toggle,
    Show,
    PlayPause,
    Prev,
    Next,
    Updates,
    Terms,
    Recommendations,
    Support,
    Quit,
}

/// Что показать в меню: окно свёрнуто или спрятано, играет ли трек, есть ли соседние.
#[cfg_attr(not(windows), allow(dead_code))] // вне Windows трей не работает
pub struct MenuState {
    pub minimized: bool,
    pub playing: bool,
    pub can_prev: bool,
    pub can_next: bool,
}

#[cfg(windows)]
mod windows;

#[cfg(windows)]
pub use windows::{install, remove, wake_existing};

#[cfg(not(windows))]
mod stub {
    use super::{Event, MenuState};

    /// Значка нет — обратные вызовы не понадобятся, но принимаем их, чтобы
    /// вызывающий код (gui.rs) не зависел от платформы.
    pub fn install(_on_event: impl Fn(Event) + 'static, _state: impl Fn() -> MenuState + 'static) {}

    pub fn remove() {}

    pub fn wake_existing() -> bool {
        false
    }
}

#[cfg(not(windows))]
pub use stub::{install, remove, wake_existing};
