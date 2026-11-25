//! Где хранить токен. Windows: шифрование DPAPI, расшифровать может только та же учётная
//! запись на том же компьютере, в config.json лежит base64 шифротекста. Linux и macOS: связка
//! ключей пользователя (Secret Service / Keychain), в config.json только имя записи.

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
            c.unlock()?;
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
            // ensure_unlocked только проверяет. KeePassXC отдаёт записи запертыми до Unlock (#81).
            item.unlock()?;
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
            with(|ss| {
                let found = ss.search_items(attributes(name))?;
                found.unlocked.iter().chain(&found.locked).try_for_each(|i| {
                    i.unlock()?;
                    i.delete()
                })
            });
        }
    }
}

/// Связка ключей macOS (Keychain, Security.framework). Токен входа и токен обновления лежат
/// в одной записи (JSON) и читаются не больше одного раза за запуск: при ad-hoc подписи после
/// обновления macOS спросит подтверждение максимум один раз, а не на каждое обращение.
#[cfg(target_os = "macos")]
mod keychain {
    use crate::config::log;
    use objc2::msg_send;
    use objc2::runtime::AnyObject;
    use objc2_foundation::{NSData, NSMutableDictionary, NSString};
    use std::collections::HashMap;
    use std::ffi::c_void;
    use std::sync::Mutex;

    const SERVICE: &str = "io.github.spolzermod.yamusic-rs";
    const ACCOUNT: &str = "tokens";
    const ERR_SEC_SUCCESS: i32 = 0;
    const ERR_SEC_DUPLICATE_ITEM: i32 = -25299;
    const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;

    #[link(name = "Security", kind = "framework")]
    extern "C" {
        static kSecClass: *const AnyObject;
        static kSecClassGenericPassword: *const AnyObject;
        static kSecAttrService: *const AnyObject;
        static kSecAttrAccount: *const AnyObject;
        static kSecAttrLabel: *const AnyObject;
        static kSecValueData: *const AnyObject;
        static kSecReturnData: *const AnyObject;
        static kSecMatchLimit: *const AnyObject;
        static kSecMatchLimitOne: *const AnyObject;

        fn SecItemAdd(attributes: *const c_void, result: *mut *const c_void) -> i32;
        fn SecItemCopyMatching(query: *const c_void, result: *mut *const c_void) -> i32;
        fn SecItemUpdate(query: *const c_void, attributes_to_update: *const c_void) -> i32;
        fn SecItemDelete(query: *const c_void) -> i32;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        static kCFBooleanTrue: *const AnyObject;
        fn CFDataGetLength(the_data: *const c_void) -> isize;
        fn CFDataGetBytePtr(the_data: *const c_void) -> *const u8;
        fn CFRelease(cf: *const c_void);
    }

    #[derive(Default)]
    struct Cache {
        loaded: bool,
        unavailable: bool,
        map: HashMap<String, String>,
    }

    static CACHE: Mutex<Option<Cache>> = Mutex::new(None);

    fn with_cache<R>(f: impl FnOnce(&mut Cache) -> R) -> R {
        let mut guard = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        f(guard.get_or_insert_with(Cache::default))
    }

    fn dict_set(d: &NSMutableDictionary<AnyObject, AnyObject>, key: *const AnyObject, val: *const AnyObject) {
        unsafe {
            let _: () = msg_send![d, setObject: val, forKey: key];
        }
    }

    fn base_query() -> objc2::rc::Retained<NSMutableDictionary<AnyObject, AnyObject>> {
        let d = NSMutableDictionary::<AnyObject, AnyObject>::new();
        let service = NSString::from_str(SERVICE);
        let account = NSString::from_str(ACCOUNT);
        unsafe {
            dict_set(&d, kSecClass, kSecClassGenericPassword);
            dict_set(&d, kSecAttrService, (&*service as *const NSString).cast());
            dict_set(&d, kSecAttrAccount, (&*account as *const NSString).cast());
        }
        d
    }

