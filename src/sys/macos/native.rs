//! Узкий ABI к AppKit. Все вызовы (кроме обратного вызова) — из главного потока.
use super::commands::{CommandSender, MacCommand};
use std::sync::Mutex;

static SENDER: Mutex<Option<CommandSender>> = Mutex::new(None);
unsafe extern "C" {
    fn ym_start(callback: extern "C" fn(i32, f64)) -> bool;
    fn ym_prepare_window();
    fn ym_activate();
    fn ym_stop();
    fn ym_is_fullscreen() -> bool;
    fn ym_set_mini(mini: bool);
    fn ym_finish_termination();
    fn ym_light_theme() -> bool;
    fn ym_visible_area(left: *mut f64, top: *mut f64, right: *mut f64, bottom: *mut f64) -> bool;
    fn ym_ensure_visible();
}
extern "C" fn on_command(code: i32, value: f64) {
    let command = match code {
        0 => MacCommand::ShowWindow,
        1 => MacCommand::CloseWindow,
        2 => MacCommand::Quit,
        3 => MacCommand::Settings,
        4 => MacCommand::Play,
        5 => MacCommand::Pause,
        6 => MacCommand::PlayPause,
        7 => MacCommand::Next,
        8 => MacCommand::Prev,
        9 => MacCommand::Seek(value as f32),
        10 => MacCommand::Mini,
        11 => MacCommand::Edit('x'),
        12 => MacCommand::Edit('c'),
        13 => MacCommand::Edit('v'),
        14 => MacCommand::Edit('a'),
        15 => MacCommand::EnsureVisible,
        _ => return,
    };
    let sender = SENDER.lock().ok().and_then(|s| s.clone());
    if let Some(sender) = sender {
        sender.send(command);
    }
}
pub(super) fn start(sender: CommandSender) -> bool {
    *SENDER.lock().unwrap() = Some(sender);
    // SAFETY: вызывается владельцем MacIntegration с MainThreadMarker.
    let ok = unsafe { ym_start(on_command) };
    if !ok {
        *SENDER.lock().unwrap() = None;
    }
    ok
}
pub(super) fn prepare_window() {
    unsafe { ym_prepare_window() }
}
pub(super) fn activate() {
    unsafe { ym_activate() }
}
pub(super) fn stop() {
    *SENDER.lock().unwrap() = None;
    unsafe { ym_stop() }
}

pub(super) fn light_theme() -> bool {
    unsafe { ym_light_theme() }
}
pub(super) fn visible_area() -> Option<(f64, f64, f64, f64)> {
    let (mut l, mut t, mut r, mut b) = (0.0, 0.0, 0.0, 0.0);
    unsafe { ym_visible_area(&mut l, &mut t, &mut r, &mut b) }.then_some((l, t, r, b))
}
pub(super) fn ensure_visible() {
    unsafe { ym_ensure_visible() }
}

pub fn finish_termination() {
    unsafe { ym_finish_termination() }
}

pub fn is_fullscreen() -> bool {
    unsafe { ym_is_fullscreen() }
}
pub fn set_mini(mini: bool) {
    unsafe { ym_set_mini(mini) }
}
