//! Автозапуск; реализация ServiceManagement добавляется отдельным этапом.
// Автозапуск пока не поддерживается; настройка не может включиться.
pub fn autostart_enabled() -> bool {
    false
}
pub fn set_autostart(_: bool) -> bool {
    false
}
pub fn refresh_autostart() {}
