//! Шифрование токена средствами Windows (DPAPI): расшифровать его может только
//! та же учётная запись на том же компьютере. В config.json лежит base64 шифротекста.

use base64::Engine as _;

/// Дополнительная «соль» DPAPI: другие программы того же пользователя без неё не расшифруют.
/// Старое имя оставлено намеренно: с другой солью сохранённые токены перестанут читаться.
#[cfg(windows)] // используется только DPAPI
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

/// Зашифровать строку для хранения в конфиге.
pub fn seal(plain: &str) -> Option<String> {
    #[cfg(windows)]
    {
        dpapi::protect(plain.as_bytes()).map(|b| base64::engine::general_purpose::STANDARD.encode(b))
    }
    #[cfg(not(windows))]
    {
        let _ = plain;
        None
    }
}

/// Расшифровать строку из конфига; None — чужая учётная запись или испорченные данные.
pub fn open(sealed: &str) -> Option<String> {
    let raw = base64::engine::general_purpose::STANDARD.decode(sealed.trim()).ok()?;
    #[cfg(windows)]
    {
        String::from_utf8(dpapi::unprotect(&raw)?).ok()
    }
    #[cfg(not(windows))]
    {
        let _ = raw;
        None
    }
}

#[cfg(all(test, windows))]
mod tests {
    #[test]
    fn roundtrip() {
        let s = super::seal("y0_secret").unwrap();
        assert!(!s.contains("y0_secret"));
        assert_eq!(super::open(&s).as_deref(), Some("y0_secret"));
        assert_eq!(super::open("AAAA"), None);
    }
}