    fn read_raw() -> Result<Option<Vec<u8>>, i32> {
        let q = base_query();
        unsafe {
            dict_set(&q, kSecReturnData, kCFBooleanTrue);
            dict_set(&q, kSecMatchLimit, kSecMatchLimitOne);
            let mut result: *const c_void = std::ptr::null();
            let status = SecItemCopyMatching((&*q as *const NSMutableDictionary<AnyObject, AnyObject>).cast(), &mut result);
            if status == ERR_SEC_ITEM_NOT_FOUND {
                return Ok(None);
            }
            if status != ERR_SEC_SUCCESS || result.is_null() {
                return Err(status);
            }
            let len = CFDataGetLength(result).max(0) as usize;
            let ptr = CFDataGetBytePtr(result);
            let bytes = if len > 0 && !ptr.is_null() {
                std::slice::from_raw_parts(ptr, len).to_vec()
            } else {
                Vec::new()
            };
            CFRelease(result);
            Ok(Some(bytes))
        }
    }

    fn write_raw(bytes: &[u8]) -> Result<(), i32> {
        let data = NSData::with_bytes(bytes);
        let q = base_query();
        let update = NSMutableDictionary::<AnyObject, AnyObject>::new();
        unsafe {
            dict_set(&update, kSecValueData, (&*data as *const NSData).cast());
            let status = SecItemUpdate(
                (&*q as *const NSMutableDictionary<AnyObject, AnyObject>).cast(),
                (&*update as *const NSMutableDictionary<AnyObject, AnyObject>).cast(),
            );
            if status == ERR_SEC_SUCCESS {
                return Ok(());
            }
            if status != ERR_SEC_ITEM_NOT_FOUND {
                return Err(status);
            }
            let label = NSString::from_str(env!("APP_NAME"));
            dict_set(&q, kSecAttrLabel, (&*label as *const NSString).cast());
            dict_set(&q, kSecValueData, (&*data as *const NSData).cast());
            let add_status = SecItemAdd((&*q as *const NSMutableDictionary<AnyObject, AnyObject>).cast(), std::ptr::null_mut());
            if add_status == ERR_SEC_SUCCESS {
                Ok(())
            } else if add_status == ERR_SEC_DUPLICATE_ITEM {
                let q2 = base_query();
                let retry = SecItemUpdate(
                    (&*q2 as *const NSMutableDictionary<AnyObject, AnyObject>).cast(),
                    (&*update as *const NSMutableDictionary<AnyObject, AnyObject>).cast(),
                );
                if retry == ERR_SEC_SUCCESS { Ok(()) } else { Err(retry) }
            } else {
                Err(add_status)
            }
        }
    }

    fn delete_raw() -> Result<(), i32> {
        let q = base_query();
        let status = unsafe { SecItemDelete((&*q as *const NSMutableDictionary<AnyObject, AnyObject>).cast()) };
        if status == ERR_SEC_SUCCESS || status == ERR_SEC_ITEM_NOT_FOUND {
            Ok(())
        } else {
            Err(status)
        }
    }

    pub(super) fn decode_bundle(bytes: &[u8]) -> HashMap<String, String> {
        serde_json::from_slice(bytes).unwrap_or_default()
    }

    pub(super) fn encode_bundle(map: &HashMap<String, String>) -> Option<Vec<u8>> {
        serde_json::to_vec(map).ok()
    }

    fn ensure_loaded(cache: &mut Cache) {
        if cache.loaded || cache.unavailable {
            return;
        }
        match read_raw() {
            Ok(Some(bytes)) => {
                cache.map = decode_bundle(&bytes);
                cache.loaded = true;
            }
            Ok(None) => {
                cache.loaded = true;
            }
            Err(code) => {
                cache.unavailable = true;
                log(format!("связка ключей macOS недоступна (OSStatus {code}), токен останется в config.json"));
            }
        }
    }

