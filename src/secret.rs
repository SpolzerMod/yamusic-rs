//! Шифрование токена средствами Windows (DPAPI): расшифровать его может только
//! та же учётная запись на том же компьютере. В config.json лежит base64 шифротекста.
//! На macOS секреты лежат одной записью в связке ключей, а в config.json — base64 имени секрета.

use base64::Engine as _;

/// Дополнительная «соль» DPAPI: другие программы того же пользователя без неё не расшифруют.
/// Старое имя оставлено намеренно: с другой солью сохранённые токены перестанут читаться.
#[cfg(windows)]
const ENTROPY: &[u8] = b"yamusic-lite token v1";

#[cfg(windows)]
mod dpapi {
    use super::ENTROPY;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    fn blob(b: &[u8]) -> CRYPT_INTEGER_BLOB {
        CRYPT_INTEGER_BLOB { cbData: b.len() as u32, pbData: b.as_ptr() as *mut u8 }
    }

    fn take(out: CRYPT_INTEGER_BLOB) -> Vec<u8> {
        let v = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize) }.to_vec();
        unsafe { LocalFree(out.pbData as _) };
        v
    }

    pub fn protect(data: &[u8]) -> Option<Vec<u8>> {
        let (input, entropy) = (blob(data), blob(ENTROPY));
        let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
        let ok = unsafe {
            CryptProtectData(&input, std::ptr::null(), &entropy, std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut out)
        };
        (ok != 0).then(|| take(out))
    }

    pub fn unprotect(data: &[u8]) -> Option<Vec<u8>> {
        let (input, entropy) = (blob(data), blob(ENTROPY));
        let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
        let ok = unsafe {
            CryptUnprotectData(&input, std::ptr::null_mut(), &entropy, std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut out)
        };
        (ok != 0).then(|| take(out))
    }
}

#[cfg(target_os = "macos")]
mod keychain {
    use security_framework::passwords::{delete_generic_password, get_generic_password, set_generic_password};
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    #[cfg(not(test))]
    const SERVICE: &str = "yamusic-rs";
    #[cfg(test)]
    const SERVICE: &str = "yamusic-rs-test";
    /// Все секреты лежат одним JSON в одной записи: macOS спрашивает пароль связки отдельно
    /// на каждый доступ к записи, а у ad-hoc подписанной сборки разрешение не переживает обновление.
    const ACCOUNT: &str = "auth";

    type Secrets = BTreeMap<String, String>;
    /// Содержимое записи, прочитанное один раз за процесс.
    static CACHE: Mutex<Option<Secrets>> = Mutex::new(None);
    #[cfg(test)]
    pub static TEST_LOCK: Mutex<()> = Mutex::new(());
    /// Сколько раз тест видел запись в связку ключей.
    #[cfg(test)]
    static WRITES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    fn write_item(m: &Secrets) -> bool {
        #[cfg(test)]
        WRITES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if m.is_empty() {
            let _ = delete_generic_password(SERVICE, ACCOUNT);
            return true;
        }
        // Обновление на месте: set_generic_password меняет запись, а не удаляет и создаёт заново.
        serde_json::to_vec(m).is_ok_and(|json| set_generic_password(SERVICE, ACCOUNT, &json).is_ok())
    }

    fn load() -> Secrets {
        get_generic_password(SERVICE, ACCOUNT).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    fn with<R>(f: impl FnOnce(&mut Secrets) -> R) -> R {
        let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        f(cache.get_or_insert_with(load))
    }

    /// Пишет в связку ключей только изменившееся значение.
    pub fn set(account: &str, secret: &str) -> bool {
        with(|m| {
            if m.get(account).is_some_and(|v| v == secret) {
                return true;
            }
            let old = m.insert(account.into(), secret.into());
            if write_item(m) {
                return true;
            }
            match old {
                Some(v) => m.insert(account.into(), v),
                None => m.remove(account),
            };
            false
        })
    }

    pub fn get(account: &str) -> Option<String> {
        with(|m| m.get(account).cloned())
    }

    pub fn remove(account: &str) {
        with(|m| {
            if m.remove(account).is_some() {
                write_item(m);
            }
        });
    }

    #[cfg(test)]
    pub fn forget_cache() {
        *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// Обе записи в одном элементе, то же значение второй раз не пишется, выход всё убирает.
        #[test]
        fn single_item() {
            use std::sync::atomic::Ordering::Relaxed;
            let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            forget_cache();
            let start = WRITES.load(Relaxed);
            assert!(set("token", "t1"));
            assert!(set("token", "t1"));
            assert!(set("refresh", "r1"));
            assert_eq!(WRITES.load(Relaxed) - start, 2);
            forget_cache();
            assert_eq!(get("token").as_deref(), Some("t1"));
            assert_eq!(get("refresh").as_deref(), Some("r1"));
            assert!(get_generic_password(SERVICE, "token").is_err());
            remove("token");
            remove("refresh");
            forget_cache();
            assert!(get_generic_password(SERVICE, ACCOUNT).is_err());
            assert_eq!(get("token"), None);
        }
    }
}

/// Зашифровать строку для хранения в конфиге. `name` отличает секреты друг от друга: на
/// Windows он не нужен, на macOS это имя записи в связке ключей.
pub fn seal(name: &str, plain: &str) -> Option<String> {
    #[cfg(windows)]
    {
        let _ = name;
        dpapi::protect(plain.as_bytes()).map(|b| base64::engine::general_purpose::STANDARD.encode(b))
    }
    #[cfg(target_os = "macos")]
    {
        keychain::set(name, plain).then(|| base64::engine::general_purpose::STANDARD.encode(name))
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = (name, plain);
        None
    }
}

/// Убрать секрет из связки ключей (macOS), когда он больше не нужен. На Windows ничего не хранится.
pub fn forget(name: &str) {
    #[cfg(target_os = "macos")]
    keychain::remove(name);
    #[cfg(not(target_os = "macos"))]
    let _ = name;
}

/// Расшифровать строку из конфига; None — чужая учётная запись или испорченные данные.
pub fn open(sealed: &str) -> Option<String> {
    let raw = base64::engine::general_purpose::STANDARD.decode(sealed.trim()).ok()?;
    #[cfg(windows)]
    {
        String::from_utf8(dpapi::unprotect(&raw)?).ok()
    }
    #[cfg(target_os = "macos")]
    {
        keychain::get(&String::from_utf8(raw).ok()?)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = raw;
        None
    }
}

#[cfg(all(test, any(windows, target_os = "macos")))]
mod tests {
    #[test]
    fn roundtrip() {
        #[cfg(target_os = "macos")]
        let _g = super::keychain::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // Отдельное имя: записи самого приложения тест не трогает.
        let s = super::seal("test-roundtrip", "y0_secret").unwrap();
        assert!(!s.contains("y0_secret"));
        assert_eq!(super::open(&s).as_deref(), Some("y0_secret"));
        assert_eq!(super::open("AAAA"), None);
        super::forget("test-roundtrip");
        // В связке ключей запись исчезает; DPAPI ничего не хранит, и шифротекст читается всегда.
        #[cfg(target_os = "macos")]
        assert_eq!(super::open(&s), None);
    }
}
