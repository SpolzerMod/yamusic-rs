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
    use crate::config::log;
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
    /// errSecItemNotFound: записи ещё нет.
    const NOT_FOUND: i32 = -25300;

    type Secrets = BTreeMap<String, String>;

    enum Read {
        Found(Secrets),
        Missing,
        Failed,
    }

    /// Откуда берутся секреты и куда пишутся; в тестах вместо связки ключей подставляется заглушка.
    trait Store {
        fn read(&mut self) -> Read;
        fn write(&mut self, m: &Secrets) -> bool;
    }

    struct Keychain;

    impl Store for Keychain {
        fn read(&mut self) -> Read {
            match get_generic_password(SERVICE, ACCOUNT) {
                Ok(b) => Read::Found(serde_json::from_slice(&b).unwrap_or_else(|e| {
                    log(format!("связка ключей: запись повреждена: {e}"));
                    Secrets::new()
                })),
                Err(e) if e.code() == NOT_FOUND => Read::Missing,
                Err(e) => {
                    log(format!("связка ключей: чтение не удалось: {e}"));
                    Read::Failed
                }
            }
        }

        fn write(&mut self, m: &Secrets) -> bool {
            #[cfg(test)]
            WRITES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let res = if m.is_empty() {
                match delete_generic_password(SERVICE, ACCOUNT) {
                    Err(e) if e.code() != NOT_FOUND => Err(e.to_string()),
                    _ => Ok(()),
                }
            } else {
                // Обновление на месте: set_generic_password меняет запись, а не удаляет и создаёт заново.
                serde_json::to_vec(m).map_err(|e| e.to_string()).and_then(|json| set_generic_password(SERVICE, ACCOUNT, &json).map_err(|e| e.to_string()))
            };
            if let Err(e) = &res {
                log(format!("связка ключей: запись не удалась: {e}"));
            }
            res.is_ok()
        }
    }

    /// Состояние за один запуск: что прочитано и что можно писать.
    #[derive(Default)]
    struct Cache {
        /// Содержимое записи, прочитанное один раз за процесс.
        secrets: Option<Secrets>,
        /// Запись не удалось прочитать (отказ в доступе, связка закрыта). Повторно за этот запуск
        /// не читаем, чтобы не спрашивать пароль снова и снова; следующий запуск попробует опять.
        unreadable: bool,
        /// Запись не удалась. Автоматические сохранения (громкость и прочее) больше не пишут,
        /// чтобы не вызывать запрос пароля на каждое; явный вход или выход пробует ещё раз.
        blocked: bool,
        /// После нечитаемой записи вход или выход обязаны перезаписать её, даже если в памяти
        /// пусто и «ничего не изменилось».
        force: bool,
    }

    impl Cache {
        /// None — связка недоступна: тогда ничего не читаем и не пишем, чтобы не затереть запись.
        fn ready(&mut self, st: &mut dyn Store) -> Option<&mut Secrets> {
            if self.secrets.is_none() {
                if self.unreadable {
                    return None;
                }
                self.secrets = Some(match st.read() {
                    Read::Found(m) => m,
                    Read::Missing => Secrets::new(),
                    Read::Failed => {
                        self.unreadable = true;
                        return None;
                    }
                });
            }
            self.secrets.as_mut()
        }

        fn get(&mut self, st: &mut dyn Store, account: &str) -> Option<String> {
            self.ready(st)?.get(account).cloned()
        }

        /// Some — записать секрет, None — убрать. Все изменения — одной записью в связку ключей,
        /// и только если что-то изменилось. Не вышло — всё остаётся как было.
        fn apply(&mut self, st: &mut dyn Store, changes: &[(&str, Option<&str>)]) -> bool {
            let (blocked, force) = (self.blocked, self.force);
            let Some(m) = self.ready(st) else { return false };
            let before = m.clone();
            for (name, value) in changes {
                match value {
                    Some(v) => m.insert(name.to_string(), v.to_string()),
                    None => m.remove(*name),
                };
            }
            if *m == before && !force {
                return true;
            }
            let ok = !blocked && st.write(m);
            if ok {
                self.force = false;
                return true;
            }
            if let Some(m) = self.secrets.as_mut() {
                *m = before;
            }
            self.blocked = true;
            false
        }

        /// Пользователь сам вошёл или вышел. Если запись не читалась, берём чистый лист и
        /// обязательно перезаписываем её новым состоянием; если запись не удалась — пробуем ещё раз.
        fn explicit_change(&mut self) {
            if self.unreadable {
                self.unreadable = false;
                self.secrets = Some(Secrets::new());
                self.force = true;
            }
            self.blocked = false;
        }
    }

    static CACHE: Mutex<Option<Cache>> = Mutex::new(None);
    #[cfg(test)]
    pub static TEST_LOCK: Mutex<()> = Mutex::new(());
    /// Сколько раз тест видел запись в связку ключей.
    #[cfg(test)]
    static WRITES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    fn with<R>(f: impl FnOnce(&mut Cache) -> R) -> R {
        let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        f(cache.get_or_insert_with(Cache::default))
    }

    pub fn unreadable() -> bool {
        with(|c| c.unreadable)
    }

    pub fn get(account: &str) -> Option<String> {
        with(|c| c.get(&mut Keychain, account))
    }

    pub fn apply(changes: &[(&str, Option<&str>)]) -> bool {
        with(|c| c.apply(&mut Keychain, changes))
    }

    pub fn explicit_change() {
        with(Cache::explicit_change);
    }

    #[cfg(test)]
    pub fn forget_cache() {
        *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// Заглушка вместо связки ключей: что прочитается, что записано и сколько было записей.
        struct Fake {
            read: Option<Secrets>,
            fail_read: bool,
            fail_write: bool,
            writes: usize,
        }

        impl Fake {
            fn new(read: Option<Secrets>) -> Fake {
                Fake { read, fail_read: false, fail_write: false, writes: 0 }
            }
        }

        impl Store for Fake {
            fn read(&mut self) -> Read {
                match (&self.read, self.fail_read) {
                    (_, true) => Read::Failed,
                    (Some(m), _) => Read::Found(m.clone()),
                    (None, _) => Read::Missing,
                }
            }

            fn write(&mut self, m: &Secrets) -> bool {
                self.writes += 1;
                if self.fail_write {
                    return false;
                }
                self.read = (!m.is_empty()).then(|| m.clone());
                true
            }
        }

        fn old_account() -> Secrets {
            Secrets::from([("token".to_string(), "old".to_string()), ("refresh".to_string(), "oldr".to_string())])
        }

        /// Запись не читалась, но пользователь вошёл: новое состояние обязано её перезаписать.
        #[test]
        fn login_after_failed_read_overwrites() {
            let mut st = Fake::new(Some(old_account()));
            st.fail_read = true;
            let mut c = Cache::default();
            assert!(!c.apply(&mut st, &[("token", Some("new"))]));
            assert!(c.unreadable && st.writes == 0);
            c.explicit_change();
            assert!(!c.unreadable);
            assert!(c.apply(&mut st, &[("token", Some("new")), ("refresh", Some("newr"))]));
            assert_eq!(st.writes, 1);
            assert_eq!(st.read.as_ref().and_then(|m| m.get("token")).map(String::as_str), Some("new"));
            assert_eq!(st.read.as_ref().and_then(|m| m.get("refresh")).map(String::as_str), Some("newr"));
        }

        /// Выход после нечитаемой записи тоже пишет: старый аккаунт не должен вернуться.
        #[test]
        fn logout_after_failed_read_clears_item() {
            let mut st = Fake::new(Some(old_account()));
            st.fail_read = true;
            let mut c = Cache::default();
            assert_eq!(c.get(&mut st, "token"), None);
            c.explicit_change();
            assert!(c.apply(&mut st, &[("token", None), ("refresh", None)]));
            assert_eq!(st.writes, 1);
            assert!(st.read.is_none());
            // Дальше без изменений ничего не пишется.
            assert!(c.apply(&mut st, &[("token", None), ("refresh", None)]));
            assert_eq!(st.writes, 1);
        }

        /// После неудачной записи автоматические сохранения не повторяют её; вход пробует снова.
        #[test]
        fn failed_write_is_not_retried() {
            let mut st = Fake::new(None);
            st.fail_write = true;
            let mut c = Cache::default();
            assert!(!c.apply(&mut st, &[("token", Some("t"))]));
            assert_eq!(st.writes, 1);
            for _ in 0..3 {
                assert!(!c.apply(&mut st, &[("token", Some("t"))]));
            }
            assert_eq!(st.writes, 1);
            // Состояние откатилось: без изменений запись и не нужна.
            assert!(c.apply(&mut st, &[("token", None)]));
            c.explicit_change();
            st.fail_write = false;
            assert!(c.apply(&mut st, &[("token", Some("t"))]));
            assert_eq!(st.writes, 2);
        }

        /// Обе записи в одном элементе, то же значение второй раз не пишется, выход всё убирает.
        /// Настоящая связка ключей, но служба тестовая.
        #[test]
        fn single_item() {
            use std::sync::atomic::Ordering::Relaxed;
            let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            forget_cache();
            let start = WRITES.load(Relaxed);
            assert!(apply(&[("token", Some("t1")), ("refresh", Some("r1"))]));
            assert!(apply(&[("token", Some("t1")), ("refresh", Some("r1"))]));
            assert_eq!(WRITES.load(Relaxed) - start, 1);
            forget_cache();
            assert_eq!(get("token").as_deref(), Some("t1"));
            assert_eq!(get("refresh").as_deref(), Some("r1"));
            assert!(get_generic_password(SERVICE, "token").is_err());
            assert!(apply(&[("token", None), ("refresh", None)]));
            assert_eq!(WRITES.load(Relaxed) - start, 2);
            forget_cache();
            assert!(get_generic_password(SERVICE, ACCOUNT).is_err());
            assert_eq!(get("token"), None);
        }
    }
}