    pub fn store(name: &str, secret: &str) -> bool {
        with_cache(|cache| {
            ensure_loaded(cache);
            if cache.unavailable {
                return false;
            }
            if cache.map.get(name).is_some_and(|v| v == secret) {
                return true;
            }
            let mut next = cache.map.clone();
            next.insert(name.to_string(), secret.to_string());
            let Some(bytes) = encode_bundle(&next) else { return false };
            match write_raw(&bytes) {
                Ok(()) => {
                    cache.map = next;
                    cache.loaded = true;
                    true
                }
                Err(code) => {
                    cache.unavailable = true;
                    log(format!("связка ключей macOS: не удалось сохранить (OSStatus {code})"));
                    false
                }
            }
        })
    }

    pub fn load(name: &str) -> Option<String> {
        with_cache(|cache| {
            ensure_loaded(cache);
            cache.map.get(name).cloned()
        })
    }

    pub fn remove(name: &str) {
        with_cache(|cache| {
            ensure_loaded(cache);
            if cache.unavailable || cache.map.remove(name).is_none() {
                return;
            }
            let res = if cache.map.is_empty() {
                delete_raw()
            } else if let Some(bytes) = encode_bundle(&cache.map) {
                write_raw(&bytes)
            } else {
                Ok(())
            };
            if let Err(code) = res {
                log(format!("связка ключей macOS: не удалось очистить запись (OSStatus {code})"));
            }
        });
    }
}

/// Префикс в config.json: секрет лежит в связке ключей, а не рядом.
#[cfg(target_os = "linux")]
const IN_KEYRING: &str = "keyring:";
#[cfg(target_os = "macos")]
const IN_KEYCHAIN: &str = "keychain:";

/// Спрятать секрет для хранения в конфиге. `name` различает секреты в связке ключей,
/// DPAPI на Windows он не нужен. None — спрятать негде.
pub fn seal(name: &str, plain: &str) -> Option<String> {
    #[cfg(windows)]
    {
        let _ = name;
        dpapi::protect(plain.as_bytes()).map(|b| base64::engine::general_purpose::STANDARD.encode(b))
    }
    #[cfg(target_os = "linux")]
    {
        keyring::store(name, plain).then(|| format!("{IN_KEYRING}{name}"))
    }
    #[cfg(target_os = "macos")]
    {
        keychain::store(name, plain).then(|| format!("{IN_KEYCHAIN}{name}"))
    }
    // Android: config.json и так лежит в личной папке приложения, другим приложениям её не прочесть.
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        let _ = (name, plain);
        None
    }
}

/// Достать секрет из конфига; None — чужая учётная запись, нет записи или испорченные данные.
pub fn open(sealed: &str) -> Option<String> {
    #[cfg(windows)]
    {
        let raw = base64::engine::general_purpose::STANDARD.decode(sealed.trim()).ok()?;
        String::from_utf8(dpapi::unprotect(&raw)?).ok()
    }
    #[cfg(target_os = "linux")]
    {
        keyring::load(sealed.trim().strip_prefix(IN_KEYRING)?)
    }
    #[cfg(target_os = "macos")]
    {
        keychain::load(sealed.trim().strip_prefix(IN_KEYCHAIN)?)
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        let _ = sealed;
        None
    }
}

/// Секрет больше не нужен (вышли из аккаунта). DPAPI ничего не хранит, связку ключей чистим.
pub fn forget(name: &str) {
    #[cfg(target_os = "linux")]
    keyring::remove(name);
    #[cfg(target_os = "macos")]
    keychain::remove(name);
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let _ = name;
}

#[cfg(all(test, target_os = "macos"))]
mod macos_tests {
    use super::keychain::{decode_bundle, encode_bundle};
    use std::collections::HashMap;

    #[test]
    fn bundle_roundtrip() {
        let mut map = HashMap::new();
        map.insert("token".into(), "y0_access".into());
        map.insert("refresh".into(), "y0_refresh".into());
        let raw = encode_bundle(&map).unwrap();
        assert_eq!(decode_bundle(&raw), map);
        assert!(decode_bundle(b"not json").is_empty());
    }
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
