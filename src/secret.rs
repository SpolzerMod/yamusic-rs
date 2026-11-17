//! Где хранить токен. Windows: шифрование DPAPI, расшифровать может только та же учётная
//! запись на том же компьютере, в config.json лежит base64 шифротекста. Linux: связка ключей
//! пользователя через Secret Service, macOS: связка ключей Keychain. В обоих случаях
//! в config.json только имя записи.

#[cfg(windows)]
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

/// Связка ключей Linux через Secret Service (GNOME Keyring, KWallet, KeePassXC). Секрет лежит
/// там, в config.json остаётся только имя записи. Сессия шифрованная (Диффи — Хеллман и AES),
/// так что по шине D-Bus секрет открытым не идёт.
#[cfg(target_os = "linux")]
mod keyring {
    use crate::config::log;
    use secret_service::blocking::SecretService;
    use secret_service::EncryptionType;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;

    /// Что уже лежит в связке: те же значения повторно не пишем. Конфиг сохраняется на каждое
    /// изменение настроек, а запись в связку — это запрос по D-Bus.
    static STORED: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);
    /// Службы нет (нет шины или демона): до конца работы больше не пытаемся.
    static MISSING: AtomicBool = AtomicBool::new(false);

    fn attributes(name: &str) -> HashMap<&str, &str> {
        HashMap::from([("application", "yamusic-rs"), ("secret", name)])
    }

    fn label(name: &str) -> String {
        let what = if name == "refresh" { "токен обновления" } else { "токен входа" };
        format!("{}: {what}", env!("APP_NAME"))
    }

    fn with<R>(f: impl FnOnce(&SecretService) -> Result<R, secret_service::Error>) -> Option<R> {
        if MISSING.load(Ordering::Relaxed) {
            return None;
        }
        let ss = match SecretService::connect(EncryptionType::Dh) {
            Ok(ss) => ss,
            Err(e) => {
                MISSING.store(true, Ordering::Relaxed);
                log(format!("связка ключей недоступна, токен останется в config.json: {e}"));
                return None;
            }
        };
        f(&ss).map_err(|e| log(format!("связка ключей: {e}"))).ok()
    }

    fn cached(f: impl FnOnce(&mut HashMap<String, String>)) {
        f(STORED.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new));
    }

    pub fn store(name: &str, secret: &str) -> bool {
        let mut same = false;
        cached(|m| same = m.get(name).is_some_and(|v| v == secret));
        if same {
            return true;
        }
        let saved = with(|ss| {
            let c = ss.get_default_collection().or_else(|_| ss.get_any_collection())?;
            c.ensure_unlocked()?;
            c.create_item(&label(name), attributes(name), secret.as_bytes(), true, "text/plain")?;
            Ok(())
        });
        if saved.is_some() {
            cached(|m| {
                m.insert(name.into(), secret.into());
            });
        }
        saved.is_some()
    }

    pub fn load(name: &str) -> Option<String> {
        let secret = with(|ss| {
            let found = ss.search_items(attributes(name))?;
            let Some(item) = found.unlocked.first().or(found.locked.first()) else { return Ok(None) };
            item.ensure_unlocked()?;
            Ok(String::from_utf8(item.get_secret()?).ok())
        })??;
        cached(|m| {
            m.insert(name.into(), secret.clone());
        });
        Some(secret)
    }

    pub fn remove(name: &str) {
        let mut known = false;
        cached(|m| known = m.remove(name).is_some());
        if known {
            with(|ss| ss.search_items(attributes(name))?.unlocked.iter().try_for_each(|i| i.delete()));
        }
    }
}

/// Связка ключей macOS (Keychain): общий пароль приложения с service `yamusic-rs`
/// и account по имени секрета. Видно в «Связке ключей» по имени yamusic-rs.
#[cfg(target_os = "macos")]
mod keyring {
    use crate::config::log;
    use security_framework::passwords::{delete_generic_password, get_generic_password, set_generic_password};
    use std::collections::HashMap;
    use std::sync::Mutex;

