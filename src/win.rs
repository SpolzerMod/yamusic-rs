//! Платформенный слой: показать/спрятать окно, открыть ссылку, тема системы,
//! автозапуск, диалог выбора файла. Реализация выбирается по cfg: `windows` —
//! обёртки над Win32, `unix` — macOS (окном управляет slint/winit).

#[cfg(windows)]
mod windows;

#[cfg(not(windows))]
mod unix;

#[cfg(windows)]
pub use windows::*;

#[cfg(not(windows))]
pub use unix::*;
