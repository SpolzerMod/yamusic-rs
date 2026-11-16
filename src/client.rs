//! Как приложение представляется серверам Яндекса.
//!
//! Всё, что зависит от чужих клиентов и может поменяться на стороне Яндекса, собрано здесь:
//! OAuth-приложение, ключи подписи, заголовки клиента. Если что-то сломается после обновления
//! сервиса, править нужно только этот файл.

use std::sync::OnceLock;

/// Адрес API Музыки.
pub const API_BASE: &str = "https://api.music.yandex.net";

/// OAuth-приложение Яндекс Музыки для Android: им пользуются сторонние клиенты.
/// Вход — по коду устройства на ya.ru/device.
pub const OAUTH_CLIENT_ID: &str = "23cabbbdc6cd418abb4b39c32c41195d";
pub const OAUTH_CLIENT_SECRET: &str = "53bc75238f0c4d08a118e51fe9203300";
pub const OAUTH_BASE: &str = "https://oauth.yandex.ru";

/// Соль ссылки на mp3 из download-info.
pub const MP3_SALT: &str = "XGRlBW9FXlekgbPrRHuSiA";
/// Ключ подписи запросов get-file-info и текстов песен.
pub const SIGN_KEY: &str = "p93jhgh689SBReK6ghtw62";

/// Заголовок клиента для старых методов (стрим, лайки, очередь).
pub const ANDROID_CLIENT: &str = "YandexMusicAndroid/24023621";
/// Версия десктопного клиента, если официальное приложение не установлено.
const DESKTOP_VERSION_FALLBACK: &str = "5.126.1";

/// Заголовок десктопного клиента для лендингов и блоков. Версия берётся из установленного
/// официального приложения, чтобы не устаревать вместе с ним.
pub fn desktop_client() -> &'static str {
    static CLIENT: OnceLock<String> = OnceLock::new();
    CLIENT.get_or_init(|| {
        let version = crate::assets::Asar::open()
            .and_then(|mut a| a.read("package.json"))
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
            .and_then(|v| v.get("version")?.as_str().map(String::from))
            .filter(|v| v.chars().all(|c| c.is_ascii_digit() || c == '.'))
            .unwrap_or_else(|| DESKTOP_VERSION_FALLBACK.to_string());
        format!("YandexMusicDesktopAppWindows/{version}")
    })
}

/// Токен уходит только по https и только на хосты Яндекса.
pub fn token_allowed(url: &reqwest::Url) -> bool {
    if url.scheme() != "https" {
        return false;
    }
    let Some(host) = url.host_str() else { return false };
    ["yandex.net", "yandex.ru"].iter().any(|d| host == *d || host.ends_with(&format!(".{d}")))
}

/// В браузер открываем только https: ссылки приходят и от сервера (баннеры, концерты), а
/// `file:`, `ms-settings:` и прочие схемы запускают на компьютере что-то совсем другое.
pub fn is_https(url: &str) -> bool {
    reqwest::Url::parse(url).is_ok_and(|u| u.scheme() == "https")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_only_https() {
        assert!(is_https("https://ya.ru/device"));
        assert!(is_https("https://music.yandex.ru/legal/recommendations/ru/#music"));
        for bad in ["http://ya.ru", "file:///etc/passwd", "ms-settings:privacy", "javascript:alert(1)", "httpx://ya.ru", "", "ya.ru"] {
            assert!(!is_https(bad), "{bad}");
        }
    }

    #[test]
    fn hosts() {
        let ok = |s: &str| token_allowed(&reqwest::Url::parse(s).unwrap());
        assert!(ok("https://api.music.yandex.net/account/status"));
        assert!(ok("https://storage.mds.yandex.net/download-info/1"));
        assert!(!ok("http://api.music.yandex.net/"));
        assert!(!ok("https://evil.com/yandex.net"));
        assert!(!ok("https://yandex.net.evil.com/"));
        assert!(!ok("https://notyandex.ru/"));
    }
}