    const SERVICE: &str = "yamusic-rs";

    /// Что уже лежит в связке: конфиг сохраняется на каждое изменение настроек, а каждая
    /// запись в Keychain у неподписанной сборки может спросить пароль.
    static STORED: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

    fn cached(f: impl FnOnce(&mut HashMap<String, String>)) {
        f(STORED.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new));
    }

    pub fn store(name: &str, secret: &str) -> bool {
        let mut same = false;
        cached(|m| same = m.get(name).is_some_and(|v| v == secret));
        if same {
            return true;
        }
        match set_generic_password(SERVICE, name, secret.as_bytes()) {
            Ok(()) => {
                cached(|m| {
                    m.insert(name.into(), secret.into());
                });
                true
            }
            Err(e) => {
                log(format!("связка ключей: {e}"));
                false
            }
        }
    }

    pub fn load(name: &str) -> Option<String> {
        let secret = match get_generic_password(SERVICE, name) {
            Ok(s) => s,
            // errSecItemNotFound: записи просто нет (ещё не входили или уже вышли).
            Err(e) if e.code() == -25300 => return None,
            Err(e) => {
                log(format!("связка ключей: {e}"));
                return None;
            }
        };
        let secret = String::from_utf8(secret).ok()?;
        cached(|m| {
            m.insert(name.into(), secret.clone());
        });
        Some(secret)
    }

    pub fn remove(name: &str) {
        cached(|m| {
            m.remove(name);
        });
        let _ = delete_generic_password(SERVICE, name);
    }
}

/// Префикс в config.json: секрет лежит в связке ключей, а не рядом.
#[cfg(unix)]
const IN_KEYRING: &str = "keyring:";

/// Спрятать секрет для хранения в конфиге. `name` различает секреты в связке ключей,
/// DPAPI на Windows он не нужен. None — спрятать негде.
pub fn seal(name: &str, plain: &str) -> Option<String> {
    #[cfg(windows)]
    {
        let _ = name;
        dpapi::protect(plain.as_bytes()).map(|b| base64::engine::general_purpose::STANDARD.encode(b))
    }
    #[cfg(unix)]
    {
        keyring::store(name, plain).then(|| format!("{IN_KEYRING}{name}"))
    }
}

/// Достать секрет из конфига; None — чужая учётная запись, нет записи или испорченные данные.
pub fn open(sealed: &str) -> Option<String> {
    #[cfg(windows)]
    {
        let raw = base64::engine::general_purpose::STANDARD.decode(sealed.trim()).ok()?;
        String::from_utf8(dpapi::unprotect(&raw)?).ok()
    }
    #[cfg(unix)]
    {
        keyring::load(sealed.trim().strip_prefix(IN_KEYRING)?)
    }
}

/// Секрет больше не нужен (вышли из аккаунта). DPAPI ничего не хранит, связку ключей чистим.
pub fn forget(name: &str) {
    #[cfg(unix)]
    keyring::remove(name);
    #[cfg(windows)]
    let _ = name;
}

#[cfg(all(test, windows))]
mod tests {
    #[test]
    fn roundtrip() {
        let s = super::seal("token", "y0_secret").unwrap();
        assert!(!s.contains("y0_secret"));
        assert_eq!(super::open(&s).as_deref(), Some("y0_secret"));
        assert_eq!(super::open("AAAA"), None);
    }
}

/// Пишет в настоящую связку ключей пользователя, поэтому только вручную:
/// `cargo test -- --ignored keychain`.
#[cfg(all(test, target_os = "macos"))]
mod tests {
    #[test]
    #[ignore]
    fn keychain_roundtrip() {
        let s = super::seal("test", "y0_secret").unwrap();
        assert_eq!(s, "keyring:test");
        assert_eq!(super::keyring::load("test").as_deref(), Some("y0_secret"));
        super::forget("test");
        assert_eq!(super::keyring::load("test"), None);
    }
}
