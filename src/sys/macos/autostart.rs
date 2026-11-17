//! Автозапуск через ServiceManagement, без записи LaunchAgent и повышенных прав.
use std::{
    cell::RefCell,
    ffi::{c_char, CStr},
};
unsafe extern "C" {
    fn ym_autostart_status() -> i32;
    fn ym_autostart_set(enabled: bool, error: *mut c_char, capacity: usize) -> bool;
    fn ym_autostart_settings();
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Status {
    Unavailable,
    Disabled,
    Enabled,
    RequiresApproval,
    NotFound,
}
impl From<i32> for Status {
    fn from(value: i32) -> Self {
        match value {
            0 => Self::Disabled,
            1 => Self::Enabled,
            2 => Self::RequiresApproval,
            3 => Self::NotFound,
            _ => Self::Unavailable,
        }
    }
}
thread_local! { static ERROR: RefCell<Option<String>> = const { RefCell::new(None) }; }
pub fn status() -> Status {
    unsafe { ym_autostart_status() }.into()
}
pub fn autostart_enabled() -> bool {
    matches!(status(), Status::Enabled | Status::RequiresApproval)
}
pub fn set_autostart(on: bool) -> bool {
    let mut message = [0 as c_char; 1024];
    let ok = unsafe { ym_autostart_set(on, message.as_mut_ptr(), message.len()) };
    let error = if ok {
        None
    } else {
        Some(unsafe { CStr::from_ptr(message.as_ptr()) }.to_string_lossy().into_owned())
    };
    if let Some(error) = &error {
        crate::config::log(format!("автозапуск macOS: {error}"));
    }
    ERROR.with(|e| *e.borrow_mut() = error);
    ok
}
pub fn detail() -> String {
    ERROR.with(|e| e.borrow().clone()).unwrap_or_else(|| {
        match status() {
            Status::Unavailable => "Для автозапуска переместите .app в «Программы» или ~/Applications",
            Status::Disabled => "Открывать приложение при входе в macOS, без запуска музыки",
            Status::Enabled => "Приложение будет открываться при входе в macOS",
            Status::RequiresApproval => "Разрешите запуск в системных настройках → Основные → Объекты входа",
            Status::NotFound => "macOS не нашла приложение. Проверьте его расположение",
        }
        .into()
    })
}
pub fn open_settings() {
    unsafe { ym_autostart_settings() }
}
// Система хранит регистрацию; обычный запуск не должен менять выбор пользователя.
pub fn refresh_autostart() {}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pending_approval_is_not_enabled() {
        assert_eq!(Status::from(2), Status::RequiresApproval);
        assert_ne!(Status::from(2), Status::Enabled);
        assert_eq!(Status::from(99), Status::Unavailable);
    }
}
