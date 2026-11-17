//! Общее для Linux и macOS: один экземпляр через unix-сокет.

use std::io::Read;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::Mutex;

/// На Linux — в XDG_RUNTIME_DIR. На macOS его нет, но TMPDIR там и так у каждого
/// пользователя свой (/var/folders/…).
fn socket_path() -> PathBuf {
    let app = env!("CARGO_PKG_NAME");
    match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(dir) => PathBuf::from(dir).join(format!("{app}.sock")),
        None => std::env::temp_dir().join(format!("{app}-{}.sock", std::env::var("USER").unwrap_or_default())),
    }
}

static LISTENER: Mutex<Option<UnixListener>> = Mutex::new(None);

/// Занять сокет единственного экземпляра. Сокет остаётся на диске и после падения,
/// поэтому занятый, но молчащий файл считается брошенным.
pub fn claim_instance() -> bool {
    let path = socket_path();
    let listener = match UnixListener::bind(&path) {
        Ok(l) => l,
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            if UnixStream::connect(&path).is_ok() {
                return false;
            }
            let _ = std::fs::remove_file(&path);
            match UnixListener::bind(&path) {
                Ok(l) => l,
                Err(_) => return true,
            }
        }
        // Нет доступа к папке — работаем без проверки второго запуска.
        Err(_) => return true,
    };
    *LISTENER.lock().unwrap() = Some(listener);
    true
}

/// Второй запуск: попросить работающий экземпляр показать окно.
pub fn wake_existing() {
    use std::io::Write;
    if let Ok(mut s) = UnixStream::connect(socket_path()) {
        let _ = s.write_all(b"show");
    }
}

/// Звать `f` из фонового потока, когда второй запуск просит показать окно.
/// Пустое соединение — это проверка из `claim_instance`, на неё не реагируем.
pub fn on_wake(f: impl Fn() + Send + 'static) {
    let Some(listener) = LISTENER.lock().unwrap().take() else { return };
    std::thread::spawn(move || {
        for mut s in listener.incoming().flatten() {
            let mut buf = [0u8; 4];
            if s.read(&mut buf).is_ok_and(|n| n > 0) {
                f();
            }
        }
    });
}

pub fn cleanup() {
    let _ = std::fs::remove_file(socket_path());
}
