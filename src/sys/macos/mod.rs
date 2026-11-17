//! Системный слой macOS. Нативные обработчики отправляют команды в поток Slint.
pub mod autostart;
mod commands;
mod instance;
pub mod media;
mod native;
mod window;

pub use autostart::{autostart_enabled, refresh_autostart, set_autostart};
pub use commands::{MacCommand, MacIntegration};
pub use instance::{claim_instance, cleanup, wake_existing};
pub use native::{finish_termination, is_fullscreen, set_mini};
pub use window::{created, edit, ensure_visible, hide, prepare, raise, scale, system_light_theme, work_area};

use slint::winit_030::WinitWindowAccessor;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
const APP_ID: &str = "yamusic-rs";

fn parented(w: &slint::Window, dialog: rfd::FileDialog) -> rfd::FileDialog {
    w.with_winit_window(|parent| dialog.clone().set_parent(parent)).unwrap_or(dialog)
}
pub fn pick_image(w: &slint::Window, title: &str) -> Option<String> {
    parented(
        w,
        rfd::FileDialog::new()
            .set_title(title)
            .add_filter("Изображения", &["jpg", "jpeg", "png"]),
    )
    .pick_file()
    .map(|p| p.to_string_lossy().into_owned())
}
pub fn pick_folder(w: &slint::Window, title: &str, start: &str) -> Option<String> {
    parented(w, rfd::FileDialog::new().set_title(title).set_directory(start))
        .pick_folder()
        .map(|p| p.to_string_lossy().into_owned())
}
pub fn open_url(url: &str) {
    if let Ok(mut child) = Command::new("/usr/bin/open").arg(url).spawn() {
        std::thread::spawn(move || child.wait());
    }
}
pub fn release_memory() {}
fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}
pub fn config_base() -> PathBuf {
    home().join("Library/Application Support")
}
pub fn cache_base() -> PathBuf {
    home().join("Library/Caches")
}
pub fn music_dir() -> PathBuf {
    home().join("Music")
}
pub fn hostname() -> Option<String> {
    let o = Command::new("/bin/hostname").output().ok()?;
    Some(String::from_utf8(o.stdout).ok()?.trim().to_owned())
}
pub fn copy_text(text: &str) {
    if let Ok(mut c) = Command::new("/usr/bin/pbcopy").stdin(Stdio::piped()).spawn() {
        if let Some(mut input) = c.stdin.take() {
            let _ = input.write_all(text.as_bytes());
        }
        let _ = c.wait();
    }
}