/// Зашифровать секреты для хранения в конфиге: пары «имя — значение». Для `None` секрет
/// убирается из хранилища. Результат по порядку: маркер для config.json или None (убран
/// или не вышло). `name` отличает секреты друг от друга: на Windows он не нужен, на macOS
/// это ключ внутри записи в связке ключей, и все изменения уходят в неё одной записью.
pub fn seal_all<const N: usize>(items: [(&str, Option<&str>); N]) -> [Option<String>; N] {
    #[cfg(windows)]
    {
        items.map(|(_, plain)| {
            plain.and_then(|p| dpapi::protect(p.as_bytes())).map(|b| base64::engine::general_purpose::STANDARD.encode(b))
        })
    }
    #[cfg(target_os = "macos")]
    {
        let ok = keychain::apply(&items);
        items.map(|(name, plain)| (ok && plain.is_some()).then(|| base64::engine::general_purpose::STANDARD.encode(name)))
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        items.map(|_| None)
    }
}

/// Пользователь сам вошёл или вышел (macOS): новое состояние нужно записать, даже если связка
/// ключей перед этим не прочиталась, и пробовать записать ещё раз, если прошлая запись не удалась.
pub fn explicit_change() {
    #[cfg(target_os = "macos")]
    keychain::explicit_change();
}

/// Связку ключей не удалось прочитать (macOS): секреты в ней целы, просто сейчас недоступны.
/// Тогда ссылки на них в config.json нужно сохранить, чтобы следующий запуск попробовал снова.
pub fn unreadable() -> bool {
    #[cfg(target_os = "macos")]
    {
        keychain::unreadable()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
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
        let [s] = super::seal_all([("test-roundtrip", Some("y0_secret"))]);
        let s = s.unwrap();
        assert!(!s.contains("y0_secret"));
        assert_eq!(super::open(&s).as_deref(), Some("y0_secret"));
        assert_eq!(super::open("AAAA"), None);
        let [gone] = super::seal_all([("test-roundtrip", None)]);
        assert_eq!(gone, None);
        // В связке ключей запись исчезает; DPAPI ничего не хранит, и шифротекст читается всегда.
        #[cfg(target_os = "macos")]
        assert_eq!(super::open(&s), None);
    }
}
